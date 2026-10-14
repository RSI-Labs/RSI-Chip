//! Exact integer primitives for simple polygons with arbitrary edge angles.
//!
//! This module is deliberately independent from the mature Manhattan layout
//! editor/router. It provides a first-class typed representation and exact
//! area/bounds/containment queries while Boolean and DRC integration remain
//! explicit future work.

use crate::layout::{LayoutCoordinate, LayoutPoint, LayoutRect};

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
}
