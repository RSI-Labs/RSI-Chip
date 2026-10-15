//! Polygon- and via-level layout database with deterministic editing and DRC.
//!
//! The database uses signed database-unit coordinates. Polygons are required
//! to be simple and rectilinear, which covers Manhattan routing shapes while
//! keeping geometry, hit testing, spacing, enclosure, and intersection checks
//! exact and dependency-free.

use crate::any_angle::{any_angle_boolean, AnyAngleBooleanOperation, AnyAnglePolygon};
use crate::DrcSeverity;
use std::collections::{BTreeMap, BTreeSet};

mod document;
mod editor;
mod geometry;

pub use editor::*;
use geometry::*;

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
    antenna_ratio: Option<f64>,
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

fn rule_deck_positive_f64(tokens: &[&str], index: usize, line_no: usize) -> Result<f64, String> {
    let value = tokens
        .get(index)
        .and_then(|value| value.parse::<f64>().ok())
        .ok_or_else(|| format!("rule deck line {line_no} requires a positive number"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "rule deck line {line_no} requires a positive number"
        ));
    }
    Ok(value)
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
    if let Some(ratio) = document.antenna_ratio {
        out.push_str(&format!("antenna_ratio {ratio}\n"));
    }
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
    let mut saw_antenna_ratio = false;
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
            Some("antenna_ratio") => {
                if !saw_version {
                    return Err("rule deck requires an initial 'version 1'".into());
                }
                if tokens.len() != 2 || saw_antenna_ratio {
                    return Err(format!(
                        "rule deck line {line_no} must contain one positive antenna_ratio"
                    ));
                }
                document.antenna_ratio = Some(rule_deck_positive_f64(&tokens, 1, line_no)?);
                saw_antenna_ratio = true;
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

#[cfg(test)]
#[path = "layout/tests.rs"]
mod tests;
