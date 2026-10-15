//! Physical import/export, DEF blockages, antenna repair, power, verification, and compile errors.

use super::*;

fn force_layer_change_placement(
    nl: &eda_core::netlist::Netlist,
    placement: &eda_pnr::Placement,
) -> Result<eda_pnr::Placement, String> {
    let mut forced = placement.clone();
    forced.cell_pos[0] = eda_pnr::Pos { row: 0, site: 0 };
    let (mut row, mut site) = (1usize, 0usize);
    for ci in 1..nl.cell_count() {
        let width = forced.cell_width[ci].max(1);
        if site + width > eda_pnr::SITES_PER_ROW {
            row += 1;
            site = 0;
        }
        forced.cell_pos[ci] = eda_pnr::Pos { row, site };
        site += width;
    }
    forced.rows = row + 1;
    eda_pnr::check_legal(nl, &forced)?;
    Ok(forced)
}

pub(super) fn register(c: &mut CaseList) {
    c.add("api_timing_driven_placement_from_sta", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::timing_driven_place_from_timing(&flow.netlist, &flow.timing, 4)?;
        eda_pnr::check_legal(&flow.netlist, &result.placement)?;
        if !eda_api::timing_driven_placement_report(&result).contains("weighted HPWL") {
            Err(eda_api::timing_driven_placement_report(&result))
        } else {
            Ok(())
        }
    });
    c.add("api_timing_driven_placement_explicit_weights", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let critical_net = (0..flow.netlist.net_count())
            .find(|net| {
                let net = flow.netlist.net(*net);
                !net.drivers.is_empty() || !net.sinks.is_empty()
            })
            .ok_or_else(|| "netlist has no connected nets".to_string())?;
        let options = eda_api::TimingDrivenPlacementOptions {
            net_weights: std::collections::HashMap::from([(critical_net, 25.0)]),
            default_net_weight: 1.0,
            max_passes: 4,
        };
        let result = eda_api::timing_driven_place(&flow.netlist, &options)?;
        if result.final_weighted_hpwl <= result.initial_weighted_hpwl
            && eda_api::timing_driven_placement_report(&result).contains("weighted HPWL")
        {
            Ok(())
        } else {
            Err(eda_api::timing_driven_placement_report(&result))
        }
    });
    c.add("api_physical_export_preserves_vias", || {
        let src = "module m(input clk, rst, output reg [3:0] q);
            always @(posedge clk or posedge rst)
                if (rst) q <= 4'd0; else q <= q + 4'd1;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let placement = force_layer_change_placement(&flow.netlist, &flow.placement)?;
        let routing = eda_pnr::route(&flow.netlist, &placement)?;
        let (_, def_text) = eda_api::export_physical(&flow.netlist, &placement, &routing)?;
        let def = eda_api::parse_physical_def(&def_text)?;
        let via_count = def.nets.iter().map(|net| net.vias.len()).sum::<usize>();
        if via_count != routing.via_locations.len() || via_count == 0 {
            Err(format!(
                "DEF vias={} explicit={}",
                via_count,
                routing.via_locations.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_apply_def_locations_to_spice", || {
        let def_text = r#"
DESIGN spatial_layout ;
ROW ROW_0 core 0 0 N DO 20 BY 1 STEP 100 0 ;
COMPONENTS 2 ;
- R1 RES + PLACED ( 100 0 ) N ;
- R2 RES + PLACED ( 500 0 ) N ;
END COMPONENTS
"#;
        let def = eda_api::parse_physical_def(def_text)?;
        let spice = r#"
spatial layout
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=91 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr=0.7 spatialscale=0.04
.end
"#;
        let mut circuit = eda_api::parse_spice(spice)?;
        let matched = eda_api::apply_physical_def_locations(&mut circuit, &def, 100.0)?;
        let r1 = circuit.locations.get("R1").copied();
        let r2 = circuit.locations.get("R2").copied();
        if matched != 2
            || r1 != Some(eda_api::ElementLocation {
                x_m: 1e-6,
                y_m: 0.0,
            })
            || r2 != Some(eda_api::ElementLocation {
                x_m: 5e-6,
                y_m: 0.0,
            })
        {
            Err(format!("matched={matched}, R1={r1:?}, R2={r2:?}"))
        } else {
            let mc = eda_api::monte_carlo(&circuit, &circuit.analyses[0])?;
            if mc.samples.len() != 256
                || mc.samples[0].randomized_objects != 2
                || !mc.statistics.standard_deviation.is_finite()
            {
                Err(format!("{mc:?}"))
            } else {
                Ok(())
            }
        }
    });
    c.add("api_power_analysis", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let report = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        if report.total_power_uw <= 0.0 || report.vectors != 2 {
            Err(format!(
                "total={}, vectors={}",
                report.total_power_uw, report.vectors
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_physical_verification", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::physical_verification(&flow.netlist, &flow.placement, &flow.routing)?;
        if result.has_errors() {
            Err(eda_pnr::drc_report(&result))
        } else {
            Ok(())
        }
    });
    c.add("api_lef_def_physical_export", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if !flow.lef.contains("MACRO XOR2") || !flow.def.contains("COMPONENTS") {
            Err("API flow physical export incomplete".into())
        } else if eda_pnr::parse_def(&flow.def).is_err() {
            Err("API DEF does not re-parse".into())
        } else {
            Ok(())
        }
    });
    c.add("api_import_physical_def_text_round_trip", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let placement_blockage = eda_api::PlacementBlockage::new("text_keepout", 1, 2, 4)?;
        let placement = eda_api::place_with_blockages(&flow.netlist, &[placement_blockage])?;
        let baseline = eda_api::route_maze(&flow.netlist, &placement)?;
        let segment = baseline
            .segments
            .first()
            .ok_or("baseline routing has no segments")?;
        let route_options = eda_api::RouteOptions::new(5)?.with_blocked_channel(
            "text_route_keepout",
            segment.orientation,
            segment.channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &placement, &route_options)?;
        let (_, def_text) = eda_api::export_physical(&flow.netlist, &placement, &routing)?;
        let (imported_placement, imported_routing) =
            eda_api::import_physical_def_text(&flow.netlist, &def_text)?;
        let drc =
            eda_api::physical_verification(&flow.netlist, &imported_placement, &imported_routing)?;
        if imported_placement == placement
            && imported_routing == routing
            && !drc.has_errors()
            && def_text.contains("TRACKS 5 ;")
        {
            Ok(())
        } else {
            Err(format!(
                "placement={:?}\nrouting={:?}\n{}",
                imported_placement,
                imported_routing,
                eda_pnr::drc_report(&drc)
            ))
        }
    });
    c.add("api_def_blockage_export_round_trip", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let placement_blockage = eda_api::PlacementBlockage::new("macro", 1, 2, 5)?;
        let placement = eda_api::place_with_blockages(
            &flow.netlist,
            std::slice::from_ref(&placement_blockage),
        )?;
        let baseline = eda_api::route_maze(&flow.netlist, &placement)?;
        let segment = baseline
            .segments
            .first()
            .ok_or("baseline routing is empty")?;
        let route_options = eda_api::RouteOptions::new(4)?.with_blocked_channel(
            "route_keepout",
            segment.orientation,
            segment.channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &placement, &route_options)?;
        let (_, def) = eda_api::export_physical(&flow.netlist, &placement, &routing)?;
        let parsed = eda_api::parse_physical_def(&def)?;
        if parsed.placement_blockages.len() != 1
            || parsed.placement_blockages[0].name != placement_blockage.name
            || parsed.placement_blockages[0].row != placement_blockage.row
            || parsed.placement_blockages[0].start_site != placement_blockage.start_site
            || parsed.placement_blockages[0].end_site != placement_blockage.end_site
            || parsed.route_blockages.len() != 1
            || parsed.route_blockages[0].name != "route_keepout"
            || parsed.route_blockages[0].orientation != segment.orientation
            || parsed.route_blockages[0].channel != segment.channel
        {
            Err(format!(
                "placement={:?} route={:?}",
                parsed.placement_blockages, parsed.route_blockages
            ))
        } else {
            let (imported_placement, imported_routing) =
                eda_api::import_physical_def(&flow.netlist, &parsed)?;
            if imported_placement == placement
                && imported_routing == routing
                && eda_pnr::check_routing(&imported_routing).is_ok()
            {
                Ok(())
            } else {
                Err(format!(
                    "imported placement={:?} routing={:?}",
                    imported_placement, imported_routing
                ))
            }
        }
    });
    c.add("api_repair_antenna_violations", || {
        use eda_core::library::CellLibrary;
        use eda_core::netlist::{Cell, CellKind, Dir};
        use std::sync::Arc;

        let library = Arc::new(CellLibrary::std());
        let mut netlist = eda_core::netlist::Netlist::new("antenna");
        netlist.set_library(library.clone());
        let input = netlist.add_net("a");
        let routed = netlist.add_net("long");
        let output = netlist.add_net("z");
        let buffer = library.find("BUF").unwrap();
        netlist.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![input],
            output: routed,
        });
        netlist.add_cell(Cell {
            name: "sink".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![routed],
            output,
        });
        netlist.add_port("a", Dir::In, vec![input]);
        netlist.add_port("z", Dir::Out, vec![output]);
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 30 },
            ],
            cell_width: vec![2, 2],
            blockages: Vec::new(),
        };
        let routing = eda_pnr::Routing {
            segments: vec![
                eda_pnr::Segment {
                    net: routed,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 1,
                    end: 31,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: routed,
                    orientation: eda_pnr::Orientation::Vertical,
                    channel: 31,
                    start: 0,
                    end: 3,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: output,
                    orientation: eda_pnr::Orientation::Vertical,
                    channel: 31,
                    start: 3,
                    end: 4,
                    track: 1,
                },
            ],
            vias: 1,
            via_locations: vec![eda_pnr::Via {
                net: routed,
                x: 31,
                y: 0,
            }],
            tracks_per_channel: 2,
            congested_channels: Vec::new(),
            blocked_channels: Vec::new(),
        };
        let before = eda_api::physical_verification(&netlist, &placement, &routing)?;
        let result = eda_api::repair_antennas(
            &netlist,
            &placement,
            &routing,
            &eda_api::AntennaRepairOptions::default(),
        )?;
        let report = eda_api::antenna_repair_report(&result);
        if before.has_errors()
            || !before.has_warnings()
            || !result.all_antennas_repaired
            || result.diodes_inserted != 1
            || result.repairs.len() != 1
            || result.repairs[0].old_ratio <= 16.0
            || result.repairs[0].new_ratio > 16.0
            || result.netlist.cell_count() != netlist.cell_count() + 1
            || result.physical_placement.cell_pos.len() != netlist.cell_count() + 1
            || eda_pnr::check_routing(&result.routing).is_err()
            || !report.contains("Antenna repair: 1 diode(s), 1 net(s), status PASS")
        {
            Err(format!(
                "before={:?}\n{}",
                before.violations,
                eda_pnr::drc_report(&before)
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_error_unknown_module", || {
        if eda_api::compile("module m; ghost u(); endmodule", None).is_ok() {
            Err("should fail".into())
        } else {
            Ok(())
        }
    });
}
