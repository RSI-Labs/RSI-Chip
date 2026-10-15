//! Routing, physical timing, multi-corner physical flows, parasitics, and timing-driven placement.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    c.add("api_verilog_export", || {
        let src = "module m(input a, b, output y); assign y = a ^ b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        if !flow.verilog.contains("module") {
            Err("no verilog export".into())
        } else {
            Ok(())
        }
    });
    c.add("api_place_and_route_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        eda_pnr::check_legal(&flow.netlist, &flow.placement)?;
        eda_pnr::check_routing(&flow.routing)?;
        if flow.routing.segments.is_empty() {
            Err("API flow did not route any segments".into())
        } else {
            Ok(())
        }
    });
    c.add("api_hard_placement_blockage", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let blockage =
            eda_api::PlacementBlockage::new("macro_keepout", 0, 0, eda_api::SITES_PER_ROW - 1)?;
        let placement =
            eda_api::place_with_blockages(&flow.netlist, std::slice::from_ref(&blockage))?;
        let routing = eda_api::route_maze(&flow.netlist, &placement)?;
        let drc = eda_api::physical_verification(&flow.netlist, &placement, &routing)?;
        let optimized = eda_api::optimize_timing_driven_placement(
            &flow.netlist,
            placement.clone(),
            &eda_api::TimingDrivenPlacementOptions {
                max_passes: 1,
                ..Default::default()
            },
        )?;
        if placement.blockages != vec![blockage]
            || placement.cell_pos.iter().any(|position| position.row == 0)
            || drc.has_errors()
            || optimized.placement.blockages != placement.blockages
            || eda_pnr::check_legal(&flow.netlist, &optimized.placement).is_err()
            || !eda_pnr::report(&flow.netlist, &placement).contains("hard placement blockages: 1")
        {
            Err(format!(
                "placement={:?} optimized={:?} drc={:?}",
                placement, optimized.placement, drc.violations
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_route_with_capacity_options", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let options = eda_api::RouteOptions::new(4)?;
        let routing = eda_api::route_with_options(&flow.netlist, &flow.placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 4 || routing.segments.is_empty() {
            Err(format!(
                "tracks={} segments={}",
                routing.tracks_per_channel,
                routing.segments.len()
            ))
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let routing = eda_api::route_maze(&flow.netlist, &flow.placement)?;
        eda_pnr::check_routing(&routing)?;
        if routing.segments.is_empty() {
            Err("maze router produced no segments".into())
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing_capacity_options", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let options = eda_api::RouteOptions::new(2)?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &flow.placement, &options)?;
        eda_pnr::check_routing(&routing)?;
        if routing.tracks_per_channel != 2 {
            Err(format!("tracks={}", routing.tracks_per_channel))
        } else {
            Ok(())
        }
    });
    c.add("api_maze_routing_hard_channel_blockage", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let baseline = eda_api::route_maze(&flow.netlist, &flow.placement)?;
        let Some(blocked_segment) = baseline.segments.first() else {
            return Err("baseline maze route is empty".into());
        };
        let orientation = blocked_segment.orientation;
        let channel = blocked_segment.channel;
        let options = eda_api::RouteOptions::new(4)?.with_blocked_channel(
            "api_keepout",
            orientation,
            channel,
        )?;
        let routing = eda_api::route_maze_with_options(&flow.netlist, &flow.placement, &options)?;
        let drc = eda_api::physical_verification(&flow.netlist, &flow.placement, &routing)?;
        if routing.segments.is_empty()
            || routing.blocked_channels.len() != 1
            || routing
                .segments
                .iter()
                .any(|segment| segment.orientation == orientation && segment.channel == channel)
            || drc.has_errors()
        {
            Err(format!(
                "blocked={:?} segments={:?} drc={:?}",
                routing.blocked_channels, routing.segments, drc.violations
            ))
        } else if eda_api::RouteChannelBlockage::new("", orientation, channel).is_ok() {
            Err("invalid empty routing blockage accepted".into())
        } else {
            let ripup = eda_api::route_maze_ripgup(&flow.netlist, &flow.placement, &options, 4)?;
            if ripup
                .routing
                .segments
                .iter()
                .any(|segment| segment.orientation == orientation && segment.channel == channel)
            {
                Err("ripgup router crossed hard blockage".into())
            } else if eda_pnr::check_routing(&ripup.routing).is_err() {
                Err("ripgup blockage route is illegal".into())
            } else {
                Ok(())
            }
        }
    });
    c.add("api_negotiated_ripgup_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::route_maze_ripgup(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            8,
        )?;
        eda_pnr::check_routing(&result.routing)?;
        let drc = eda_pnr::drc(&flow.netlist, &flow.placement, &result.routing)?;
        if drc.has_errors() || result.passes == 0 {
            Err(format!("passes={} drc={:?}", result.passes, drc.violations))
        } else {
            Ok(())
        }
    });
    c.add("api_timing_driven_ripgup_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let critical_net = (0..flow.netlist.net_count())
            .find(|net| {
                let net = flow.netlist.net(*net);
                !net.drivers.is_empty() || !net.sinks.is_empty()
            })
            .ok_or_else(|| "netlist has no internal routed net".to_string())?;
        let timing = eda_api::TimingDrivenRoutingOptions {
            net_weights: std::collections::HashMap::from([(critical_net, 10.0)]),
            default_net_weight: 1.0,
        };
        let result = eda_api::route_maze_timing_ripgup(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            &timing,
            8,
        )?;
        eda_pnr::check_routing(&result.ripup.routing)?;
        if result.final_weighted_wirelength <= result.initial_weighted_wirelength
            && eda_api::timing_driven_ripgup_report(&result).contains("weighted wirelength")
        {
            Ok(())
        } else {
            Err(eda_api::timing_driven_ripgup_report(&result))
        }
    });
    c.add("api_timing_driven_ripgup_from_sta", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::route_maze_timing_ripgup_from_timing(
            &flow.netlist,
            &flow.placement,
            &eda_api::RouteOptions::default(),
            &flow.timing,
            8,
        )?;
        eda_pnr::check_routing(&result.ripup.routing)?;
        if result.ripup.passes > 0 {
            Ok(())
        } else {
            Err(eda_api::timing_driven_ripgup_report(&result))
        }
    });
    c.add("api_physical_timing_iteration_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 2,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        if result.baseline.sdf.interconnects.is_empty()
            || result.optimized.len() != 2
            || result.best().drc.has_errors()
            || !result
                .best()
                .parasitic_report
                .unmatched_netlist_nets
                .is_empty()
            || !report.contains("Physical timing flow")
            || !report.contains("iteration baseline")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_physical_timing_flow_zero_iterations", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 0,
                ..Default::default()
            },
        )?;
        if result.optimized.is_empty()
            && result.best().index == usize::MAX
            && !result.best().sdf.interconnects.is_empty()
        {
            Ok(())
        } else {
            Err(eda_api::physical_timing_flow_report(&result))
        }
    });
    c.add("api_physical_timing_flow_rejects_invalid_options", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let bad = eda_api::PhysicalTimingFlowOptions {
            placement_passes: 0,
            ..Default::default()
        };
        if eda_api::run_physical_timing_flow(&flow.netlist, &bad).is_ok() {
            Err("invalid physical timing options accepted".into())
        } else {
            Ok(())
        }
    });
    c.add(
        "api_constrained_timing_weights_prioritize_critical_path",
        || {
            let src = "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
        "#,
            )?;
            let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
            let weights =
                eda_api::timing_weights_from_constrained_timing(&flow.netlist, &constrained, 9.0)?;
            let d = flow.netlist.net_by_name("d").ok_or("missing d")?;
            let q1 = flow.netlist.net_by_name("q1").ok_or("missing q1")?;
            let q2 = flow.netlist.net_by_name("q2").ok_or("missing q2")?;
            if !weights.is_empty()
                && weights[&q1] == 10.0
                && weights[&d] > weights[&q2]
                && weights[&d] < weights[&q1]
            {
                Ok(())
            } else {
                Err(format!(
                    "d={} q1={} q2={:?}",
                    weights[&d],
                    weights[&q1],
                    weights.get(&q2)
                ))
            }
        },
    );
    c.add(
        "api_waiver_aware_timing_weights_exclude_waived_path",
        || {
            let src = "module m(input a, b, output y, z);
            assign y = ~a;
            assign z = ~b;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.0 -clock clk [get_ports b]
            set_output_delay 0.1 -clock clk [get_ports y]
            set_output_delay 0.1 -clock clk [get_ports z]
        "#,
            )?;
            let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
            let mut store =
                eda_api::timing_violation_store(&constrained.clock, &flow.netlist, &constrained)?;
            let waiver =
                eda_api::timing_violation_waiver("accepted-y", &constrained.clock, "y", -10.0)?
                    .with_kind(eda_api::TimingViolationKind::Output);
            eda_api::add_timing_violation_waiver(&mut store, waiver)?;
            let raw =
                eda_api::timing_weights_from_constrained_timing(&flow.netlist, &constrained, 9.0)?;
            let waiver_aware = eda_api::timing_weights_from_waiver_aware_timing(
                &flow.netlist,
                &constrained,
                &store,
                9.0,
            )?;
            let y = flow.netlist.net_by_name("y").ok_or("missing y")?;
            let a = flow.netlist.net_by_name("a").ok_or("missing a")?;
            let z = flow.netlist.net_by_name("z").ok_or("missing z")?;
            let b = flow.netlist.net_by_name("b").ok_or("missing b")?;
            if raw.contains_key(&y)
                && raw.contains_key(&a)
                && !waiver_aware.contains_key(&y)
                && !waiver_aware.contains_key(&a)
                && waiver_aware.contains_key(&z)
                && waiver_aware.contains_key(&b)
                && store.violations().len() == 2
                && store.len() == 1
            {
                Ok(())
            } else {
                Err(format!(
                    "raw={:?} waiver-aware={:?} raw-count={} active-count={}",
                    raw,
                    waiver_aware,
                    store.violations().len(),
                    store.len()
                ))
            }
        },
    );

    c.add("api_waiver_aware_placement_and_routing", || {
        let src = "module m(input a, b, output y, z);
            assign y = ~a;
            assign z = ~b;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
            set_output_delay 0.1 -clock clk [get_ports z]
        "#,
        )?;
        let constrained = eda_api::constrained_timing(&flow.netlist, &constraints)?;
        let mut store =
            eda_api::timing_violation_store(&constrained.clock, &flow.netlist, &constrained)?;
        let waiver = eda_api::timing_violation_waiver("all", &constrained.clock, "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        eda_api::add_timing_violation_waiver(&mut store, waiver)?;
        let placement = eda_api::timing_driven_place_from_waiver_aware_timing(
            &flow.netlist,
            &constrained,
            &store,
            1,
        )?;
        let routing = eda_api::route_maze_timing_ripgup_from_waiver_aware_timing(
            &flow.netlist,
            &placement.placement,
            &eda_api::RouteOptions::default(),
            &constrained,
            &store,
            1,
        )?;
        if placement.placement.cell_pos.len() == flow.netlist.cell_count()
            && routing.ripup.routing.wirelength() > 0
        {
            Ok(())
        } else {
            Err(format!(
                "cells={} wirelength={}",
                placement.placement.cell_pos.len(),
                routing.ripup.routing.wirelength()
            ))
        }
    });

    c.add("api_slack_driven_physical_timing_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 3.0 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        if result.baseline.constrained_timing.is_some()
            && result.best().constrained_timing.is_some()
            && result
                .best()
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack.is_finite())
                .unwrap_or(false)
            && report.contains("constrained slack:")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_physical_timing_flow_accepts_waivers", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let waiver = eda_api::timing_violation_waiver("physical", "clk", "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                waivers: vec![waiver],
                iterations: 1,
                ..Default::default()
            },
        )?;
        let bad = eda_api::PhysicalTimingFlowOptions {
            waivers: vec![eda_api::timing_violation_waiver(
                "orphan", "clk", "y", -1.0,
            )?],
            ..Default::default()
        };
        if result.optimized.len() == 1
            && result.best().constrained_timing.is_some()
            && matches!(
                eda_api::run_physical_timing_flow(&flow.netlist, &bad),
                Err(error) if error.contains("requires SDC constraints")
            )
        {
            Ok(())
        } else {
            Err(eda_api::physical_timing_flow_report(&result))
        }
    });

    c.add("api_physical_timing_flow_repairs_antennas", || {
        use eda_core::library::CellLibrary;
        use eda_core::netlist::{Cell, CellKind, Dir};
        use std::sync::Arc;

        let library = Arc::new(CellLibrary::std());
        let mut netlist = eda_core::netlist::Netlist::new("flow-antenna");
        netlist.set_library(library.clone());
        let input = netlist.add_net("a");
        let long = netlist.add_net("long");
        let output = netlist.add_net("z");
        let buffer = library.find("BUF").unwrap();
        netlist.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![input],
            output: long,
        });
        netlist.add_cell(Cell {
            name: "sink".into(),
            kind: CellKind::Lib(buffer),
            inputs: vec![long],
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
        let result = eda_api::run_physical_timing_flow(
            &netlist,
            &eda_api::PhysicalTimingFlowOptions {
                initial_placement: Some(placement),
                iterations: 0,
                repair_antennas: true,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let diodes = result
            .baseline
            .antenna_repairs
            .iter()
            .map(|repair| repair.diodes_inserted)
            .sum::<usize>();
        if result.baseline.antenna_repairs.len() != 1
            || diodes != 1
            || result.baseline.antenna_warnings != 0
            || result.baseline.annotated_netlist.cell_count() != netlist.cell_count() + 1
            || result.baseline.placement.cell_pos.len() != netlist.cell_count() + 1
            || result.baseline.drc.has_errors()
            || eda_pnr::check_routing(&result.baseline.routing).is_err()
            || !report.contains("antenna diodes 1, antenna warnings 0")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_flow_repairs_hold", || {
        let src = "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
        )?;
        let placement = eda_pnr::Placement {
            rows: 4,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 3, site: 26 },
            ],
            cell_width: flow.placement.cell_width.clone(),
            blockages: Vec::new(),
        };
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                initial_placement: Some(placement),
                hold_repair: Some(eda_api::HoldRepairOptions::default()),
                iterations: 0,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let hold = result
            .hold_repair
            .as_ref()
            .ok_or("physical flow omitted hold repair")?;
        if hold.buffers_inserted == 0
            || !hold.all_active_violations_repaired
            || result
                .baseline
                .constrained_timing
                .as_ref()
                .map(|timing| timing.hold_slack < 0.0)
                .unwrap_or(true)
            || result.baseline.annotated_netlist.cell_count() <= flow.netlist.cell_count()
            || result.baseline.placement.cell_pos.len()
                != result.baseline.annotated_netlist.cell_count()
            || result.baseline.drc.has_errors()
            || eda_pnr::check_routing(&result.baseline.routing).is_err()
            || !report.contains("Physical timing flow:")
            || !report.contains("Hold repair:")
            || !report.contains("status PASS")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add(
        "api_physical_timing_flow_repairs_hold_each_iteration",
        || {
            let src = "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
         endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.0 -clock clk [get_ports d]
            set_clock_uncertainty 1.2 -hold [get_clocks clk]
        "#,
            )?;
            let result = eda_api::run_physical_timing_flow(
                &flow.netlist,
                &eda_api::PhysicalTimingFlowOptions {
                    constraints: Some(constraints),
                    hold_repair: Some(eda_api::HoldRepairOptions {
                        max_buffers_per_endpoint: 1,
                        ..Default::default()
                    }),
                    iterations: 1,
                    ..Default::default()
                },
            )?;
            let report = eda_api::physical_timing_flow_report(&result);
            let baseline_buffers = result
                .baseline
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>();
            let optimized_buffers = result.optimized[0]
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>();
            if baseline_buffers != 1
                || result.baseline.all_active_hold_repaired
                || optimized_buffers != 1
                || !result.optimized[0].all_active_hold_repaired
                || result.baseline.annotated_netlist.cell_count() != flow.netlist.cell_count() + 1
                || result.optimized[0].annotated_netlist.cell_count()
                    != flow.netlist.cell_count() + 2
                || !report.contains("hold status LIMITED")
                || !report.contains("hold status PASS")
            {
                Err(report)
            } else {
                Ok(())
            }
        },
    );
    c.add("api_physical_timing_flow_with_timing_driven_cts", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::physical_timing_flow_report(&result);
        let clock_tree = result
            .clock_tree
            .as_ref()
            .ok_or("physical flow omitted CTS result")?;
        let inserted = clock_tree.cts.buffers_inserted;
        if inserted == 0
            || result.baseline.annotated_netlist.cell_count()
                != flow.netlist.cell_count() + inserted
            || result.baseline.placement.cell_pos.len() != flow.netlist.cell_count() + inserted
            || result.baseline.drc.has_errors()
            || result.best().drc.has_errors()
            || !clock_tree.target_met
            || !report.contains("Timing-driven CTS search:")
            || !report.contains("target MET")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_flow_exports_best_verilog", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 8.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                constraints: Some(constraints),
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let best = result.best();
        let text = eda_api::export_verilog(&best.annotated_netlist)?;
        let inserted = result
            .clock_tree
            .as_ref()
            .map(|tree| tree.cts.buffers_inserted)
            .unwrap_or(0);
        let missing_instance = best
            .annotated_netlist
            .cells()
            .iter()
            .map(|cell| cell.name.as_str())
            .filter(|name| !text.contains(name))
            .collect::<Vec<_>>();
        let exported_flow = eda_api::run_flow(&text, Some("shift"))?;
        if inserted == 0
            || best.annotated_netlist.cell_count() != flow.netlist.cell_count() + inserted
            || !missing_instance.is_empty()
            || !exported_flow.equivalent
        {
            Err(format!(
                "inserted={inserted}, cells={}, missing={missing_instance:?}, equivalent={}\n{text}",
                best.annotated_netlist.cell_count(),
                exported_flow.equivalent
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_post_route_power_ir", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let best = result.best();
        let vectors = eda_api::default_power_vectors(&best.annotated_netlist);
        let pre_route =
            eda_api::quick_power(&best.annotated_netlist, &eda_api::PowerOptions::default())?;
        let post_route = eda_api::analyze_physical_timing_power_ir(
            best,
            &vectors,
            &eda_api::PostRoutePowerIrOptions::default(),
        )?;
        let effective_cap = post_route.effective_capacitance_pf.values().sum::<f64>();
        if effective_cap <= 0.0
            || post_route.parasitic_dynamic_power_uw <= 0.0
            || post_route.power.dynamic_power_uw <= pre_route.dynamic_power_uw
            || post_route.ir.cells.len() != best.annotated_netlist.cell_count()
            || (post_route.ir.total_power_uw - post_route.power.total_power_uw).abs() > 1e-12
        {
            Err(format!(
                "cap={effective_cap}, parasitic={}, pre={}, post={}, ir cells={}, ir power={}\n{}",
                post_route.parasitic_dynamic_power_uw,
                pre_route.dynamic_power_uw,
                post_route.power.dynamic_power_uw,
                best.annotated_netlist.cell_count(),
                post_route.ir.total_power_uw,
                eda_api::post_route_power_ir_report(&post_route)
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_physical_timing_post_route_power_from_waveform", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = (a ^ b) & (a | b); endmodule",
            None,
        )?;
        let result = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let waveform = eda_api::parse_vcd(
            r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
#10
1!
0"
1#
"#,
        )?;
        let post_route = eda_api::analyze_physical_timing_power_ir_from_waveform(
            result.best(),
            &waveform,
            &eda_api::PostRoutePowerIrOptions::default(),
        )?;
        let a = post_route
            .power
            .activity_by_net
            .get("a")
            .ok_or("post-route waveform omitted input 'a'")?;
        if a.transitions != 1
            || post_route.parasitic_dynamic_power_uw <= 0.0
            || !post_route
                .effective_capacitance_pf
                .values()
                .all(|value| value.is_finite() && *value >= 0.0)
        {
            Err(eda_api::post_route_power_ir_report(&post_route))
        } else {
            Ok(())
        }
    });

    c.add("api_mmsc_post_route_power_properties", || {
        let source = r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.06
    set_scenario_property post_route.max_total_drop_v 0.04
    set_scenario_property post_route.domain.A.voltage_v 0.65
    set_scenario_property post_route.domain.A.vdd_ohm_per_site 0.08
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_scenario_property post_route.frequency_mhz 250.0
    set_scenario_property post_route.voltage_scaled_capacitance false
    set_scenario_property post_route.capacitive_energy_per_pf_pj 0.81
endscenario
"#;
        let definitions = eda_api::parse_timing_scenarios(source)?;
        let scenarios = eda_api::post_route_power_scenarios_from_timing_scenarios(&definitions)?;
        if scenarios.len() != 2
            || scenarios[0].name != "slow"
            || (scenarios[0].options.ir.voltage_v - 0.9).abs() > 1e-12
            || (scenarios[0].options.ir.vdd_ohm_per_site - 0.05).abs() > 1e-12
            || (scenarios[0].options.ir.gnd_ohm_per_site - 0.06).abs() > 1e-12
            || (scenarios[0].options.ir.max_total_drop_v - 0.04).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].voltage_v - 0.65).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].vdd_ohm_per_site - 0.08).abs() > 1e-12
            || (scenarios[0].options.upf.domains["A"].gnd_ohm_per_site - 0.06).abs() > 1e-12
            || !scenarios[1].options.upf.domains.is_empty()
            || scenarios[1].name != "fast"
            || (scenarios[1].options.power.frequency_mhz - 250.0).abs() > 1e-12
            || scenarios[1].options.voltage_scaled_capacitance
            || (scenarios[1].options.power.capacitive_energy_per_pf_pj - 0.81).abs() > 1e-12
        {
            return Err(format!("{scenarios:?}"));
        }

        let bad = eda_api::parse_timing_scenarios(
            "scenario bad\ncreate_clock -name clk -period 1 [get_ports a]\nset_scenario_property post_route.unknown 1\nendscenario",
        )?;
        if eda_api::post_route_power_scenarios_from_timing_scenarios(&bad).is_ok() {
            Err("unknown post-route property was accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("api_multi_corner_post_route_power_ir", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let physical_scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios: physical_scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let power_scenarios = vec![
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "slow",
                eda_api::PostRoutePowerIrOptions {
                    ir: eda_api::IrOptions {
                        voltage_v: 0.9,
                        vdd_ohm_per_site: 0.05,
                        gnd_ohm_per_site: 0.05,
                        ..eda_api::IrOptions::default()
                    },
                    ..eda_api::PostRoutePowerIrOptions::default()
                },
            )?,
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "fast",
                eda_api::PostRoutePowerIrOptions::default(),
            )?,
        ];
        let Some(reference) = result.best().corners.first() else {
            return Err("MMMC physical flow omitted power reference".into());
        };
        let vectors = eda_api::default_power_vectors(&reference.evaluation.annotated_netlist);
        let power = eda_api::analyze_multi_corner_physical_timing_power_ir(
            result.best(),
            &vectors,
            &power_scenarios,
        )?;
        let report = eda_api::multi_corner_post_route_power_ir_report(&power);
        let slow = &power.corners[0];
        let fast = &power.corners[1];
        let mismatch_scenarios = vec![power_scenarios[1].clone(), power_scenarios[0].clone()];
        let mismatch = eda_api::analyze_multi_corner_physical_timing_power_ir(
            result.best(),
            &vectors,
            &mismatch_scenarios,
        );
        if power.corners.len() != 2
            || slow.scenario != "slow"
            || fast.scenario != "fast"
            || power.worst_drop_corner != 0
            || slow.result.ir.worst_drop_v <= fast.result.ir.worst_drop_v
            || !report.contains("worst IR drop 'slow'")
            || mismatch.is_ok()
        {
            Err(format!(
                "worst_drop={:?}, slow={}, fast={}, mismatch={:?}\n{report}",
                power.worst_drop_corner,
                slow.result.ir.worst_drop_v,
                fast.result.ir.worst_drop_v,
                mismatch.err()
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_multi_corner_physical_timing_flow", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let slow = &result.baseline.corners[0];
        let fast = &result.baseline.corners[1];
        if result.baseline.corners.len() == 2
            && result.baseline.placement.rows == slow.evaluation.placement.rows
            && result.baseline.placement.cell_pos == slow.evaluation.placement.cell_pos
            && result.baseline.placement.cell_width == slow.evaluation.placement.cell_width
            && result.baseline.placement.rows == fast.evaluation.placement.rows
            && result.baseline.placement.cell_pos == fast.evaluation.placement.cell_pos
            && result.baseline.placement.cell_width == fast.evaluation.placement.cell_width
            && result.baseline.routing.wirelength() == slow.evaluation.routing.wirelength()
            && result.baseline.routing.wirelength() == fast.evaluation.routing.wirelength()
            && slow.evaluation.timing.critical_path_delay
                > fast.evaluation.timing.critical_path_delay
            && result.baseline.corners[result.baseline.worst_slack_corner].scenario == "slow"
            && result.best().index != usize::MAX
            && report.contains("Multi-corner physical timing flow")
            && report.contains("scenario 'slow' *")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_multi_corner_physical_timing_flow_with_cts", || {
        let src = "module shift(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 10.0 [get_ports clk]
            set_input_delay 0.5 -clock clk [get_ports d]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                Vec::new(),
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                timing_driven_cts: Some(eda_api::TimingDrivenCtsOptions {
                    target_skew_ns: 0.1,
                    ..Default::default()
                }),
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let clock_tree = result
            .clock_tree
            .as_ref()
            .ok_or("MMMC physical flow omitted CTS result")?;
        let inserted = clock_tree.cts.buffers_inserted;
        let slow = &result.baseline.corners[0];
        let fast = &result.baseline.corners[1];
        if inserted == 0
            || slow.evaluation.annotated_netlist.cell_count()
                != flow.netlist.cell_count() + inserted
            || fast.evaluation.annotated_netlist.cell_count()
                != slow.evaluation.annotated_netlist.cell_count()
            || slow.evaluation.drc.has_errors()
            || fast.evaluation.drc.has_errors()
            || !clock_tree.target_met
            || !report.contains("Multi-corner physical timing flow")
            || !report.contains("Timing-driven CTS search:")
            || !report.contains("target MET")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add(
        "api_multi_corner_physical_timing_flow_repairs_antennas",
        || {
            use eda_core::library::CellLibrary;
            use eda_core::netlist::{Cell, CellKind, Dir};
            use std::sync::Arc;

            let library = Arc::new(CellLibrary::std());
            let mut netlist = eda_core::netlist::Netlist::new("mmmc-antenna");
            netlist.set_library(library.clone());
            let input = netlist.add_net("a");
            let long = netlist.add_net("long");
            let output = netlist.add_net("z");
            let buffer = library.find("BUF").unwrap();
            netlist.add_cell(Cell {
                name: "driver".into(),
                kind: CellKind::Lib(buffer),
                inputs: vec![input],
                output: long,
            });
            netlist.add_cell(Cell {
                name: "sink".into(),
                kind: CellKind::Lib(buffer),
                inputs: vec![long],
                output,
            });
            netlist.add_port("a", Dir::In, vec![input]);
            netlist.add_port("z", Dir::Out, vec![output]);
            let slow_view = scaled_timing_view(&netlist, 2.0)?;
            let fast_view = scaled_timing_view(&netlist, 0.5)?;
            let slow_constraints = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 8.0 [get_ports a]",
            )?;
            let fast_constraints = slow_constraints.clone();
            let scenarios = vec![
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "slow",
                    &slow_view,
                    &slow_constraints,
                    Vec::new(),
                ),
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "fast",
                    &fast_view,
                    &fast_constraints,
                    Vec::new(),
                ),
            ];
            let placement = eda_pnr::Placement {
                rows: 4,
                cell_pos: vec![
                    eda_pnr::Pos { row: 0, site: 0 },
                    eda_pnr::Pos { row: 3, site: 30 },
                ],
                cell_width: vec![2, 2],
                blockages: Vec::new(),
            };
            let result = eda_api::run_multi_corner_physical_timing_flow(
                &eda_api::MultiCornerPhysicalTimingFlowOptions {
                    scenarios,
                    initial_placement: Some(placement),
                    iterations: 0,
                    repair_antennas: true,
                    ..Default::default()
                },
            )?;
            let report = eda_api::multi_corner_physical_timing_flow_report(&result);
            let diodes = result
                .baseline
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>();
            let slow = &result.baseline.corners[0];
            let fast = &result.baseline.corners[1];
            if diodes != 1
                || result.baseline.antenna_warnings != 0
                || slow.evaluation.annotated_netlist.cell_count() != netlist.cell_count() + 1
                || fast.evaluation.annotated_netlist.cell_count() != netlist.cell_count() + 1
                || result.baseline.placement.cell_pos.len() != netlist.cell_count() + 1
                || slow.evaluation.drc.has_errors()
                || fast.evaluation.drc.has_errors()
                || !report.contains("antenna diodes 1, antenna warnings 0")
            {
                Err(report)
            } else {
                Ok(())
            }
        },
    );

    c.add("api_multi_corner_physical_timing_waiver_objective", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
        )?;
        let slow_waiver = eda_api::timing_violation_waiver("slow-only", "slow", "y", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                vec![slow_waiver],
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        if result.baseline.corners[result.baseline.worst_slack_corner].scenario == "slow"
            && result.baseline.corners[0]
                .violations
                .waived_violations()
                .len()
                == 1
            && result.baseline.corners[1]
                .violations
                .waived_violations()
                .is_empty()
            && result.optimized[0].optimization_corner == Some(1)
            && report.contains("scenario 'fast':")
            && report.contains("optimization objective")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_multicorner_physical_flow_violation_registry", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let slow_view = scaled_timing_view(&flow.netlist, 2.0)?;
        let fast_view = scaled_timing_view(&flow.netlist, 0.5)?;
        let slow_constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 0.1 [get_ports a]
            set_output_delay 0.1 -clock clk [get_ports y]
        "#,
        )?;
        let fast_constraints = slow_constraints.clone();
        let slow_waiver = eda_api::timing_violation_waiver("slow-only", "slow", "*", -10.0)?
            .with_kind(eda_api::TimingViolationKind::Output);
        let scenarios = vec![
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "slow",
                &slow_view,
                &slow_constraints,
                vec![slow_waiver],
            ),
            eda_api::MultiCornerPhysicalTimingScenario::new(
                "fast",
                &fast_view,
                &fast_constraints,
                Vec::new(),
            ),
        ];
        let result = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let registry = &result.baseline.registry;
        let report = eda_api::multi_corner_physical_timing_flow_report(&result);
        let slow_store = registry
            .stores()
            .get("slow")
            .ok_or("registry omitted slow store")?;
        let fast_store = registry
            .stores()
            .get("fast")
            .ok_or("registry omitted fast store")?;
        if !slow_store.is_empty()
            || slow_store.waived_violations().is_empty()
            || fast_store.is_empty()
            || !fast_store.waived_violations().is_empty()
            || registry.summaries().get("output:y").map(|summary| {
                summary.worst_scenario.as_str() != "fast" || summary.scenarios.len() != 1
            }) != Some(false)
            || !registry.waivers().is_empty()
            || !report.contains("active violations ")
            || !report.contains("cross-corner checks ")
            || result.optimized.len() != 1
            || result.optimized[0].registry_update.is_none()
            || result.optimized[0].registry.generation()
                != result.baseline.registry.generation() + 1
            || !report.contains("registry lifecycle:")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add(
        "api_multi_corner_physical_timing_rejects_width_change",
        || {
            let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
            let flow = eda_api::run_flow(src, None)?;
            let mut other = flow.netlist.clone();
            let library = other
                .library()
                .cloned()
                .ok_or_else(|| "physical timing view requires a library".to_string())?;
            let mut changed = (*library).clone();
            for cell in &mut changed.cells {
                cell.width_sites += 1;
            }
            other.set_library(std::sync::Arc::new(changed));
            let constraints = eda_api::parse_timing_constraints(
                "create_clock -name clk -period 4.0 [get_ports a]",
            )?;
            let scenarios = vec![
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "reference",
                    &flow.netlist,
                    &constraints,
                    Vec::new(),
                ),
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "incompatible",
                    &other,
                    &constraints,
                    Vec::new(),
                ),
            ];
            match eda_api::run_multi_corner_physical_timing_flow(
                &eda_api::MultiCornerPhysicalTimingFlowOptions {
                    scenarios,
                    ..Default::default()
                },
            ) {
                Err(error) if error.contains("width differs") => Ok(()),
                other => Err(format!(
                    "expected physical topology rejection, got {:?}",
                    other
                )),
            }
        },
    );

    c.add("api_extract_interconnect_parasitics_from_routing", || {
        let src = "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let model = eda_api::InterconnectModel::default();
        let spef = eda_api::extract_interconnect_parasitics(&flow.netlist, &flow.routing, &model)?;
        let report = eda_api::parasitic_analysis(&flow.netlist, &spef)?;
        let expected_grounded = flow.routing.wirelength() as f64
            * model.wire_capacitance_pf_per_site
            + flow.routing.vias as f64 * model.via_capacitance_pf;
        let expected_resistance = flow.routing.wirelength() as f64
            * model.wire_resistance_ohm_per_site
            + flow.routing.vias as f64 * model.via_resistance_ohm;
        let sdf = eda_api::parasitics_to_sdf(&flow.netlist, &spef)?;
        let parsed_sdf = eda_api::parse_delay_file(&sdf)?;
        let quality = eda_api::analyze_signal_quality_with_parasitics(
            &flow.netlist,
            &report,
            &eda_api::QualityOptions {
                max_capacitance_pf: Some(0.0),
                ..Default::default()
            },
        )?;
        if report.unmatched_netlist_nets.is_empty()
            && report.net_count == flow.netlist.net_count()
            && (report.total_grounded_cap_pf - expected_grounded).abs() < 1e-12
            && (report.total_resistance_ohm - expected_resistance).abs() < 1e-12
            && report.worst_delay_ns > 0.0
            && !parsed_sdf.interconnects.is_empty()
            && quality.capacitances_checked > 0
        {
            Ok(())
        } else {
            Err(eda_api::parasitic_report(&report))
        }
    });
    c.add("api_routing_parasitics_write_round_trip", || {
        let src = "module m(input a, b, output y); assign y = a & b; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let spef = eda_api::extract_interconnect_parasitics(
            &flow.netlist,
            &flow.routing,
            &eda_api::InterconnectModel::default(),
        )?;
        let text = eda_api::write_parasitics(&spef)?;
        let reparsed = eda_api::parse_parasitics(&text)?;
        let before = eda_api::parasitic_analysis(&flow.netlist, &spef)?;
        let after = eda_api::parasitic_analysis(&flow.netlist, &reparsed)?;
        if after.net_count == before.net_count
            && after.unmatched_netlist_nets.is_empty()
            && (after.total_resistance_ohm - before.total_resistance_ohm).abs() < 1e-12
            && (after.total_grounded_cap_pf - before.total_grounded_cap_pf).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!("before={:?}\nafter={:?}", before, after))
        }
    });
    c.add("api_routing_coupling_extraction", || {
        let mut netlist = eda_core::netlist::Netlist::new("coupled");
        let left = netlist.add_net("left");
        let right = netlist.add_net("right");
        let routing = eda_pnr::Routing {
            segments: vec![
                eda_pnr::Segment {
                    net: left,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 0,
                    end: 5,
                    track: 0,
                },
                eda_pnr::Segment {
                    net: right,
                    orientation: eda_pnr::Orientation::Horizontal,
                    channel: 0,
                    start: 2,
                    end: 7,
                    track: 1,
                },
            ],
            vias: 0,
            via_locations: Vec::new(),
            tracks_per_channel: 2,
            congested_channels: Vec::new(),
            blocked_channels: Vec::new(),
        };
        let model = eda_api::InterconnectModel {
            coupling_capacitance_pf_per_site: 0.1,
            ..Default::default()
        };
        let spef = eda_api::extract_interconnect_parasitics(&netlist, &routing, &model)?;
        let report = eda_api::parasitic_analysis(&netlist, &spef)?;
        let left = report
            .nets
            .iter()
            .find(|net| net.net == "left")
            .ok_or("missing left")?;
        let right = report
            .nets
            .iter()
            .find(|net| net.net == "right")
            .ok_or("missing right")?;
        if (left.coupling_cap_pf - 0.3).abs() < 1e-12 && (right.coupling_cap_pf - 0.3).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!("{:?}\n{:?}", left, right))
        }
    });
}
