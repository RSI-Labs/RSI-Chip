//! Layout document storage, geometry operations, rule decks, DRC, and SVG rendering.

use super::*;

impl LayoutDocument {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_layer_rule(&mut self, rule: LayoutLayerRule) -> Result<(), String> {
        rule.validate()?;
        self.layer_rules.insert(rule.layer.clone(), rule);
        Ok(())
    }

    pub fn set_via_rule(&mut self, rule: LayoutViaRule) -> Result<(), String> {
        rule.validate()?;
        self.via_rules.insert(rule.via.clone(), rule);
        Ok(())
    }

    pub fn layer_rule(&self, layer: &str) -> Option<&LayoutLayerRule> {
        self.layer_rules.get(layer)
    }

    pub fn via_rule(&self, via: &str) -> Option<&LayoutViaRule> {
        self.via_rules.get(via)
    }

    pub fn antenna_ratio(&self) -> Option<f64> {
        self.antenna_ratio
    }

    pub fn set_antenna_ratio(&mut self, ratio: f64) -> Result<(), String> {
        if !ratio.is_finite() || ratio <= 0.0 {
            return Err("layout antenna ratio must be finite and positive".into());
        }
        self.antenna_ratio = Some(ratio);
        Ok(())
    }

    pub fn add_shape(&mut self, shape: LayoutShape) -> Result<(), String> {
        if shape.id.trim().is_empty() {
            return Err("layout shape id cannot be empty".into());
        }
        if shape.layer.trim().is_empty() || shape.net.trim().is_empty() {
            return Err(format!(
                "layout shape '{}' must name its layer and net",
                shape.id
            ));
        }
        if let LayoutGeometry::Polygon(polygon) = &shape.geometry {
            polygon.validate()?;
        }
        if let LayoutGeometry::AnyAngle(polygon) = &shape.geometry {
            polygon.validate()?;
        }
        if self.shapes.iter().any(|existing| existing.id == shape.id)
            || self.vias.iter().any(|existing| existing.id == shape.id)
        {
            return Err(format!("duplicate layout object id '{}'", shape.id));
        }
        self.shapes.push(shape);
        Ok(())
    }

    pub fn add_via(&mut self, via: LayoutVia) -> Result<(), String> {
        if via.id.trim().is_empty()
            || via.net.trim().is_empty()
            || via.cut_layer.trim().is_empty()
            || via.bottom_layer.trim().is_empty()
            || via.top_layer.trim().is_empty()
        {
            return Err("layout via requires id, net, and layer names".into());
        }
        LayoutRect::new(via.cut.x1, via.cut.y1, via.cut.x2, via.cut.y2)?;
        if self.shapes.iter().any(|existing| existing.id == via.id)
            || self.vias.iter().any(|existing| existing.id == via.id)
        {
            return Err(format!("duplicate layout object id '{}'", via.id));
        }
        self.vias.push(via);
        Ok(())
    }

    pub fn remove_shape(&mut self, id: &str) -> Result<LayoutShape, String> {
        let index = self
            .shapes
            .iter()
            .position(|shape| shape.id == id)
            .ok_or_else(|| format!("unknown layout shape '{}'", id))?;
        Ok(self.shapes.remove(index))
    }

    pub fn remove_via(&mut self, id: &str) -> Result<LayoutVia, String> {
        let index = self
            .vias
            .iter()
            .position(|via| via.id == id)
            .ok_or_else(|| format!("unknown layout via '{}'", id))?;
        Ok(self.vias.remove(index))
    }

    pub fn move_shape(
        &mut self,
        id: &str,
        dx: LayoutCoordinate,
        dy: LayoutCoordinate,
    ) -> Result<(), String> {
        let index = self
            .shapes
            .iter()
            .position(|shape| shape.id == id)
            .ok_or_else(|| format!("unknown layout shape '{}'", id))?;
        self.shapes[index].geometry = match &self.shapes[index].geometry {
            LayoutGeometry::Rect(rect) => {
                let x1 = rect.x1.checked_add(dx).ok_or("layout x overflow")?;
                let x2 = rect.x2.checked_add(dx).ok_or("layout x overflow")?;
                let y1 = rect.y1.checked_add(dy).ok_or("layout y overflow")?;
                let y2 = rect.y2.checked_add(dy).ok_or("layout y overflow")?;
                LayoutGeometry::Rect(LayoutRect::new(x1, y1, x2, y2)?)
            }
            LayoutGeometry::Polygon(polygon) => {
                LayoutGeometry::Polygon(polygon.translated(dx, dy)?)
            }
            LayoutGeometry::AnyAngle(polygon) => {
                LayoutGeometry::AnyAngle(polygon.translated(dx, dy)?)
            }
        };
        Ok(())
    }

    pub fn set_shape_geometry(&mut self, id: &str, geometry: LayoutGeometry) -> Result<(), String> {
        let index = self
            .shapes
            .iter()
            .position(|shape| shape.id == id)
            .ok_or_else(|| format!("unknown layout shape '{}'", id))?;
        if let LayoutGeometry::Polygon(polygon) = &geometry {
            polygon.validate()?;
        }
        if let LayoutGeometry::AnyAngle(polygon) = &geometry {
            polygon.validate()?;
        }
        self.shapes[index].geometry = geometry;
        Ok(())
    }

    pub fn shapes(&self) -> &[LayoutShape] {
        &self.shapes
    }

    pub fn vias(&self) -> &[LayoutVia] {
        &self.vias
    }

    pub fn hit_test(&self, point: LayoutPoint) -> Option<&LayoutShape> {
        self.shapes
            .iter()
            .rev()
            .find(|shape| shape.geometry.contains_point(point))
    }

    pub fn hit_layout_vertex(
        &self,
        point: LayoutPoint,
        radius: LayoutCoordinate,
    ) -> Option<LayoutSelection> {
        if radius < 0 {
            return None;
        }
        self.shapes
            .iter()
            .rev()
            .filter_map(|shape| {
                shape
                    .geometry
                    .vertices()
                    .iter()
                    .position(|vertex| {
                        (vertex.x - point.x).abs() <= radius && (vertex.y - point.y).abs() <= radius
                    })
                    .map(|vertex| LayoutSelection::ShapeVertex {
                        id: shape.id.clone(),
                        vertex,
                    })
            })
            .next()
    }

    pub fn hit_layout(&self, point: LayoutPoint) -> Option<LayoutSelection> {
        if let Some(vertex) = self.hit_layout_vertex(point, 3) {
            return Some(vertex);
        }
        if let Some(via) = self.vias.iter().rev().find(|via| {
            point.x >= via.cut.x1
                && point.x <= via.cut.x2
                && point.y >= via.cut.y1
                && point.y <= via.cut.y2
        }) {
            return Some(LayoutSelection::Via { id: via.id.clone() });
        }
        self.hit_test(point).map(|shape| LayoutSelection::Shape {
            id: shape.id.clone(),
        })
    }

    fn density_violations(&self, layer: &str, rule: &LayoutLayerRule) -> Vec<LayoutDrcViolation> {
        if rule.min_density_bp.is_none() && rule.max_density_bp.is_none() {
            return Vec::new();
        }
        let geometries = self
            .shapes
            .iter()
            .filter(|shape| shape.layer == layer)
            .map(|shape| shape.geometry.clone())
            .collect::<Vec<_>>();
        if geometries.is_empty() {
            return Vec::new();
        }
        let union = match manhattan_union(&geometries) {
            Ok(union) => union,
            Err(error) => {
                return vec![LayoutDrcViolation {
                    code: "LAYOUT.DENSITY_GEOMETRY".into(),
                    severity: DrcSeverity::Error,
                    message: format!("layer '{}' density geometry is invalid: {}", layer, error),
                    layer: Some(layer.into()),
                    net: None,
                    object: Some(layer.into()),
                }]
            }
        };
        if union.rectangles.is_empty() {
            return Vec::new();
        }
        let min_x = union
            .rectangles
            .iter()
            .map(|rect| rect.x1)
            .min()
            .unwrap_or_default();
        let max_x = union
            .rectangles
            .iter()
            .map(|rect| rect.x2)
            .max()
            .unwrap_or_default();
        let min_y = union
            .rectangles
            .iter()
            .map(|rect| rect.y1)
            .min()
            .unwrap_or_default();
        let max_y = union
            .rectangles
            .iter()
            .map(|rect| rect.y2)
            .max()
            .unwrap_or_default();
        let center_x = min_x.saturating_add((max_x - min_x) / 2);
        let center_y = min_y.saturating_add((max_y - min_y) / 2);
        let mut x_positions = vec![center_x - rule.density_window / 2];
        let mut y_positions = vec![center_y - rule.density_window / 2];
        for rect in &union.rectangles {
            x_positions.extend([
                rect.x1,
                rect.x2,
                rect.x1.checked_sub(rule.density_window).unwrap_or(rect.x1),
                rect.x2.checked_sub(rule.density_window).unwrap_or(rect.x2),
            ]);
            y_positions.extend([
                rect.y1,
                rect.y2,
                rect.y1.checked_sub(rule.density_window).unwrap_or(rect.y1),
                rect.y2.checked_sub(rule.density_window).unwrap_or(rect.y2),
            ]);
        }
        x_positions.sort_unstable();
        x_positions.dedup();
        y_positions.sort_unstable();
        y_positions.dedup();

        let mut violations = Vec::new();
        let mut evaluated = BTreeSet::new();
        for x in x_positions {
            for y in &y_positions {
                if !evaluated.insert((x, *y)) {
                    continue;
                }
                let Some(x2) = x.checked_add(rule.density_window) else {
                    continue;
                };
                let Some(y2) = y.checked_add(rule.density_window) else {
                    continue;
                };
                let Ok(window) = LayoutRect::new(x, *y, x2, y2) else {
                    continue;
                };
                if window.x2 <= min_x
                    || window.x1 >= max_x
                    || window.y2 <= min_y
                    || window.y1 >= max_y
                {
                    continue;
                }
                violations.extend(Self::density_window_violations(
                    layer,
                    rule,
                    &union.rectangles,
                    window,
                ));
            }
        }
        if evaluated.is_empty() {
            return vec![LayoutDrcViolation {
                code: "LAYOUT.DENSITY_WINDOW".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "layer '{}' density window {} is invalid",
                    layer, rule.density_window
                ),
                layer: Some(layer.into()),
                net: None,
                object: Some(layer.into()),
            }];
        }
        violations
    }

    fn density_window_violations(
        layer: &str,
        rule: &LayoutLayerRule,
        rectangles: &[LayoutRect],
        window: LayoutRect,
    ) -> Vec<LayoutDrcViolation> {
        let covered = rectangles
            .iter()
            .map(|rect| {
                let x1 = i128::from(rect.x1.max(window.x1));
                let x2 = i128::from(rect.x2.min(window.x2));
                let y1 = i128::from(rect.y1.max(window.y1));
                let y2 = i128::from(rect.y2.min(window.y2));
                (x2 - x1).max(0) * (y2 - y1).max(0)
            })
            .sum::<i128>();
        let window_area = i128::from(window.width()) * i128::from(window.height());
        if window_area == 0 {
            return Vec::new();
        }
        let density_bp = (covered * 10_000) / window_area;
        let density_percent = f64::from(density_bp.clamp(0, 10_000) as u32) / 100.0;
        let mut violations = Vec::new();
        if rule
            .min_density_bp
            .is_some_and(|required| density_bp < i128::from(required))
        {
            violations.push(LayoutDrcViolation {
                code: "LAYOUT.MIN_DENSITY".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "layer '{}' density {:.3}% at ({},{}) is below required {:.3}% in {}x{} window",
                    layer,
                    density_percent,
                    window.x1,
                    window.y1,
                    f64::from(rule.min_density_bp.unwrap_or(0)) / 100.0,
                    window.width(),
                    window.height()
                ),
                layer: Some(layer.into()),
                net: None,
                object: Some(layer.into()),
            });
        }
        if rule
            .max_density_bp
            .is_some_and(|limit| density_bp > i128::from(limit))
        {
            violations.push(LayoutDrcViolation {
                code: "LAYOUT.MAX_DENSITY".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "layer '{}' density {:.3}% at ({},{}) exceeds limit {:.3}% in {}x{} window",
                    layer,
                    density_percent,
                    window.x1,
                    window.y1,
                    f64::from(rule.max_density_bp.unwrap_or(10_000)) / 100.0,
                    window.width(),
                    window.height()
                ),
                layer: Some(layer.into()),
                net: None,
                object: Some(layer.into()),
            });
        }
        violations
    }

    pub fn check_layout_drc(&self) -> LayoutDrcResult {
        let mut violations = Vec::new();
        for (layer, rule) in &self.layer_rules {
            violations.extend(self.density_violations(layer, rule));
        }
        for shape in &self.shapes {
            let Some(rule) = self.layer_rules.get(&shape.layer) else {
                violations.push(LayoutDrcViolation {
                    code: "LAYOUT.UNKNOWN_LAYER".into(),
                    severity: DrcSeverity::Error,
                    message: format!("shape '{}' uses ruleless layer '{}'", shape.id, shape.layer),
                    layer: Some(shape.layer.clone()),
                    net: Some(shape.net.clone()),
                    object: Some(shape.id.clone()),
                });
                continue;
            };
            let (actual_width, width_below_rule) = match &shape.geometry {
                LayoutGeometry::Rect(rect) => {
                    let width = rect.width().min(rect.height());
                    (width as f64, width < rule.min_width)
                }
                LayoutGeometry::Polygon(polygon) => {
                    let width = polygon
                        .segments()
                        .map(|(a, b)| (a.x - b.x).abs().max((a.y - b.y).abs()))
                        .min()
                        .unwrap_or(0);
                    (width as f64, width < rule.min_width)
                }
                LayoutGeometry::AnyAngle(polygon) => {
                    let minimum_edge_squared = polygon
                        .edge_lengths_squared()
                        .into_iter()
                        .min()
                        .unwrap_or(0);
                    let required_squared = u128::try_from(rule.min_width)
                        .ok()
                        .and_then(|width| width.checked_mul(width))
                        .unwrap_or(u128::MAX);
                    (
                        (minimum_edge_squared as f64).sqrt(),
                        minimum_edge_squared < required_squared,
                    )
                }
            };
            if width_below_rule {
                violations.push(LayoutDrcViolation {
                    code: "LAYOUT.MIN_WIDTH".into(),
                    severity: DrcSeverity::Error,
                    message: format!(
                        "shape '{}' edge width {:.3} is below layer minimum {}",
                        shape.id, actual_width, rule.min_width
                    ),
                    layer: Some(shape.layer.clone()),
                    net: Some(shape.net.clone()),
                    object: Some(shape.id.clone()),
                });
            }
            match shape.geometry.area2() {
                Err(error) => violations.push(LayoutDrcViolation {
                    code: "LAYOUT.GEOMETRY".into(),
                    severity: DrcSeverity::Error,
                    message: format!("shape '{}' has invalid area: {}", shape.id, error),
                    layer: Some(shape.layer.clone()),
                    net: Some(shape.net.clone()),
                    object: Some(shape.id.clone()),
                }),
                Ok(area2) if area2 < i128::from(rule.min_area) * 2 => {
                    violations.push(LayoutDrcViolation {
                        code: "LAYOUT.MIN_AREA".into(),
                        severity: DrcSeverity::Error,
                        message: format!(
                            "shape '{}' area {} is below layer minimum {}",
                            shape.id,
                            area2 / 2,
                            rule.min_area
                        ),
                        layer: Some(shape.layer.clone()),
                        net: Some(shape.net.clone()),
                        object: Some(shape.id.clone()),
                    });
                }
                Ok(_) => {}
            }
            if rule.min_notch > 0 {
                match &shape.geometry {
                    LayoutGeometry::Rect(_) => {}
                    LayoutGeometry::Polygon(polygon) => {
                        if polygon
                            .minimum_notch_width()
                            .is_some_and(|notch| notch < rule.min_notch)
                        {
                            violations.push(LayoutDrcViolation {
                                code: "LAYOUT.MIN_NOTCH".into(),
                                severity: DrcSeverity::Error,
                                message: format!(
                                    "shape '{}' notch {} is below layer minimum {}",
                                    shape.id,
                                    polygon.minimum_notch_width().unwrap_or_default(),
                                    rule.min_notch
                                ),
                                layer: Some(shape.layer.clone()),
                                net: Some(shape.net.clone()),
                                object: Some(shape.id.clone()),
                            });
                        }
                    }
                    LayoutGeometry::AnyAngle(_) => {
                        violations.push(LayoutDrcViolation {
                            code: "LAYOUT.NOTCH_GEOMETRY".into(),
                            severity: DrcSeverity::Error,
                            message: format!(
                                "shape '{}' uses an arbitrary-angle polygon that does not yet support notch DRC",
                                shape.id
                            ),
                            layer: Some(shape.layer.clone()),
                            net: Some(shape.net.clone()),
                            object: Some(shape.id.clone()),
                        });
                    }
                }
            }
        }

        for (index, left) in self.shapes.iter().enumerate() {
            for right in self.shapes.iter().skip(index + 1) {
                if left.net == right.net {
                    continue;
                }
                if left.layer == right.layer {
                    match positive_area_overlap(&left.geometry, &right.geometry) {
                        Ok(true) => {
                            violations.push(LayoutDrcViolation {
                                code: "LAYOUT.SHORT".into(),
                                severity: DrcSeverity::Error,
                                message: format!(
                                    "different-net shapes '{}' and '{}' overlap on layer '{}'",
                                    left.id, right.id, left.layer
                                ),
                                layer: Some(left.layer.clone()),
                                net: Some(left.net.clone()),
                                object: Some(format!("{}|{}", left.id, right.id)),
                            });
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            violations.push(LayoutDrcViolation {
                                code: "LAYOUT.GEOMETRY".into(),
                                severity: DrcSeverity::Error,
                                message: format!(
                                    "shapes '{}' and '{}' overlap check failed: {}",
                                    left.id, right.id, error
                                ),
                                layer: Some(left.layer.clone()),
                                net: Some(left.net.clone()),
                                object: Some(format!("{}|{}", left.id, right.id)),
                            });
                            continue;
                        }
                    }
                }
                let required = if left.layer == right.layer {
                    self.layer_rules
                        .get(&left.layer)
                        .map(|rule| rule.min_spacing)
                } else {
                    let left_to_right = self
                        .layer_rules
                        .get(&left.layer)
                        .and_then(|rule| rule.spacing_to.get(&right.layer).copied());
                    let right_to_left = self
                        .layer_rules
                        .get(&right.layer)
                        .and_then(|rule| rule.spacing_to.get(&left.layer).copied());
                    [left_to_right, right_to_left].into_iter().flatten().max()
                };
                let Some(required) = required else {
                    continue;
                };
                let spacing = left.geometry.check_spacing_to(&right.geometry, required);
                let violation = match spacing {
                    Ok(result) if !result.compliant => Some(Ok(result.actual_distance_dbu)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                };
                if let Some(distance) = violation {
                    violations.push(LayoutDrcViolation {
                        code: "LAYOUT.SPACING".into(),
                        severity: DrcSeverity::Error,
                        message: format!(
                            "shapes '{}' and '{}' distance {} is below layer minimum {}",
                            left.id,
                            right.id,
                            distance
                                .as_ref()
                                .map(|value| format!("{value:.3}"))
                                .unwrap_or_else(|error| format!("invalid: {error}")),
                            required
                        ),
                        layer: Some(left.layer.clone()),
                        net: Some(left.net.clone()),
                        object: Some(format!("{}|{}", left.id, right.id)),
                    });
                }
            }
        }

        for via in &self.vias {
            let Some(rule) = self.via_rules.get(&via.rule) else {
                violations.push(LayoutDrcViolation {
                    code: "LAYOUT.UNKNOWN_VIA".into(),
                    severity: DrcSeverity::Error,
                    message: format!("via '{}' uses unknown rule '{}'", via.id, via.rule),
                    layer: Some(via.cut_layer.clone()),
                    net: Some(via.net.clone()),
                    object: Some(via.id.clone()),
                });
                continue;
            };
            if rule.cut_layer != via.cut_layer
                || rule.bottom_layer != via.bottom_layer
                || rule.top_layer != via.top_layer
            {
                violations.push(LayoutDrcViolation {
                    code: "LAYOUT.VIA_RULE_MISMATCH".into(),
                    severity: DrcSeverity::Error,
                    message: format!("via '{}' does not match rule '{}'", via.id, via.rule),
                    layer: Some(via.cut_layer.clone()),
                    net: Some(via.net.clone()),
                    object: Some(via.id.clone()),
                });
            }
            if via.cut.width() != rule.cut_width || via.cut.height() != rule.cut_height {
                violations.push(LayoutDrcViolation {
                    code: "LAYOUT.VIA_CUT_SIZE".into(),
                    severity: DrcSeverity::Error,
                    message: format!("via '{}' has an illegal cut size", via.id),
                    layer: Some(via.cut_layer.clone()),
                    net: Some(via.net.clone()),
                    object: Some(via.id.clone()),
                });
            }
            for (layer, enclosure, extension_x, extension_y) in [
                (
                    &rule.bottom_layer,
                    rule.min_bottom_enclosure,
                    rule.min_bottom_extension_x,
                    rule.min_bottom_extension_y,
                ),
                (
                    &rule.top_layer,
                    rule.min_top_enclosure,
                    rule.min_top_extension_x,
                    rule.min_top_extension_y,
                ),
            ] {
                let dx = enclosure.max(extension_x);
                let dy = enclosure.max(extension_y);
                let required = via.cut.expanded_by(dx, dy).unwrap_or(via.cut);
                let enclosed = self.shapes.iter().any(|shape| {
                    shape.net == via.net
                        && shape.layer == *layer
                        && shape.geometry.contains(&LayoutGeometry::Rect(required))
                });
                if !enclosed {
                    violations.push(LayoutDrcViolation {
                        code: "LAYOUT.VIA_ENCLOSURE".into(),
                        severity: DrcSeverity::Error,
                        message: format!(
                            "via '{}' lacks enclosure/extension ({}x{}, {}x{}) on layer '{}'",
                            via.id, enclosure, extension_x, enclosure, extension_y, layer
                        ),
                        layer: Some(layer.clone()),
                        net: Some(via.net.clone()),
                        object: Some(via.id.clone()),
                    });
                }
            }
        }

        for (index, left) in self.vias.iter().enumerate() {
            for right in self.vias.iter().skip(index + 1) {
                if left.cut_layer != right.cut_layer {
                    continue;
                }
                let cuts_overlap = left.cut.x1 < right.cut.x2
                    && right.cut.x1 < left.cut.x2
                    && left.cut.y1 < right.cut.y2
                    && right.cut.y1 < left.cut.y2;
                if cuts_overlap {
                    violations.push(LayoutDrcViolation {
                        code: "LAYOUT.VIA_CUT_OVERLAP".into(),
                        severity: DrcSeverity::Error,
                        message: format!(
                            "via cuts '{}' and '{}' overlap on layer '{}'",
                            left.id, right.id, left.cut_layer
                        ),
                        layer: Some(left.cut_layer.clone()),
                        net: Some(left.net.clone()),
                        object: Some(format!("{}|{}", left.id, right.id)),
                    });
                    continue;
                }
                let required = [
                    self.via_rules.get(&left.rule),
                    self.via_rules.get(&right.rule),
                ]
                .into_iter()
                .flatten()
                .map(|rule| rule.min_cut_spacing)
                .max()
                .unwrap_or(0);
                if required == 0 {
                    continue;
                }
                let spacing = LayoutGeometry::Rect(left.cut)
                    .check_spacing_to(&LayoutGeometry::Rect(right.cut), required);
                let violation = match spacing {
                    Ok(result) if !result.compliant => Some(Ok(result.actual_distance_dbu)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                };
                if let Some(distance) = violation {
                    violations.push(LayoutDrcViolation {
                        code: "LAYOUT.VIA_CUT_SPACING".into(),
                        severity: DrcSeverity::Error,
                        message: format!(
                            "via cuts '{}' and '{}' distance {} are below minimum {}",
                            left.id,
                            right.id,
                            distance
                                .as_ref()
                                .map(|value| format!("{value:.3}"))
                                .unwrap_or_else(|error| format!("invalid: {error}")),
                            required
                        ),
                        layer: Some(left.cut_layer.clone()),
                        net: Some(left.net.clone()),
                        object: Some(format!("{}|{}", left.id, right.id)),
                    });
                }
            }
        }
        LayoutDrcResult { violations }
    }

    pub fn suggest_layout_fixes(&self) -> Vec<LayoutDrcSuggestion> {
        let mut suggestions = Vec::new();
        for shape in &self.shapes {
            let Some(rule) = self.layer_rules.get(&shape.layer) else {
                continue;
            };
            let LayoutGeometry::Rect(rect) = &shape.geometry else {
                continue;
            };
            let mut replacement = *rect;
            if replacement.width() < rule.min_width {
                replacement.x2 += rule.min_width - replacement.width();
            }
            if replacement.height() < rule.min_width {
                replacement.y2 += rule.min_width - replacement.height();
            }
            if replacement.area() < rule.min_area {
                let deficit = rule.min_area - replacement.area();
                let height = replacement.height().max(1);
                let grow = (deficit + height - 1) / height;
                replacement.x2 += grow.max(1);
            }
            if replacement != *rect {
                suggestions.push(LayoutDrcSuggestion {
                    code: "LAYOUT.GROW_SHAPE".into(),
                    message: format!(
                        "grow shape '{}' to satisfy layer '{}' minimum geometry",
                        shape.id, shape.layer
                    ),
                    edit: LayoutEdit::SetShapeGeometry {
                        id: shape.id.clone(),
                        geometry: LayoutGeometry::Rect(replacement),
                    },
                });
            }
        }

        for via in &self.vias {
            let Some(rule) = self.via_rules.get(&via.rule) else {
                continue;
            };
            for (layer, enclosure) in [
                (&rule.bottom_layer, rule.min_bottom_enclosure),
                (&rule.top_layer, rule.min_top_enclosure),
            ] {
                let Ok(required) = via.cut.expanded(enclosure) else {
                    continue;
                };
                let enclosed = self.shapes.iter().any(|shape| {
                    shape.net == via.net
                        && shape.layer == *layer
                        && shape.geometry.contains(&LayoutGeometry::Rect(required))
                });
                if enclosed {
                    continue;
                }
                suggestions.push(LayoutDrcSuggestion {
                    code: "LAYOUT.ADD_VIA_ENCLOSURE".into(),
                    message: format!(
                        "add {}-unit enclosure for via '{}' on layer '{}'",
                        enclosure, via.id, layer
                    ),
                    edit: LayoutEdit::AddShape(LayoutShape {
                        id: format!("{}_{}_enclosure", via.id, layer),
                        layer: layer.clone(),
                        net: via.net.clone(),
                        geometry: LayoutGeometry::Rect(required),
                    }),
                });
            }
        }

        for (index, left) in self.shapes.iter().enumerate() {
            for right in self.shapes.iter().skip(index + 1) {
                if left.layer != right.layer || left.net == right.net {
                    continue;
                }
                let Some(rule) = self.layer_rules.get(&left.layer) else {
                    continue;
                };
                match left
                    .geometry
                    .check_spacing_to(&right.geometry, rule.min_spacing)
                {
                    Ok(result) if result.compliant => continue,
                    Ok(_) => {}
                    Err(_) => continue,
                }
                let left_bbox = left.geometry.bbox();
                let right_bbox = right.geometry.bbox();
                let required_x = left_bbox.x2 + rule.min_spacing;
                let dx = (required_x - right_bbox.x1).max(0);
                suggestions.push(LayoutDrcSuggestion {
                    code: "LAYOUT.MOVE_FOR_SPACING".into(),
                    message: format!(
                        "move shape '{}' {} units right to clear shape '{}'",
                        right.id, dx, left.id
                    ),
                    edit: LayoutEdit::MoveShape {
                        id: right.id.clone(),
                        dx,
                        dy: 0,
                    },
                });
            }
        }
        suggestions
    }

    pub fn write_svg(&self) -> Result<String, String> {
        self.validate_layout_ids()?;
        let mut x1 = 0_i64;
        let mut y1 = 0_i64;
        let mut x2 = 100_i64;
        let mut y2 = 100_i64;
        if !self.shapes.is_empty() || !self.vias.is_empty() {
            let mut boxes = Vec::new();
            boxes.extend(self.shapes.iter().map(|shape| shape.geometry.bbox()));
            boxes.extend(self.vias.iter().map(|via| via.cut));
            x1 = boxes.iter().map(|rect| rect.x1).min().unwrap_or_default();
            y1 = boxes.iter().map(|rect| rect.y1).min().unwrap_or_default();
            x2 = boxes.iter().map(|rect| rect.x2).max().unwrap_or_default();
            y2 = boxes.iter().map(|rect| rect.y2).max().unwrap_or_default();
        }
        let margin = ((x2 - x1).max(y2 - y1) / 20).max(1);
        x1 -= margin;
        y1 -= margin;
        x2 += margin;
        y2 += margin;
        let mut out = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" width=\"100%\" height=\"600\">\n",
            x1,
            y1,
            (x2 - x1).max(1),
            (y2 - y1).max(1)
        );
        for (index, shape) in self.shapes.iter().enumerate() {
            let bbox = shape.geometry.bbox();
            let color = 30 + (index * 47) % 240;
            match &shape.geometry {
                LayoutGeometry::Rect(_) => out.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"hsl({},70%,55%)\" fill-opacity=\"0.45\" stroke=\"hsl({},70%,35%)\" stroke-width=\"1\"/>\n",
                    bbox.x1,
                    bbox.y1,
                    bbox.width(),
                    bbox.height(),
                    color,
                    color
                )),
                LayoutGeometry::Polygon(polygon) => {
                    let points = polygon
                        .points
                        .iter()
                        .map(|point| format!("{},{}", point.x, point.y))
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push_str(&format!(
                        "<polygon points=\"{}\" fill=\"hsl({},70%,55%)\" fill-opacity=\"0.45\" stroke=\"hsl({},70%,35%)\" stroke-width=\"1\"/>\n",
                        points, color, color
                    ));
                }
                LayoutGeometry::AnyAngle(polygon) => {
                    let points = polygon
                        .points()
                        .iter()
                        .map(|point| format!("{},{}", point.x, point.y))
                        .collect::<Vec<_>>()
                        .join(" ");
                    out.push_str(&format!(
                        "<polygon points=\"{}\" fill=\"hsl({},70%,55%)\" fill-opacity=\"0.45\" stroke=\"hsl({},70%,35%)\" stroke-width=\"1\"/>\n",
                        points, color, color
                    ));
                }
            }
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"#111827\">{}</text>\n",
                bbox.x1,
                bbox.y1.saturating_sub(2),
                ((x2 - x1).max(1) / 30).max(4),
                xml_escape(&format!("{} / {}", shape.net, shape.id))
            ));
        }
        for via in &self.vias {
            out.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#111827\" fill-opacity=\"0.75\" stroke=\"#000\" stroke-dasharray=\"4 2\"/>\n",
                via.cut.x1,
                via.cut.y1,
                via.cut.width(),
                via.cut.height()
            ));
        }
        out.push_str("</svg>\n");
        Ok(out)
    }

    fn validate_layout_ids(&self) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();
        for shape in &self.shapes {
            if !ids.insert(shape.id.as_str()) {
                return Err(format!("duplicate layout shape id '{}'", shape.id));
            }
        }
        for via in &self.vias {
            if !ids.insert(via.id.as_str()) {
                return Err(format!("duplicate layout via id '{}'", via.id));
            }
        }
        Ok(())
    }
}
