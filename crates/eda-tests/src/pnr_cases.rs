//! Place & route cases (exercising the eda-pnr crate).

use crate::CaseList;
use eda_core::netlist::{Cell, CellKind, Netlist};
use std::sync::OnceLock;

fn synth_cached(src: &'static str) -> &'static Netlist {
    static DESIGNS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Netlist>>,
    > = OnceLock::new();
    let m = DESIGNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    g.entry(src).or_insert_with(|| {
        Box::leak(Box::new(
            eda_synth::synthesize(
                Box::leak(Box::new(eda_hdl::compile(src, None).expect("compile"))),
                &eda_synth::SynthOptions {
                    optimize: true,
                    map_lib: true,
                },
            )
            .expect("synth")
            .netlist,
        ))
    })
}

fn congested_fixture() -> (Netlist, eda_pnr::Placement) {
    let mut nl = Netlist::new("congested_routes");
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
    let placement = eda_pnr::Placement {
        rows: 1,
        cell_pos: vec![
            eda_pnr::Pos { row: 0, site: 0 },
            eda_pnr::Pos { row: 0, site: 4 },
            eda_pnr::Pos { row: 0, site: 8 },
            eda_pnr::Pos { row: 0, site: 12 },
        ],
        cell_width: vec![2, 2, 2, 2],
        blockages: Vec::new(),
    };
    (nl, placement)
}

pub fn register(c: &mut CaseList) {
    any_angle_direct_routes(c);

    c.add("pnr_place_legal_rows", || {
        let nl =
            synth_cached("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        let p = eda_pnr::place(nl)?;
        eda_pnr::check_legal(nl, &p)
    });
    c.add("pnr_all_cells_placed", || {
        let nl = synth_cached("module m(input s, a, b, output y); assign y = s ? b : a; endmodule");
        let p = eda_pnr::place(nl)?;
        if p.cell_pos.len() != nl.cell_count() {
            Err(format!(
                "placed {} of {}",
                p.cell_pos.len(),
                nl.cell_count()
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_no_overlap", || {
        let nl =
            synth_cached("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        let p = eda_pnr::place(nl)?;
        let mut slots: std::collections::HashSet<(usize, usize, usize)> =
            std::collections::HashSet::new();
        for (ci, pos) in p.cell_pos.iter().enumerate() {
            let w = p.cell_width[ci].max(1);
            for k in 0..w {
                if !slots.insert((pos.row, pos.site + k, 0)) {
                    return Err(format!(
                        "cells overlap at {:?}",
                        (pos.row, pos.site + k, ci)
                    ));
                }
            }
        }
        Ok(())
    });
    c.add("pnr_hpwl_positive", || {
        let nl = synth_cached(
            "module m(input a, b, c, d, output y); assign y = (a & b) | (c & d); endmodule",
        );
        let p = eda_pnr::place(nl)?;
        let h = eda_pnr::hpwl(nl, &p);
        if h < 0.0 || !h.is_finite() {
            Err(format!("hpwl {}", h))
        } else {
            Ok(())
        }
    });
    c.add("pnr_timing_driven_placement_reduces_weighted_hpwl", || {
        let mut nl = Netlist::new("critical_place");
        let sources = (0..3)
            .map(|index| nl.add_net(format!("src{}", index)))
            .collect::<Vec<_>>();
        let sinks = (0..3)
            .map(|index| nl.add_net(format!("sink{}", index)))
            .collect::<Vec<_>>();
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
        let options = eda_pnr::TimingDrivenPlacementOptions {
            net_weights: std::collections::HashMap::from([(sources[0], 100.0)]),
            default_net_weight: 1.0,
            max_passes: 4,
        };
        let result = eda_pnr::place_timing_driven(&nl, &options)?;
        eda_pnr::check_legal(&nl, &result.placement)?;
        if result.improving_moves == 0
            || result.final_weighted_hpwl >= result.initial_weighted_hpwl
            || !eda_pnr::timing_driven_placement_report(&result).contains("weighted HPWL")
        {
            Err(eda_pnr::timing_driven_placement_report(&result))
        } else {
            Ok(())
        }
    });
    c.add("pnr_timing_driven_placement_supports_route", || {
        let nl =
            synth_cached("module m(input a, b, c, output y); assign y = (a & b) | c; endmodule");
        let options = eda_pnr::TimingDrivenPlacementOptions::default();
        let result = eda_pnr::place_timing_driven(nl, &options)?;
        let routing = eda_pnr::route(nl, &result.placement)?;
        eda_pnr::check_legal(nl, &result.placement)?;
        eda_pnr::check_routing(&routing)?;
        if routing.segments.is_empty() {
            Err("timing-driven placement produced no routable segments".into())
        } else {
            Ok(())
        }
    });
    c.add("pnr_global_route_legal", || {
        let nl = synth_cached(
            "module m(input a, b, c, d, output y); assign y = (a & b) | (c & d); endmodule",
        );
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        eda_pnr::check_routing(&r)?;
        if r.segments.is_empty() || r.wirelength() == 0 {
            Err("router produced an empty route".into())
        } else {
            Ok(())
        }
    });
    c.add("pnr_routing_report", || {
        let nl = synth_cached("module m(input a, b, output y); assign y = a & b; endmodule");
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        let text = eda_pnr::routing_report(&r);
        if text.contains("segments") && text.contains("wirelength") {
            Ok(())
        } else {
            Err(text)
        }
    });
    c.add("pnr_route_capacity_options", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions::new(2)?;
        let routing = eda_pnr::route_with_options(&nl, &placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 2
            || !routing.congested_channels.is_empty()
            || (routing.utilization() - 1.0).abs() > 1e-9
        {
            Err(format!(
                "tracks={} congested={:?} utilization={}",
                routing.tracks_per_channel,
                routing.congested_channels,
                routing.utilization()
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_maze_router_detours_local_conflict", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions::new(1)?;
        if eda_pnr::route_with_options(&nl, &placement, &options).is_ok() {
            return Err("fixture no longer exercises local routing conflict".into());
        }
        let routing = eda_pnr::route_maze_with_options(&nl, &placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        let drc = eda_pnr::drc(&nl, &placement, &routing)?;
        if drc.has_errors() || !routing.congested_channels.is_empty() || routing.vias < 2 {
            Err(format!(
                "drc={:?} congested={:?} vias={}",
                drc.violations, routing.congested_channels, routing.vias
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_maze_router_uses_explicit_capacity", || {
        let nl =
            synth_cached("module m(input a, b, c, output y); assign y = (a & b) | c; endmodule");
        let placement = eda_pnr::place(nl)?;
        let options = eda_pnr::RouteOptions::new(3)?;
        let routing = eda_pnr::route_maze_with_options(nl, &placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 3 || routing.segments.is_empty() {
            Err(format!(
                "tracks={} segments={}",
                routing.tracks_per_channel,
                routing.segments.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_negotiated_ripgup_router_converges", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions::new(1)?;
        let result = eda_pnr::route_maze_ripgup(&nl, &placement, &options, 8)?;
        eda_pnr::check_routing(&result.routing)?;
        let drc = eda_pnr::drc(&nl, &placement, &result.routing)?;
        if drc.has_errors() || !result.routing.congested_channels.is_empty() || result.passes == 0 {
            Err(format!("passes={} drc={:?}", result.passes, drc.violations))
        } else {
            Ok(())
        }
    });
    c.add("pnr_negotiated_ripgup_rejects_invalid_pass_count", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions::new(1)?;
        if eda_pnr::route_maze_ripgup(&nl, &placement, &options, 0).is_ok() {
            Err("zero rip-up passes accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("pnr_timing_driven_ripgup_prioritizes_critical_net", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions::new(1)?;
        let timing = eda_pnr::TimingDrivenRoutingOptions {
            net_weights: std::collections::HashMap::from([(1, 10.0)]),
            default_net_weight: 1.0,
        };
        let result = eda_pnr::route_maze_timing_ripgup(&nl, &placement, &options, &timing, 8)?;
        eda_pnr::check_routing(&result.ripup.routing)?;
        if result.final_weighted_wirelength >= result.initial_weighted_wirelength
            || !eda_pnr::timing_driven_ripgup_report(&result).contains("weighted wirelength")
        {
            Err(eda_pnr::timing_driven_ripgup_report(&result))
        } else {
            Ok(())
        }
    });
    c.add(
        "pnr_timing_driven_ripgup_rejects_invalid_weight_net",
        || {
            let (nl, placement) = congested_fixture();
            let options = eda_pnr::RouteOptions::new(1)?;
            let timing = eda_pnr::TimingDrivenRoutingOptions {
                net_weights: std::collections::HashMap::from([(nl.net_count(), 2.0)]),
                default_net_weight: 1.0,
            };
            if eda_pnr::route_maze_timing_ripgup(&nl, &placement, &options, &timing, 8).is_ok() {
                Err("invalid timing routing net accepted".into())
            } else {
                Ok(())
            }
        },
    );
    c.add("api_extend_physical_placement", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        let mut netlist = flow.netlist.clone();
        let n = netlist.add_net("eco_extension_net");
        netlist.add_cell(Cell {
            name: "eco_extension_buf".into(),
            kind: CellKind::Buf,
            inputs: vec![flow.netlist.net_by_name("y").ok_or("missing output net")?],
            output: n,
        });
        let extended = eda_api::extend_physical_placement(&netlist, &flow.placement)?;
        eda_pnr::check_legal(&netlist, &extended)?;
        if extended.cell_pos.len() != netlist.cell_count()
            || extended.cell_width.len() != netlist.cell_count()
        {
            Err(format!(
                "positions={} widths={} cells={}",
                extended.cell_pos.len(),
                extended.cell_width.len(),
                netlist.cell_count()
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_congestion_reports_partial_route", || {
        let (nl, placement) = congested_fixture();
        let options = eda_pnr::RouteOptions {
            tracks_per_channel: 1,
            allow_congestion: true,
            blocked_channels: Vec::new(),
        };
        let routing = eda_pnr::route_with_options(&nl, &placement, &options)?;
        let drc = eda_pnr::drc(&nl, &placement, &routing)?;
        if routing.congested_channels.is_empty() || !drc.has_errors() {
            Err(format!(
                "congested={:?} drc={}",
                routing.congested_channels,
                eda_pnr::drc_report(&drc)
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_explicit_vias_connect_layer_changes", || {
        let nl = synth_cached(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        eda_pnr::check_routing(&r)?;
        if r.via_locations.is_empty() || r.vias != r.via_locations.len() {
            Err(format!(
                "count={} explicit={}",
                r.vias,
                r.via_locations.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_missing_via_is_reported_as_open", || {
        let nl = synth_cached(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let p = eda_pnr::place(nl)?;
        let mut r = eda_pnr::route(nl, &p)?;
        r.via_locations.pop();
        r.vias = r.via_locations.len();
        let drc = eda_pnr::drc(nl, &p, &r)?;
        if drc.violations.iter().any(|v| v.code == "OPEN-001") {
            Ok(())
        } else {
            Err(eda_pnr::drc_report(&drc))
        }
    });
    c.add("pnr_lef_export", || {
        let nl = synth_cached("module m(input a, b, output y); assign y = a & b; endmodule");
        let p = eda_pnr::place(nl)?;
        let lef = eda_pnr::write_lef(nl, &p)?;
        if !lef.contains("VERSION 5.8")
            || !lef.contains("SITE core")
            || !lef.contains("MACRO AND2")
            || !lef.contains("PIN a")
            || !lef.contains("PIN y")
        {
            Err(format!(
                "LEF content incomplete: {}",
                &lef[..lef.len().min(200)]
            ))
        } else {
            Ok(())
        }
    });
    c.add("pnr_def_export_and_parse", || {
        let nl =
            synth_cached("module m(input a, b, c, output y); assign y = (a & b) | c; endmodule");
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        let text = eda_pnr::write_def(nl, &p, &r)?;
        let parsed = eda_pnr::parse_def(&text)?;
        if parsed.design != "m"
            || parsed.rows != p.rows
            || parsed.components.len() != nl.cell_count()
            || parsed.nets.len() != nl.net_count()
        {
            Err(format!(
                "DEF round-trip design={} rows={} components={} nets={}",
                parsed.design,
                parsed.rows,
                parsed.components.len(),
                parsed.nets.len()
            ))
        } else if parsed
            .nets
            .iter()
            .map(|net| net.routes.len())
            .sum::<usize>()
            != r.segments.len()
        {
            Err("DEF route count mismatch".into())
        } else {
            Ok(())
        }
    });
    c.add("pnr_drc_clean_generated_layout", || {
        let nl = synth_cached(
            "module m(input a, b, c, d, output y); assign y = (a & b) | (c & d); endmodule",
        );
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        let d = eda_pnr::drc(nl, &p, &r)?;
        if d.has_errors() {
            Err(eda_pnr::drc_report(&d))
        } else {
            Ok(())
        }
    });
    c.add("pnr_drc_detects_open", || {
        let nl = synth_cached("module m(input a, b, output y); assign y = a & b; endmodule");
        let p = eda_pnr::place(nl)?;
        let mut r = eda_pnr::route(nl, &p)?;
        r.segments.clear();
        let d = eda_pnr::drc(nl, &p, &r)?;
        if d.violations.iter().any(|v| v.code == "OPEN-001") {
            Ok(())
        } else {
            Err(eda_pnr::drc_report(&d))
        }
    });
    c.add("pnr_drc_detects_bad_track", || {
        let nl = synth_cached("module m(input a, b, output y); assign y = a ^ b; endmodule");
        let p = eda_pnr::place(nl)?;
        let mut r = eda_pnr::route(nl, &p)?;
        r.segments[0].track = r.tracks_per_channel + 1;
        let d = eda_pnr::drc(nl, &p, &r)?;
        if d.violations.iter().any(|v| v.code == "ROUTE-001") {
            Ok(())
        } else {
            Err(eda_pnr::drc_report(&d))
        }
    });
    c.add("pnr_drc_report_content", || {
        let nl = synth_cached("module m(input a, output y); assign y = ~a; endmodule");
        let p = eda_pnr::place(nl)?;
        let r = eda_pnr::route(nl, &p)?;
        let d = eda_pnr::drc(nl, &p, &r)?;
        let text = eda_pnr::drc_report(&d);
        if text.contains("DRC report") && text.contains("error") {
            Ok(())
        } else {
            Err(text)
        }
    });
    let circuits: Vec<&'static str> = vec![
        "module m(input a, b, output y); assign y = a & b; endmodule",
        "module m(input a, b, output y); assign y = a | b; endmodule",
        "module m(input a, b, output y); assign y = a ^ b; endmodule",
        "module m(input a, b, output y); assign y = ~(a & b); endmodule",
        "module m(input [1:0] a, output y); assign y = &a; endmodule",
        "module m(input [1:0] a, output y); assign y = |a; endmodule",
        "module m(input [1:0] a, output y); assign y = ^a; endmodule",
        "module m(input [3:0] a, output [3:0] y); assign y = ~a; endmodule",
        "module m(input [3:0] a, output [3:0] y); assign y = a << 1; endmodule",
        "module m(input [3:0] a, output [3:0] y); assign y = a >> 1; endmodule",
        "module m(input [3:0] a, b, output [3:0] y); assign y = a & b; endmodule",
        "module m(input [3:0] a, b, output [3:0] y); assign y = a | b; endmodule",
    ];
    for (i, src) in circuits.into_iter().enumerate() {
        c.add(format!("pnr_drc_circuit_{}", i), move || {
            let nl = synth_cached(src);
            let p = eda_pnr::place(nl)?;
            let r = eda_pnr::route(nl, &p)?;
            let d = eda_pnr::drc(nl, &p, &r)?;
            if d.has_errors() {
                Err(eda_pnr::drc_report(&d))
            } else {
                Ok(())
            }
        });
    }
    c.add("pnr_report_contains_cells", || {
        let nl = synth_cached("module m(input a, b, output y); assign y = a & b; endmodule");
        let p = eda_pnr::place(nl)?;
        let report = eda_pnr::report(nl, &p);
        if report.is_empty() || !report.contains("cells") {
            Err("report content".into())
        } else {
            Ok(())
        }
    });
    c.add("pnr_rows_fit_total_width", || {
        let nl =
            synth_cached("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        let p = eda_pnr::place(nl)?;
        if p.rows == 0 {
            Err("no rows".into())
        } else {
            Ok(())
        }
    });
}

fn any_angle_direct_routes(c: &mut CaseList) {
    for index in 0_usize..100 {
        c.add(
            format!("pnr_any_angle_direct_route_{index:03}"),
            move || {
                let variant = index % 5;
                let dx = 2 + (index % 9) as i64;
                let dy = 1 + ((index / 3) % 11) as i64;
                match variant {
                    0 => {
                        let request = eda_api::AnyAngleRouteRequest::new(
                            index,
                            eda_api::LayoutPoint::new(0, 0),
                            eda_api::LayoutPoint::new(dx, dy),
                        )?;
                        let routing = eda_api::route_any_angle_direct(
                            &[request],
                            &[],
                            &eda_api::AnyAngleRoutingOptions::default(),
                        )?;
                        let expected = (dx * dx + dy * dy) as f64;
                        if routing.routes.len() != 1
                            || routing.routes[0].net != index
                            || routing.routes[0].length_squared.numerator != expected as i128
                            || routing.routes[0].length_squared.denominator != 1
                            || (routing.total_length() - expected.sqrt()).abs() > 1e-12
                        {
                            return Err(format!(
                                "single route={routing:?}, expected squared {expected}"
                            ));
                        }
                    }
                    1 => {
                        let spacing = 3 + (index % 15) as i64;
                        let requests = [
                            eda_api::AnyAngleRouteRequest::new(
                                10,
                                eda_api::LayoutPoint::new(0, 0),
                                eda_api::LayoutPoint::new(dx, 0),
                            )?,
                            eda_api::AnyAngleRouteRequest::new(
                                11,
                                eda_api::LayoutPoint::new(0, spacing),
                                eda_api::LayoutPoint::new(dx, spacing),
                            )?,
                        ];
                        let routing = eda_api::route_any_angle_direct(
                            &requests,
                            &[],
                            &eda_api::AnyAngleRoutingOptions {
                                min_spacing: spacing,
                            },
                        )?;
                        if routing.routes.len() != 2
                            || (routing.total_length() - 2.0 * dx as f64).abs() > 1e-12
                        {
                            return Err(format!("parallel routes={routing:?}"));
                        }
                        let violation = eda_api::route_any_angle_direct(
                            &requests,
                            &[],
                            &eda_api::AnyAngleRoutingOptions {
                                min_spacing: spacing + 1,
                            },
                        );
                        let error = violation.err().ok_or("expected spacing violation")?;
                        if !error.contains("violate spacing") {
                            return Err(format!("unexpected spacing error: {error}"));
                        }
                    }
                    2 => {
                        let blockage = eda_api::AnyAnglePolygon::new([
                            eda_api::LayoutPoint::new(dx, -3),
                            eda_api::LayoutPoint::new(dx + 5, 0),
                            eda_api::LayoutPoint::new(dx, 3),
                        ])?;
                        let crossing = eda_api::AnyAngleRouteRequest::new(
                            20,
                            eda_api::LayoutPoint::new(0, 0),
                            eda_api::LayoutPoint::new(2 * dx + 10, 0),
                        )?;
                        let error = eda_api::route_any_angle_direct(
                            &[crossing],
                            &[blockage],
                            &eda_api::AnyAngleRoutingOptions::default(),
                        )
                        .err()
                        .ok_or("expected blockage violation")?;
                        if !error.contains("crosses a layout blockage") {
                            return Err(format!("unexpected blockage error: {error}"));
                        }
                    }
                    3 => {
                        let requests = [
                            eda_api::AnyAngleRouteRequest::new(
                                30,
                                eda_api::LayoutPoint::new(0, 0),
                                eda_api::LayoutPoint::new(dx, dy),
                            )?,
                            eda_api::AnyAngleRouteRequest::new(
                                30,
                                eda_api::LayoutPoint::new(dx, 0),
                                eda_api::LayoutPoint::new(0, dy),
                            )?,
                        ];
                        let routing = eda_api::route_any_angle_direct(
                            &requests,
                            &[],
                            &eda_api::AnyAngleRoutingOptions::default(),
                        )?;
                        let expected_length = 2.0 * ((dx * dx + dy * dy) as f64).sqrt();
                        if routing.routes.len() != 2
                            || routing.routes.iter().any(|route| route.net != 30)
                            || (routing.total_length() - expected_length).abs() > 1e-12
                        {
                            return Err(format!("same-net routes={routing:?}"));
                        }
                    }
                    _ => {
                        let error = eda_api::AnyAngleRouteRequest::new(
                            40,
                            eda_api::LayoutPoint::new(dx, dy),
                            eda_api::LayoutPoint::new(dx, dy),
                        )
                        .err()
                        .ok_or("expected zero-length route rejection")?;
                        if !error.contains("endpoints must differ") {
                            return Err(format!("unexpected endpoint error: {error}"));
                        }
                    }
                }
                Ok(())
            },
        );
    }
}
