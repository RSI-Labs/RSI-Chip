//! Polygon- and via-level layout database with deterministic editing and DRC.
//!
//! The database uses signed database-unit coordinates. Polygons are required
//! to be simple and rectilinear, which covers Manhattan routing shapes while
//! keeping geometry, hit testing, spacing, enclosure, and intersection checks
//! exact and dependency-free.

use crate::any_angle::{any_angle_boolean, AnyAngleBooleanOperation, AnyAnglePolygon};
use crate::DrcSeverity;
use std::collections::{BTreeMap, BTreeSet};

pub type LayoutCoordinate = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayoutPoint {
    pub x: LayoutCoordinate,
    pub y: LayoutCoordinate,
}

impl LayoutPoint {
    pub fn new(x: LayoutCoordinate, y: LayoutCoordinate) -> Self {
        Self { x, y }
    }

    pub fn translated(self, dx: LayoutCoordinate, dy: LayoutCoordinate) -> Self {
        Self::new(
            self.x.checked_add(dx).expect("layout x overflow"),
            self.y.checked_add(dy).expect("layout y overflow"),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutRect {
    pub x1: LayoutCoordinate,
    pub y1: LayoutCoordinate,
    pub x2: LayoutCoordinate,
    pub y2: LayoutCoordinate,
}

impl LayoutRect {
    pub fn new(
        x1: LayoutCoordinate,
        y1: LayoutCoordinate,
        x2: LayoutCoordinate,
        y2: LayoutCoordinate,
    ) -> Result<Self, String> {
        if x1 == x2 || y1 == y2 {
            return Err("layout rectangle must have positive width and height".into());
        }
        Ok(Self {
            x1: x1.min(x2),
            y1: y1.min(y2),
            x2: x1.max(x2),
            y2: y1.max(y2),
        })
    }

    pub fn width(self) -> i64 {
        self.x2.saturating_sub(self.x1)
    }

    pub fn height(self) -> i64 {
        self.y2.saturating_sub(self.y1)
    }

    pub fn area(self) -> i64 {
        self.width().saturating_mul(self.height())
    }

    pub fn bbox(self) -> Self {
        self
    }

    pub fn expanded(self, margin: i64) -> Result<Self, String> {
        if margin < 0 {
            return Err("layout expansion margin cannot be negative".into());
        }
        Self::new(
            self.x1 - margin,
            self.y1 - margin,
            self.x2 + margin,
            self.y2 + margin,
        )
    }

    pub fn expanded_by(self, dx: i64, dy: i64) -> Result<Self, String> {
        if dx < 0 || dy < 0 {
            return Err("layout expansion margins cannot be negative".into());
        }
        Self::new(self.x1 - dx, self.y1 - dy, self.x2 + dx, self.y2 + dy)
    }

    pub fn vertices(self) -> [LayoutPoint; 4] {
        [
            LayoutPoint::new(self.x1, self.y1),
            LayoutPoint::new(self.x2, self.y1),
            LayoutPoint::new(self.x2, self.y2),
            LayoutPoint::new(self.x1, self.y2),
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutPolygon {
    pub points: Vec<LayoutPoint>,
}

impl LayoutPolygon {
    pub fn new(points: impl Into<Vec<LayoutPoint>>) -> Result<Self, String> {
        let out = Self {
            points: points.into(),
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.points.len() < 4 {
            return Err("layout polygon needs at least four vertices".into());
        }
        if self.points.first() == self.points.last() {
            return Err("layout polygon must omit the repeated closing vertex".into());
        }
        for pair in self.points.windows(2) {
            if pair[0] == pair[1] {
                return Err("layout polygon has duplicate consecutive vertices".into());
            }
            let horizontal = pair[0].y == pair[1].y;
            let vertical = pair[0].x == pair[1].x;
            if !horizontal && !vertical {
                return Err("layout polygon must be rectilinear".into());
            }
        }
        let last = *self.points.last().expect("nonempty polygon");
        let first = self.points[0];
        if !(last.y == first.y || last.x == first.x) {
            return Err("layout polygon closing edge must be rectilinear".into());
        }
        if last == first {
            return Err("layout polygon closing vertex cannot repeat first vertex".into());
        }
        if self
            .segments()
            .any(|(a, b)| a == b || ((a.x != b.x) && (a.y != b.y)))
        {
            return Err("layout polygon has a non-rectilinear edge".into());
        }
        if self.has_nonadjacent_edge_intersection() {
            return Err("layout polygon is self-intersecting".into());
        }
        if self.area() == 0 {
            return Err("layout polygon has zero area".into());
        }
        Ok(())
    }

    fn segments(&self) -> impl Iterator<Item = (LayoutPoint, LayoutPoint)> + '_ {
        self.points
            .iter()
            .copied()
            .zip(self.points.iter().cycle().skip(1).copied())
    }

    fn has_nonadjacent_edge_intersection(&self) -> bool {
        let segments: Vec<_> = self.segments().collect();
        for (i, &(a, b)) in segments.iter().enumerate() {
            for (j, &(c, d)) in segments.iter().enumerate().skip(i + 2) {
                let adjacent = i == 0 && j + 1 == segments.len();
                if !adjacent && segments_intersect(a, b, c, d) {
                    return true;
                }
            }
        }
        false
    }

    pub fn area(&self) -> i64 {
        let mut twice_area = 0_i64;
        for (a, b) in self.segments() {
            twice_area +=
                a.x.saturating_mul(b.y)
                    .saturating_sub(b.x.saturating_mul(a.y));
        }
        twice_area.unsigned_abs() as i64 / 2
    }

    pub fn bbox(&self) -> LayoutRect {
        let xs = self.points.iter().map(|point| point.x);
        let ys = self.points.iter().map(|point| point.y);
        LayoutRect {
            x1: xs.clone().min().unwrap_or_default(),
            y1: ys.clone().min().unwrap_or_default(),
            x2: xs.max().unwrap_or_default(),
            y2: ys.max().unwrap_or_default(),
        }
    }

    pub fn translated(&self, dx: LayoutCoordinate, dy: LayoutCoordinate) -> Result<Self, String> {
        Self::new(
            self.points
                .iter()
                .map(|point| point.translated(dx, dy))
                .collect::<Vec<_>>(),
        )
    }

    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        let mut inside = false;
        let mut previous = *self.points.last().expect("nonempty polygon");
        for current in &self.points {
            let (a, b) = (previous, *current);
            if (a.y > point.y) != (b.y > point.y) {
                let x_intersection = a.x + (point.y - a.y).saturating_mul(b.x - a.x) / (b.y - a.y);
                if point.x < x_intersection {
                    inside = !inside;
                }
            }
            previous = *current;
        }
        inside
    }

    /// Return the narrowest concave notch opening on a horizontal slice.
    ///
    /// The polygon database is simply connected and rectilinear. A scanline
    /// between each adjacent pair of horizontal edge coordinates yields
    /// disjoint covered intervals; gaps between those intervals are notch
    /// openings. Exterior space to the left/right of the outermost intervals is
    /// intentionally ignored.
    pub fn minimum_notch_width(&self) -> Option<i64> {
        let mut ys = self.points.iter().map(|point| point.y).collect::<Vec<_>>();
        ys.sort_unstable();
        ys.dedup();
        let mut minimum = None::<i64>;
        for pair in ys.windows(2) {
            let scan_y2 = i128::from(pair[0]) + i128::from(pair[1]);
            let mut crossings = Vec::new();
            for (a, b) in self.segments() {
                if a.x != b.x {
                    continue;
                }
                let a_y2 = i128::from(a.y) * 2;
                let b_y2 = i128::from(b.y) * 2;
                if (a_y2 < scan_y2 && b_y2 > scan_y2) || (b_y2 < scan_y2 && a_y2 > scan_y2) {
                    crossings.push(a.x);
                }
            }
            crossings.sort_unstable();
            crossings.dedup();
            if crossings.len() >= 4 {
                for interval_index in 1..crossings.len() / 2 {
                    let opening = crossings[interval_index * 2]
                        .saturating_sub(crossings[2 * interval_index - 1]);
                    if opening > 0 {
                        minimum = Some(minimum.map_or(opening, |current| current.min(opening)));
                    }
                }
            }
        }
        minimum
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutGeometry {
    Rect(LayoutRect),
    Polygon(LayoutPolygon),
    AnyAngle(AnyAnglePolygon),
}

impl LayoutGeometry {
    pub fn bbox(&self) -> LayoutRect {
        match self {
            Self::Rect(rect) => *rect,
            Self::Polygon(polygon) => polygon.bbox(),
            Self::AnyAngle(polygon) => polygon.bbox(),
        }
    }

    pub fn area(&self) -> i64 {
        match self {
            Self::Rect(rect) => rect.area(),
            Self::Polygon(polygon) => polygon.area(),
            Self::AnyAngle(polygon) => polygon.area().map(|area| area as i64).unwrap_or(i64::MAX),
        }
    }

    pub fn area2(&self) -> Result<i128, String> {
        Ok(match self {
            Self::Rect(rect) => i128::from(rect.area()) * 2,
            Self::Polygon(polygon) => i128::from(polygon.area()) * 2,
            Self::AnyAngle(polygon) => {
                let area2 = polygon.signed_area2()?;
                if area2 >= 0 {
                    area2
                } else if area2 == i128::MIN {
                    return Err("layout polygon area overflow".into());
                } else {
                    -area2
                }
            }
        })
    }

    pub fn try_into_any_angle(&self) -> Result<AnyAnglePolygon, String> {
        match self {
            Self::Rect(rect) => AnyAnglePolygon::new(rect.vertices()),
            Self::Polygon(polygon) => AnyAnglePolygon::new(polygon.points.clone()),
            Self::AnyAngle(polygon) => Ok(polygon.clone()),
        }
    }

    pub fn vertices(&self) -> Vec<LayoutPoint> {
        match self {
            Self::Rect(rect) => rect.vertices().to_vec(),
            Self::Polygon(polygon) => polygon.points.clone(),
            Self::AnyAngle(polygon) => polygon.points().to_vec(),
        }
    }

    pub fn segments(&self) -> Vec<(LayoutPoint, LayoutPoint)> {
        match self {
            Self::Rect(rect) => {
                let vertices = rect.vertices();
                vec![
                    (vertices[0], vertices[1]),
                    (vertices[1], vertices[2]),
                    (vertices[2], vertices[3]),
                    (vertices[3], vertices[0]),
                ]
            }
            Self::Polygon(polygon) => polygon.segments().collect(),
            Self::AnyAngle(polygon) => polygon
                .points()
                .iter()
                .copied()
                .zip(polygon.points().iter().cycle().skip(1).copied())
                .collect(),
        }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        if matches!(self, Self::AnyAngle(_)) || matches!(other, Self::AnyAngle(_)) {
            return self
                .try_into_any_angle()
                .and_then(|left| Ok((left, other.try_into_any_angle()?)))
                .map(|(left, right)| left.intersects(&right))
                .unwrap_or(true);
        }
        if !bbox_intersects(self.bbox(), other.bbox()) {
            return false;
        }
        for (a, b) in self.segments() {
            for (c, d) in other.segments() {
                if segments_intersect(a, b, c, d) {
                    return true;
                }
            }
        }
        other
            .vertices()
            .iter()
            .any(|point| self.contains_point(*point))
            || self
                .vertices()
                .iter()
                .any(|point| other.contains_point(*point))
    }

    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        match self {
            Self::Rect(rect) => {
                point.x >= rect.x1 && point.x <= rect.x2 && point.y >= rect.y1 && point.y <= rect.y2
            }
            Self::Polygon(polygon) => polygon.contains_point(point),
            Self::AnyAngle(polygon) => polygon.contains_point(point),
        }
    }

    pub fn contains(&self, other: &Self) -> bool {
        let outer = self.bbox();
        let inner = other.bbox();
        outer.x1 <= inner.x1
            && outer.y1 <= inner.y1
            && outer.x2 >= inner.x2
            && outer.y2 >= inner.y2
            && other
                .vertices()
                .iter()
                .all(|point| self.contains_point(*point))
            && !self.segments().iter().any(|(a, b)| {
                other
                    .segments()
                    .iter()
                    .any(|(c, d)| segments_properly_cross(*a, *b, *c, *d))
            })
    }

    pub fn distance(&self, other: &Self) -> f64 {
        self.try_into_any_angle()
            .and_then(|left| Ok((left, other.try_into_any_angle()?)))
            .and_then(|(left, right)| Ok(left.distance_squared_to(&right)?.sqrt()))
            .unwrap_or(f64::INFINITY)
    }

    pub fn check_spacing_to(
        &self,
        other: &Self,
        required_dbu: i64,
    ) -> Result<crate::any_angle::AnyAngleSpacingResult, String> {
        let left = self.try_into_any_angle()?;
        let right = other.try_into_any_angle()?;
        left.check_spacing_to(&right, required_dbu)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutShape {
    pub id: String,
    pub layer: String,
    pub net: String,
    pub geometry: LayoutGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutVia {
    pub id: String,
    pub rule: String,
    pub net: String,
    pub cut_layer: String,
    pub bottom_layer: String,
    pub top_layer: String,
    pub cut: LayoutRect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLayerRule {
    pub layer: String,
    pub min_width: i64,
    pub min_spacing: i64,
    pub min_area: i64,
    /// Minimum layer density in basis points (0..=10_000).
    pub min_density_bp: Option<u32>,
    /// Maximum layer density in basis points (0..=10_000).
    pub max_density_bp: Option<u32>,
    /// Side length of the square density evaluation window in DBU.
    pub density_window: i64,
    /// Minimum concave notch opening for a rectilinear polygon, in DBU.
    pub min_notch: i64,
    /// Minimum spacing from this layer to another layer for different nets.
    pub spacing_to: BTreeMap<String, i64>,
}

impl LayoutLayerRule {
    pub fn new(
        layer: impl Into<String>,
        min_width: i64,
        min_spacing: i64,
        min_area: i64,
    ) -> Result<Self, String> {
        let out = Self {
            layer: layer.into(),
            min_width,
            min_spacing,
            min_area,
            min_density_bp: None,
            max_density_bp: None,
            density_window: 1_000,
            min_notch: 0,
            spacing_to: BTreeMap::new(),
        };
        out.validate()?;
        Ok(out)
    }

    pub fn with_density(
        mut self,
        min_density_bp: Option<u32>,
        max_density_bp: Option<u32>,
        density_window: i64,
    ) -> Result<Self, String> {
        self.min_density_bp = min_density_bp;
        self.max_density_bp = max_density_bp;
        self.density_window = density_window;
        self.validate()?;
        Ok(self)
    }

    pub fn with_notch(mut self, min_notch: i64) -> Result<Self, String> {
        self.min_notch = min_notch;
        self.validate()?;
        Ok(self)
    }

    pub fn with_spacing_to(
        mut self,
        other_layer: impl Into<String>,
        min_spacing: i64,
    ) -> Result<Self, String> {
        self.spacing_to.insert(other_layer.into(), min_spacing);
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.layer.trim().is_empty() {
            return Err("layout layer name cannot be empty".into());
        }
        if self.min_width <= 0 || self.min_spacing < 0 || self.min_area < 0 {
            return Err(format!("invalid layout rule for layer '{}'", self.layer));
        }
        if self.min_notch < 0 {
            return Err(format!(
                "layout rule for layer '{}' has negative minimum notch",
                self.layer
            ));
        }
        for (layer, spacing) in &self.spacing_to {
            if layer.trim().is_empty() {
                return Err(format!(
                    "layout rule for layer '{}' has an empty cross-layer spacing name",
                    self.layer
                ));
            }
            if *spacing < 0 {
                return Err(format!(
                    "layout rule for layer '{}' has negative spacing to layer '{}'",
                    self.layer, layer
                ));
            }
        }
        if let Some(value) = self.min_density_bp {
            if value > 10_000 {
                return Err(format!(
                    "layer '{}' minimum density cannot exceed 100%",
                    self.layer
                ));
            }
        }
        if let Some(value) = self.max_density_bp {
            if value > 10_000 {
                return Err(format!(
                    "layer '{}' maximum density cannot exceed 100%",
                    self.layer
                ));
            }
        }
        if let (Some(min), Some(max)) = (self.min_density_bp, self.max_density_bp) {
            if min > max {
                return Err(format!("layer '{}' density range is inverted", self.layer));
            }
        }
        if (self.min_density_bp.is_some() || self.max_density_bp.is_some())
            && self.density_window <= 0
        {
            return Err(format!(
                "layer '{}' density window must be positive",
                self.layer
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutViaRule {
    pub via: String,
    pub cut_layer: String,
    pub bottom_layer: String,
    pub top_layer: String,
    pub cut_width: i64,
    pub cut_height: i64,
    pub min_bottom_enclosure: i64,
    pub min_top_enclosure: i64,
    pub min_cut_spacing: i64,
    pub min_bottom_extension_x: i64,
    pub min_bottom_extension_y: i64,
    pub min_top_extension_x: i64,
    pub min_top_extension_y: i64,
}

impl LayoutViaRule {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        via: impl Into<String>,
        cut_layer: impl Into<String>,
        bottom_layer: impl Into<String>,
        top_layer: impl Into<String>,
        cut_width: i64,
        cut_height: i64,
        min_bottom_enclosure: i64,
        min_top_enclosure: i64,
    ) -> Result<Self, String> {
        let out = Self {
            via: via.into(),
            cut_layer: cut_layer.into(),
            bottom_layer: bottom_layer.into(),
            top_layer: top_layer.into(),
            cut_width,
            cut_height,
            min_bottom_enclosure,
            min_top_enclosure,
            min_cut_spacing: 0,
            min_bottom_extension_x: 0,
            min_bottom_extension_y: 0,
            min_top_extension_x: 0,
            min_top_extension_y: 0,
        };
        out.validate()?;
        Ok(out)
    }

    pub fn with_cut_spacing(mut self, min_cut_spacing: i64) -> Result<Self, String> {
        self.min_cut_spacing = min_cut_spacing;
        self.validate()?;
        Ok(self)
    }

    pub fn with_extensions(
        mut self,
        bottom_x: i64,
        bottom_y: i64,
        top_x: i64,
        top_y: i64,
    ) -> Result<Self, String> {
        self.min_bottom_extension_x = bottom_x;
        self.min_bottom_extension_y = bottom_y;
        self.min_top_extension_x = top_x;
        self.min_top_extension_y = top_y;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.via.trim().is_empty()
            || self.cut_layer.trim().is_empty()
            || self.bottom_layer.trim().is_empty()
            || self.top_layer.trim().is_empty()
        {
            return Err("layout via rule names cannot be empty".into());
        }
        if self.cut_width <= 0
            || self.cut_height <= 0
            || self.min_bottom_enclosure < 0
            || self.min_top_enclosure < 0
            || self.min_cut_spacing < 0
            || self.min_bottom_extension_x < 0
            || self.min_bottom_extension_y < 0
            || self.min_top_extension_x < 0
            || self.min_top_extension_y < 0
        {
            return Err(format!(
                "invalid geometry in layout via rule '{}'",
                self.via
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutDrcViolation {
    pub code: String,
    pub severity: DrcSeverity,
    pub message: String,
    pub layer: Option<String>,
    pub net: Option<String>,
    pub object: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutDrcResult {
    pub violations: Vec<LayoutDrcViolation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutDrcSuggestion {
    pub code: String,
    pub message: String,
    pub edit: LayoutEdit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManhattanBooleanOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

impl LayoutDrcResult {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn error_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|violation| violation.severity == DrcSeverity::Error)
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutDrcWaiver {
    pub id: String,
    pub code: String,
    pub layer: Option<String>,
    pub net: Option<String>,
    pub object: Option<String>,
    pub owner: String,
    pub reason: String,
}

impl LayoutDrcWaiver {
    pub fn new(
        id: impl Into<String>,
        code: impl Into<String>,
        layer: Option<String>,
        net: Option<String>,
        object: Option<String>,
        owner: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<Self, String> {
        let out = Self {
            id: id.into(),
            code: code.into(),
            layer,
            net,
            object,
            owner: owner.into(),
            reason: reason.into(),
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.code.trim().is_empty() {
            return Err("layout DRC waiver requires id and violation code".into());
        }
        if self.owner.trim().is_empty() || self.reason.trim().is_empty() {
            return Err(format!(
                "layout DRC waiver '{}' requires owner and reason",
                self.id
            ));
        }
        Ok(())
    }

    fn matches(&self, violation: &LayoutDrcViolation) -> bool {
        self.code == violation.code
            && self
                .layer
                .as_ref()
                .is_none_or(|layer| violation.layer.as_ref().is_some_and(|value| value == layer))
            && self
                .net
                .as_ref()
                .is_none_or(|net| violation.net.as_ref().is_some_and(|value| value == net))
            && self.object.as_ref().is_none_or(|object| {
                violation
                    .object
                    .as_ref()
                    .is_some_and(|value| value == object)
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutDrcWaivedViolation {
    pub violation: LayoutDrcViolation,
    pub waiver_id: String,
    pub owner: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayoutDrcWaiverResult {
    pub active: LayoutDrcResult,
    pub waived: Vec<LayoutDrcWaivedViolation>,
    pub used_waiver_ids: Vec<String>,
}

/// Apply auditable selectors to a completed layout DRC result.
///
/// Waivers never suppress DRC generation itself; they form a separate review
/// layer so callers retain both active and waived evidence.
pub fn apply_layout_drc_waivers(
    result: &LayoutDrcResult,
    waivers: &[LayoutDrcWaiver],
) -> Result<LayoutDrcWaiverResult, String> {
    let mut ids = BTreeSet::new();
    for waiver in waivers {
        waiver.validate()?;
        if !ids.insert(waiver.id.clone()) {
            return Err(format!("duplicate layout DRC waiver '{}'", waiver.id));
        }
    }
    let mut active = LayoutDrcResult::default();
    let mut waived = Vec::new();
    let mut used = BTreeSet::new();
    for violation in &result.violations {
        if let Some(waiver) = waivers.iter().find(|waiver| waiver.matches(violation)) {
            waived.push(LayoutDrcWaivedViolation {
                violation: violation.clone(),
                waiver_id: waiver.id.clone(),
                owner: waiver.owner.clone(),
                reason: waiver.reason.clone(),
            });
            used.insert(waiver.id.clone());
        } else {
            active.violations.push(violation.clone());
        }
    }
    Ok(LayoutDrcWaiverResult {
        active,
        waived,
        used_waiver_ids: used.into_iter().collect(),
    })
}

pub fn layout_drc_waiver_report(result: &LayoutDrcWaiverResult) -> String {
    let mut out = format!(
        "Layout DRC waiver report: {} active, {} waived, {} waiver(s) used\n",
        result.active.violations.len(),
        result.waived.len(),
        result.used_waiver_ids.len()
    );
    for waived in &result.waived {
        out.push_str(&format!(
            "  waived {} [{}] by '{}' owner='{}' reason='{}'\n",
            waived.violation.code,
            waived.violation.object.as_deref().unwrap_or("<unknown>"),
            waived.waiver_id,
            waived.owner,
            waived.reason
        ));
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutDocument {
    layer_rules: BTreeMap<String, LayoutLayerRule>,
    via_rules: BTreeMap<String, LayoutViaRule>,
    shapes: Vec<LayoutShape>,
    vias: Vec<LayoutVia>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutEdit {
    SetLayerRule(LayoutLayerRule),
    SetViaRule(LayoutViaRule),
    AddShape(LayoutShape),
    RemoveShape {
        id: String,
    },
    MoveShape {
        id: String,
        dx: LayoutCoordinate,
        dy: LayoutCoordinate,
    },
    SetShapeGeometry {
        id: String,
        geometry: LayoutGeometry,
    },
    AddVia(LayoutVia),
    RemoveVia {
        id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayoutEditor {
    document: LayoutDocument,
    undo: Vec<LayoutDocument>,
    redo: Vec<LayoutDocument>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManhattanBooleanResult {
    pub rectangles: Vec<LayoutRect>,
    pub area: i64,
}

pub fn manhattan_boolean(
    left: &LayoutGeometry,
    right: &LayoutGeometry,
    operation: ManhattanBooleanOp,
) -> Result<ManhattanBooleanResult, String> {
    if matches!(left, LayoutGeometry::AnyAngle(_)) || matches!(right, LayoutGeometry::AnyAngle(_)) {
        return Err("Manhattan Boolean operations do not accept arbitrary-angle polygons".into());
    }
    if let LayoutGeometry::Polygon(polygon) = left {
        polygon.validate()?;
    }
    if let LayoutGeometry::Polygon(polygon) = right {
        polygon.validate()?;
    }
    let scaled_left = scaled_geometry(left, 2)?;
    let scaled_right = scaled_geometry(right, 2)?;
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for point in left.vertices().iter().chain(right.vertices().iter()) {
        xs.push(scale_coordinate(point.x, 2)?);
        ys.push(scale_coordinate(point.y, 2)?);
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    if xs.len() < 2 || ys.len() < 2 {
        return Ok(ManhattanBooleanResult {
            rectangles: Vec::new(),
            area: 0,
        });
    }

    let mut occupied = std::collections::BTreeSet::new();
    for x_index in 0..xs.len() - 1 {
        for y_index in 0..ys.len() - 1 {
            let center = LayoutPoint::new(
                (xs[x_index] + xs[x_index + 1]) / 2,
                (ys[y_index] + ys[y_index + 1]) / 2,
            );
            let a = scaled_left.contains_point(center);
            let b = scaled_right.contains_point(center);
            let present = match operation {
                ManhattanBooleanOp::Union => a || b,
                ManhattanBooleanOp::Intersection => a && b,
                ManhattanBooleanOp::Difference => a && !b,
                ManhattanBooleanOp::SymmetricDifference => a != b,
            };
            if present {
                occupied.insert((x_index, y_index));
            }
        }
    }

    let scaled_rectangles = partition_occupied_cells(&xs, &ys, &occupied)?;
    let rectangles = scaled_rectangles
        .into_iter()
        .map(unscale_rect)
        .collect::<Result<Vec<_>, String>>()?;
    let area = rectangles.iter().map(|rect| rect.area()).sum();
    Ok(ManhattanBooleanResult { rectangles, area })
}

pub fn manhattan_union(geometries: &[LayoutGeometry]) -> Result<ManhattanBooleanResult, String> {
    for geometry in geometries {
        if matches!(geometry, LayoutGeometry::AnyAngle(_)) {
            return Err("Manhattan union does not accept arbitrary-angle polygons".into());
        }
        if let LayoutGeometry::Polygon(polygon) = geometry {
            polygon.validate()?;
        }
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let scaled_geometries = geometries
        .iter()
        .map(|geometry| scaled_geometry(geometry, 2))
        .collect::<Result<Vec<_>, String>>()?;
    for geometry in geometries {
        for point in geometry.vertices() {
            xs.push(scale_coordinate(point.x, 2)?);
            ys.push(scale_coordinate(point.y, 2)?);
        }
    }
    // Scale all coordinate boundaries by two so every grid cell has an integer
    // interior sample point even when adjacent database-unit edges differ by 1.
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    if xs.len() < 2 || ys.len() < 2 {
        return Ok(ManhattanBooleanResult {
            rectangles: Vec::new(),
            area: 0,
        });
    }
    let mut occupied = std::collections::BTreeSet::new();
    for x_index in 0..xs.len() - 1 {
        for y_index in 0..ys.len() - 1 {
            let center = LayoutPoint::new(
                (xs[x_index] + xs[x_index + 1]) / 2,
                (ys[y_index] + ys[y_index + 1]) / 2,
            );
            if scaled_geometries
                .iter()
                .any(|geometry| geometry.contains_point(center))
            {
                occupied.insert((x_index, y_index));
            }
        }
    }
    let scaled_rectangles = partition_occupied_cells(&xs, &ys, &occupied)?;
    let rectangles = scaled_rectangles
        .into_iter()
        .map(unscale_rect)
        .collect::<Result<Vec<_>, String>>()?;
    let area = rectangles.iter().map(|rect| rect.area()).sum();
    Ok(ManhattanBooleanResult { rectangles, area })
}

fn partition_occupied_cells(
    xs: &[i64],
    ys: &[i64],
    occupied: &std::collections::BTreeSet<(usize, usize)>,
) -> Result<Vec<LayoutRect>, String> {
    let mut used = std::collections::BTreeSet::new();
    let mut rectangles = Vec::new();
    for y in 0..ys.len() - 1 {
        for x in 0..xs.len() - 1 {
            let start = (x, y);
            if !occupied.contains(&start) || used.contains(&start) {
                continue;
            }
            let mut x_end = x;
            while x_end + 1 < xs.len() - 1
                && occupied.contains(&(x_end + 1, y))
                && !used.contains(&(x_end + 1, y))
            {
                x_end += 1;
            }
            let mut y_end = y;
            'grow: while y_end + 1 < ys.len() - 1 {
                for x in x..=x_end {
                    let cell = (x, y_end + 1);
                    if !occupied.contains(&cell) || used.contains(&cell) {
                        break 'grow;
                    }
                }
                y_end += 1;
            }
            for cell_x in x..=x_end {
                for cell_y in y..=y_end {
                    used.insert((cell_x, cell_y));
                }
            }
            rectangles.push(LayoutRect::new(xs[x], ys[y], xs[x_end + 1], ys[y_end + 1])?);
        }
    }
    Ok(rectangles)
}

fn scaled_geometry(geometry: &LayoutGeometry, factor: i64) -> Result<LayoutGeometry, String> {
    match geometry {
        LayoutGeometry::AnyAngle(_) => {
            Err("Manhattan Boolean operations do not accept arbitrary-angle polygons".into())
        }
        LayoutGeometry::Rect(rect) => Ok(LayoutGeometry::Rect(LayoutRect::new(
            rect.x1.checked_mul(factor).ok_or("layout x overflow")?,
            rect.y1.checked_mul(factor).ok_or("layout y overflow")?,
            rect.x2.checked_mul(factor).ok_or("layout x overflow")?,
            rect.y2.checked_mul(factor).ok_or("layout y overflow")?,
        )?)),
        LayoutGeometry::Polygon(polygon) => Ok(LayoutGeometry::Polygon(LayoutPolygon::new(
            polygon
                .points
                .iter()
                .map(|point| {
                    Ok(LayoutPoint::new(
                        scale_coordinate(point.x, factor)?,
                        scale_coordinate(point.y, factor)?,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?,
        )?)),
    }
}

fn rule_deck_i64(tokens: &[&str], index: usize, line_no: usize) -> Result<i64, String> {
    tokens
        .get(index)
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| format!("rule deck line {line_no} requires an i64 argument"))
}

fn rule_deck_u32(tokens: &[&str], index: usize, line_no: usize) -> Result<u32, String> {
    tokens
        .get(index)
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| format!("rule deck line {line_no} requires a u32 argument"))
}

/// Serialize only a document's layer/via rule deck in deterministic text form.
pub fn write_layout_rule_deck(document: &LayoutDocument) -> Result<String, String> {
    for rule in document.layer_rules.values() {
        rule.validate()?;
    }
    for rule in document.via_rules.values() {
        rule.validate()?;
    }
    let mut out = String::from("version 1\n");
    for rule in document.layer_rules.values() {
        out.push_str(&format!(
            "layer {} min_width {} min_spacing {} min_area {}",
            rule.layer, rule.min_width, rule.min_spacing, rule.min_area
        ));
        if let Some(min) = rule.min_density_bp {
            out.push_str(&format!(" min_density_bp {min}"));
        }
        if let Some(max) = rule.max_density_bp {
            out.push_str(&format!(" max_density_bp {max}"));
        }
        if rule.min_density_bp.is_some() || rule.max_density_bp.is_some() {
            out.push_str(&format!(" density_window {}", rule.density_window));
        }
        if rule.min_notch > 0 {
            out.push_str(&format!(" min_notch {}", rule.min_notch));
        }
        out.push('\n');
    }
    for rule in document.layer_rules.values() {
        for (other_layer, spacing) in &rule.spacing_to {
            out.push_str(&format!(
                "spacing {} {} {spacing}\n",
                rule.layer, other_layer
            ));
        }
    }
    for rule in document.via_rules.values() {
        out.push_str(&format!(
            "via {} cut_layer {} bottom_layer {} top_layer {} cut_width {} cut_height {} bottom_enclosure {} top_enclosure {} bottom_extension_x {} bottom_extension_y {} top_extension_x {} top_extension_y {} cut_spacing {}\n",
            rule.via,
            rule.cut_layer,
            rule.bottom_layer,
            rule.top_layer,
            rule.cut_width,
            rule.cut_height,
            rule.min_bottom_enclosure,
            rule.min_top_enclosure,
            rule.min_bottom_extension_x,
            rule.min_bottom_extension_y,
            rule.min_top_extension_x,
            rule.min_top_extension_y,
            rule.min_cut_spacing
        ));
    }
    Ok(out)
}

/// Parse a deterministic layer/via rule deck into a fresh layout document.
pub fn read_layout_rule_deck(source: &str) -> Result<LayoutDocument, String> {
    let mut document = LayoutDocument::new();
    let mut seen_layers = BTreeSet::new();
    let mut seen_vias = BTreeSet::new();
    let mut seen_spacing = BTreeSet::new();
    let mut saw_version = false;
    for (line_index, raw) in source.lines().enumerate() {
        let line_no = line_index + 1;
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        match tokens.first().copied() {
            Some("version") => {
                if saw_version || tokens.len() != 2 || tokens[1] != "1" {
                    return Err(format!(
                        "rule deck line {line_no} must contain exactly 'version 1' once"
                    ));
                }
                saw_version = true;
            }
            Some("layer") => {
                if !saw_version {
                    return Err("rule deck requires an initial 'version 1'".into());
                }
                if tokens.len() < 8 || tokens.len() % 2 != 0 {
                    return Err(format!(
                        "rule deck line {line_no} has malformed layer syntax"
                    ));
                }
                let layer = tokens[1];
                if !seen_layers.insert(layer.to_string()) {
                    return Err(format!(
                        "rule deck line {line_no} duplicates layer '{layer}'"
                    ));
                }
                let mut rule = LayoutLayerRule::new(
                    layer,
                    rule_deck_i64(&tokens, 3, line_no)?,
                    rule_deck_i64(&tokens, 5, line_no)?,
                    rule_deck_i64(&tokens, 7, line_no)?,
                )?;
                let mut index = 8;
                while index < tokens.len() {
                    let keyword = tokens[index];
                    if index + 1 >= tokens.len() {
                        return Err(format!(
                            "rule deck line {line_no} lacks a value for '{keyword}'"
                        ));
                    }
                    rule = match keyword {
                        "min_density_bp" => {
                            let value = rule_deck_u32(&tokens, index + 1, line_no)?;
                            let max = rule.max_density_bp;
                            let window = rule.density_window;
                            rule.with_density(Some(value), max, window)?
                        }
                        "max_density_bp" => {
                            let value = rule_deck_u32(&tokens, index + 1, line_no)?;
                            let min = rule.min_density_bp;
                            let window = rule.density_window;
                            rule.with_density(min, Some(value), window)?
                        }
                        "density_window" => {
                            let value = rule_deck_i64(&tokens, index + 1, line_no)?;
                            let min = rule.min_density_bp;
                            let max = rule.max_density_bp;
                            rule.with_density(min, max, value)?
                        }
                        "min_notch" => {
                            rule.with_notch(rule_deck_i64(&tokens, index + 1, line_no)?)?
                        }
                        other => {
                            return Err(format!(
                                "rule deck line {line_no} has unknown layer keyword '{other}'"
                            ))
                        }
                    };
                    index += 2;
                }
                document.set_layer_rule(rule)?;
            }
            Some("spacing") => {
                if tokens.len() != 4 {
                    return Err(format!(
                        "rule deck line {line_no} must be 'spacing LAYER LAYER DBU'"
                    ));
                }
                let (left, right) = (tokens[1], tokens[2]);
                if !seen_layers.contains(left) || !seen_layers.contains(right) {
                    return Err(format!(
                        "rule deck line {line_no} references an undefined layer"
                    ));
                }
                if !seen_spacing.insert((left.to_string(), right.to_string())) {
                    return Err(format!(
                        "rule deck line {line_no} duplicates a spacing rule"
                    ));
                }
                let spacing = rule_deck_i64(&tokens, 3, line_no)?;
                let rule = document
                    .layer_rules
                    .get(left)
                    .cloned()
                    .ok_or_else(|| format!("rule deck line {line_no} lacks layer '{left}'"))?;
                document.set_layer_rule(rule.with_spacing_to(right, spacing)?)?;
            }
            Some("via") => {
                if tokens.len() != 26 {
                    return Err(format!("rule deck line {line_no} has malformed via syntax"));
                }
                let via = tokens[1];
                if !seen_vias.insert(via.to_string()) {
                    return Err(format!("rule deck line {line_no} duplicates via '{via}'"));
                }
                let mut index = 9;
                let mut values = [0_i64; 9];
                for value in &mut values {
                    *value = rule_deck_i64(&tokens, index, line_no)?;
                    index += 2;
                }
                let cut_layer = tokens[3];
                let bottom_layer = tokens[5];
                let top_layer = tokens[7];
                for layer in [bottom_layer, top_layer] {
                    if !seen_layers.contains(layer) {
                        return Err(format!(
                            "rule deck line {line_no} references undefined layer '{layer}'"
                        ));
                    }
                }
                let rule = LayoutViaRule::new(
                    via,
                    cut_layer,
                    bottom_layer,
                    top_layer,
                    values[0],
                    values[1],
                    values[2],
                    values[3],
                )?
                .with_extensions(values[4], values[5], values[6], values[7])?
                .with_cut_spacing(values[8])?;
                document.set_via_rule(rule)?;
            }
            _ => return Err(format!("rule deck line {line_no} has unknown syntax")),
        }
    }
    if !saw_version {
        return Err("rule deck requires 'version 1'".into());
    }
    Ok(document)
}

fn scale_coordinate(value: i64, factor: i64) -> Result<i64, String> {
    value
        .checked_mul(factor)
        .ok_or_else(|| "layout coordinate overflow".into())
}

fn unscale_rect(rect: LayoutRect) -> Result<LayoutRect, String> {
    LayoutRect::new(rect.x1 / 2, rect.y1 / 2, rect.x2 / 2, rect.y2 / 2)
}

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

impl LayoutEditor {
    pub fn new(document: LayoutDocument) -> Result<Self, String> {
        Ok(Self {
            document,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn document(&self) -> &LayoutDocument {
        &self.document
    }

    pub fn apply(&mut self, edit: LayoutEdit) -> Result<(), String> {
        let old = self.document.clone();
        match edit {
            LayoutEdit::SetLayerRule(rule) => self.document.set_layer_rule(rule)?,
            LayoutEdit::SetViaRule(rule) => self.document.set_via_rule(rule)?,
            LayoutEdit::AddShape(shape) => self.document.add_shape(shape)?,
            LayoutEdit::RemoveShape { id } => {
                self.document.remove_shape(&id)?;
            }
            LayoutEdit::MoveShape { id, dx, dy } => self.document.move_shape(&id, dx, dy)?,
            LayoutEdit::SetShapeGeometry { id, geometry } => {
                self.document.set_shape_geometry(&id, geometry)?;
            }
            LayoutEdit::AddVia(via) => self.document.add_via(via)?,
            LayoutEdit::RemoveVia { id } => {
                self.document.remove_via(&id)?;
            }
        }
        self.undo.push(old);
        self.redo.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(previous) = self.undo.pop() else {
            return Ok(false);
        };
        self.redo
            .push(std::mem::replace(&mut self.document, previous));
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        let Some(next) = self.redo.pop() else {
            return Ok(false);
        };
        self.undo.push(std::mem::replace(&mut self.document, next));
        Ok(true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutSelection {
    None,
    Shape { id: String },
    ShapeVertex { id: String, vertex: usize },
    Via { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutTool {
    Select,
    Rectangle,
    Polygon,
    AnyAnglePolygon,
    Via,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutEvent {
    PointerDown { point: LayoutPoint },
    PointerMove { point: LayoutPoint },
    PointerUp { point: LayoutPoint },
    SetTool { tool: LayoutTool },
    FinishPolygon,
    Undo,
    Redo,
    Delete,
    CheckDrc,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LayoutEventOutcome {
    Ignored,
    SelectionChanged,
    Dragging {
        id: String,
        bbox: LayoutRect,
    },
    ShapeMoved {
        id: String,
    },
    ShapeVertexMoved {
        id: String,
        vertex: usize,
        point: LayoutPoint,
    },
    RectanglePreviewChanged {
        start: LayoutPoint,
        end: LayoutPoint,
    },
    PolygonPreviewChanged {
        points: Vec<LayoutPoint>,
    },
    ShapeAdded {
        id: String,
    },
    ViaAdded {
        id: String,
    },
    ToolChanged {
        tool: LayoutTool,
    },
    UndoApplied {
        changed: bool,
    },
    RedoApplied {
        changed: bool,
    },
    Deleted {
        selection: LayoutSelection,
    },
    DrcChecked {
        clean: bool,
        error_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutSession {
    editor: LayoutEditor,
    tool: LayoutTool,
    active_layer: String,
    active_net: String,
    active_via_rule: String,
    selection: LayoutSelection,
    drag: Option<LayoutDrag>,
    rectangle_start: Option<LayoutPoint>,
    polygon_points: Vec<LayoutPoint>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutDrag {
    bbox: LayoutRect,
    pointer_start: LayoutPoint,
    pointer_current: LayoutPoint,
}

impl LayoutSession {
    pub fn new(
        document: LayoutDocument,
        active_layer: impl Into<String>,
        active_net: impl Into<String>,
        active_via_rule: impl Into<String>,
    ) -> Result<Self, String> {
        let active_layer = active_layer.into();
        let active_net = active_net.into();
        let active_via_rule = active_via_rule.into();
        if active_layer.trim().is_empty() || active_net.trim().is_empty() {
            return Err("layout session requires an active layer and net".into());
        }
        if document.layer_rule(&active_layer).is_none() {
            return Err(format!(
                "active layout layer '{}' has no rule",
                active_layer
            ));
        }
        if !active_via_rule.trim().is_empty() && document.via_rule(&active_via_rule).is_none() {
            return Err(format!("active via rule '{}' is unknown", active_via_rule));
        }
        Ok(Self {
            editor: LayoutEditor::new(document)?,
            tool: LayoutTool::Select,
            active_layer,
            active_net,
            active_via_rule,
            selection: LayoutSelection::None,
            drag: None,
            rectangle_start: None,
            polygon_points: Vec::new(),
        })
    }

    pub fn document(&self) -> &LayoutDocument {
        self.editor.document()
    }

    pub fn tool(&self) -> LayoutTool {
        self.tool
    }

    pub fn selection(&self) -> &LayoutSelection {
        &self.selection
    }

    pub fn properties(&self) -> BTreeMap<String, String> {
        let mut properties = BTreeMap::new();
        properties.insert(
            "tool".into(),
            match self.tool {
                LayoutTool::Select => "select".into(),
                LayoutTool::Rectangle => "rectangle".into(),
                LayoutTool::Polygon => "polygon".into(),
                LayoutTool::AnyAnglePolygon => "any-angle polygon".into(),
                LayoutTool::Via => "via".into(),
            },
        );
        properties.insert("activeLayer".into(), self.active_layer.clone());
        properties.insert("activeNet".into(), self.active_net.clone());
        if !self.active_via_rule.is_empty() {
            properties.insert("activeViaRule".into(), self.active_via_rule.clone());
        }
        match &self.selection {
            LayoutSelection::None => {
                properties.insert("selection".into(), "none".into());
            }
            LayoutSelection::Shape { id } => {
                properties.insert("selection".into(), "shape".into());
                properties.insert("id".into(), id.clone());
                if let Some(shape) = self
                    .document()
                    .shapes()
                    .iter()
                    .find(|shape| &shape.id == id)
                {
                    properties.insert("layer".into(), shape.layer.clone());
                    properties.insert("net".into(), shape.net.clone());
                    properties.insert(
                        "geometry".into(),
                        match &shape.geometry {
                            LayoutGeometry::Rect(_) => "rectangle".into(),
                            LayoutGeometry::Polygon(_) => "polygon".into(),
                            LayoutGeometry::AnyAngle(_) => "any-angle polygon".into(),
                        },
                    );
                    let bbox = shape.geometry.bbox();
                    properties.insert(
                        "bbox".into(),
                        format!("({}, {}) .. ({}, {})", bbox.x1, bbox.y1, bbox.x2, bbox.y2),
                    );
                    properties.insert(
                        "area".into(),
                        shape
                            .geometry
                            .area2()
                            .map(|area2| format!("{}", area2 as f64 / 2.0))
                            .unwrap_or_else(|error| format!("invalid: {error}")),
                    );
                }
            }
            LayoutSelection::ShapeVertex { id, vertex } => {
                properties.insert("selection".into(), "shape-vertex".into());
                properties.insert("id".into(), id.clone());
                properties.insert("vertex".into(), vertex.to_string());
                if let Some(geometry) = self
                    .document()
                    .shapes()
                    .iter()
                    .find(|shape| &shape.id == id)
                    .map(|shape| shape.geometry.clone())
                {
                    let points = match &geometry {
                        LayoutGeometry::Polygon(polygon) => Some(polygon.points.clone()),
                        LayoutGeometry::AnyAngle(polygon) => Some(polygon.points().to_vec()),
                        _ => None,
                    };
                    if let Some(points) = points {
                        if let Some(point) = points.get(*vertex) {
                            properties
                                .insert("point".into(), format!("({}, {})", point.x, point.y));
                        }
                        properties.insert("vertices".into(), points.len().to_string());
                    }
                }
            }
            LayoutSelection::Via { id } => {
                properties.insert("selection".into(), "via".into());
                properties.insert("id".into(), id.clone());
                if let Some(via) = self.document().vias().iter().find(|via| &via.id == id) {
                    properties.insert("rule".into(), via.rule.clone());
                    properties.insert("net".into(), via.net.clone());
                    properties.insert("cutLayer".into(), via.cut_layer.clone());
                    properties.insert("bottomLayer".into(), via.bottom_layer.clone());
                    properties.insert("topLayer".into(), via.top_layer.clone());
                    properties.insert(
                        "cut".into(),
                        format!(
                            "({}, {}) .. ({}, {})",
                            via.cut.x1, via.cut.y1, via.cut.x2, via.cut.y2
                        ),
                    );
                }
            }
        }
        properties
    }

    pub fn render_svg(&self) -> Result<String, String> {
        self.document().write_svg()
    }

    pub fn process(&mut self, event: LayoutEvent) -> Result<LayoutEventOutcome, String> {
        match event {
            LayoutEvent::SetTool { tool } => {
                self.cancel_pending();
                let changed = self.tool != tool;
                self.tool = tool;
                if changed {
                    Ok(LayoutEventOutcome::ToolChanged { tool })
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
            LayoutEvent::PointerDown { point } => self.pointer_down(point),
            LayoutEvent::PointerMove { point } => self.pointer_move(point),
            LayoutEvent::PointerUp { point } => self.pointer_up(point),
            LayoutEvent::FinishPolygon => self.finish_polygon(),
            LayoutEvent::Undo => {
                self.cancel_pending();
                let changed = self.editor.undo()?;
                if changed {
                    self.selection = LayoutSelection::None;
                }
                Ok(LayoutEventOutcome::UndoApplied { changed })
            }
            LayoutEvent::Redo => {
                self.cancel_pending();
                let changed = self.editor.redo()?;
                if changed {
                    self.selection = LayoutSelection::None;
                }
                Ok(LayoutEventOutcome::RedoApplied { changed })
            }
            LayoutEvent::Delete => self.delete_selection(),
            LayoutEvent::CheckDrc => {
                let result = self.document().check_layout_drc();
                Ok(LayoutEventOutcome::DrcChecked {
                    clean: result.is_clean(),
                    error_count: result.error_count(),
                })
            }
        }
    }

    fn cancel_pending(&mut self) {
        self.drag = None;
        self.rectangle_start = None;
        self.polygon_points.clear();
    }

    fn cancel_transient(&mut self) {
        self.drag = None;
        self.rectangle_start = None;
    }

    fn pointer_down(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        self.cancel_transient();
        match self.tool {
            LayoutTool::Rectangle => {
                self.rectangle_start = Some(point);
                Ok(LayoutEventOutcome::RectanglePreviewChanged {
                    start: point,
                    end: point,
                })
            }
            LayoutTool::Via => Ok(LayoutEventOutcome::Ignored),
            LayoutTool::Polygon => {
                let next = polygon_next_point(&self.polygon_points, point);
                if self.polygon_points.last() == Some(&next) {
                    return Ok(LayoutEventOutcome::PolygonPreviewChanged {
                        points: self.polygon_points.clone(),
                    });
                }
                self.polygon_points.push(next);
                Ok(LayoutEventOutcome::PolygonPreviewChanged {
                    points: self.polygon_points.clone(),
                })
            }
            LayoutTool::AnyAnglePolygon => {
                if self.polygon_points.last() == Some(&point) {
                    return Ok(LayoutEventOutcome::PolygonPreviewChanged {
                        points: self.polygon_points.clone(),
                    });
                }
                self.polygon_points.push(point);
                Ok(LayoutEventOutcome::PolygonPreviewChanged {
                    points: self.polygon_points.clone(),
                })
            }
            LayoutTool::Select => {
                let selection = self
                    .document()
                    .hit_layout(point)
                    .unwrap_or(LayoutSelection::None);
                let changed = selection != self.selection;
                if let LayoutSelection::Shape { id } | LayoutSelection::ShapeVertex { id, .. } =
                    &selection
                {
                    let shape = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| &shape.id == id)
                        .ok_or_else(|| format!("selected shape '{}' disappeared", id))?;
                    self.drag = Some(LayoutDrag {
                        bbox: shape.geometry.bbox(),
                        pointer_start: point,
                        pointer_current: point,
                    });
                }
                self.selection = selection;
                if changed {
                    Ok(LayoutEventOutcome::SelectionChanged)
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
        }
    }

    fn pointer_move(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        if let Some(start) = self.rectangle_start {
            return Ok(LayoutEventOutcome::RectanglePreviewChanged { start, end: point });
        }
        let Some(drag) = self.drag.as_mut() else {
            return Ok(LayoutEventOutcome::Ignored);
        };
        drag.pointer_current = point;
        match &self.selection {
            LayoutSelection::Shape { id } | LayoutSelection::ShapeVertex { id, .. } => {
                Ok(LayoutEventOutcome::Dragging {
                    id: id.clone(),
                    bbox: dragged_bbox(*drag),
                })
            }
            _ => Ok(LayoutEventOutcome::Ignored),
        }
    }

    fn pointer_up(&mut self, point: LayoutPoint) -> Result<LayoutEventOutcome, String> {
        match self.tool {
            LayoutTool::Rectangle => {
                let Some(start) = self.rectangle_start.take() else {
                    return Ok(LayoutEventOutcome::Ignored);
                };
                if start == point {
                    return Ok(LayoutEventOutcome::Ignored);
                }
                let id = self.unique_id("shape");
                self.editor.apply(LayoutEdit::AddShape(LayoutShape {
                    id: id.clone(),
                    layer: self.active_layer.clone(),
                    net: self.active_net.clone(),
                    geometry: LayoutGeometry::Rect(LayoutRect::new(
                        start.x, start.y, point.x, point.y,
                    )?),
                }))?;
                self.selection = LayoutSelection::Shape { id: id.clone() };
                Ok(LayoutEventOutcome::ShapeAdded { id })
            }
            LayoutTool::Via => {
                if self.active_via_rule.trim().is_empty() {
                    return Err("via tool requires an active via rule".into());
                }
                let rule = self
                    .document()
                    .via_rule(&self.active_via_rule)
                    .ok_or_else(|| format!("unknown active via rule '{}'", self.active_via_rule))?;
                let x1 = point.x - rule.cut_width / 2;
                let x2 = x1 + rule.cut_width;
                let y1 = point.y - rule.cut_height / 2;
                let y2 = y1 + rule.cut_height;
                let id = self.unique_id("via");
                self.editor.apply(LayoutEdit::AddVia(LayoutVia {
                    id: id.clone(),
                    rule: rule.via.clone(),
                    net: self.active_net.clone(),
                    cut_layer: rule.cut_layer.clone(),
                    bottom_layer: rule.bottom_layer.clone(),
                    top_layer: rule.top_layer.clone(),
                    cut: LayoutRect::new(x1, y1, x2, y2)?,
                }))?;
                self.selection = LayoutSelection::Via { id: id.clone() };
                Ok(LayoutEventOutcome::ViaAdded { id })
            }
            LayoutTool::Polygon | LayoutTool::AnyAnglePolygon => Ok(LayoutEventOutcome::Ignored),
            LayoutTool::Select => {
                let Some(drag) = self.drag.take() else {
                    return Ok(LayoutEventOutcome::Ignored);
                };
                let mut drag = drag;
                drag.pointer_current = point;
                let target = dragged_bbox(drag);
                if let LayoutSelection::Shape { id } = &self.selection {
                    let id = id.clone();
                    let geometry = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| shape.id == id)
                        .ok_or_else(|| format!("dragged shape '{}' disappeared", id))?
                        .geometry
                        .clone();
                    let current = geometry.bbox();
                    let dx = target.x1 - current.x1;
                    let dy = target.y1 - current.y1;
                    self.editor.apply(LayoutEdit::MoveShape {
                        id: id.clone(),
                        dx,
                        dy,
                    })?;
                    Ok(LayoutEventOutcome::ShapeMoved { id })
                } else if let LayoutSelection::ShapeVertex { id, vertex } = &self.selection {
                    let id = id.clone();
                    let vertex = *vertex;
                    let geometry = self
                        .document()
                        .shapes()
                        .iter()
                        .find(|shape| shape.id == id)
                        .ok_or_else(|| format!("dragged shape '{}' disappeared", id))?
                        .geometry
                        .clone();
                    let mut reported_point = point;
                    let geometry = match geometry {
                        LayoutGeometry::Polygon(mut polygon) => {
                            let previous = polygon.points
                                [(vertex + polygon.points.len() - 1) % polygon.points.len()];
                            let next_index = (vertex + 1) % polygon.points.len();
                            let constrained = if vertex % 2 == 1 {
                                polygon.points[next_index].x = point.x;
                                LayoutPoint::new(point.x, previous.y)
                            } else {
                                polygon.points[next_index].y = point.y;
                                LayoutPoint::new(previous.x, point.y)
                            };
                            polygon.points[vertex] = constrained;
                            reported_point = constrained;
                            LayoutGeometry::Polygon(LayoutPolygon::new(polygon.points)?)
                        }
                        LayoutGeometry::AnyAngle(polygon) => {
                            let mut points = polygon.points().to_vec();
                            if vertex >= points.len() {
                                return Err("selected any-angle vertex is out of range".into());
                            }
                            points[vertex] = point;
                            LayoutGeometry::AnyAngle(AnyAnglePolygon::new(points)?)
                        }
                        _ => {
                            return Err("selected vertex does not belong to a polygon".into());
                        }
                    };
                    self.editor.apply(LayoutEdit::SetShapeGeometry {
                        id: id.clone(),
                        geometry,
                    })?;
                    Ok(LayoutEventOutcome::ShapeVertexMoved {
                        id,
                        vertex,
                        point: reported_point,
                    })
                } else {
                    Ok(LayoutEventOutcome::Ignored)
                }
            }
        }
    }

    fn finish_polygon_any_angle(&mut self) -> Result<LayoutEventOutcome, String> {
        if self.tool != LayoutTool::AnyAnglePolygon {
            return Ok(LayoutEventOutcome::Ignored);
        }
        let points = std::mem::take(&mut self.polygon_points);
        let polygon = AnyAnglePolygon::new(points)?;
        let id = self.unique_id("shape");
        self.editor.apply(LayoutEdit::AddShape(LayoutShape {
            id: id.clone(),
            layer: self.active_layer.clone(),
            net: self.active_net.clone(),
            geometry: LayoutGeometry::AnyAngle(polygon),
        }))?;
        self.selection = LayoutSelection::Shape { id: id.clone() };
        Ok(LayoutEventOutcome::ShapeAdded { id })
    }

    fn finish_polygon(&mut self) -> Result<LayoutEventOutcome, String> {
        if self.tool == LayoutTool::AnyAnglePolygon {
            return self.finish_polygon_any_angle();
        }
        if self.tool != LayoutTool::Polygon {
            return Ok(LayoutEventOutcome::Ignored);
        }
        let points = std::mem::take(&mut self.polygon_points);
        let polygon = LayoutPolygon::new(points)?;
        let id = self.unique_id("shape");
        self.editor.apply(LayoutEdit::AddShape(LayoutShape {
            id: id.clone(),
            layer: self.active_layer.clone(),
            net: self.active_net.clone(),
            geometry: LayoutGeometry::Polygon(polygon),
        }))?;
        self.selection = LayoutSelection::Shape { id: id.clone() };
        Ok(LayoutEventOutcome::ShapeAdded { id })
    }
    fn delete_selection(&mut self) -> Result<LayoutEventOutcome, String> {
        self.cancel_pending();
        let selection = self.selection.clone();
        match selection {
            LayoutSelection::Shape { ref id } => {
                self.editor
                    .apply(LayoutEdit::RemoveShape { id: id.clone() })?;
            }
            LayoutSelection::Via { ref id } => {
                self.editor
                    .apply(LayoutEdit::RemoveVia { id: id.clone() })?;
            }
            LayoutSelection::ShapeVertex { .. } => return Ok(LayoutEventOutcome::Ignored),
            LayoutSelection::None => return Ok(LayoutEventOutcome::Ignored),
        }
        self.selection = LayoutSelection::None;
        Ok(LayoutEventOutcome::Deleted { selection })
    }

    fn unique_id(&self, prefix: &str) -> String {
        let mut index = 1_usize;
        loop {
            let candidate = format!("{prefix}{index}");
            let exists = self
                .document()
                .shapes()
                .iter()
                .any(|shape| shape.id == candidate)
                || self.document().vias().iter().any(|via| via.id == candidate);
            if !exists {
                return candidate;
            }
            index += 1;
        }
    }
}

fn dragged_bbox(drag: LayoutDrag) -> LayoutRect {
    LayoutRect::new(
        drag.bbox.x1 + drag.pointer_current.x - drag.pointer_start.x,
        drag.bbox.y1 + drag.pointer_current.y - drag.pointer_start.y,
        drag.bbox.x2 + drag.pointer_current.x - drag.pointer_start.x,
        drag.bbox.y2 + drag.pointer_current.y - drag.pointer_start.y,
    )
    .unwrap_or(drag.bbox)
}

fn polygon_next_point(points: &[LayoutPoint], click: LayoutPoint) -> LayoutPoint {
    let Some(previous) = points.last() else {
        return click;
    };
    if points.len() % 2 == 1 {
        LayoutPoint::new(click.x, previous.y)
    } else {
        LayoutPoint::new(previous.x, click.y)
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn bbox_intersects(a: LayoutRect, b: LayoutRect) -> bool {
    a.x1 <= b.x2 && b.x1 <= a.x2 && a.y1 <= b.y2 && b.y1 <= a.y2
}

fn orientation(a: LayoutPoint, b: LayoutPoint, c: LayoutPoint) -> i64 {
    (b.x - a.x).saturating_mul(c.y - b.y) - (b.y - a.y).saturating_mul(c.x - b.x)
}

fn on_segment(a: LayoutPoint, b: LayoutPoint, point: LayoutPoint) -> bool {
    point.x >= a.x.min(b.x)
        && point.x <= a.x.max(b.x)
        && point.y >= a.y.min(b.y)
        && point.y <= a.y.max(b.y)
}

fn segments_intersect(a: LayoutPoint, b: LayoutPoint, c: LayoutPoint, d: LayoutPoint) -> bool {
    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    (abc.signum() != abd.signum() && cda.signum() != cdb.signum())
        || (abc == 0 && on_segment(a, b, c))
        || (abd == 0 && on_segment(a, b, d))
        || (cda == 0 && on_segment(c, d, a))
        || (cdb == 0 && on_segment(c, d, b))
}

fn segments_properly_cross(a: LayoutPoint, b: LayoutPoint, c: LayoutPoint, d: LayoutPoint) -> bool {
    let abc = orientation(a, b, c);
    let abd = orientation(a, b, d);
    let cda = orientation(c, d, a);
    let cdb = orientation(c, d, b);
    abc.signum() * abd.signum() < 0 && cda.signum() * cdb.signum() < 0
}

fn positive_area_overlap(left: &LayoutGeometry, right: &LayoutGeometry) -> Result<bool, String> {
    if !matches!(left, LayoutGeometry::AnyAngle(_)) && !matches!(right, LayoutGeometry::AnyAngle(_))
    {
        return manhattan_boolean(left, right, ManhattanBooleanOp::Intersection)
            .map(|result| result.area > 0);
    }
    any_angle_boolean(
        &left.try_into_any_angle()?,
        &right.try_into_any_angle()?,
        AnyAngleBooleanOperation::Intersection,
    )
    .map(|result| result.total_area > 0.0)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(write_layout_rule_deck(&restored).unwrap(), written);

        assert!(read_layout_rule_deck("").is_err());
        assert!(read_layout_rule_deck("version 2\n").is_err());
        assert!(read_layout_rule_deck("version 1\nlayer M1\n").is_err());
        assert!(read_layout_rule_deck("version 1\nspacing M1 M2 10\n").is_err());
        assert!(read_layout_rule_deck("version 1\nvia V1 V1 M1 M2\n").is_err());
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
            LayoutDrcWaiver::new("", "LAYOUT.MIN_WIDTH", None, None, None, "owner", "reason")
                .is_err()
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
}
