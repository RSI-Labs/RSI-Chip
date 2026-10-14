//! Clock-domain inference and crossing-analysis cases.

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
    c.add("cdc_single_clock_has_no_crossing", || {
        let nl = synth(
            "module m(input clk, d, output reg q, r); always @(posedge clk) begin q <= d; r <= q; end endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result.clocks.len() == 1 && result.crossings.is_empty() && result.clean() {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_two_flop_synchronizer_detected", || {
        let nl = synth(
            "module m(input clk_a, clk_b, d, output reg a_q, sync0, sync1); always @(posedge clk_a) a_q <= d; always @(posedge clk_b) sync0 <= a_q; always @(posedge clk_b) sync1 <= sync0; endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result.clocks.len() == 2
            && result.crossings.len() == 1
            && result.crossings[0].synchronized
            && result.clean()
        {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_unsynchronized_crossing_is_error", || {
        let nl = synth(
            "module m(input clk_a, clk_b, d, output reg a_q, b_q);
                always @(posedge clk_a) a_q <= d;
                always @(posedge clk_b) b_q <= a_q;
             endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result.has_errors()
            && result
                .findings
                .iter()
                .any(|finding| finding.kind == "unsynchronized-crossing")
        {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_combinational_crossing_is_traced", || {
        let nl = synth(
            "module m(input clk_a, clk_b, d, output reg a_q, b_q); always @(posedge clk_a) a_q <= d; always @(posedge clk_b) b_q <= (a_q & d); endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        let crossing = result
            .crossings
            .iter()
            .find(|path| path.source_clock == "clk_a" && path.destination_clock == "clk_b")
            .ok_or("no crossing")?;
        if !crossing.combinational_cells.is_empty() && result.has_errors() {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_same_domain_logic_is_not_crossing", || {
        let nl = synth(
            "module m(input clk, d, output reg q, r);
                always @(posedge clk) q <= d;
                always @(posedge clk) r <= q | d;
             endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result.crossings.is_empty() {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_multi_bit_bus_skew_reported", || {
        let nl = synth(
            "module m(input clk_a, clk_b, input [2:0] d, output reg [2:0] a_q, b_q);
                always @(posedge clk_a) a_q <= d;
                always @(posedge clk_b) b_q <= a_q;
             endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result
            .findings
            .iter()
            .any(|finding| finding.kind == "multi-bit-crossing")
        {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_clock_inverter_preserves_domain", || {
        let nl = synth(
            "module m(input clk, d, output reg q, r); always @(posedge clk) q <= d; always @(negedge clk) r <= q; endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        if result.clocks.len() == 1 && result.crossings.is_empty() {
            Ok(())
        } else {
            Err(eda_cdc::report(&result))
        }
    });

    c.add("cdc_report_contains_domains_and_findings", || {
        let nl = synth(
            "module m(input clk_a, clk_b, d, output reg a_q, b_q);
                always @(posedge clk_a) a_q <= d;
                always @(posedge clk_b) b_q <= a_q;
             endmodule",
        );
        let result = eda_cdc::analyze(&nl)?;
        let text = eda_cdc::report(&result);
        if text.contains("CDC report")
            && text.contains("clk_a")
            && text.contains("clk_b")
            && text.contains("unsynchronized-crossing")
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_cdc_analysis", || {
        let flow = eda_api::run_flow(
            "module m(input clk_a, clk_b, d, output reg a_q, sync0, sync1);
                always @(posedge clk_a) a_q <= d;
                always @(posedge clk_b) sync0 <= a_q;
                always @(posedge clk_b) sync1 <= sync0;
             endmodule",
            None,
        )?;
        let result = eda_api::clock_domain_crossing(&flow.netlist)?;
        if result.clean() && eda_api::cdc_report(&result).contains("synchronized-crossing") {
            Ok(())
        } else {
            Err(eda_api::cdc_report(&result))
        }
    });
    c.add("cdc_hierarchical_crossing_attribution", || {
        let nl = synth(
            r#"
            module source(input clk_a, d, output reg q);
                always @(posedge clk_a) q <= d;
            endmodule
            module destination(input clk_b, d, output reg q);
                always @(posedge clk_b) q <= d;
            endmodule
            module top(input clk_a, clk_b, d, output q);
                wire crossing;
                source u_source(.clk_a(clk_a), .d(d), .q(crossing));
                destination u_destination(.clk_b(clk_b), .d(crossing), .q(q));
            endmodule
        "#,
        );
        let result = eda_cdc::analyze(&nl)?;
        let crossing = result
            .crossings
            .first()
            .ok_or("hierarchical CDC omitted crossing")?;
        let source = crossing
            .source_hierarchy
            .as_deref()
            .ok_or("source hierarchy missing")?;
        let destination = crossing
            .destination_hierarchy
            .as_deref()
            .ok_or("destination hierarchy missing")?;
        let summaries = result.hierarchy_summaries();
        let report = eda_cdc::report(&result);
        if source.contains("u_source")
            && destination.contains("u_destination")
            && crossing
                .hierarchy_instances
                .iter()
                .any(|instance| instance.contains("u_source"))
            && crossing
                .hierarchy_instances
                .iter()
                .any(|instance| instance.contains("u_destination"))
            && summaries.len() == 2
            && summaries.values().all(|summary| {
                summary.source_crossings + summary.destination_crossings == 1
                    && summary.unsynchronized == 1
            })
            && report.contains("hierarchy distribution:")
            && report.contains("synchronized=0, unsynchronized=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("api_cdc_hierarchy_summary", || {
        let flow = eda_api::run_flow(
            r#"
            module source(input clk_a, d, output reg q);
                always @(posedge clk_a) q <= d;
            endmodule
            module synchronizer(input clk_b, d, output reg sync0, sync1);
                always @(posedge clk_b) sync0 <= d;
                always @(posedge clk_b) sync1 <= sync0;
            endmodule
            module top(input clk_a, clk_b, d, output q);
                wire crossing;
                source u_source(.clk_a(clk_a), .d(d), .q(crossing));
                synchronizer u_sync(.clk_b(clk_b), .d(crossing), .sync0(), .sync1(q));
            endmodule
        "#,
            Some("top"),
        )?;
        let result = eda_api::clock_domain_crossing(&flow.netlist)?;
        let summaries: std::collections::BTreeMap<Option<String>, eda_api::CdcHierarchySummary> =
            result.hierarchy_summaries();
        let report = eda_api::cdc_report(&result);
        if result.clean()
            && summaries.len() == 2
            && summaries
                .values()
                .all(|summary| summary.synchronized == 1 && summary.unsynchronized == 0)
            && report.contains("hierarchy: top.u_source -> top.u_sync")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
}
