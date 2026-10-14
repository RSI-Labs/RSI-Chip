//! Exact integer primitives for simple polygons with arbitrary edge angles.
//!
//! This module is deliberately independent from the mature Manhattan layout
//! editor/router. It provides a first-class typed representation and exact
//! area/bounds/containment queries while Boolean and DRC integration remain
//! explicit future work.

use crate::layout::{LayoutCoordinate, LayoutPoint, LayoutRect};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyAnglePolygon {
    points: Vec<LayoutPoint>,
}

/// A nonnegative rational squared distance.
///
/// The denominator remains explicit so projected perpendicular distances can
/// be compared without floating-point rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RationalSquare {
    pub numerator: i128,
    pub denominator: i128,
}

impl RationalSquare {
    pub fn zero() -> Self {
        Self {
            numerator: 0,
            denominator: 1,
        }
    }

    pub fn from_integer(value: i128) -> Result<Self, String> {
        if value < 0 {
            return Err("squared distance cannot be negative".into());
        }
        Ok(Self::new(value, 1))
    }

    fn new(numerator: i128, denominator: i128) -> Self {
        debug_assert!(numerator >= 0);
        debug_assert!(denominator > 0);
        Self {
            numerator,
            denominator,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.numerator == 0
    }

    pub fn less_equal_integer_square(&self, square: i128) -> Result<bool, String> {
        if square < 0 {
            return Err("comparison square cannot be negative".into());
        }
        let scaled_limit = square
            .checked_mul(self.denominator)
            .ok_or_else(|| "rational distance comparison overflow".to_string())?;
        Ok(self.numerator <= scaled_limit)
    }

    pub fn greater_equal_integer_square(&self, square: i128) -> Result<bool, String> {
        if square < 0 {
            return Err("comparison square cannot be negative".into());
        }
        let scaled_limit = square
            .checked_mul(self.denominator)
            .ok_or_else(|| "rational distance comparison overflow".to_string())?;
        Ok(self.numerator >= scaled_limit)
    }

    pub fn sqrt(&self) -> f64 {
        (self.numerator as f64 / self.denominator as f64).sqrt()
    }
}

/// An exact rational point produced by arbitrary-angle Boolean operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AnyAngleRationalPoint {
    pub x_numerator: i128,
    pub x_denominator: i128,
    pub y_numerator: i128,
    pub y_denominator: i128,
}

impl AnyAngleRationalPoint {
    pub fn integer(point: LayoutPoint) -> Self {
        Self {
            x_numerator: i128::from(point.x),
            x_denominator: 1,
            y_numerator: i128::from(point.y),
            y_denominator: 1,
        }
    }

    pub fn new(
        x_numerator: i128,
        x_denominator: i128,
        y_numerator: i128,
        y_denominator: i128,
    ) -> Result<Self, String> {
        let (x_numerator, x_denominator) = normalize_rational(x_numerator, x_denominator)?;
        let (y_numerator, y_denominator) = normalize_rational(y_numerator, y_denominator)?;
        Ok(Self {
            x_numerator,
            x_denominator,
            y_numerator,
            y_denominator,
        })
    }

    pub fn x(&self) -> f64 {
        self.x_numerator as f64 / self.x_denominator as f64
    }

    pub fn y(&self) -> f64 {
        self.y_numerator as f64 / self.y_denominator as f64
    }

    fn midpoint(&self, other: Self) -> Result<Self, String> {
        let x_numerator = self
            .x_numerator
            .checked_mul(other.x_denominator)
            .and_then(|left| {
                other
                    .x_numerator
                    .checked_mul(self.x_denominator)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or_else(|| "any-angle rational midpoint overflow".to_string())?;
        let x_denominator = self
            .x_denominator
            .checked_mul(other.x_denominator)
            .and_then(|value| value.checked_mul(2))
            .ok_or_else(|| "any-angle rational midpoint overflow".to_string())?;
        let y_numerator = self
            .y_numerator
            .checked_mul(other.y_denominator)
            .and_then(|left| {
                other
                    .y_numerator
                    .checked_mul(self.y_denominator)
                    .and_then(|right| left.checked_add(right))
            })
            .ok_or_else(|| "any-angle rational midpoint overflow".to_string())?;
        let y_denominator = self
            .y_denominator
            .checked_mul(other.y_denominator)
            .and_then(|value| value.checked_mul(2))
            .ok_or_else(|| "any-angle rational midpoint overflow".to_string())?;
        Self::new(x_numerator, x_denominator, y_numerator, y_denominator)
    }
}

/// A Boolean result polygon with exact rational vertices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyAngleRationalPolygon {
    points: Vec<AnyAngleRationalPoint>,
}

impl AnyAngleRationalPolygon {
    pub fn new(points: impl Into<Vec<AnyAngleRationalPoint>>) -> Result<Self, String> {
        let polygon = Self {
            points: points.into(),
        };
        if polygon.points.len() < 3 {
            return Err("any-angle rational polygon needs at least three vertices".into());
        }
        if polygon.points.first() == polygon.points.last() {
            return Err("any-angle rational polygon must omit the repeated closing vertex".into());
        }
        if polygon.points.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("any-angle rational polygon has duplicate consecutive vertices".into());
        }
        Ok(polygon)
    }

    pub fn points(&self) -> &[AnyAngleRationalPoint] {
        &self.points
    }

    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    pub fn signed_area(&self) -> f64 {
        let mut twice_area = 0.0;
        for index in 0..self.points.len() {
            let a = self.points[index];
            let b = self.points[(index + 1) % self.points.len()];
            twice_area += a.x() * b.y() - b.x() * a.y();
        }
        twice_area / 2.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnyAngleBooleanOperation {
    Union,
    Intersection,
    Difference,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnyAngleBooleanResult {
    pub polygons: Vec<AnyAngleRationalPolygon>,
    pub total_area: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnyAngleSpacingResult {
    pub required_dbu: i64,
    pub actual_distance_dbu: f64,
    pub compliant: bool,
}

impl AnyAnglePolygon {
    pub fn new(points: impl Into<Vec<LayoutPoint>>) -> Result<Self, String> {
        let polygon = Self {
            points: points.into(),
        };
        polygon.validate()?;
        Ok(polygon)
    }

    pub fn points(&self) -> &[LayoutPoint] {
        &self.points
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.points.len() < 3 {
            return Err("any-angle polygon needs at least three vertices".into());
        }
        if self.points.first() == self.points.last() {
            return Err("any-angle polygon must omit the repeated closing vertex".into());
        }
        for pair in self.points.windows(2) {
            if pair[0] == pair[1] {
                return Err("any-angle polygon has duplicate consecutive vertices".into());
            }
        }
        if self.has_nonadjacent_edge_intersection() {
            return Err("any-angle polygon is self-intersecting".into());
        }
        if self.signed_area2()? == 0 {
            return Err("any-angle polygon has zero area".into());
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
                if !adjacent && general_segments_intersect(a, b, c, d) {
                    return true;
                }
            }
        }
        false
    }

    /// Exact signed area multiplied by two, in square database units.
    pub fn signed_area2(&self) -> Result<i128, String> {
        let mut twice_area = 0_i128;
        for (a, b) in self.segments() {
            let term = i128::from(a.x)
                .checked_mul(i128::from(b.y))
                .and_then(|left| {
                    i128::from(b.x)
                        .checked_mul(i128::from(a.y))
                        .and_then(|right| left.checked_sub(right))
                })
                .ok_or_else(|| "any-angle polygon area overflow".to_string())?;
            twice_area = twice_area
                .checked_add(term)
                .ok_or_else(|| "any-angle polygon area overflow".to_string())?;
        }
        Ok(twice_area)
    }

    /// Unsigned area as a floating-point convenience. Callers that require
    /// exact half-unit precision should use [`Self::signed_area2`].
    pub fn area(&self) -> Result<f64, String> {
        Ok(self.signed_area2()?.unsigned_abs() as f64 / 2.0)
    }

    pub fn is_clockwise(&self) -> Result<bool, String> {
        Ok(self.signed_area2()? < 0)
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
        let points = self
            .points
            .iter()
            .map(|point| -> Result<LayoutPoint, String> {
                Ok(LayoutPoint::new(
                    point
                        .x
                        .checked_add(dx)
                        .ok_or_else(|| "any-angle polygon x overflow".to_string())?,
                    point
                        .y
                        .checked_add(dy)
                        .ok_or_else(|| "any-angle polygon y overflow".to_string())?,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(points)
    }

    pub fn contains_point(&self, point: LayoutPoint) -> bool {
        let mut inside = false;
        for (a, b) in self.segments() {
            if point_on_segment(point, a, b) {
                return true;
            }
            if (a.y > point.y) != (b.y > point.y) {
                let cross = i128::from(b.x - a.x) * i128::from(point.y - a.y)
                    - i128::from(b.y - a.y) * i128::from(point.x - a.x);
                let dy = i128::from(b.y - a.y);
                if cross * dy > 0 {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// Returns true when the boundaries intersect or either interior contains
    /// a vertex of the other polygon.
    pub fn intersects(&self, other: &Self) -> bool {
        self.segments().any(|(a, b)| {
            other
                .segments()
                .any(|(c, d)| general_segments_intersect(a, b, c, d))
        }) || self.points.iter().any(|point| other.contains_point(*point))
            || other.points.iter().any(|point| self.contains_point(*point))
    }

    pub fn intersects_segment(&self, a: LayoutPoint, b: LayoutPoint) -> bool {
        self.segments()
            .any(|(c, d)| general_segments_intersect(a, b, c, d))
            || self.contains_point(a)
            || self.contains_point(b)
    }

    /// Exact squared distance between two disjoint simple polygons.
    pub fn distance_squared_to(&self, other: &Self) -> Result<RationalSquare, String> {
        if self.intersects(other) {
            return Ok(RationalSquare::zero());
        }
        let mut minimum = None::<RationalSquare>;
        for (a, b) in self.segments() {
            for (c, d) in other.segments() {
                let candidate = segment_distance_squared(a, b, c, d)?;
                minimum = Some(match minimum {
                    Some(current) if rational_less_equal(current, candidate)? => current,
                    _ => candidate,
                });
            }
        }
        minimum.ok_or_else(|| "any-angle polygon distance requires edges".to_string())
    }

    pub fn check_spacing_to(
        &self,
        other: &Self,
        required_dbu: i64,
    ) -> Result<AnyAngleSpacingResult, String> {
        if required_dbu < 0 {
            return Err("any-angle spacing requirement cannot be negative".into());
        }
        let distance_squared = self.distance_squared_to(other)?;
        let required_square = i128::from(required_dbu)
            .checked_mul(i128::from(required_dbu))
            .ok_or_else(|| "any-angle spacing requirement overflow".to_string())?;
        Ok(AnyAngleSpacingResult {
            required_dbu,
            actual_distance_dbu: distance_squared.sqrt(),
            compliant: distance_squared.greater_equal_integer_square(required_square)?,
        })
    }

    pub fn edge_lengths_squared(&self) -> Vec<u128> {
        self.segments()
            .map(|(a, b)| {
                let dx = (i128::from(b.x) - i128::from(a.x)).unsigned_abs();
                let dy = (i128::from(b.y) - i128::from(a.y)).unsigned_abs();
                dx * dx + dy * dy
            })
            .collect()
    }

    pub fn perimeter(&self) -> f64 {
        self.edge_lengths_squared()
            .into_iter()
            .map(|length| (length as f64).sqrt())
            .sum()
    }
}

fn gcd_u128(left: u128, right: u128) -> u128 {
    let mut left = left;
    let mut right = right;
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn normalize_rational(numerator: i128, denominator: i128) -> Result<(i128, i128), String> {
    if denominator == 0 {
        return Err("any-angle rational denominator cannot be zero".into());
    }
    let sign = if (numerator < 0) != (denominator < 0) {
        -1_i128
    } else {
        1_i128
    };
    let mut numerator = numerator.unsigned_abs();
    let mut denominator = denominator.unsigned_abs();
    let divisor = gcd_u128(numerator, denominator);
    if divisor != 0 {
        numerator /= divisor;
        denominator /= divisor;
    }
    if numerator > i128::MAX as u128 || denominator > i128::MAX as u128 {
        return Err("any-angle rational coordinate overflow".into());
    }
    Ok((sign * numerator as i128, denominator as i128))
}

fn compare_rational(
    left_numerator: i128,
    left_denominator: i128,
    right_numerator: i128,
    right_denominator: i128,
) -> Result<std::cmp::Ordering, String> {
    let (_, _) = (
        normalize_rational(left_numerator, left_denominator)?,
        normalize_rational(right_numerator, right_denominator)?,
    );
    let left = left_numerator
        .checked_mul(right_denominator)
        .ok_or_else(|| "any-angle rational comparison overflow".to_string())?;
    let right = right_numerator
        .checked_mul(left_denominator)
        .ok_or_else(|| "any-angle rational comparison overflow".to_string())?;
    Ok(left.cmp(&right))
}

fn rational_point_on_integer_segment(
    point: AnyAngleRationalPoint,
    a: LayoutPoint,
    b: LayoutPoint,
) -> Result<bool, String> {
    let dx = i128::from(b.x - a.x);
    let dy = i128::from(b.y - a.y);
    let relative_y = point
        .y_numerator
        .checked_sub(
            i128::from(a.y)
                .checked_mul(point.y_denominator)
                .ok_or_else(|| "any-angle containment overflow".to_string())?,
        )
        .ok_or_else(|| "any-angle containment overflow".to_string())?;
    let relative_x = point
        .x_numerator
        .checked_sub(
            i128::from(a.x)
                .checked_mul(point.x_denominator)
                .ok_or_else(|| "any-angle containment overflow".to_string())?,
        )
        .ok_or_else(|| "any-angle containment overflow".to_string())?;
    let cross = dx
        .checked_mul(relative_y)
        .and_then(|left| left.checked_mul(point.x_denominator))
        .and_then(|left| {
            dy.checked_mul(relative_x)
                .and_then(|right| right.checked_mul(point.y_denominator))
                .map(|right| left - right)
        })
        .ok_or_else(|| "any-angle containment overflow".to_string())?;
    if cross != 0 {
        return Ok(false);
    }
    let x_order = compare_rational(point.x_numerator, point.x_denominator, i128::from(a.x), 1)?;
    let y_order = compare_rational(point.y_numerator, point.y_denominator, i128::from(a.y), 1)?;
    let x_end_order = compare_rational(point.x_numerator, point.x_denominator, i128::from(b.x), 1)?;
    let y_end_order = compare_rational(point.y_numerator, point.y_denominator, i128::from(b.y), 1)?;
    let x_inside = if a.x <= b.x {
        x_order != std::cmp::Ordering::Less && x_end_order != std::cmp::Ordering::Greater
    } else {
        x_order != std::cmp::Ordering::Greater && x_end_order != std::cmp::Ordering::Less
    };
    let y_inside = if a.y <= b.y {
        y_order != std::cmp::Ordering::Less && y_end_order != std::cmp::Ordering::Greater
    } else {
        y_order != std::cmp::Ordering::Greater && y_end_order != std::cmp::Ordering::Less
    };
    Ok(x_inside && y_inside)
}

fn rational_polygon_contains(
    polygon: &AnyAnglePolygon,
    point: AnyAngleRationalPoint,
) -> Result<bool, String> {
    let mut inside = false;
    for (a, b) in polygon.segments() {
        if rational_point_on_integer_segment(point, a, b)? {
            return Ok(true);
        }
        let py_above_a =
            compare_rational(point.y_numerator, point.y_denominator, i128::from(a.y), 1)?
                == std::cmp::Ordering::Greater;
        let py_above_b =
            compare_rational(point.y_numerator, point.y_denominator, i128::from(b.y), 1)?
                == std::cmp::Ordering::Greater;
        if py_above_a == py_above_b {
            continue;
        }
        let dy_numerator = point
            .y_numerator
            .checked_sub(
                i128::from(a.y)
                    .checked_mul(point.y_denominator)
                    .ok_or_else(|| "any-angle containment overflow".to_string())?,
            )
            .ok_or_else(|| "any-angle containment overflow".to_string())?;
        let edge_dx = i128::from(b.x - a.x);
        let edge_dy = i128::from(b.y - a.y);
        let intersection_numerator = i128::from(a.x)
            .checked_mul(edge_dy)
            .and_then(|base| base.checked_mul(point.y_denominator))
            .and_then(|base| base.checked_add(edge_dx.checked_mul(dy_numerator)?))
            .ok_or_else(|| "any-angle containment overflow".to_string())?;
        let intersection_denominator = edge_dy
            .checked_mul(point.y_denominator)
            .ok_or_else(|| "any-angle containment overflow".to_string())?;
        let (intersection_numerator, intersection_denominator) =
            normalize_rational(intersection_numerator, intersection_denominator)?;
        if compare_rational(
            intersection_numerator,
            intersection_denominator,
            point.x_numerator,
            point.x_denominator,
        )? == std::cmp::Ordering::Greater
        {
            inside = !inside;
        }
    }
    Ok(inside)
}

fn rational_polygon_on_boundary(
    polygon: &AnyAnglePolygon,
    point: AnyAngleRationalPoint,
) -> Result<bool, String> {
    for (a, b) in polygon.segments() {
        if rational_point_on_integer_segment(point, a, b)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn rational_polygon_strict_contains(
    polygon: &AnyAnglePolygon,
    point: AnyAngleRationalPoint,
) -> Result<bool, String> {
    Ok(
        rational_polygon_contains(polygon, point)?
            && !rational_polygon_on_boundary(polygon, point)?,
    )
}

fn segment_parameter(
    point: AnyAngleRationalPoint,
    a: LayoutPoint,
    b: LayoutPoint,
) -> Result<(i128, i128), String> {
    let dx = i128::from(b.x - a.x);
    let dy = i128::from(b.y - a.y);
    let px = point
        .x_numerator
        .checked_sub(
            i128::from(a.x)
                .checked_mul(point.x_denominator)
                .ok_or_else(|| "any-angle segment parameter overflow".to_string())?,
        )
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    let py = point
        .y_numerator
        .checked_sub(
            i128::from(a.y)
                .checked_mul(point.y_denominator)
                .ok_or_else(|| "any-angle segment parameter overflow".to_string())?,
        )
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    let x_term = dx
        .checked_mul(px)
        .and_then(|value| value.checked_mul(point.y_denominator))
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    let y_term = dy
        .checked_mul(py)
        .and_then(|value| value.checked_mul(point.x_denominator))
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    let numerator = x_term
        .checked_add(y_term)
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    let denominator = (dx * dx + dy * dy)
        .checked_mul(point.x_denominator)
        .and_then(|value| value.checked_mul(point.y_denominator))
        .ok_or_else(|| "any-angle segment parameter overflow".to_string())?;
    normalize_rational(numerator, denominator)
}

fn line_intersection_point(
    a: LayoutPoint,
    b: LayoutPoint,
    c: LayoutPoint,
    d: LayoutPoint,
) -> Result<Option<AnyAngleRationalPoint>, String> {
    let segment_dx = i128::from(b.x - a.x);
    let segment_dy = i128::from(b.y - a.y);
    let other_dx = i128::from(d.x - c.x);
    let other_dy = i128::from(d.y - c.y);
    let denominator = segment_dx
        .checked_mul(other_dy)
        .and_then(|left| segment_dy.checked_mul(other_dx).map(|right| left - right))
        .ok_or_else(|| "any-angle Boolean intersection overflow".to_string())?;
    if denominator == 0 {
        return Ok(None);
    }
    let numerator = i128::from(c.x - a.x)
        .checked_mul(other_dy)
        .and_then(|left| {
            i128::from(c.y - a.y)
                .checked_mul(other_dx)
                .map(|right| left - right)
        })
        .ok_or_else(|| "any-angle Boolean intersection overflow".to_string())?;
    let x_numerator = i128::from(a.x)
        .checked_mul(denominator)
        .and_then(|base| base.checked_add(segment_dx.checked_mul(numerator)?))
        .ok_or_else(|| "any-angle Boolean intersection overflow".to_string())?;
    let y_numerator = i128::from(a.y)
        .checked_mul(denominator)
        .and_then(|base| base.checked_add(segment_dy.checked_mul(numerator)?))
        .ok_or_else(|| "any-angle Boolean intersection overflow".to_string())?;
    let point = AnyAngleRationalPoint::new(x_numerator, denominator, y_numerator, denominator)?;
    if rational_point_on_integer_segment(point, a, b)?
        && rational_point_on_integer_segment(point, c, d)?
    {
        Ok(Some(point))
    } else {
        Ok(None)
    }
}

fn split_segment_by_polygon(
    a: LayoutPoint,
    b: LayoutPoint,
    polygon: &AnyAnglePolygon,
) -> Result<Vec<AnyAngleRationalPoint>, String> {
    let mut points = vec![
        AnyAngleRationalPoint::integer(a),
        AnyAngleRationalPoint::integer(b),
    ];
    for (c, d) in polygon.segments() {
        if let Some(point) = line_intersection_point(a, b, c, d)? {
            points.push(point);
        }
        if point_on_segment(c, a, b) {
            points.push(AnyAngleRationalPoint::integer(c));
        }
        if point_on_segment(d, a, b) {
            points.push(AnyAngleRationalPoint::integer(d));
        }
    }
    let mut sorted = Vec::with_capacity(points.len());
    for point in points {
        let mut index = sorted.len();
        while index > 0 {
            let previous_parameter = segment_parameter(sorted[index - 1], a, b)?;
            let point_parameter = segment_parameter(point, a, b)?;
            if compare_rational(
                point_parameter.0,
                point_parameter.1,
                previous_parameter.0,
                previous_parameter.1,
            )? == std::cmp::Ordering::Less
            {
                index -= 1;
            } else {
                break;
            }
        }
        if !sorted.contains(&point) {
            sorted.insert(index, point);
        }
    }
    Ok(sorted)
}

#[derive(Clone, Copy, Debug)]
struct RationalSegment {
    start: AnyAngleRationalPoint,
    end: AnyAngleRationalPoint,
}

impl RationalSegment {
    fn undirected_key(self) -> (AnyAngleRationalPoint, AnyAngleRationalPoint) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

fn collect_boundary_segments(
    left: &AnyAnglePolygon,
    right: &AnyAnglePolygon,
    operation: AnyAngleBooleanOperation,
) -> Result<Vec<RationalSegment>, String> {
    let mut out = Vec::new();
    for (a, b) in left.segments() {
        let points = split_segment_by_polygon(a, b, right)?;
        for pair in points.windows(2) {
            let midpoint = pair[0].midpoint(pair[1])?;
            let keep = match operation {
                AnyAngleBooleanOperation::Union => {
                    !rational_polygon_strict_contains(right, midpoint)?
                }
                AnyAngleBooleanOperation::Intersection => {
                    rational_polygon_contains(right, midpoint)?
                }
                AnyAngleBooleanOperation::Difference => {
                    !rational_polygon_contains(right, midpoint)?
                }
            };
            if keep {
                out.push(RationalSegment {
                    start: pair[0],
                    end: pair[1],
                });
            }
        }
    }
    for (a, b) in right.segments() {
        let points = split_segment_by_polygon(a, b, left)?;
        for pair in points.windows(2) {
            let midpoint = pair[0].midpoint(pair[1])?;
            let keep = match operation {
                AnyAngleBooleanOperation::Union => {
                    !rational_polygon_strict_contains(left, midpoint)?
                }
                AnyAngleBooleanOperation::Intersection => {
                    rational_polygon_contains(left, midpoint)?
                }
                AnyAngleBooleanOperation::Difference => {
                    rational_polygon_strict_contains(left, midpoint)?
                }
            };
            if keep {
                let mut start = pair[0];
                let mut end = pair[1];
                if operation == AnyAngleBooleanOperation::Difference {
                    std::mem::swap(&mut start, &mut end);
                }
                out.push(RationalSegment { start, end });
            }
        }
    }
    out.sort_by_key(|segment| segment.undirected_key());
    out.dedup_by_key(|segment| segment.undirected_key());
    Ok(out)
}

fn points_are_collinear(
    first: AnyAngleRationalPoint,
    middle: AnyAngleRationalPoint,
    last: AnyAngleRationalPoint,
) -> Result<bool, String> {
    let rational_difference = |left_numerator: i128,
                               left_denominator: i128,
                               right_numerator: i128,
                               right_denominator: i128|
     -> Result<(i128, i128), String> {
        let numerator = left_numerator
            .checked_mul(right_denominator)
            .and_then(|left| {
                right_numerator
                    .checked_mul(left_denominator)
                    .map(|right| left - right)
            })
            .ok_or_else(|| "any-angle collinearity overflow".to_string())?;
        let denominator = left_denominator
            .checked_mul(right_denominator)
            .ok_or_else(|| "any-angle collinearity overflow".to_string())?;
        normalize_rational(numerator, denominator)
    };
    let (left_dx_numerator, left_dx_denominator) = rational_difference(
        middle.x_numerator,
        middle.x_denominator,
        first.x_numerator,
        first.x_denominator,
    )?;
    let (left_dy_numerator, left_dy_denominator) = rational_difference(
        middle.y_numerator,
        middle.y_denominator,
        first.y_numerator,
        first.y_denominator,
    )?;
    let (right_dx_numerator, right_dx_denominator) = rational_difference(
        last.x_numerator,
        last.x_denominator,
        first.x_numerator,
        first.x_denominator,
    )?;
    let (right_dy_numerator, right_dy_denominator) = rational_difference(
        last.y_numerator,
        last.y_denominator,
        first.y_numerator,
        first.y_denominator,
    )?;
    let left = left_dx_numerator
        .checked_mul(right_dy_numerator)
        .ok_or_else(|| "any-angle collinearity overflow".to_string())?;
    let right = left_dy_numerator
        .checked_mul(right_dx_numerator)
        .ok_or_else(|| "any-angle collinearity overflow".to_string())?;
    let denominator = left_dx_denominator
        .checked_mul(right_dy_denominator)
        .and_then(|left| {
            left_dy_denominator
                .checked_mul(right_dx_denominator)
                .map(|right| left * right)
        })
        .ok_or_else(|| "any-angle collinearity overflow".to_string())?;
    Ok(denominator != 0 && left == right)
}

fn assemble_boundary_polygons(
    segments: Vec<RationalSegment>,
) -> Result<Vec<AnyAngleRationalPolygon>, String> {
    let mut starts = BTreeMap::<AnyAngleRationalPoint, Vec<usize>>::new();
    for (index, segment) in segments.iter().enumerate() {
        starts.entry(segment.start).or_default().push(index);
    }
    let mut used = vec![false; segments.len()];
    let mut polygons = Vec::new();
    for seed in 0..segments.len() {
        if used[seed] {
            continue;
        }
        used[seed] = true;
        let mut points = vec![segments[seed].start, segments[seed].end];
        let mut cursor = segments[seed].end;
        while cursor != segments[seed].start {
            let Some(candidates) = starts.get(&cursor) else {
                return Err("any-angle Boolean boundary did not close".into());
            };
            let next = candidates
                .iter()
                .copied()
                .find(|index| !used[*index])
                .ok_or_else(|| "any-angle Boolean boundary did not close".to_string())?;
            used[next] = true;
            cursor = segments[next].end;
            if cursor != segments[seed].start {
                points.push(cursor);
            }
        }
        while points.len() > 3 {
            let last = points.len() - 1;
            if points_are_collinear(points[last - 2], points[last - 1], points[last])? {
                points.remove(last - 1);
            } else {
                break;
            }
        }
        if points.len() >= 3 {
            polygons.push(AnyAngleRationalPolygon::new(points)?);
        }
    }
    Ok(polygons)
}

/// Perform an exact Boolean operation on two simple arbitrary-angle polygons.
///
/// Result vertices are rational because arbitrary-angle edge intersections
/// generally do not lie on integer database-unit coordinates. Multiple result
/// polygons can represent disjoint pieces or interior holes; caller-side area
/// should use absolute loop areas rather than signed orientation.
pub fn any_angle_boolean(
    left: &AnyAnglePolygon,
    right: &AnyAnglePolygon,
    operation: AnyAngleBooleanOperation,
) -> Result<AnyAngleBooleanResult, String> {
    let segments = collect_boundary_segments(left, right, operation)?;
    let polygons = assemble_boundary_polygons(segments)?;
    let total_area = polygons
        .iter()
        .map(AnyAngleRationalPolygon::signed_area)
        .sum::<f64>()
        .abs();
    Ok(AnyAngleBooleanResult {
        polygons,
        total_area,
    })
}

fn orientation(a: LayoutPoint, b: LayoutPoint, c: LayoutPoint) -> i128 {
    i128::from(b.x - a.x) * i128::from(c.y - a.y) - i128::from(b.y - a.y) * i128::from(c.x - a.x)
}

fn point_on_segment(point: LayoutPoint, a: LayoutPoint, b: LayoutPoint) -> bool {
    orientation(a, b, point) == 0
        && point.x >= a.x.min(b.x)
        && point.x <= a.x.max(b.x)
        && point.y >= a.y.min(b.y)
        && point.y <= a.y.max(b.y)
}

pub(crate) fn general_segments_intersect(
    a: LayoutPoint,
    b: LayoutPoint,
    c: LayoutPoint,
    d: LayoutPoint,
) -> bool {
    let o1 = orientation(a, b, c);
    let o2 = orientation(a, b, d);
    let o3 = orientation(c, d, a);
    let o4 = orientation(c, d, b);
    if (o1 < 0 && o2 > 0 || o1 > 0 && o2 < 0) && (o3 < 0 && o4 > 0 || o3 > 0 && o4 < 0) {
        return true;
    }
    (o1 == 0 && point_on_segment(c, a, b))
        || (o2 == 0 && point_on_segment(d, a, b))
        || (o3 == 0 && point_on_segment(a, c, d))
        || (o4 == 0 && point_on_segment(b, c, d))
}

fn checked_rational_square(numerator: i128, denominator: i128) -> Result<RationalSquare, String> {
    if numerator < 0 || denominator <= 0 {
        return Err("invalid rational squared distance".into());
    }
    let common = gcd(numerator, denominator);
    let numerator = numerator / common;
    let denominator = denominator / common;
    Ok(RationalSquare::new(numerator, denominator))
}

fn gcd(left: i128, right: i128) -> i128 {
    let mut left = left.unsigned_abs();
    let mut right = right.unsigned_abs();
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left.try_into().unwrap_or(1)
}

fn rational_less_equal(left: RationalSquare, right: RationalSquare) -> Result<bool, String> {
    let left_scaled = left
        .numerator
        .checked_mul(right.denominator)
        .ok_or_else(|| "rational distance comparison overflow".to_string())?;
    let right_scaled = right
        .numerator
        .checked_mul(left.denominator)
        .ok_or_else(|| "rational distance comparison overflow".to_string())?;
    Ok(left_scaled <= right_scaled)
}

fn min_rational(left: RationalSquare, right: RationalSquare) -> Result<RationalSquare, String> {
    if rational_less_equal(left, right)? {
        Ok(left)
    } else {
        Ok(right)
    }
}

fn endpoint_distance_squared(a: LayoutPoint, b: LayoutPoint) -> Result<RationalSquare, String> {
    let dx = i128::from(a.x) - i128::from(b.x);
    let dy = i128::from(a.y) - i128::from(b.y);
    let square = dx
        .checked_mul(dx)
        .and_then(|x| dy.checked_mul(dy).and_then(|y| x.checked_add(y)))
        .ok_or_else(|| "any-angle distance overflow".to_string())?;
    RationalSquare::from_integer(square)
}

fn point_segment_distance_squared(
    point: LayoutPoint,
    a: LayoutPoint,
    b: LayoutPoint,
) -> Result<RationalSquare, String> {
    let dx = i128::from(point.x) - i128::from(a.x);
    let dy = i128::from(point.y) - i128::from(a.y);
    let vx = i128::from(b.x) - i128::from(a.x);
    let vy = i128::from(b.y) - i128::from(a.y);
    let length_squared = vx
        .checked_mul(vx)
        .and_then(|x| vy.checked_mul(vy).and_then(|y| x.checked_add(y)))
        .ok_or_else(|| "any-angle distance overflow".to_string())?;
    if length_squared == 0 {
        return Err("any-angle polygon has a zero-length edge".into());
    }
    let dot = dx
        .checked_mul(vx)
        .and_then(|x| dy.checked_mul(vy).and_then(|y| x.checked_add(y)))
        .ok_or_else(|| "any-angle distance overflow".to_string())?;
    if dot <= 0 {
        return endpoint_distance_squared(point, a);
    }
    if dot >= length_squared {
        return endpoint_distance_squared(point, b);
    }
    let point_squared = dx
        .checked_mul(dx)
        .and_then(|x| dy.checked_mul(dy).and_then(|y| x.checked_add(y)))
        .ok_or_else(|| "any-angle distance overflow".to_string())?;
    let numerator = point_squared
        .checked_mul(length_squared)
        .and_then(|scaled| {
            dot.checked_mul(dot)
                .and_then(|dot_squared| scaled.checked_sub(dot_squared))
        })
        .ok_or_else(|| "any-angle distance overflow".to_string())?;
    checked_rational_square(numerator, length_squared)
}

pub(crate) fn segment_distance_squared(
    a: LayoutPoint,
    b: LayoutPoint,
    c: LayoutPoint,
    d: LayoutPoint,
) -> Result<RationalSquare, String> {
    if general_segments_intersect(a, b, c, d) {
        return Ok(RationalSquare::zero());
    }
    let candidates = [
        point_segment_distance_squared(c, a, b)?,
        point_segment_distance_squared(d, a, b)?,
        point_segment_distance_squared(a, c, d)?,
        point_segment_distance_squared(b, c, d)?,
    ];
    let mut minimum = candidates[0];
    for candidate in candidates.into_iter().skip(1) {
        minimum = min_rational(minimum, candidate)?;
    }
    Ok(minimum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arbitrary_triangle_queries_are_exact() {
        let triangle = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(7, 3),
            LayoutPoint::new(2, 9),
        ])
        .unwrap();
        assert_eq!(triangle.signed_area2().unwrap(), 57);
        assert!((triangle.area().unwrap() - 28.5).abs() < 1e-12);
        assert_eq!(
            triangle.bbox(),
            LayoutRect {
                x1: 0,
                y1: 0,
                x2: 7,
                y2: 9
            }
        );
        assert!(triangle.contains_point(LayoutPoint::new(3, 4)));
        assert!(!triangle.contains_point(LayoutPoint::new(20, 4)));
        assert!(triangle.contains_point(LayoutPoint::new(7, 3)));
    }

    #[test]
    fn concave_arbitrary_polygon_contains_notch() {
        let polygon = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(10, 2),
            LayoutPoint::new(12, 12),
            LayoutPoint::new(5, 7),
            LayoutPoint::new(1, 11),
        ])
        .unwrap();
        assert!(polygon.contains_point(LayoutPoint::new(2, 2)));
        assert!(!polygon.contains_point(LayoutPoint::new(6, 9)));
        assert!(polygon.contains_point(LayoutPoint::new(5, 7)));
    }

    #[test]
    fn rejects_invalid_arbitrary_polygons() {
        assert!(AnyAnglePolygon::new([LayoutPoint::new(0, 0), LayoutPoint::new(5, 5),]).is_err());
        assert!(AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(5, 5),
            LayoutPoint::new(10, 10),
        ])
        .is_err());
        assert!(AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(10, 0),
            LayoutPoint::new(0, 10),
            LayoutPoint::new(10, 10),
        ])
        .is_err());
        assert!(AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(5, 0),
            LayoutPoint::new(5, 5),
            LayoutPoint::new(0, 0),
        ])
        .is_err());
    }

    #[test]
    fn arbitrary_polygon_spacing_is_exact() {
        let left = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(10, 0),
            LayoutPoint::new(0, 10),
        ])
        .unwrap();
        let right = left.translated(15, 0).unwrap();
        let distance = left.distance_squared_to(&right).unwrap();
        assert_eq!(distance, RationalSquare::from_integer(25).unwrap());
        assert!((distance.sqrt() - 5.0).abs() < 1e-12);
        let exact = left.check_spacing_to(&right, 5).unwrap();
        let too_close = left.check_spacing_to(&right, 6).unwrap();
        assert!(exact.compliant);
        assert!(!too_close.compliant);

        let touching = left.translated(10, 0).unwrap();
        assert!(left.distance_squared_to(&touching).unwrap().is_zero());
        assert!(!left.check_spacing_to(&touching, 1).unwrap().compliant);

        let contained = AnyAnglePolygon::new([
            LayoutPoint::new(1, 1),
            LayoutPoint::new(4, 1),
            LayoutPoint::new(1, 4),
        ])
        .unwrap();
        assert!(left.distance_squared_to(&contained).unwrap().is_zero());
    }

    #[test]
    fn arbitrary_polygon_perpendicular_distance_is_rational() {
        let left = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(0, 10),
            LayoutPoint::new(10, 10),
        ])
        .unwrap();
        let right = AnyAnglePolygon::new([
            LayoutPoint::new(11, 0),
            LayoutPoint::new(14, 0),
            LayoutPoint::new(14, 3),
        ])
        .unwrap();
        let distance = left.distance_squared_to(&right).unwrap();
        assert_eq!(distance, checked_rational_square(121, 2).unwrap());
        assert!((distance.sqrt() - f64::sqrt(60.5)).abs() < 1e-12);
        assert!(left.check_spacing_to(&right, 7).unwrap().compliant);
        assert!(!left.check_spacing_to(&right, 8).unwrap().compliant);
    }

    #[test]
    fn arbitrary_angle_boolean_operations_are_exact() {
        let left = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(6, 0),
            LayoutPoint::new(6, 6),
            LayoutPoint::new(0, 6),
        ])
        .unwrap();
        let right = AnyAnglePolygon::new([
            LayoutPoint::new(3, 3),
            LayoutPoint::new(9, 3),
            LayoutPoint::new(9, 9),
            LayoutPoint::new(3, 9),
        ])
        .unwrap();

        let union = any_angle_boolean(&left, &right, AnyAngleBooleanOperation::Union).unwrap();
        assert_eq!(union.polygons.len(), 1);
        assert!((union.total_area - 63.0).abs() < 1e-12);

        let intersection =
            any_angle_boolean(&left, &right, AnyAngleBooleanOperation::Intersection).unwrap();
        assert_eq!(intersection.polygons.len(), 1);
        assert!((intersection.total_area - 9.0).abs() < 1e-12);

        let difference =
            any_angle_boolean(&left, &right, AnyAngleBooleanOperation::Difference).unwrap();
        assert_eq!(difference.polygons.len(), 1);
        assert!((difference.total_area - 27.0).abs() < 1e-12);
    }

    #[test]
    fn arbitrary_angle_boolean_produces_rational_vertices() {
        let triangle = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(4, 0),
            LayoutPoint::new(0, 4),
        ])
        .unwrap();
        let overlapping = AnyAnglePolygon::new([
            LayoutPoint::new(1, 1),
            LayoutPoint::new(5, 2),
            LayoutPoint::new(2, 5),
        ])
        .unwrap();
        let result = any_angle_boolean(
            &triangle,
            &overlapping,
            AnyAngleBooleanOperation::Intersection,
        )
        .unwrap();
        assert_eq!(result.polygons.len(), 1);
        assert!((result.total_area - 1.2).abs() < 1e-12);
        let points = result.polygons[0].points();
        assert_eq!(points.len(), 3);
        assert!(
            points.iter().any(|point| {
                point.x_denominator == 5 && point.y_denominator == 5 && point.x_numerator % 5 != 0
            }),
            "{points:?}"
        );
    }

    #[test]
    fn arbitrary_angle_boolean_handles_identity_containment_and_disjoint() {
        let outer = AnyAnglePolygon::new([
            LayoutPoint::new(0, 0),
            LayoutPoint::new(10, 0),
            LayoutPoint::new(10, 10),
            LayoutPoint::new(0, 10),
        ])
        .unwrap();
        let identical = outer.clone();
        assert_eq!(
            any_angle_boolean(&outer, &identical, AnyAngleBooleanOperation::Union)
                .unwrap()
                .polygons
                .len(),
            1
        );
        assert!(
            any_angle_boolean(&outer, &identical, AnyAngleBooleanOperation::Difference)
                .unwrap()
                .polygons
                .is_empty()
        );

        let inner = AnyAnglePolygon::new([
            LayoutPoint::new(2, 2),
            LayoutPoint::new(8, 2),
            LayoutPoint::new(8, 8),
            LayoutPoint::new(2, 8),
        ])
        .unwrap();
        let hole = any_angle_boolean(&outer, &inner, AnyAngleBooleanOperation::Difference).unwrap();
        assert_eq!(hole.polygons.len(), 2);
        assert!((hole.total_area - 64.0).abs() < 1e-12);

        let disjoint = AnyAnglePolygon::new([
            LayoutPoint::new(20, 0),
            LayoutPoint::new(30, 0),
            LayoutPoint::new(30, 10),
            LayoutPoint::new(20, 10),
        ])
        .unwrap();
        let union = any_angle_boolean(&outer, &disjoint, AnyAngleBooleanOperation::Union).unwrap();
        assert_eq!(union.polygons.len(), 2);
        assert!((union.total_area - 200.0).abs() < 1e-12);
        assert!(
            any_angle_boolean(&outer, &disjoint, AnyAngleBooleanOperation::Intersection)
                .unwrap()
                .polygons
                .is_empty()
        );
    }
}
