//! Exact visibility-graph routing for arbitrary-angle two-pin nets.
//!
//! The router is intentionally geometric and dependency-free. It constructs a
//! bounded set of clearance candidates around polygon blockage corners, finds a
//! shortest legal polyline with Dijkstra, and retains exact squared length for
//! every segment. Unlike the direct router, paths may bend around blockages.

use crate::any_angle::{
    general_segments_intersect, segment_distance_squared, AnyAnglePolygon, RationalSquare,
};
use crate::any_angle_route::AnyAngleRouteRequest;
use crate::layout::{LayoutCoordinate, LayoutPoint};
use eda_core::cancellation::FlowCancellation;
use std::collections::BinaryHeap;

const PRESENT_ROUTE_CONFLICT_PENALTY: f64 = 1_000.0;
const HISTORY_CONFLICT_PENALTY: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnyAngleMazeRoutingOptions {
    pub min_spacing: LayoutCoordinate,
    pub max_search_nodes: usize,
}

impl Default for AnyAngleMazeRoutingOptions {
    fn default() -> Self {
        Self {
            min_spacing: 0,
            max_search_nodes: 512,
        }
    }
}

impl AnyAngleMazeRoutingOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.min_spacing < 0 {
            return Err("any-angle maze routing spacing cannot be negative".into());
        }
        if self.max_search_nodes < 2 {
            return Err("any-angle maze routing requires at least two search nodes".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyAngleMazeRoute {
    pub net: usize,
    pub points: Vec<LayoutPoint>,
    pub segment_lengths_squared: Vec<RationalSquare>,
}

impl AnyAngleMazeRoute {
    pub fn total_length(&self) -> f64 {
        self.segment_lengths_squared
            .iter()
            .map(RationalSquare::sqrt)
            .sum()
    }

    pub fn segment_count(&self) -> usize {
        self.points.len().saturating_sub(1)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnyAngleMazeRouting {
    pub routes: Vec<AnyAngleMazeRoute>,
}

impl AnyAngleMazeRouting {
    pub fn total_length(&self) -> f64 {
        self.routes
            .iter()
            .map(AnyAngleMazeRoute::total_length)
            .sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnyAngleRouteConflict {
    pub left_net: usize,
    pub right_net: usize,
    pub left_segment: usize,
    pub right_segment: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnyAngleMazeRipupResult {
    pub routing: AnyAngleMazeRouting,
    pub passes: usize,
    pub initial_conflicts: Vec<AnyAngleRouteConflict>,
}

#[derive(Clone, Copy)]
struct SearchNode {
    cost: f64,
    index: usize,
}

enum SearchPolicy<'a> {
    Hard,
    Negotiated {
        history: &'a [(LayoutPoint, LayoutPoint)],
    },
}

impl Eq for SearchNode {}

impl PartialEq for SearchNode {
    fn eq(&self, other: &Self) -> bool {
        self.cost.total_cmp(&other.cost) == std::cmp::Ordering::Equal && self.index == other.index
    }
}

impl Ord for SearchNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for SearchNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn segment_length_squared(start: LayoutPoint, end: LayoutPoint) -> Result<RationalSquare, String> {
    let dx = i128::from(end.x) - i128::from(start.x);
    let dy = i128::from(end.y) - i128::from(start.y);
    let value = dx
        .checked_mul(dx)
        .and_then(|x| dy.checked_mul(dy).and_then(|y| x.checked_add(y)))
        .ok_or_else(|| "any-angle maze segment length overflow".to_string())?;
    RationalSquare::from_integer(value)
}

fn required_spacing_squared(spacing: LayoutCoordinate) -> Result<i128, String> {
    i128::from(spacing)
        .checked_mul(i128::from(spacing))
        .ok_or_else(|| "any-angle maze routing spacing overflow".to_string())
}

fn point_segment_meets_spacing(
    point: LayoutPoint,
    start: LayoutPoint,
    end: LayoutPoint,
    required_squared: i128,
) -> Result<bool, String> {
    let segment_dx = i128::from(end.x - start.x);
    let segment_dy = i128::from(end.y - start.y);
    let start_dx = i128::from(point.x - start.x);
    let start_dy = i128::from(point.y - start.y);
    let end_dx = i128::from(point.x - end.x);
    let end_dy = i128::from(point.y - end.y);
    let length_squared = segment_dx
        .checked_mul(segment_dx)
        .and_then(|x| {
            segment_dy
                .checked_mul(segment_dy)
                .and_then(|y| x.checked_add(y))
        })
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    if length_squared == 0 {
        return Err("any-angle point spacing segment has zero length".into());
    }
    let start_dot = segment_dx
        .checked_mul(start_dx)
        .and_then(|x| {
            segment_dy
                .checked_mul(start_dy)
                .and_then(|y| x.checked_add(y))
        })
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    if start_dot <= 0 {
        let distance_squared = start_dx
            .checked_mul(start_dx)
            .and_then(|x| {
                start_dy
                    .checked_mul(start_dy)
                    .and_then(|y| x.checked_add(y))
            })
            .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
        return Ok(distance_squared >= required_squared);
    }
    let end_dot = segment_dx
        .checked_mul(end_dx)
        .and_then(|x| {
            segment_dy
                .checked_mul(end_dy)
                .and_then(|y| x.checked_add(y))
        })
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    if end_dot >= 0 {
        let distance_squared = end_dx
            .checked_mul(end_dx)
            .and_then(|x| end_dy.checked_mul(end_dy).and_then(|y| x.checked_add(y)))
            .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
        return Ok(distance_squared >= required_squared);
    }
    let cross = segment_dx
        .checked_mul(start_dy)
        .and_then(|left| segment_dy.checked_mul(start_dx).map(|right| left - right))
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    let cross_squared = cross
        .checked_mul(cross)
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    let scaled_required = required_squared
        .checked_mul(length_squared)
        .ok_or_else(|| "any-angle point spacing overflow".to_string())?;
    Ok(cross_squared >= scaled_required)
}

fn point_has_clearance(
    point: LayoutPoint,
    blockages: &[AnyAnglePolygon],
    spacing: LayoutCoordinate,
) -> Result<bool, String> {
    let required = required_spacing_squared(spacing)?;
    for blockage in blockages {
        if blockage.contains_point(point) {
            return Ok(false);
        }
        for index in 0..blockage.points().len() {
            let left = blockage.points()[index];
            let right = blockage.points()[(index + 1) % blockage.points().len()];
            if !point_segment_meets_spacing(point, left, right, required)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn segment_has_clearance(
    start: LayoutPoint,
    end: LayoutPoint,
    blockages: &[AnyAnglePolygon],
    spacing: LayoutCoordinate,
) -> Result<bool, String> {
    let required = required_spacing_squared(spacing)?;
    for blockage in blockages {
        if blockage.intersects_segment(start, end) {
            return Ok(false);
        }
        for index in 0..blockage.points().len() {
            let left = blockage.points()[index];
            let right = blockage.points()[(index + 1) % blockage.points().len()];
            if !segment_distance_squared(start, end, left, right)?
                .greater_equal_integer_square(required)?
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn route_segment_conflict_count(
    start: LayoutPoint,
    end: LayoutPoint,
    route: &AnyAngleMazeRoute,
    net: usize,
    spacing: LayoutCoordinate,
) -> Result<usize, String> {
    if route.net == net {
        return Ok(0);
    }
    let mut conflicts = 0;
    let required = required_spacing_squared(spacing)?;
    for index in 0..route.points.len().saturating_sub(1) {
        let left = route.points[index];
        let right = route.points[index + 1];
        if general_segments_intersect(start, end, left, right) {
            conflicts += 1;
            continue;
        }
        if !segment_distance_squared(start, end, left, right)?
            .greater_equal_integer_square(required)?
        {
            conflicts += 1;
        }
    }
    Ok(conflicts)
}

fn candidate_nodes(
    request: &AnyAngleRouteRequest,
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
    existing_routes: &[AnyAngleMazeRoute],
) -> Result<Vec<LayoutPoint>, String> {
    let radius = options
        .min_spacing
        .checked_mul(2)
        .and_then(|value| value.checked_add(1))
        .unwrap_or(LayoutCoordinate::MAX);
    let radius = radius.max(1);
    let mut nodes = vec![request.start, request.end];
    let offsets = [
        (radius, 0),
        (-radius, 0),
        (0, radius),
        (0, -radius),
        (radius, radius),
        (radius, -radius),
        (-radius, radius),
        (-radius, -radius),
    ];
    let min_x = request.start.x.min(request.end.x);
    let max_x = request.start.x.max(request.end.x);
    let min_y = request.start.y.min(request.end.y);
    let max_y = request.start.y.max(request.end.y);
    for (x, y) in [
        (min_x.checked_sub(radius), min_y.checked_sub(radius)),
        (max_x.checked_add(radius), min_y.checked_sub(radius)),
        (min_x.checked_sub(radius), max_y.checked_add(radius)),
        (max_x.checked_add(radius), max_y.checked_add(radius)),
    ] {
        if let (Some(x), Some(y)) = (x, y) {
            let candidate = LayoutPoint::new(x, y);
            if candidate != request.start
                && candidate != request.end
                && point_has_clearance(candidate, blockages, options.min_spacing)?
            {
                nodes.push(candidate);
            }
        }
    }
    for blockage in blockages {
        for point in blockage.points() {
            for (dx, dy) in offsets {
                let Some(x) = point.x.checked_add(dx) else {
                    continue;
                };
                let Some(y) = point.y.checked_add(dy) else {
                    continue;
                };
                let candidate = LayoutPoint::new(x, y);
                if candidate != request.start
                    && candidate != request.end
                    && point_has_clearance(candidate, blockages, options.min_spacing)?
                {
                    nodes.push(candidate);
                }
            }
        }
    }
    for route in existing_routes {
        let xs = route.points.iter().map(|point| point.x);
        let ys = route.points.iter().map(|point| point.y);
        let min_x = xs.clone().min().unwrap_or_default();
        let max_x = xs.max().unwrap_or_default();
        let min_y = ys.clone().min().unwrap_or_default();
        let max_y = ys.max().unwrap_or_default();
        for (x, y) in [
            (min_x.checked_sub(radius), min_y.checked_sub(radius)),
            (max_x.checked_add(radius), min_y.checked_sub(radius)),
            (min_x.checked_sub(radius), max_y.checked_add(radius)),
            (max_x.checked_add(radius), max_y.checked_add(radius)),
        ] {
            if let (Some(x), Some(y)) = (x, y) {
                nodes.push(LayoutPoint::new(x, y));
            }
        }
    }
    nodes.sort();
    nodes.dedup();
    nodes.retain(|node| *node != request.start && *node != request.end);
    nodes.truncate(options.max_search_nodes.saturating_sub(2));
    nodes.insert(0, request.end);
    nodes.insert(0, request.start);
    if !nodes.contains(&request.start) || !nodes.contains(&request.end) {
        return Err("any-angle maze node limit excludes a route endpoint".into());
    }
    Ok(nodes)
}

fn route_one(
    request: &AnyAngleRouteRequest,
    routes: &[AnyAngleMazeRoute],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
    cancellation: &FlowCancellation,
    policy: SearchPolicy<'_>,
) -> Result<AnyAngleMazeRoute, String> {
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    if !point_has_clearance(request.start, blockages, options.min_spacing)? {
        return Err(format!(
            "any-angle maze route for net {} starts inside or too close to a blockage",
            request.net
        ));
    }
    if !point_has_clearance(request.end, blockages, options.min_spacing)? {
        return Err(format!(
            "any-angle maze route for net {} ends inside or too close to a blockage",
            request.net
        ));
    }

    let nodes = candidate_nodes(request, blockages, options, routes)?;
    let mut distances = vec![f64::INFINITY; nodes.len()];
    let mut previous = vec![usize::MAX; nodes.len()];
    let mut visited = vec![false; nodes.len()];
    let mut queue = BinaryHeap::new();
    distances[0] = 0.0;
    queue.push(SearchNode {
        cost: 0.0,
        index: 0,
    });

    while let Some(SearchNode { cost, index }) = queue.pop() {
        if cancellation.is_cancelled() {
            return Err("any-angle maze routing cancelled".into());
        }
        if visited[index] || cost > distances[index] {
            continue;
        }
        visited[index] = true;
        if index == 1 {
            break;
        }
        for next in 0..nodes.len() {
            if next == index || visited[next] {
                continue;
            }
            if !segment_has_clearance(nodes[index], nodes[next], blockages, options.min_spacing)? {
                continue;
            }
            let length = segment_length_squared(nodes[index], nodes[next])?.sqrt();
            let mut edge_cost = length;
            let mut present_conflicts = 0_usize;
            for route in routes {
                let conflicts = route_segment_conflict_count(
                    nodes[index],
                    nodes[next],
                    route,
                    request.net,
                    options.min_spacing,
                )?;
                if conflicts > 0 && matches!(policy, SearchPolicy::Hard) {
                    present_conflicts = usize::MAX;
                    break;
                }
                present_conflicts += conflicts;
            }
            if present_conflicts == usize::MAX {
                continue;
            }
            edge_cost += present_conflicts as f64 * PRESENT_ROUTE_CONFLICT_PENALTY;
            if let SearchPolicy::Negotiated { history } = policy {
                let mut history_conflicts = 0_usize;
                for (left, right) in history {
                    if *left == nodes[index] && *right == nodes[next]
                        || (*left == nodes[next] && *right == nodes[index])
                    {
                        history_conflicts += 1;
                        continue;
                    }
                    if general_segments_intersect(nodes[index], nodes[next], *left, *right)
                        || !segment_distance_squared(nodes[index], nodes[next], *left, *right)?
                            .greater_equal_integer_square(required_spacing_squared(
                                options.min_spacing,
                            )?)?
                    {
                        history_conflicts += 1;
                    }
                }
                edge_cost += history_conflicts as f64 * HISTORY_CONFLICT_PENALTY;
            }
            let next_cost = cost + edge_cost;
            if next_cost + f64::EPSILON < distances[next] {
                distances[next] = next_cost;
                previous[next] = index;
                queue.push(SearchNode {
                    cost: next_cost,
                    index: next,
                });
            }
        }
    }

    if !visited[1] {
        return Err(format!(
            "any-angle maze routing found no legal path for net {}",
            request.net
        ));
    }
    let mut indices = Vec::new();
    let mut cursor = 1;
    while cursor != usize::MAX {
        indices.push(cursor);
        cursor = previous[cursor];
    }
    indices.reverse();
    let points = indices
        .iter()
        .map(|index| nodes[*index])
        .collect::<Vec<_>>();
    let segment_lengths_squared = points
        .windows(2)
        .map(|pair| segment_length_squared(pair[0], pair[1]))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(AnyAngleMazeRoute {
        net: request.net,
        points,
        segment_lengths_squared,
    })
}

/// Route arbitrary-angle two-pin requests around simple polygon blockages.
pub fn route_any_angle_maze(
    requests: &[AnyAngleRouteRequest],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
) -> Result<AnyAngleMazeRouting, String> {
    route_any_angle_maze_cancellable(requests, blockages, options, &FlowCancellation::new())
}

/// Cancellable arbitrary-angle visibility-graph routing.
pub fn route_any_angle_maze_cancellable(
    requests: &[AnyAngleRouteRequest],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
    cancellation: &FlowCancellation,
) -> Result<AnyAngleMazeRouting, String> {
    options.validate()?;
    if cancellation.is_cancelled() {
        return Err("any-angle maze routing cancelled".into());
    }
    if requests.is_empty() {
        return Err("any-angle maze routing requires at least one request".into());
    }
    let mut routes = Vec::with_capacity(requests.len());
    for request in requests {
        routes.push(route_one(
            request,
            &routes,
            blockages,
            options,
            cancellation,
            SearchPolicy::Hard,
        )?);
    }
    Ok(AnyAngleMazeRouting { routes })
}

fn segment_pair_conflicts(
    left: &AnyAngleMazeRoute,
    right: &AnyAngleMazeRoute,
    spacing: LayoutCoordinate,
) -> Result<Vec<(usize, usize)>, String> {
    let mut conflicts = Vec::new();
    if left.net == right.net {
        return Ok(conflicts);
    }
    let required = required_spacing_squared(spacing)?;
    for left_segment in 0..left.segment_count() {
        for right_segment in 0..right.segment_count() {
            let a = left.points[left_segment];
            let b = left.points[left_segment + 1];
            let c = right.points[right_segment];
            let d = right.points[right_segment + 1];
            if general_segments_intersect(a, b, c, d)
                || !segment_distance_squared(a, b, c, d)?.greater_equal_integer_square(required)?
            {
                conflicts.push((left_segment, right_segment));
            }
        }
    }
    Ok(conflicts)
}

fn routing_conflicts(
    routes: &[AnyAngleMazeRoute],
    spacing: LayoutCoordinate,
) -> Result<Vec<AnyAngleRouteConflict>, String> {
    let mut conflicts = Vec::new();
    for (left_index, left) in routes.iter().enumerate() {
        for right in routes.iter().skip(left_index + 1) {
            for (left_segment, right_segment) in segment_pair_conflicts(left, right, spacing)? {
                conflicts.push(AnyAngleRouteConflict {
                    left_net: left.net,
                    right_net: right.net,
                    left_segment,
                    right_segment,
                });
            }
        }
    }
    Ok(conflicts)
}

fn add_history_conflicts(
    history: &mut Vec<(LayoutPoint, LayoutPoint)>,
    routes: &[AnyAngleMazeRoute],
    conflicts: &[AnyAngleRouteConflict],
) {
    for conflict in conflicts {
        if let Some(left) = routes
            .iter()
            .find(|route| route.net == conflict.left_net)
            .and_then(|route| {
                route
                    .points
                    .get(conflict.left_segment)
                    .zip(route.points.get(conflict.left_segment + 1))
                    .map(|(start, end)| (*start, *end))
            })
        {
            history.push(left);
        }
        if let Some(right) = routes
            .iter()
            .find(|route| route.net == conflict.right_net)
            .and_then(|route| {
                route
                    .points
                    .get(conflict.right_segment)
                    .zip(route.points.get(conflict.right_segment + 1))
                    .map(|(start, end)| (*start, *end))
            })
        {
            history.push(right);
        }
    }
}

/// Negotiated arbitrary-angle rip-up and rerouting.
pub fn route_any_angle_maze_ripgup(
    requests: &[AnyAngleRouteRequest],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
    max_passes: usize,
) -> Result<AnyAngleMazeRipupResult, String> {
    route_any_angle_maze_ripgup_cancellable(
        requests,
        blockages,
        options,
        max_passes,
        &FlowCancellation::new(),
    )
}

pub fn route_any_angle_maze_ripgup_cancellable(
    requests: &[AnyAngleRouteRequest],
    blockages: &[AnyAnglePolygon],
    options: &AnyAngleMazeRoutingOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<AnyAngleMazeRipupResult, String> {
    options.validate()?;
    if max_passes == 0 {
        return Err("arbitrary-angle rip-up routing requires at least one pass".into());
    }
    if cancellation.is_cancelled() {
        return Err("arbitrary-angle rip-up routing cancelled".into());
    }
    if requests.is_empty() {
        return Err("any-angle maze routing requires at least one request".into());
    }

    let empty_history = Vec::new();
    let mut routes = Vec::with_capacity(requests.len());
    for request in requests {
        routes.push(route_one(
            request,
            &[],
            blockages,
            options,
            cancellation,
            SearchPolicy::Negotiated {
                history: &empty_history,
            },
        )?);
    }
    let initial_conflicts = routing_conflicts(&routes, options.min_spacing)?;
    let mut history = Vec::new();
    add_history_conflicts(&mut history, &routes, &initial_conflicts);

    for pass in 1..=max_passes {
        if cancellation.is_cancelled() {
            return Err("arbitrary-angle rip-up routing cancelled".into());
        }
        for (index, request) in requests.iter().enumerate().take(routes.len()) {
            routes.remove(index);
            let replacement = route_one(
                request,
                &routes,
                blockages,
                options,
                cancellation,
                SearchPolicy::Negotiated { history: &history },
            );
            match replacement {
                Ok(replacement) => routes.insert(index, replacement),
                Err(error) => return Err(error),
            }
        }
        let conflicts = routing_conflicts(&routes, options.min_spacing)?;
        if conflicts.is_empty() {
            return Ok(AnyAngleMazeRipupResult {
                routing: AnyAngleMazeRouting { routes },
                passes: pass,
                initial_conflicts,
            });
        }
        add_history_conflicts(&mut history, &routes, &conflicts);
    }

    let remaining = routing_conflicts(&routes, options.min_spacing)?;
    Err(format!(
        "arbitrary-angle rip-up routing did not converge after {max_passes} pass(es); {} conflict(s) remain",
        remaining.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maze_router_uses_direct_segment_when_unblocked() {
        let request =
            AnyAngleRouteRequest::new(7, LayoutPoint::new(0, 0), LayoutPoint::new(3, 5)).unwrap();
        let routing =
            route_any_angle_maze(&[request], &[], &AnyAngleMazeRoutingOptions::default()).unwrap();
        assert_eq!(routing.routes.len(), 1);
        assert_eq!(routing.routes[0].points.len(), 2);
        assert_eq!(routing.routes[0].segment_lengths_squared.len(), 1);
        assert_eq!(routing.routes[0].segment_lengths_squared[0].numerator, 34);
        assert!((routing.total_length() - f64::sqrt(34.0)).abs() < 1e-12);
    }

    #[test]
    fn maze_router_detours_around_blockage() {
        let blockage = AnyAnglePolygon::new([
            LayoutPoint::new(-2, -2),
            LayoutPoint::new(2, -2),
            LayoutPoint::new(0, 3),
        ])
        .unwrap();
        let request =
            AnyAngleRouteRequest::new(4, LayoutPoint::new(-10, 0), LayoutPoint::new(10, 0))
                .unwrap();
        let options = AnyAngleMazeRoutingOptions {
            min_spacing: 1,
            max_search_nodes: 128,
        };
        let routing =
            route_any_angle_maze(&[request], std::slice::from_ref(&blockage), &options).unwrap();
        let route = &routing.routes[0];
        assert!(route.points.len() > 2, "{:?}", route.points);
        assert!(route.total_length() > 20.0);
        for index in 0..route.segment_count() {
            assert!(!blockage.intersects_segment(route.points[index], route.points[index + 1]));
        }
    }

    #[test]
    fn maze_router_detours_around_existing_net() {
        let first = AnyAngleRouteRequest::new(0, LayoutPoint::new(-10, 0), LayoutPoint::new(10, 0))
            .unwrap();
        let second =
            AnyAngleRouteRequest::new(1, LayoutPoint::new(0, -10), LayoutPoint::new(0, 10))
                .unwrap();
        let routing = route_any_angle_maze(
            &[first, second],
            &[],
            &AnyAngleMazeRoutingOptions {
                min_spacing: 0,
                max_search_nodes: 64,
            },
        )
        .unwrap();
        assert_eq!(routing.routes[0].points.len(), 2);
        assert!(routing.routes[1].points.len() > 2);
        assert!(routing_conflicts(&routing.routes, 0).unwrap().is_empty());
    }

    #[test]
    fn negotiated_ripgup_resolves_crossing_nets() {
        let first = AnyAngleRouteRequest::new(0, LayoutPoint::new(-10, 0), LayoutPoint::new(10, 0))
            .unwrap();
        let second =
            AnyAngleRouteRequest::new(1, LayoutPoint::new(0, -10), LayoutPoint::new(0, 10))
                .unwrap();
        let result = route_any_angle_maze_ripgup(
            &[first, second],
            &[],
            &AnyAngleMazeRoutingOptions {
                min_spacing: 0,
                max_search_nodes: 96,
            },
            8,
        )
        .unwrap();
        assert!(!result.initial_conflicts.is_empty());
        assert!(result.passes <= 8);
        assert!(routing_conflicts(&result.routing.routes, 0)
            .unwrap()
            .is_empty());
        assert!(result
            .routing
            .routes
            .iter()
            .any(|route| route.points.len() > 2));
    }

    #[test]
    fn negotiated_ripgup_validates_passes_and_cancellation() {
        let request =
            AnyAngleRouteRequest::new(0, LayoutPoint::new(0, 0), LayoutPoint::new(10, 0)).unwrap();
        assert_eq!(
            route_any_angle_maze_ripgup(
                &[request],
                &[],
                &AnyAngleMazeRoutingOptions::default(),
                0,
            )
            .unwrap_err(),
            "arbitrary-angle rip-up routing requires at least one pass"
        );
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            route_any_angle_maze_ripgup_cancellable(
                &[request],
                &[],
                &AnyAngleMazeRoutingOptions::default(),
                8,
                &cancellation,
            )
            .unwrap_err(),
            "arbitrary-angle rip-up routing cancelled"
        );
    }

    #[test]
    fn maze_router_validates_options_and_node_limit() {
        let request =
            AnyAngleRouteRequest::new(0, LayoutPoint::new(0, 0), LayoutPoint::new(10, 0)).unwrap();
        assert!(route_any_angle_maze(
            &[request],
            &[],
            &AnyAngleMazeRoutingOptions {
                min_spacing: -1,
                max_search_nodes: 8,
            }
        )
        .is_err());
        assert!(route_any_angle_maze(
            &[request],
            &[],
            &AnyAngleMazeRoutingOptions {
                min_spacing: 0,
                max_search_nodes: 0,
            }
        )
        .is_err());
        assert!(route_any_angle_maze(
            &[request],
            &[],
            &AnyAngleMazeRoutingOptions {
                min_spacing: 0,
                max_search_nodes: 1,
            }
        )
        .is_err());
    }

    #[test]
    fn maze_router_honors_cancellation() {
        let request =
            AnyAngleRouteRequest::new(0, LayoutPoint::new(0, 0), LayoutPoint::new(10, 0)).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            route_any_angle_maze_cancellable(
                &[request],
                &[],
                &AnyAngleMazeRoutingOptions::default(),
                &cancellation,
            )
            .unwrap_err(),
            "any-angle maze routing cancelled"
        );
    }
}
