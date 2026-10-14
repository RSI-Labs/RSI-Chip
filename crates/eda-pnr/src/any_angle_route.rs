//! Direct two-pin arbitrary-angle routing with exact geometry checks.
//!
//! This bounded router connects each request with one straight segment. It is
//! intentionally separate from the mature Manhattan maze router and is suitable
//! for diagonal flywires and simple routing checks, not multi-pin detour or
//! congestion optimization.

use crate::any_angle::{
    general_segments_intersect, segment_distance_squared, AnyAnglePolygon, RationalSquare,
};
use crate::layout::{LayoutCoordinate, LayoutPoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnyAngleRouteRequest {
    pub net: usize,
    pub start: LayoutPoint,
    pub end: LayoutPoint,
}

impl AnyAngleRouteRequest {
    pub fn new(net: usize, start: LayoutPoint, end: LayoutPoint) -> Result<Self, String> {
        if start == end {
            return Err("any-angle route endpoints must differ".into());
        }
        Ok(Self { net, start, end })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnyAngleRoute {
    pub net: usize,
    pub start: LayoutPoint,
    pub end: LayoutPoint,
    pub length_squared: RationalSquare,
}

impl AnyAngleRoute {
    pub fn length(&self) -> f64 {
        self.length_squared.sqrt()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnyAngleRoutingOptions {
    pub min_spacing: LayoutCoordinate,
}

impl AnyAngleRoutingOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.min_spacing < 0 {
            return Err("any-angle routing spacing cannot be negative".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnyAngleRouting {
    pub routes: Vec<AnyAngleRoute>,
}

impl AnyAngleRouting {
    pub fn total_length(&self) -> f64 {
        self.routes.iter().map(AnyAngleRoute::length).sum()
    }
}

pub fn route_any_angle_direct(
    requests: &[AnyAngleRouteRequest],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleRoutingOptions,
) -> Result<AnyAngleRouting, String> {
    options.validate()?;
    let mut routes = Vec::with_capacity(requests.len());
    for request in requests {
        let dx = i128::from(request.end.x) - i128::from(request.start.x);
        let dy = i128::from(request.end.y) - i128::from(request.start.y);
        let length_squared = dx
            .checked_mul(dx)
            .and_then(|x| dy.checked_mul(dy).and_then(|y| x.checked_add(y)))
            .ok_or_else(|| "any-angle route length overflow".to_string())?;
        let route = AnyAngleRoute {
            net: request.net,
            start: request.start,
            end: request.end,
            length_squared: RationalSquare::from_integer(length_squared)?,
        };
        if blockages
            .iter()
            .any(|blockage| blockage.intersects_segment(route.start, route.end))
        {
            return Err(format!(
                "any-angle route for net {} crosses a layout blockage",
                route.net
            ));
        }
        routes.push(route);
    }

    let required_squared = i128::from(options.min_spacing)
        .checked_mul(i128::from(options.min_spacing))
        .ok_or_else(|| "any-angle routing spacing overflow".to_string())?;
    for (index, left) in routes.iter().enumerate() {
        for right in routes.iter().skip(index + 1) {
            if left.net == right.net {
                continue;
            }
            if general_segments_intersect(left.start, left.end, right.start, right.end) {
                return Err(format!(
                    "any-angle routes for nets {} and {} cross",
                    left.net, right.net
                ));
            }
            let distance_squared =
                segment_distance_squared(left.start, left.end, right.start, right.end)?;
            if !distance_squared.greater_equal_integer_square(required_squared)? {
                return Err(format!(
                    "any-angle nets {} and {} violate spacing {}",
                    left.net, right.net, options.min_spacing
                ));
            }
        }
    }
    Ok(AnyAngleRouting { routes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_diagonal_route_is_exact() {
        let request =
            AnyAngleRouteRequest::new(7, LayoutPoint::new(0, 0), LayoutPoint::new(3, 5)).unwrap();
        let routing =
            route_any_angle_direct(&[request], &[], &AnyAngleRoutingOptions::default()).unwrap();
        assert_eq!(routing.routes.len(), 1);
        assert_eq!(routing.routes[0].length_squared.numerator, 34);
        assert_eq!(routing.routes[0].length_squared.denominator, 1);
        assert!((routing.total_length() - f64::sqrt(34.0)).abs() < 1e-12);
    }

    #[test]
    fn direct_routes_reject_blockages_and_conflicts() {
        let blockage = AnyAnglePolygon::new([
            LayoutPoint::new(-2, -2),
            LayoutPoint::new(2, -2),
            LayoutPoint::new(0, 3),
        ])
        .unwrap();
        let blocked =
            AnyAngleRouteRequest::new(0, LayoutPoint::new(-4, 0), LayoutPoint::new(4, 0)).unwrap();
        let error =
            route_any_angle_direct(&[blocked], &[blockage], &AnyAngleRoutingOptions::default())
                .unwrap_err();
        assert!(error.contains("crosses a layout blockage"));

        let left =
            AnyAngleRouteRequest::new(0, LayoutPoint::new(0, 0), LayoutPoint::new(10, 10)).unwrap();
        let right =
            AnyAngleRouteRequest::new(1, LayoutPoint::new(0, 10), LayoutPoint::new(10, 0)).unwrap();
        let crossing =
            route_any_angle_direct(&[left, right], &[], &AnyAngleRoutingOptions::default())
                .unwrap_err();
        assert!(crossing.contains("cross"));

        let separated =
            AnyAngleRouteRequest::new(1, LayoutPoint::new(0, 12), LayoutPoint::new(10, 12))
                .unwrap();
        let exact = route_any_angle_direct(
            &[
                AnyAngleRouteRequest::new(0, LayoutPoint::new(0, 0), LayoutPoint::new(10, 0))
                    .unwrap(),
                separated,
            ],
            &[],
            &AnyAngleRoutingOptions { min_spacing: 12 },
        )
        .unwrap();
        assert_eq!(exact.routes.len(), 2);
    }
}
