//! Clock-tree synthesis and post-CTS validation cases.

use crate::CaseList;
use eda_core::netlist::Netlist;

fn synth(src: &'static str) -> Netlist {
    let design = eda_hdl::compile(src, None).expect("compile");
    eda_synth::synthesize(
        &design,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .expect("synthesis")
    .netlist
}

pub fn register(c: &mut CaseList) {
    c.add("cts_single_flop_gets_buffer", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        if result.trees.len() != 1
            || result.trees[0].sinks.len() != 1
            || result.buffers_inserted != 1
            || result.trees[0].levels != 1
        {
            Err(eda_cts::report(&result))
        } else {
            Ok(())
        }
    });

    c.add("cts_balances_multi_flop_clock", || {
        let nl = synth(
            "module m(input clk, rst, output reg [7:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 8'd0; else q <= q + 8'd1;
             endmodule",
        );
        let branch = 4usize;
        let result = eda_cts::synthesize(
            &nl,
            &eda_cts::CtsOptions {
                branch_factor: branch,
                wire_delay_ns_per_unit: 0.01,
            },
        )?;
        let tree = result.trees.first().ok_or("no tree")?;
        if tree.sinks.len() != 8
            || tree.levels != 2
            || tree.max_fanout > branch
            || result.buffers_inserted != 5
        {
            Err(format!(
                "sinks={} levels={} fanout={} buffers={}",
                tree.sinks.len(),
                tree.levels,
                tree.max_fanout,
                result.buffers_inserted
            ))
        } else {
            Ok(())
        }
    });

    c.add("cts_preserves_function", || {
        let nl = synth(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        let equivalence = eda_formal::check_equivalent(&nl, &result.netlist)?;
        if equivalence.equivalent {
            Ok(())
        } else {
            Err(format!(
                "post-CTS equivalence failed: {:?}",
                equivalence.notes
            ))
        }
    });

    c.add("cts_reports_insertion_delay", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        let tree = result.trees.first().ok_or("no tree")?;
        if tree.levels == 1 && tree.estimated_insertion_delay_ns > 0.0 {
            Ok(())
        } else {
            Err(format!(
                "levels={} insertion={}",
                tree.levels, tree.estimated_insertion_delay_ns
            ))
        }
    });

    c.add("cts_handles_two_clock_domains", || {
        let nl = synth(
            "module m(input clk_a, clk_b, d, output reg a_q, b_q);
                always @(posedge clk_a) a_q <= d;
                always @(posedge clk_b) b_q <= a_q;
             endmodule",
        );
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        if result.trees.len() == 2
            && result
                .trees
                .iter()
                .map(|tree| tree.sinks.len())
                .sum::<usize>()
                == 2
            && result.buffers_inserted == 2
        {
            Ok(())
        } else {
            Err(eda_cts::report(&result))
        }
    });

    c.add("cts_connects_all_sinks_through_tree", || {
        let nl = synth(
            "module m(input clk, rst, output reg [4:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 5'd0; else q <= q + 5'd1;
             endmodule",
        );
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        let tree = result.trees.first().ok_or("no tree")?;
        for sink in &tree.sinks {
            let connected = result.netlist.cell(sink.cell).inputs[sink.clock_pin];
            if connected == tree.root || result.netlist.net(connected).drivers.len() != 1 {
                return Err(format!("sink {} not connected", sink.cell));
            }
        }
        Ok(())
    });

    c.add("cts_report_content", || {
        let nl = synth(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        let text = eda_cts::report(&result);
        if text.contains("CTS report") && text.contains("CLOCK clk") && text.contains("sinks=4") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("cts_rejects_invalid_branch_factor", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        if eda_cts::synthesize(
            &nl,
            &eda_cts::CtsOptions {
                branch_factor: 1,
                wire_delay_ns_per_unit: 0.01,
            },
        )
        .is_ok()
        {
            Err("branch factor 1 accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("cts_rejects_combinational_design", || {
        let nl = synth("module m(input a,b,output y); assign y=a&b; endmodule");
        if eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default()).is_ok() {
            Err("combinational design unexpectedly accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("api_clock_tree_synthesis", || {
        let flow = eda_api::run_flow(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
            None,
        )?;
        let result = eda_api::clock_tree_synthesis(&flow.netlist, &eda_api::CtsOptions::default())?;
        if result.buffers_inserted == 1 && eda_api::cts_report(&result).contains("CLOCK clk") {
            Ok(())
        } else {
            Err(eda_api::cts_report(&result))
        }
    });
    c.add("cts_placement_aware_clusters_nearby_sinks", || {
        let nl = synth(
            "module m(input clk, d, output reg [7:0] q);
                always @(posedge clk) q <= d;
             endmodule",
        );
        let placement = eda_pnr::place(&nl)?;
        eda_pnr::check_legal(&nl, &placement)?;
        let result =
            eda_cts::synthesize_with_placement(&nl, &placement, &eda_cts::CtsOptions::default())?;
        let tree = result.trees.first().ok_or("no tree")?;
        if !tree.placement_aware {
            return Err("tree is not placement aware".into());
        }

        // Check that the sink order is physical, not original cell-id order.
        let locations: Vec<_> = tree
            .sinks
            .iter()
            .map(|sink| placement.cell_pos[sink.cell])
            .collect();
        if locations
            .windows(2)
            .any(|pair| (pair[0].row, pair[0].site) > (pair[1].row, pair[1].site))
        {
            return Err(format!("sink locations are unsorted: {:?}", locations));
        }

        // With 8 sinks, branch 4, and two levels, nearby sink pairs should
        // share a leaf buffer.
        let mut leaf_groups: std::collections::BTreeMap<usize, Vec<usize>> =
            std::collections::BTreeMap::new();
        for sink in &tree.sinks {
            let clock_net = result.netlist.cell(sink.cell).inputs[sink.clock_pin];
            let driver = result.netlist.net(clock_net).drivers[0];
            leaf_groups.entry(driver).or_default().push(sink.cell);
        }
        if leaf_groups.values().any(|group| group.len() != 2) {
            return Err(format!("unexpected leaf fanout groups: {:?}", leaf_groups));
        }
        if tree.estimated_wire_delay_ns <= 0.0
            || tree.estimated_physical_skew_ns < 0.0
            || tree.estimated_skew_ns < tree.estimated_physical_skew_ns
        {
            return Err(format!(
                "wire={} physical_skew={} skew={}",
                tree.estimated_wire_delay_ns,
                tree.estimated_physical_skew_ns,
                tree.estimated_skew_ns
            ));
        }
        Ok(())
    });
    c.add(
        "cts_rejects_bad_wire_delay",
        || match eda_cts::CtsOptions::new(4, f64::NAN) {
            Err(e) if e.contains("wire delay") => Ok(()),
            other => Err(format!("unexpected options result: {:?}", other)),
        },
    );
    c.add("cts_rejects_placement_size_mismatch", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let mut placement = eda_pnr::place(&nl)?;
        placement.cell_pos.pop();
        if eda_cts::synthesize_with_placement(&nl, &placement, &eda_cts::CtsOptions::default())
            .is_ok()
        {
            Err("mismatched placement accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("cts_creates_legal_post_cts_placement", || {
        let nl = synth(
            "module m(input clk, rst, output reg [3:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 4'd0; else q <= q + 4'd1;
             endmodule",
        );
        let placement = eda_pnr::place(&nl)?;
        let result =
            eda_cts::synthesize_with_placement(&nl, &placement, &eda_cts::CtsOptions::default())?;
        let post = result
            .physical_placement
            .as_ref()
            .ok_or("post-CTS placement missing")?;
        if post.cell_pos.len() != result.netlist.cell_count()
            || post.cell_width.len() != result.netlist.cell_count()
        {
            return Err(format!(
                "placement cells={} widths={} netlist cells={}",
                post.cell_pos.len(),
                post.cell_width.len(),
                result.netlist.cell_count()
            ));
        }
        eda_pnr::check_legal(&result.netlist, post)?;
        let routing = eda_pnr::route(&result.netlist, post)?;
        let drc = eda_pnr::drc(&result.netlist, post, &routing)?;
        if drc.has_errors() {
            Err(eda_pnr::drc_report(&drc))
        } else if !eda_cts::report(&result).contains("Physical placement") {
            Err("CTS report omitted physical placement".into())
        } else {
            Ok(())
        }
    });
    c.add("cts_logical_result_has_no_physical_placement", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let result = eda_cts::synthesize(&nl, &eda_cts::CtsOptions::default())?;
        if result.physical_placement.is_some() {
            Err("logical CTS unexpectedly invented a placement".into())
        } else {
            Ok(())
        }
    });
    c.add("api_placement_aware_clock_tree_synthesis", || {
        let flow = eda_api::run_flow(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
            None,
        )?;
        let result = eda_api::clock_tree_synthesis_with_placement(
            &flow.netlist,
            &flow.placement,
            &eda_api::CtsOptions::default(),
        )?;
        let tree = result.trees.first().ok_or("no tree")?;
        if tree.placement_aware
            && tree.estimated_wire_delay_ns >= 0.0
            && eda_api::cts_report(&result).contains("placement=aware")
        {
            Ok(())
        } else {
            Err(eda_api::cts_report(&result))
        }
    });
    c.add("api_timing_driven_cts_searches_branch_factors", || {
        let flow = eda_api::run_flow(
            "module m(input clk, rst, output reg [7:0] q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 8'd0; else q <= q + 8'd1;
             endmodule",
            None,
        )?;
        let result = eda_api::timing_driven_clock_tree_synthesis_with_placement(
            &flow.netlist,
            &flow.placement,
            &eda_api::TimingDrivenCtsOptions {
                target_skew_ns: 1.0,
                ..Default::default()
            },
        )?;
        let report = eda_api::timing_driven_cts_report(&result);
        let tree = result.cts.trees.first().ok_or("no clock tree")?;
        if result.searched_branch_factors != (2..=16).collect::<Vec<_>>()
            || result.selected_branch_factor != 8
            || tree.branch_factor != 8
            || result.cts.buffers_inserted >= 5
            || !result.target_met
            || !report.contains("target MET")
        {
            Err(report)
        } else {
            Ok(())
        }
    });
    c.add("api_post_cts_physical_flow", || {
        let flow = eda_api::run_flow(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
            None,
        )?;
        let cts = eda_api::clock_tree_synthesis_with_placement(
            &flow.netlist,
            &flow.placement,
            &eda_api::CtsOptions::default(),
        )?;
        let placement = cts.physical_placement.as_ref().ok_or("no placement")?;
        eda_pnr::check_legal(&cts.netlist, placement)?;
        let routing = eda_pnr::route(&cts.netlist, placement)?;
        let drc = eda_pnr::drc(&cts.netlist, placement, &routing)?;
        if drc.has_errors() {
            Err(eda_pnr::drc_report(&drc))
        } else {
            Ok(())
        }
    });
}
