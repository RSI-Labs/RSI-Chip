use super::checks::{check_routing, shape_via};
use super::placement::*;
use super::types::*;
use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::*;
use std::collections::*;

struct TrackAllocator {
    used: TrackUsageMap,
    blocked: std::collections::BTreeSet<(Orientation, usize)>,
}

pub(super) type TrackKey = (Orientation, usize, usize);
pub(super) type TrackRange = (usize, usize, usize);
pub(super) type TrackUsageMap = HashMap<TrackKey, Vec<TrackRange>>;
type RouteShape = (Orientation, usize, usize, usize);
type TrackAssignment = (Orientation, usize, usize, usize, usize);

impl TrackAllocator {
    fn with_blockages(blocked: std::collections::BTreeSet<(Orientation, usize)>) -> Self {
        Self {
            used: HashMap::new(),
            blocked,
        }
    }

    fn is_blocked(&self, orientation: Orientation, channel: usize) -> bool {
        self.blocked.contains(&(orientation, channel))
    }

    fn is_free(
        &self,
        orientation: Orientation,
        channel: usize,
        start: usize,
        end: usize,
        net: usize,
        track: usize,
    ) -> bool {
        if self.is_blocked(orientation, channel) {
            return false;
        }
        self.used
            .get(&(orientation, channel, track))
            .is_none_or(|ranges| {
                !ranges
                    .iter()
                    .any(|&(s, e, n)| n != net && start <= e && end >= s)
            })
    }

    fn allocate_shape(
        &mut self,
        shape: &[RouteShape],
        net: usize,
        capacity: usize,
    ) -> Option<Vec<TrackAssignment>> {
        let mut assignment = Vec::with_capacity(shape.len());
        for &(orientation, channel, start, end) in shape {
            let track = (0..capacity)
                .find(|&track| self.is_free(orientation, channel, start, end, net, track))?;
            assignment.push((orientation, channel, start, end, track));
        }
        for &(orientation, channel, start, end, track) in &assignment {
            self.used
                .entry((orientation, channel, track))
                .or_default()
                .push((start, end, net));
        }
        Some(assignment)
    }

    fn reserve(
        &mut self,
        orientation: Orientation,
        channel: usize,
        start: usize,
        end: usize,
        track: usize,
        net: usize,
    ) {
        self.used
            .entry((orientation, channel, track))
            .or_default()
            .push((start, end, net));
    }
}

/// Deterministic L-shaped global routing over the placement channel grid.
/// Each driver-to-sink connection uses one horizontal and one vertical
/// track segment; the greedy allocator avoids overlap in a channel/track.
pub fn route(nl: &Netlist, p: &Placement) -> Result<Routing, String> {
    route_with_options_cancellable(nl, p, &RouteOptions::default(), &FlowCancellation::new())
}

/// Capacity-aware deterministic global routing.
///
/// For each two-pin connection the router considers both L-shaped bend
/// choices before declaring a channel congested. This avoids many artificial
/// conflicts caused by always assigning the horizontal segment in the source
/// row and the vertical segment in the sink column.
pub fn route_with_options(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    route_with_options_cancellable(nl, p, options, &FlowCancellation::new())
}

pub fn route_with_options_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("global routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut congested = Vec::new();
    let mut vias = 0usize;
    let mut via_locations = Vec::new();

    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("global routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            if cancellation.is_cancelled() {
                return Err("global routing cancelled".into());
            }
            let shapes = vec![
                vec![
                    (
                        Orientation::Horizontal,
                        source.1,
                        source.0.min(sink.0),
                        source.0.max(sink.0),
                    ),
                    (
                        Orientation::Vertical,
                        sink.0,
                        source.1.min(sink.1),
                        source.1.max(sink.1),
                    ),
                ],
                vec![
                    (
                        Orientation::Vertical,
                        source.0,
                        source.1.min(sink.1),
                        source.1.max(sink.1),
                    ),
                    (
                        Orientation::Horizontal,
                        sink.1,
                        sink.0.min(source.0),
                        sink.0.max(source.0),
                    ),
                ],
            ];
            let mut routed = false;
            for shape in shapes {
                // Zero-length components are not physical segments.
                let shape: Vec<_> = shape
                    .into_iter()
                    .filter(|&(_, _, start, end)| start != end)
                    .collect();
                if shape.is_empty() {
                    routed = true;
                    break;
                }
                if let Some(assignment) =
                    allocator.allocate_shape(&shape, ni, options.tracks_per_channel)
                {
                    for (orientation, channel, start, end, track) in assignment {
                        segments.push(Segment {
                            net: ni,
                            orientation,
                            channel,
                            start,
                            end,
                            track,
                        });
                    }
                    if shape.len() == 2 {
                        vias += 1;
                        if let Some(via) = shape_via(&segments, ni) {
                            via_locations.push(via);
                        }
                    }
                    routed = true;
                    break;
                }
            }
            if !routed {
                // Record every channel needed by the preferred L-shape. DRC
                // will identify the actual opened pin groups when partial
                // routing is requested.
                if source.0 != sink.0 {
                    congested.push((Orientation::Horizontal, source.1));
                    congested.push((Orientation::Horizontal, sink.1));
                }
                if source.1 != sink.1 {
                    congested.push((Orientation::Vertical, source.0));
                    congested.push((Orientation::Vertical, sink.0));
                }
            }
        }
    }

    congested.sort_unstable();
    congested.dedup();
    if !congested.is_empty() {
        if options.allow_congestion {
            return Ok(Routing {
                segments,
                vias,
                via_locations,
                tracks_per_channel: options.tracks_per_channel,
                congested_channels: congested,
                blocked_channels: options.blocked_channels.clone(),
            });
        }
        return Err(format!(
            "global routing congestion in {} channel(s): {:?}",
            congested.len(),
            congested
        ));
    }
    Ok(Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: congested,
        blocked_channels: options.blocked_channels.clone(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct MazeNode {
    x: usize,
    y: usize,
    last_orientation: Option<Orientation>,
    last_track: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct MazeEdge {
    to: (usize, usize),
    orientation: Orientation,
    channel: usize,
    start: usize,
    end: usize,
    track: usize,
}

#[derive(Clone, Debug)]
struct MazeSearchStep {
    node: MazeNode,
    previous: Option<usize>,
    edge: Option<MazeEdge>,
    cost: usize,
}

fn maze_path(
    allocator: &TrackAllocator,
    capacity: usize,
    source: (usize, usize),
    sink: (usize, usize),
    max_x: usize,
    max_y: usize,
    cancellation: &FlowCancellation,
) -> Result<Option<Vec<MazeEdge>>, String> {
    if source == sink {
        return Ok(Some(Vec::new()));
    }

    #[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
    struct QueueKey {
        priority: usize,
        step: usize,
    }
    // BinaryHeap is a max-heap; store keys as Reverse so lower f-cost wins.
    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<QueueKey>> =
        std::collections::BinaryHeap::new();
    let start = MazeNode {
        x: source.0,
        y: source.1,
        last_orientation: None,
        last_track: None,
    };
    let mut steps = vec![MazeSearchStep {
        node: start,
        previous: None,
        edge: None,
        cost: 0,
    }];
    let mut best = HashMap::from([(start, 0usize)]);
    let estimate = |node: MazeNode| -> usize { node.x.abs_diff(sink.0) + node.y.abs_diff(sink.1) };
    queue.push(std::cmp::Reverse(QueueKey {
        priority: estimate(start),
        step: 0,
    }));

    let mut target_step = None;
    let mut expansions = 0usize;
    while let Some(std::cmp::Reverse(item)) = queue.pop() {
        expansions += 1;
        if expansions.is_multiple_of(128) && cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let current_index = item.step;
        let current = steps[current_index].clone();
        if current.cost > best.get(&current.node).copied().unwrap_or(usize::MAX) {
            continue;
        }
        if current.node.x == sink.0 && current.node.y == sink.1 {
            target_step = Some(current_index);
            break;
        }

        let mut neighbors = Vec::with_capacity(4);
        if current.node.x > 0 {
            neighbors.push((
                current.node.x - 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.x < max_x {
            neighbors.push((
                current.node.x + 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.y > 0 {
            neighbors.push((
                current.node.x,
                current.node.y - 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }
        if current.node.y < max_y {
            neighbors.push((
                current.node.x,
                current.node.y + 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }

        for (next_x, next_y, orientation, channel) in neighbors {
            let (start, end) = match orientation {
                Orientation::Horizontal => (current.node.x.min(next_x), current.node.x.max(next_x)),
                Orientation::Vertical => (current.node.y.min(next_y), current.node.y.max(next_y)),
            };
            let tracks = match (current.node.last_orientation, current.node.last_track) {
                (Some(previous), Some(track)) if previous == orientation => vec![track],
                _ => (0..capacity).collect::<Vec<_>>(),
            };
            for track in tracks {
                if !allocator.is_free(orientation, channel, start, end, usize::MAX, track) {
                    continue;
                }
                let next_node = MazeNode {
                    x: next_x,
                    y: next_y,
                    last_orientation: Some(orientation),
                    last_track: Some(track),
                };
                let via_penalty = usize::from(
                    current
                        .node
                        .last_orientation
                        .is_some_and(|previous| previous != orientation),
                ) * 2;
                let next_cost = current.cost + 1 + via_penalty;
                if next_cost >= best.get(&next_node).copied().unwrap_or(usize::MAX) {
                    continue;
                }
                best.insert(next_node, next_cost);
                let edge = MazeEdge {
                    to: (next_x, next_y),
                    orientation,
                    channel,
                    start,
                    end,
                    track,
                };
                steps.push(MazeSearchStep {
                    node: next_node,
                    previous: Some(current_index),
                    edge: Some(edge),
                    cost: next_cost,
                });
                let step = steps.len() - 1;
                queue.push(std::cmp::Reverse(QueueKey {
                    priority: next_cost + estimate(next_node),
                    step,
                }));
            }
        }
    }

    let Some(mut index) = target_step else {
        return Ok(None);
    };
    let mut edges = Vec::new();
    while let Some(edge) = steps[index].edge {
        edges.push(edge);
        let Some(previous) = steps[index].previous else {
            return Err("maze search traceback lost its root".into());
        };
        index = previous;
    }
    edges.reverse();
    Ok(Some(edges))
}

/// Detailed maze routing on the placement channel grid.
///
/// Unlike the two-L-shape global router, this router can take arbitrary
/// rectilinear detours. It reserves previously routed intervals and charges a
/// small bend penalty, allowing later nets to escape local channel conflicts.
pub fn route_maze(nl: &Netlist, p: &Placement) -> Result<Routing, String> {
    route_maze_with_options_cancellable(nl, p, &RouteOptions::default(), &FlowCancellation::new())
}

pub fn route_maze_with_options(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
) -> Result<Routing, String> {
    route_maze_with_options_cancellable(nl, p, options, &FlowCancellation::new())
}

pub fn route_maze_with_options_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("maze routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    let mut congested = Vec::new();

    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            if cancellation.is_cancelled() {
                return Err("maze routing cancelled".into());
            }
            let edges = maze_path(
                &allocator,
                options.tracks_per_channel,
                source,
                sink,
                SITES_PER_ROW,
                p.rows,
                cancellation,
            )?;
            let Some(edges) = edges else {
                if source.0 != sink.0 {
                    congested.push((Orientation::Horizontal, source.1));
                    congested.push((Orientation::Horizontal, sink.1));
                }
                if source.1 != sink.1 {
                    congested.push((Orientation::Vertical, source.0));
                    congested.push((Orientation::Vertical, sink.0));
                }
                continue;
            };
            if edges.is_empty() {
                continue;
            }

            for edge in &edges {
                allocator.reserve(
                    edge.orientation,
                    edge.channel,
                    edge.start,
                    edge.end,
                    edge.track,
                    ni,
                );
            }

            for edge in &edges {
                let mut merged = false;
                if let Some(last) = segments.last_mut() {
                    if last.net == ni
                        && last.orientation == edge.orientation
                        && last.channel == edge.channel
                        && last.track == edge.track
                        && (last.end == edge.start || edge.end == last.start)
                    {
                        last.start = last.start.min(edge.start);
                        last.end = last.end.max(edge.end);
                        merged = true;
                    }
                }
                if !merged {
                    segments.push(Segment {
                        net: ni,
                        orientation: edge.orientation,
                        channel: edge.channel,
                        start: edge.start,
                        end: edge.end,
                        track: edge.track,
                    });
                }
            }

            for pair in edges.windows(2) {
                if pair[0].orientation != pair[1].orientation {
                    let (x, y) = pair[0].to;
                    via_locations.push(Via { net: ni, x, y });
                }
            }
        }
    }

    congested.sort_unstable();
    congested.dedup();
    if !congested.is_empty() && !options.allow_congestion {
        return Err(format!(
            "maze routing congestion in {} channel(s): {:?}",
            congested.len(),
            congested
        ));
    }
    let vias = via_locations.len();
    let routing = Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: congested,
        blocked_channels: options.blocked_channels.clone(),
    };
    check_routing(&routing)?;
    Ok(routing)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MazeConnection {
    net: usize,
    source: (usize, usize),
    sink: (usize, usize),
}

type MazeResource = (Orientation, usize, usize, usize, usize);

fn maze_resource(edge: &MazeEdge) -> MazeResource {
    (
        edge.orientation,
        edge.channel,
        edge.track,
        edge.start,
        edge.end,
    )
}

fn insert_maze_path(
    occupancy: &mut HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    path_index: usize,
    path: &[MazeEdge],
) {
    for edge in path {
        occupancy
            .entry(maze_resource(edge))
            .or_default()
            .insert(path_index);
    }
}

fn remove_maze_path(
    occupancy: &mut HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    path_index: usize,
    path: &[MazeEdge],
) {
    for edge in path {
        if let Some(paths) = occupancy.get_mut(&maze_resource(edge)) {
            paths.remove(&path_index);
            if paths.is_empty() {
                occupancy.remove(&maze_resource(edge));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn negotiated_maze_path(
    occupancy: &HashMap<MazeResource, std::collections::BTreeSet<usize>>,
    history: &HashMap<MazeResource, usize>,
    blocked: &std::collections::BTreeSet<(Orientation, usize)>,
    paths: &[Option<Vec<MazeEdge>>],
    connections: &[MazeConnection],
    path_index: usize,
    capacity: usize,
    max_x: usize,
    max_y: usize,
    cancellation: &FlowCancellation,
) -> Result<Option<Vec<MazeEdge>>, String> {
    let connection = connections[path_index];
    let source = connection.source;
    let sink = connection.sink;
    if source == sink {
        return Ok(Some(Vec::new()));
    }

    #[derive(Eq, PartialEq, Ord, PartialOrd, Hash)]
    struct QueueKey {
        priority: usize,
        step: usize,
    }

    const PRESENT_CONGESTION_PENALTY: usize = 8;
    let mut queue: std::collections::BinaryHeap<std::cmp::Reverse<QueueKey>> =
        std::collections::BinaryHeap::new();
    let start = MazeNode {
        x: source.0,
        y: source.1,
        last_orientation: None,
        last_track: None,
    };
    let mut steps = vec![MazeSearchStep {
        node: start,
        previous: None,
        edge: None,
        cost: 0,
    }];
    let mut best = HashMap::from([(start, 0usize)]);
    let estimate = |node: MazeNode| -> usize { node.x.abs_diff(sink.0) + node.y.abs_diff(sink.1) };
    queue.push(std::cmp::Reverse(QueueKey {
        priority: estimate(start),
        step: 0,
    }));

    let mut target_step = None;
    let mut expansions = 0usize;
    while let Some(std::cmp::Reverse(item)) = queue.pop() {
        expansions += 1;
        if expansions.is_multiple_of(128) && cancellation.is_cancelled() {
            return Err("negotiated maze routing cancelled".into());
        }
        let current_index = item.step;
        let current = steps[current_index].clone();
        if current.cost > best.get(&current.node).copied().unwrap_or(usize::MAX) {
            continue;
        }
        if current.node.x == sink.0 && current.node.y == sink.1 {
            target_step = Some(current_index);
            break;
        }

        let mut neighbors = Vec::with_capacity(4);
        if current.node.x > 0 {
            neighbors.push((
                current.node.x - 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.x < max_x {
            neighbors.push((
                current.node.x + 1,
                current.node.y,
                Orientation::Horizontal,
                current.node.y,
            ));
        }
        if current.node.y > 0 {
            neighbors.push((
                current.node.x,
                current.node.y - 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }
        if current.node.y < max_y {
            neighbors.push((
                current.node.x,
                current.node.y + 1,
                Orientation::Vertical,
                current.node.x,
            ));
        }

        for (next_x, next_y, orientation, channel) in neighbors {
            if blocked.contains(&(orientation, channel)) {
                continue;
            }
            let (start, end) = match orientation {
                Orientation::Horizontal => (current.node.x.min(next_x), current.node.x.max(next_x)),
                Orientation::Vertical => (current.node.y.min(next_y), current.node.y.max(next_y)),
            };
            let tracks = match (current.node.last_orientation, current.node.last_track) {
                (Some(previous), Some(track)) if previous == orientation => vec![track],
                _ => (0..capacity).collect::<Vec<_>>(),
            };
            for track in tracks {
                let edge = MazeEdge {
                    to: (next_x, next_y),
                    orientation,
                    channel,
                    start,
                    end,
                    track,
                };
                let resource = maze_resource(&edge);
                let foreign_nets = occupancy
                    .get(&resource)
                    .map(|path_ids| {
                        path_ids
                            .iter()
                            .filter(|&&other| {
                                other != path_index
                                    && paths
                                        .get(other)
                                        .and_then(|path| path.as_ref())
                                        .is_some_and(|_| connections[other].net != connection.net)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                let history_penalty = history.get(&resource).copied().unwrap_or(0);
                let via_penalty = usize::from(
                    current
                        .node
                        .last_orientation
                        .is_some_and(|previous| previous != orientation),
                ) * 2;
                let next_cost = current.cost
                    + 1
                    + via_penalty
                    + foreign_nets * PRESENT_CONGESTION_PENALTY
                    + history_penalty;

                let next_node = MazeNode {
                    x: next_x,
                    y: next_y,
                    last_orientation: Some(orientation),
                    last_track: Some(track),
                };
                if next_cost >= best.get(&next_node).copied().unwrap_or(usize::MAX) {
                    continue;
                }
                best.insert(next_node, next_cost);
                steps.push(MazeSearchStep {
                    node: next_node,
                    previous: Some(current_index),
                    edge: Some(edge),
                    cost: next_cost,
                });
                let step = steps.len() - 1;
                queue.push(std::cmp::Reverse(QueueKey {
                    priority: next_cost + estimate(next_node),
                    step,
                }));
            }
        }
    }

    let Some(mut index) = target_step else {
        return Ok(None);
    };
    let mut edges = Vec::new();
    while let Some(edge) = steps[index].edge {
        edges.push(edge);
        let Some(previous) = steps[index].previous else {
            return Err("negotiated maze search traceback lost its root".into());
        };
        index = previous;
    }
    edges.reverse();
    Ok(Some(edges))
}

/// Result of negotiated-congestion maze routing.
#[derive(Clone, Debug)]
pub struct MazeRipupResult {
    pub routing: Routing,
    pub passes: usize,
    pub historical_conflicts: usize,
}

/// Result of timing-prioritized negotiated routing.
#[derive(Clone, Debug)]
pub struct TimingDrivenRipupResult {
    pub ripup: MazeRipupResult,
    /// Weighted wirelength of the neutral-priority baseline route.
    pub initial_weighted_wirelength: f64,
    /// Weighted wirelength after timing-prioritized rip-up/reroute.
    pub final_weighted_wirelength: f64,
}

/// Negotiated-congestion rip-up and reroute.
///
/// Every pass removes one two-pin route at a time and reroutes it with a soft
/// congestion penalty. Unlike the strictly sequential maze router, competing
/// routes may temporarily overlap; resources used by multiple nets accrue
/// history cost. Later passes therefore move repeat offenders to detours.
pub fn route_maze_ripgup(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
) -> Result<MazeRipupResult, String> {
    route_maze_ripgup_impl(nl, p, options, max_passes, None, &FlowCancellation::new())
}

pub fn route_maze_ripgup_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<MazeRipupResult, String> {
    route_maze_ripgup_impl(nl, p, options, max_passes, None, cancellation)
}

fn route_maze_ripgup_impl(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    max_passes: usize,
    timing: Option<&TimingDrivenRoutingOptions>,
    cancellation: &FlowCancellation,
) -> Result<MazeRipupResult, String> {
    if cancellation.is_cancelled() {
        return Err("rip-up routing cancelled".into());
    }
    check_legal(nl, p)?;
    options.validate()?;
    if max_passes == 0 {
        return Err("rip-up routing requires at least one pass".into());
    }
    if let Some(timing) = timing {
        timing.validate(nl.net_count())?;
    }

    let mut connections = Vec::new();
    for ni in 0..nl.net_count() {
        if cancellation.is_cancelled() {
            return Err("rip-up routing cancelled".into());
        }
        let pins = net_pins(nl, p, ni);
        if pins.len() < 2 {
            continue;
        }
        let source = pins[0];
        for &sink in &pins[1..] {
            connections.push(MazeConnection {
                net: ni,
                source,
                sink,
            });
        }
    }
    if let Some(timing) = timing {
        connections.sort_by(|left, right| {
            timing
                .weight(right.net)
                .total_cmp(&timing.weight(left.net))
                .then_with(|| left.net.cmp(&right.net))
        });
    }
    if connections.is_empty() {
        let routing = Routing {
            segments: Vec::new(),
            vias: 0,
            via_locations: Vec::new(),
            tracks_per_channel: options.tracks_per_channel,
            congested_channels: Vec::new(),
            blocked_channels: options.blocked_channels.clone(),
        };
        return Ok(MazeRipupResult {
            routing,
            passes: 0,
            historical_conflicts: 0,
        });
    }

    let mut paths: Vec<Option<Vec<MazeEdge>>> = vec![None; connections.len()];
    let blocked = options.blocked_set();
    let mut occupancy = HashMap::new();
    let mut history: HashMap<MazeResource, usize> = HashMap::new();
    let mut historical_conflicts = 0usize;
    let mut passes = 0usize;
    let mut unresolved_conflicts = 0usize;

    for pass in 0..max_passes {
        passes = pass + 1;
        for path_index in 0..connections.len() {
            if cancellation.is_cancelled() {
                return Err("rip-up routing cancelled".into());
            }
            if let Some(old_path) = paths[path_index].as_ref() {
                remove_maze_path(&mut occupancy, path_index, old_path);
            }
            let new_path = negotiated_maze_path(
                &occupancy,
                &history,
                &blocked,
                &paths,
                &connections,
                path_index,
                options.tracks_per_channel,
                SITES_PER_ROW,
                p.rows,
                cancellation,
            )?
            .ok_or_else(|| "negotiated maze routing failed to find a path".to_string())?;
            insert_maze_path(&mut occupancy, path_index, &new_path);
            paths[path_index] = Some(new_path);
        }

        let mut conflicts = Vec::new();
        for (resource, path_ids) in &occupancy {
            if cancellation.is_cancelled() {
                return Err("rip-up routing cancelled".into());
            }
            let mut nets = std::collections::BTreeSet::new();
            for &path_index in path_ids {
                nets.insert(connections[path_index].net);
            }
            if nets.len() > 1 {
                conflicts.push((*resource, nets.len() - 1));
            }
        }
        unresolved_conflicts = conflicts.len();
        if conflicts.is_empty() {
            break;
        }
        historical_conflicts += conflicts.len();
        for (resource, excess) in conflicts {
            *history.entry(resource).or_default() += excess;
        }
    }

    if unresolved_conflicts > 0 {
        return Err(format!(
            "negotiated maze routing did not converge in {} pass(es)",
            passes
        ));
    }

    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    for (path_index, path) in paths.iter().enumerate() {
        if cancellation.is_cancelled() {
            return Err("rip-up routing cancelled".into());
        }
        let Some(path) = path.as_ref() else {
            continue;
        };
        let net = connections[path_index].net;
        for edge in path {
            let mut merged = false;
            if let Some(last) = segments.last_mut() {
                if last.net == net
                    && last.orientation == edge.orientation
                    && last.channel == edge.channel
                    && last.track == edge.track
                    && (last.end == edge.start || edge.end == last.start)
                {
                    last.start = last.start.min(edge.start);
                    last.end = last.end.max(edge.end);
                    merged = true;
                }
            }
            if !merged {
                segments.push(Segment {
                    net,
                    orientation: edge.orientation,
                    channel: edge.channel,
                    start: edge.start,
                    end: edge.end,
                    track: edge.track,
                });
            }
        }
        for pair in path.windows(2) {
            if pair[0].orientation != pair[1].orientation {
                let (x, y) = pair[0].to;
                via_locations.push(Via { net, x, y });
            }
        }
    }
    let vias = via_locations.len();
    let routing = Routing {
        segments,
        vias,
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: Vec::new(),
        blocked_channels: options.blocked_channels.clone(),
    };
    let routing = match check_routing(&routing) {
        Ok(()) => routing,
        Err(_) => strict_priority_maze_routing(nl, p, options, &connections, cancellation)?,
    };
    Ok(MazeRipupResult {
        routing,
        passes,
        historical_conflicts,
    })
}

/// Conflict-free fallback for negotiated routing.
///
/// Negotiated congestion uses exact-edge history keys. Two partial-edge
/// reservations on the same track can therefore overlap even when every
/// complete-edge conflict has converged. The fallback preserves the caller's
/// connection priority while allocating full overlapping intervals strictly.
fn strict_priority_maze_routing(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    connections: &[MazeConnection],
    cancellation: &FlowCancellation,
) -> Result<Routing, String> {
    if cancellation.is_cancelled() {
        return Err("maze routing cancelled".into());
    }
    let mut allocator = TrackAllocator::with_blockages(options.blocked_set());
    let mut segments: Vec<Segment> = Vec::new();
    let mut via_locations = Vec::new();
    for connection in connections {
        if cancellation.is_cancelled() {
            return Err("maze routing cancelled".into());
        }
        let edges = maze_path(
            &allocator,
            options.tracks_per_channel,
            connection.source,
            connection.sink,
            SITES_PER_ROW,
            p.rows,
            cancellation,
        )?
        .ok_or_else(|| {
            format!(
                "strict priority maze routing failed for net '{}'",
                nl.net(connection.net).name
            )
        })?;
        if edges.is_empty() {
            continue;
        }
        for edge in &edges {
            allocator.reserve(
                edge.orientation,
                edge.channel,
                edge.start,
                edge.end,
                edge.track,
                connection.net,
            );
        }
        for edge in &edges {
            let mut merged = false;
            if let Some(last) = segments.last_mut() {
                if last.net == connection.net
                    && last.orientation == edge.orientation
                    && last.channel == edge.channel
                    && last.track == edge.track
                    && (last.end == edge.start || edge.end == last.start)
                {
                    last.start = last.start.min(edge.start);
                    last.end = last.end.max(edge.end);
                    merged = true;
                }
            }
            if !merged {
                segments.push(Segment {
                    net: connection.net,
                    orientation: edge.orientation,
                    channel: edge.channel,
                    start: edge.start,
                    end: edge.end,
                    track: edge.track,
                });
            }
        }
        for pair in edges.windows(2) {
            if pair[0].orientation != pair[1].orientation {
                via_locations.push(Via {
                    net: connection.net,
                    x: pair[0].to.0,
                    y: pair[0].to.1,
                });
            }
        }
    }
    let routing = Routing {
        segments,
        vias: via_locations.len(),
        via_locations,
        tracks_per_channel: options.tracks_per_channel,
        congested_channels: Vec::new(),
        blocked_channels: options.blocked_channels.clone(),
    };
    check_routing(&routing)?;
    Ok(routing)
}

fn weighted_routing_wirelength(routing: &Routing, timing: &TimingDrivenRoutingOptions) -> f64 {
    routing
        .segments
        .iter()
        .map(|segment| segment.length() as f64 * timing.weight(segment.net))
        .sum()
}

/// Timing-prioritized negotiated-congestion rip-up/reroute.
///
/// The baseline uses neutral net priority. The optimized run reroutes
/// high-weight nets first and reports the weighted wirelength change.
pub fn route_maze_timing_ripgup(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
) -> Result<TimingDrivenRipupResult, String> {
    route_maze_timing_ripgup_cancellable(
        nl,
        p,
        options,
        timing,
        max_passes,
        &FlowCancellation::new(),
    )
}

pub fn route_maze_timing_ripgup_cancellable(
    nl: &Netlist,
    p: &Placement,
    options: &RouteOptions,
    timing: &TimingDrivenRoutingOptions,
    max_passes: usize,
    cancellation: &FlowCancellation,
) -> Result<TimingDrivenRipupResult, String> {
    timing.validate(nl.net_count())?;
    let baseline = route_maze_ripgup_impl(nl, p, options, max_passes, None, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("timing-driven rip-up routing cancelled".into());
    }
    let optimized = route_maze_ripgup_impl(nl, p, options, max_passes, Some(timing), cancellation)?;
    let initial = weighted_routing_wirelength(&baseline.routing, timing);
    let final_weight = weighted_routing_wirelength(&optimized.routing, timing);
    Ok(TimingDrivenRipupResult {
        ripup: optimized,
        initial_weighted_wirelength: initial,
        final_weighted_wirelength: final_weight,
    })
}

pub fn timing_driven_ripgup_report(result: &TimingDrivenRipupResult) -> String {
    format!(
        "Timing-driven rip-up report: {} pass(es), {} historical conflict(s)\n  weighted wirelength : {:.3} -> {:.3}\n",
        result.ripup.passes,
        result.ripup.historical_conflicts,
        result.initial_weighted_wirelength,
        result.final_weighted_wirelength
    )
}
