//! Core PnR data model and algorithms split by placement, routing, and checks.

mod checks;
mod placement;
mod routing;
mod types;

pub use checks::{check_routing, drc, drc_report, drc_with_options, report, routing_report};
pub use placement::{
    check_legal, extend_placement, hpwl, optimize_placement_timing_driven, place,
    place_timing_driven, place_with_blockages, timing_driven_placement_report, weighted_hpwl,
};
pub use routing::{
    route, route_maze, route_maze_ripgup, route_maze_ripgup_cancellable, route_maze_timing_ripgup,
    route_maze_timing_ripgup_cancellable, route_maze_with_options,
    route_maze_with_options_cancellable, route_with_options, route_with_options_cancellable,
    timing_driven_ripgup_report, MazeRipupResult, TimingDrivenRipupResult,
};
pub use types::{
    DrcOptions, DrcResult, DrcSeverity, DrcViolation, Orientation, Placement, PlacementBlockage,
    Pos, RouteChannelBlockage, RouteOptions, Routing, Segment, TimingDrivenPlacementOptions,
    TimingDrivenPlacementResult, TimingDrivenRoutingOptions, Via, MAX_ANTENNA_RATIO, SITES_PER_ROW,
    TRACKS_PER_CHANNEL,
};
