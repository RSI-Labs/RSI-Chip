//! # eda-pnr — placement and routing estimates
//!
//! Row-based placement in standard-cell site grid (1 row = 1 site height):
//! cells are packed left-to-right in topological order, wrapping to new rows.
//! Wire estimates use HPWL (half-perimeter wire length) over net bounding
//! boxes of cell centers.
pub mod any_angle;
pub mod any_angle_maze;
pub mod any_angle_route;
pub mod layout;
pub mod layout_browser;
pub mod physical;
pub use layout_browser::{LayoutBrowserServer, LayoutBrowserService};
pub use physical::{
    parse_def, physical_from_def, write_def, write_lef, Def, DefComponent, DefNet,
    DefPlacementBlockage, DefRoute, DefRouteBlockage, DefVia,
};

// Core PnR engine split by model, placement, routing, and physical checks.
mod pnr;
pub use pnr::*;

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::cancellation::FlowCancellation;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, CellKind, Dir, Netlist};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn congested_routing_fixture() -> (Netlist, Placement) {
        let mut nl = Netlist::new("congested");
        let n0 = nl.add_net("n0");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        let n3 = nl.add_net("n3");
        nl.add_cell(Cell {
            name: "d0".into(),
            kind: CellKind::Not,
            inputs: vec![],
            output: n0,
        });
        nl.add_cell(Cell {
            name: "d1".into(),
            kind: CellKind::Not,
            inputs: vec![],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "s0".into(),
            kind: CellKind::Not,
            inputs: vec![n0],
            output: n2,
        });
        nl.add_cell(Cell {
            name: "s1".into(),
            kind: CellKind::Not,
            inputs: vec![n1],
            output: n3,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![
                Pos { row: 0, site: 0 },
                Pos { row: 0, site: 4 },
                Pos { row: 0, site: 8 },
                Pos { row: 0, site: 12 },
            ],
            cell_width: vec![2, 2, 2, 2],
            blockages: Vec::new(),
        };
        (nl, placement)
    }

    #[test]
    fn simple_place_legal() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("p");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let p = place(&nl).unwrap();
        check_legal(&nl, &p).unwrap();
        assert!(hpwl(&nl, &p) >= 0.0);
    }

    #[test]
    fn placement_avoids_hard_blockages() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("placement-blockage");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);

        let blockage = PlacementBlockage::new("macro", 0, 0, 2).unwrap();
        let placement = place_with_blockages(&nl, std::slice::from_ref(&blockage)).unwrap();
        assert_eq!(placement.blockages, vec![blockage.clone()]);
        check_legal(&nl, &placement).unwrap();
        assert!(placement
            .cell_pos
            .iter()
            .any(|position| { position.row > 0 || position.site > blockage.end_site }));
        assert!(report(&nl, &placement).contains("hard placement blockages: 1"));

        let mut illegal = placement.clone();
        illegal.cell_pos[0] = Pos {
            row: 0,
            site: blockage.start_site,
        };
        assert!(check_legal(&nl, &illegal).is_err());

        let duplicate = PlacementBlockage::new("duplicate", 0, 1, 4).unwrap();
        assert!(place_with_blockages(&nl, &[blockage, duplicate]).is_err());
        assert!(PlacementBlockage::new("", 0, 0, 2).is_err());
        assert!(PlacementBlockage::new("bad", 0, 3, 2).is_err());
    }

    #[test]
    fn maze_router_avoids_hard_channel_blockage() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("blocked");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let p = place(&nl).unwrap();

        let baseline = route_maze(&nl, &p).unwrap();
        assert!(baseline.segments.iter().any(|segment| {
            segment.orientation == Orientation::Vertical && segment.channel == 2
        }));

        let options = RouteOptions::new(4)
            .unwrap()
            .with_blocked_channel("keepout0", Orientation::Vertical, 2)
            .unwrap();
        assert!(options
            .clone()
            .with_blocked_channel("keepout0-duplicate", Orientation::Vertical, 2)
            .is_err());
        let routed = route_maze_with_options(&nl, &p, &options).unwrap();
        assert!(!routed.segments.is_empty());
        assert_eq!(routed.blocked_channels.len(), 1);
        assert!(routed.segments.iter().all(|segment| {
            !(segment.orientation == Orientation::Vertical && segment.channel == 2)
        }));
        check_routing(&routed).unwrap();
        assert!(routing_report(&routed).contains("hard blockages: 1"));

        let mut illegal = routed.clone();
        illegal.segments.push(Segment {
            net: 0,
            orientation: Orientation::Vertical,
            channel: 2,
            start: 0,
            end: 1,
            track: 0,
        });
        assert!(check_routing(&illegal).is_err());
    }

    #[test]
    fn extend_placement_legalizes_appended_cells() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("extend");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        let original = place(&nl).unwrap();
        let n = nl.add_net("n");
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![y],
            output: n,
        });
        let extended = extend_placement(&nl, &original).unwrap();
        assert_eq!(extended.cell_pos.len(), nl.cell_count());
        assert_eq!(extended.cell_width.len(), nl.cell_count());
        check_legal(&nl, &extended).unwrap();

        let mut short = original.clone();
        short.cell_pos.push(original.cell_pos[0]);
        short.cell_pos.push(original.cell_pos[0]);
        assert!(extend_placement(&nl, &short).is_err());
    }

    #[test]
    fn timing_driven_placement_prioritizes_critical_net() {
        let mut nl = Netlist::new("critical_place");
        let mut sources = Vec::new();
        let mut sinks = Vec::new();
        for index in 0..3 {
            sources.push(nl.add_net(format!("src{}", index)));
        }
        for index in 0..3 {
            sinks.push(nl.add_net(format!("sink{}", index)));
        }
        for (index, source) in sources.iter().enumerate() {
            nl.add_cell(Cell {
                name: format!("d{}", index),
                kind: CellKind::Not,
                inputs: Vec::new(),
                output: *source,
            });
        }
        for index in 0..3 {
            nl.add_cell(Cell {
                name: format!("s{}", index),
                kind: CellKind::Not,
                inputs: vec![sources[index]],
                output: sinks[index],
            });
        }

        let options = TimingDrivenPlacementOptions {
            net_weights: HashMap::from([(sources[0], 100.0)]),
            default_net_weight: 1.0,
            max_passes: 4,
        };
        let result = place_timing_driven(&nl, &options).unwrap();
        check_legal(&nl, &result.placement).unwrap();
        assert!(result.improving_moves > 0);
        assert!(result.final_weighted_hpwl < result.initial_weighted_hpwl);
        assert!(result.final_weighted_hpwl <= result.initial_weighted_hpwl);
        assert!(timing_driven_placement_report(&result).contains("weighted HPWL"));

        let d0 = nl
            .cells()
            .iter()
            .position(|cell| cell.name == "d0")
            .unwrap();
        let s0 = nl
            .cells()
            .iter()
            .position(|cell| cell.name == "s0")
            .unwrap();
        let distance = |cell: usize| result.placement.cell_pos[cell].site;
        assert!(distance(s0) - distance(d0) <= 2);

        let mut invalid = options.clone();
        invalid.net_weights.insert(nl.net_count(), 1.0);
        assert!(place_timing_driven(&nl, &invalid).is_err());
    }

    #[test]
    fn global_routes_are_legal() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("route");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: n2,
        });
        nl.add_cell(Cell {
            name: "u3".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![n2, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let routing = route(&nl, &placement).unwrap();
        check_routing(&routing).unwrap();
        assert!(!routing.segments.is_empty());
        assert!(routing.wirelength() > 0);
    }

    #[test]
    fn routing_capacity_is_configurable() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(2).unwrap();
        let routing = route_with_options(&nl, &placement, &options).unwrap();
        check_routing(&routing).unwrap();
        assert_eq!(routing.tracks_per_channel, 2);
        assert!(routing.congested_channels.is_empty());
        // Two different nets overlap in row channel 0 and use both tracks.
        assert!((routing.utilization() - 1.0).abs() < 1e-9);

        let one_track = RouteOptions::new(1).unwrap();
        assert!(route_with_options(&nl, &placement, &one_track).is_err());
    }

    #[test]
    fn maze_router_detours_instead_of_failing_on_local_conflict() {
        let (nl, placement) = congested_routing_fixture();
        let one_track = RouteOptions::new(1).unwrap();
        assert!(route_with_options(&nl, &placement, &one_track).is_err());

        let routing = route_maze_with_options(&nl, &placement, &one_track).unwrap();
        check_routing(&routing).unwrap();
        assert!(routing.congested_channels.is_empty());
        assert!(routing.vias >= 2);
        let drc_result = drc(&nl, &placement, &routing).unwrap();
        assert!(!drc_result.has_errors(), "{:?}", drc_result.violations);
        assert!(routing
            .segments
            .iter()
            .any(|segment| segment.orientation == Orientation::Horizontal
                && segment.channel == placement.rows));
    }

    #[test]
    fn negotiated_ripgup_router_converges_to_legal_solution() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let result = route_maze_ripgup(&nl, &placement, &options, 8).unwrap();
        check_routing(&result.routing).unwrap();
        assert_eq!(result.routing.congested_channels.len(), 0);
        assert!(result.passes >= 1);
        let drc_result = drc(&nl, &placement, &result.routing).unwrap();
        assert!(!drc_result.has_errors(), "{:?}", drc_result.violations);

        assert!(route_maze_ripgup(&nl, &placement, &options, 0).is_err());
    }

    #[test]
    fn routers_honor_cooperative_cancellation() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        assert_eq!(
            route_with_options_cancellable(&nl, &placement, &options, &cancellation).unwrap_err(),
            "global routing cancelled"
        );
        assert_eq!(
            route_maze_with_options_cancellable(&nl, &placement, &options, &cancellation)
                .unwrap_err(),
            "maze routing cancelled"
        );
        assert_eq!(
            route_maze_ripgup_cancellable(&nl, &placement, &options, 8, &cancellation).unwrap_err(),
            "rip-up routing cancelled"
        );
        let timing = TimingDrivenRoutingOptions {
            net_weights: HashMap::from([(1, 10.0)]),
            default_net_weight: 1.0,
        };
        assert_eq!(
            route_maze_timing_ripgup_cancellable(
                &nl,
                &placement,
                &options,
                &timing,
                8,
                &cancellation
            )
            .unwrap_err(),
            "rip-up routing cancelled"
        );
    }

    #[test]
    fn timing_driven_ripgup_prioritizes_critical_net() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions::new(1).unwrap();
        let timing = TimingDrivenRoutingOptions {
            net_weights: HashMap::from([(1, 10.0)]),
            default_net_weight: 1.0,
        };
        let result = route_maze_timing_ripgup(&nl, &placement, &options, &timing, 8).unwrap();
        check_routing(&result.ripup.routing).unwrap();
        assert!(
            result.final_weighted_wirelength < result.initial_weighted_wirelength,
            "{:?}",
            result
        );
        assert!(timing_driven_ripgup_report(&result).contains("weighted wirelength"));

        let critical_horizontal_rows: std::collections::BTreeSet<_> = result
            .ripup
            .routing
            .segments
            .iter()
            .filter(|segment| segment.net == 1 && segment.orientation == Orientation::Horizontal)
            .map(|segment| segment.channel)
            .collect();
        assert!(
            critical_horizontal_rows.contains(&0),
            "{:?}",
            result.ripup.routing
        );

        let mut invalid = timing.clone();
        invalid.net_weights.insert(nl.net_count(), 1.0);
        assert!(route_maze_timing_ripgup(&nl, &placement, &options, &invalid, 8).is_err());
    }

    #[test]
    fn layer_changes_have_explicit_vias() {
        let design = eda_hdl::compile(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
            None,
        )
        .unwrap();
        let nl = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )
        .unwrap()
        .netlist;
        let mut placement = place(&nl).unwrap();
        // Force at least two placement rows independently of the default row
        // width: a 256-site default otherwise packs this small fixture into
        // one row and produces horizontal-only routes.
        placement.cell_pos[0] = Pos { row: 0, site: 0 };
        let (mut row, mut site) = (1usize, 0usize);
        for ci in 1..nl.cell_count() {
            let width = placement.cell_width[ci].max(1);
            if site + width > SITES_PER_ROW {
                row += 1;
                site = 0;
            }
            placement.cell_pos[ci] = Pos { row, site };
            site += width;
        }
        placement.rows = row + 1;
        let mut routing = route(&nl, &placement).unwrap();
        assert!(!routing.via_locations.is_empty());
        assert_eq!(routing.vias, routing.via_locations.len());
        check_routing(&routing).unwrap();

        routing.via_locations.pop();
        routing.vias = routing.via_locations.len();
        assert!(check_routing(&routing).is_ok());
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors(), "{}", drc_report(&result));
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn partial_congested_routing_is_drc_diagnosable() {
        let (nl, placement) = congested_routing_fixture();
        let options = RouteOptions {
            tracks_per_channel: 1,
            allow_congestion: true,
            blocked_channels: Vec::new(),
        };
        let routing = route_with_options(&nl, &placement, &options).unwrap();
        assert_eq!(
            routing.congested_channels,
            vec![(Orientation::Horizontal, 0)]
        );
        check_routing(&routing).unwrap();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors());
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn drc_accepts_generated_layout() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("XOR2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let routing = route(&nl, &placement).unwrap();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(!result.has_errors(), "{}", drc_report(&result));

        let strict = DrcOptions {
            max_antenna_ratio: 0.001,
        };
        let strict_result = drc_with_options(&nl, &placement, &routing, &strict).unwrap();
        assert!(strict_result
            .violations
            .iter()
            .any(|violation| violation.code == "ANT-001"
                && violation.message.contains("exceeds 0.00")));
        assert!(drc_with_options(
            &nl,
            &placement,
            &routing,
            &DrcOptions {
                max_antenna_ratio: 0.0,
            }
        )
        .is_err());
    }

    #[test]
    fn drc_rejects_bad_track() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments[0].track = routing.tracks_per_channel + 10;
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.has_errors());
        assert!(result.violations.iter().any(|v| v.code == "ROUTE-001"));
    }

    #[test]
    fn drc_detects_open_net() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments.clear();
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.violations.iter().any(|v| v.code == "OPEN-001"));
    }

    #[test]
    fn drc_detects_unknown_routed_net() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("drc_clean");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement = place(&nl).unwrap();
        let mut routing = route(&nl, &placement).unwrap();
        routing.segments[0].net = nl.net_count() + 100;
        let result = drc(&nl, &placement, &routing).unwrap();
        assert!(result.violations.iter().any(|v| v.code == "ROUTE-002"));
    }
}
