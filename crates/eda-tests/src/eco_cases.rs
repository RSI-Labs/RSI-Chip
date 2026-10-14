//! Structural ECO parsing, application, and validation cases.

use crate::CaseList;
use eda_core::library::CellLibrary;
use eda_core::netlist::{Cell, CellKind, Dir, Netlist};
use std::sync::Arc;

fn synth() -> Netlist {
    let design = eda_hdl::compile(
        "module m(input a,b,c,output y); wire n; assign n=a&b; assign y=n|c; endmodule",
        None,
    )
    .expect("compile");
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

fn high_fanout_synth() -> Netlist {
    let design = eda_hdl::compile(
        r#"module m(input a,b,c0,c1,c2,c3,c4,c5,output [5:0] y);
            assign y[0]=a&b&c0;
            assign y[1]=a&b&c1;
            assign y[2]=a&b&c2;
            assign y[3]=a&b&c3;
            assign y[4]=a&b&c4;
            assign y[5]=a&b&c5;
           endmodule"#,
        None,
    )
    .expect("compile");
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

fn hierarchical_synth() -> Netlist {
    let design = eda_hdl::compile(
        r#"
        module leaf(input a, b, output y); assign y = a & b; endmodule
        module sub(input a, b, output y);
            leaf u_leaf(.a(a), .b(b), .y(y));
        endmodule
        module top(input a, b, output y1, y2);
            sub u1(.a(a), .b(b), .y(y1));
            sub u2(.a(a), .b(b), .y(y2));
        endmodule
    "#,
        Some("top"),
    )
    .expect("compile hierarchy");
    eda_synth::synthesize(
        &design,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .expect("synthesize hierarchy")
    .netlist
}

fn hierarchical_high_fanout_synth() -> Netlist {
    let design = eda_hdl::compile(
        r#"
        module fan(input a, b, e0, e1, e2, e3, e4, e5, output [5:0] y);
            wire n;
            assign n = a & b;
            assign y[0] = n & e0;
            assign y[1] = n & e1;
            assign y[2] = n & e2;
            assign y[3] = n & e3;
            assign y[4] = n & e4;
            assign y[5] = n & e5;
        endmodule
        module top(
            input a, b, c, d,
            input u1_e0, u1_e1, u1_e2, u1_e3, u1_e4, u1_e5,
            input u2_e0, u2_e1, u2_e2, u2_e3, u2_e4, u2_e5,
            output [5:0] y1, y2
        );
            fan u1(
                .a(a), .b(b),
                .e0(u1_e0), .e1(u1_e1), .e2(u1_e2),
                .e3(u1_e3), .e4(u1_e4), .e5(u1_e5),
                .y(y1)
            );
            fan u2(
                .a(c), .b(d),
                .e0(u2_e0), .e1(u2_e1), .e2(u2_e2),
                .e3(u2_e3), .e4(u2_e4), .e5(u2_e5),
                .y(y2)
            );
        endmodule
    "#,
        Some("top"),
    )
    .expect("compile hierarchical fanout");
    eda_synth::synthesize(
        &design,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .expect("synthesize hierarchical fanout")
    .netlist
}

pub fn register(c: &mut CaseList) {
    c.add("eco_parse_named_patch_and_operations", || {
        let patch = eda_eco::parse_eco(
            r#"
eco functional_fix;
replace_cell u1_and2 OR2;
connect_input u2_or2 1 c;
insert_buffer u2_or2 0 eco_buf eco_buf_net;
tie_input u1_and2 1 0;
"#,
        )?;
        if patch.name == "functional_fix" && patch.operations.len() == 4 {
            Ok(())
        } else {
            Err(format!("{:?}", patch))
        }
    });

    c.add("eco_replace_mapped_cell", || {
        let nl = synth();
        let patch = eda_eco::parse_eco("replace_cell u1_and2 OR2;")?;
        let result = eda_eco::apply_eco(&nl, &patch)?;
        let stats = result.netlist.stats();
        if result.operations_applied != 1 || stats.cells != nl.cell_count() {
            Err(format!("stats={:?} before={:?}", stats, nl.stats()))
        } else {
            let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
            if equivalent.equivalent {
                Err("AND -> OR unexpectedly remained equivalent".into())
            } else {
                Ok(())
            }
        }
    });

    c.add("eco_connect_input_rewires_pin", || {
        let nl = synth();
        let patch = eda_eco::parse_eco("connect_input u1_and2 1 c;")?;
        let result = eda_eco::apply_eco(&nl, &patch)?;
        let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
        if equivalent.equivalent {
            Err("rewiring b to c unexpectedly remained equivalent".into())
        } else {
            Ok(())
        }
    });

    c.add("eco_insert_buffer_preserves_function", || {
        let nl = synth();
        let patch = eda_eco::parse_eco("insert_buffer u2_or2 0 eco_buf eco_buf_net;")?;
        let result = eda_eco::apply_eco(&nl, &patch)?;
        if result.netlist.cell_count() != nl.cell_count() + 1
            || result.netlist.net_count() != nl.net_count() + 1
        {
            Err(format!(
                "cells {} nets {}",
                result.netlist.cell_count(),
                result.netlist.net_count()
            ))
        } else {
            let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
            if equivalent.equivalent {
                Ok(())
            } else {
                Err(format!(
                    "buffer insertion is not equivalent: {:?}",
                    equivalent.notes
                ))
            }
        }
    });

    c.add("eco_tie_input_changes_function", || {
        let nl = synth();
        let patch = eda_eco::parse_eco("tie_input u1_and2 1 1;")?;
        let result = eda_eco::apply_eco(&nl, &patch)?;
        let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
        if equivalent.equivalent {
            Err("tying b to one unexpectedly remained equivalent".into())
        } else {
            Ok(())
        }
    });

    c.add("eco_failure_does_not_expose_partial_patch", || {
        let nl = synth();
        let before = nl.stats();
        let patch = eda_eco::parse_eco("replace_cell u1_and2 OR2; replace_cell missing INV;")?;
        if eda_eco::apply_eco(&nl, &patch).is_ok() {
            return Err("invalid ECO unexpectedly succeeded".into());
        }
        // The input handle remains unchanged by definition; also verify that
        // a clone with identical stats did not leak a cell replacement.
        let clone = nl.clone();
        let after = clone.stats();
        if (
            after.cells,
            after.nets,
            after.comb,
            after.seq,
            after.lib_cells,
        ) != (
            before.cells,
            before.nets,
            before.comb,
            before.seq,
            before.lib_cells,
        ) {
            Err("source netlist changed after failed ECO".into())
        } else {
            Ok(())
        }
    });

    c.add("eco_rejects_invalid_names_and_syntax", || {
        for source in [
            "// empty",
            "replace_cell missing OR2;",
            "connect_input u1_and2 99 c;",
            "connect_input u1_and2 0 missing_net;",
            "replace_cell u1_and2 MISSING_CELL;",
            "insert_buffer u1_and2 0 u1_and2 new_net;",
            "tie_input u1_and2 0 2;",
        ] {
            if let Ok(patch) = eda_eco::parse_eco(source) {
                if eda_eco::apply_eco(&synth(), &patch).is_ok() {
                    return Err(format!("invalid ECO accepted: {}", source));
                }
            }
        }
        Ok(())
    });

    c.add("eco_hierarchy_scope_parses_and_selects_cell", || {
        let patch =
            eda_eco::parse_eco("eco hierarchy_fix; scope top.u1.**; replace_cell u1_and2 OR2;")?;
        if patch.scope.as_deref() != Some("top.u1.**") || patch.operations.len() != 1 {
            return Err(format!("{:?}", patch));
        }

        let nl = hierarchical_synth();
        let cell = nl.cell_by_name("u1_and2").ok_or("missing u1 cell")?;
        let hierarchy = nl
            .cell_hierarchy_instance(cell)
            .map(|instance| instance.path.clone())
            .ok_or("u1 cell has no hierarchy owner")?;
        if !hierarchy.starts_with("top.u1") {
            return Err(format!("unexpected u1 hierarchy '{}'", hierarchy));
        }
        let result = eda_eco::apply_eco(&nl, &patch)?;
        let report = eda_eco::report(&result);
        let wrong =
            eda_eco::parse_eco("eco wrong_scope; scope top.u2.**; replace_cell u1_and2 OR2;")?;
        if result.hierarchy_scope.as_deref() == Some("top.u1.**")
            && result.operations_applied == 1
            && report.contains("Hierarchy scope: top.u1.**")
            && report.contains(&format!(" at '{}'", hierarchy))
            && matches!(
                eda_eco::apply_eco(&nl, &wrong),
                Err(error) if error.contains("not found in hierarchy scope 'top.u2.**'")
            )
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("eco_hierarchy_scope_attributes_inserted_buffer", || {
        let nl = hierarchical_synth();
        let patch = eda_eco::parse_eco("insert_buffer u1_and2 1 eco_buf eco_buf_net;")?;
        let result = eda_eco::apply_eco_in_hierarchy(&nl, &patch, "top.u1.**")?;
        let target = result
            .netlist
            .cell_by_name("u1_and2")
            .ok_or("missing patched target")?;
        let buffer = result
            .netlist
            .cell_by_name("eco_buf")
            .ok_or("missing inserted buffer")?;
        let target_hierarchy = result
            .netlist
            .cell_hierarchy_instance(target)
            .map(|instance| instance.path.clone())
            .ok_or("patched target lost hierarchy")?;
        let buffer_hierarchy = result
            .netlist
            .cell_hierarchy_instance(buffer)
            .map(|instance| instance.path.clone())
            .ok_or("inserted buffer lost hierarchy")?;
        if result.hierarchy_scope.as_deref() == Some("top.u1.**")
            && buffer_hierarchy == target_hierarchy
            && eda_eco::report(&result).contains("Hierarchy scope: top.u1.**")
        {
            Ok(())
        } else {
            Err(format!(
                "target='{target_hierarchy}' buffer='{buffer_hierarchy}'\n{}",
                eda_eco::report(&result)
            ))
        }
    });

    c.add("eco_report_lists_changes", || {
        let nl = synth();
        let patch = eda_eco::parse_eco(
            "insert_buffer u2_or2 0 eco_buf eco_buf_net; tie_input u1_and2 1 0;",
        )?;
        let result = eda_eco::apply_eco(&nl, &patch)?;
        let text = eda_eco::report(&result);
        if text.contains("2 operation(s)")
            && text.contains("inserted buffer")
            && text.contains("tied")
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_eco_parse_and_apply", || {
        let flow = eda_api::run_flow(
            "module m(input a,b,c,output y); wire n; assign n=a&b; assign y=n|c; endmodule",
            None,
        )?;
        let patch = eda_api::parse_eco_patch("eco api_fix; replace_cell u1_and2 OR2;")?;
        let result = eda_api::apply_eco_patch(&flow.netlist, &patch)?;
        if result.operations_applied == 1 && eda_api::eco_report(&result).contains("api_fix") {
            Ok(())
        } else {
            Err(eda_api::eco_report(&result))
        }
    });

    c.add(
        "api_eco_replaces_sequential_cell_and_proves_equivalence",
        || {
            let library = Arc::new(CellLibrary::std());
            let mut nl = Netlist::new("sequential_eco");
            nl.set_library(library.clone());
            let clk = nl.add_net("clk");
            let d = nl.add_net("d");
            let q = nl.add_net("q");
            nl.add_cell(Cell {
                name: "q_reg".into(),
                kind: CellKind::Dff { posedge: true },
                inputs: vec![clk, d],
                output: q,
            });
            nl.add_port("clk", Dir::In, vec![clk]);
            nl.add_port("d", Dir::In, vec![d]);
            nl.add_port("q", Dir::Out, vec![q]);

            let patch = eda_api::parse_eco_patch("eco dff_map; replace_cell q_reg DFF;")?;
            let result = eda_api::apply_eco_patch(&nl, &patch)?;
            let expected = CellKind::Lib(library.find("DFF").expect("standard DFF"));
            if result.operations_applied != 1
                || result.netlist.cell(0).kind != expected
                || result.netlist.cell(0).inputs != vec![clk, d]
            {
                return Err(eda_api::eco_report(&result));
            }
            let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
            if equivalent.equivalent {
                Ok(())
            } else {
                Err(format!(
                    "sequential ECO replacement changed behavior: {:?}",
                    equivalent.notes
                ))
            }
        },
    );
    c.add("api_eco_hierarchy_scoped_patch", || {
        let src = r#"
            module leaf(input a, b, output y); assign y = a & b; endmodule
            module sub(input a, b, output y);
                leaf u_leaf(.a(a), .b(b), .y(y));
            endmodule
            module top(input a, b, output y1, y2);
                sub u1(.a(a), .b(b), .y(y1));
                sub u2(.a(a), .b(b), .y(y2));
            endmodule
        "#;
        let flow = eda_api::run_flow(src, Some("top"))?;
        let patch = eda_api::parse_eco_patch("replace_cell u1_and2 OR2;")?;
        let result = eda_api::apply_eco_patch_in_hierarchy(&flow.netlist, &patch, "top.u1.**")?;
        let report = eda_api::eco_report(&result);
        if result.operations_applied == 1
            && result.hierarchy_scope.as_deref() == Some("top.u1.**")
            && report.contains("Hierarchy scope: top.u1.**")
            && matches!(
                eda_api::apply_eco_patch_in_hierarchy(
                    &flow.netlist,
                    &patch,
                    "top.u2.**"
                ),
                Err(error) if error.contains("not found in hierarchy scope")
            )
        {
            Ok(())
        } else {
            Err(report)
        }
    });
    c.add("eco_automatic_fanout_repair_preserves_function", || {
        let nl = high_fanout_synth();
        let result = eda_eco::repair_fanout(
            &nl,
            &eda_eco::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "fanout_fix".into(),
            },
        )?;
        let equivalent = eda_formal::check_equivalent(&nl, &result.netlist)?;
        if !equivalent.equivalent {
            return Err(format!(
                "fanout repair changed function: {:?}",
                equivalent.counterexample
            ));
        }
        if result.netlist.cell_count() <= nl.cell_count() || result.operations_applied == 0 {
            return Err(eda_eco::report(&result));
        }
        for net_id in 0..result.netlist.net_count() {
            let fanout = result.netlist.net(net_id).sinks.len();
            if fanout > 2 {
                return Err(format!(
                    "net '{}' fanout {} > 2",
                    result.netlist.net(net_id).name,
                    fanout
                ));
            }
        }
        Ok(())
    });
    c.add("eco_fanout_repair_legalizes_physical_placement", || {
        let nl = high_fanout_synth();
        let placement = eda_pnr::place(&nl)?;
        let result = eda_eco::repair_fanout_with_placement(
            &nl,
            &placement,
            &eda_eco::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "physical_fanout_fix".into(),
            },
        )?;
        let post = result
            .physical_placement
            .as_ref()
            .ok_or("no ECO placement")?;
        if post.cell_pos.len() != result.netlist.cell_count()
            || post.cell_width.len() != result.netlist.cell_count()
        {
            return Err(format!(
                "positions={} widths={} cells={}",
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
        } else if !eda_eco::report(&result).contains("Physical placement") {
            Err("ECO report omitted physical placement".into())
        } else {
            Ok(())
        }
    });
    c.add(
        "eco_automatic_fanout_repair_clears_quality_violation",
        || {
            let nl = high_fanout_synth();
            let options = eda_sta::QualityOptions {
                max_fanout: Some(2),
                ..Default::default()
            };
            let before = eda_sta::analyze_quality(&nl, &options)?;
            let result = eda_eco::repair_fanout(
                &nl,
                &eda_eco::FanoutRepairOptions {
                    max_fanout: 2,
                    ..Default::default()
                },
            )?;
            let after = eda_sta::analyze_quality(&result.netlist, &options)?;
            if before.violations.is_empty() || after.has_violations() {
                Err(format!(
                    "before:\n{}after:\n{}",
                    eda_sta::quality_report(&before),
                    eda_sta::quality_report(&after)
                ))
            } else {
                Ok(())
            }
        },
    );
    c.add("eco_fanout_repair_respects_hierarchy_scope", || {
        let nl = hierarchical_high_fanout_synth();
        let all = eda_eco::repair_fanout(
            &nl,
            &eda_eco::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "all_fanout".into(),
            },
        )?;
        let scoped = eda_eco::repair_fanout_in_hierarchy(
            &nl,
            &eda_eco::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "u1_fanout".into(),
            },
            "top.u1.**",
        )?;
        let equivalent = eda_formal::check_equivalent(&nl, &scoped.netlist)?;
        let inserted_hierarchies: Vec<String> = (0..scoped.netlist.cell_count())
            .filter(|&cell| scoped.netlist.cell(cell).name.contains("eco_buf"))
            .filter_map(|cell| {
                scoped
                    .netlist
                    .cell_hierarchy_instance(cell)
                    .map(|instance| instance.path.clone())
            })
            .collect();
        if all.operations_applied != 2
            || scoped.operations_applied != 1
            || !equivalent.equivalent
            || inserted_hierarchies.is_empty()
            || !inserted_hierarchies
                .iter()
                .all(|path| path.starts_with("top.u1"))
            || scoped.hierarchy_scope.as_deref() != Some("top.u1.**")
            || !eda_eco::report(&scoped).contains("Hierarchy scope: top.u1.**")
        {
            Err(format!(
                "all={} scoped={} hierarchies={:?}\n{}",
                all.operations_applied,
                scoped.operations_applied,
                inserted_hierarchies,
                eda_eco::report(&scoped)
            ))
        } else {
            Ok(())
        }
    });
    c.add("eco_automatic_fanout_repair_rejects_zero", || {
        if eda_eco::repair_fanout(
            &high_fanout_synth(),
            &eda_eco::FanoutRepairOptions {
                max_fanout: 0,
                ..Default::default()
            },
        )
        .is_ok()
        {
            Err("zero fanout limit accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("api_automatic_fanout_repair", || {
        let flow = eda_api::run_flow(
            r#"module m(input a,b,c0,c1,c2,c3,c4,c5,output [5:0] y);
                assign y[0]=a&b&c0;
                assign y[1]=a&b&c1;
                assign y[2]=a&b&c2;
                assign y[3]=a&b&c3;
                assign y[4]=a&b&c4;
                assign y[5]=a&b&c5;
               endmodule"#,
            None,
        )?;
        let result = eda_api::repair_fanout(
            &flow.netlist,
            &eda_api::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "api_fanout_fix".into(),
            },
        )?;
        if result.operations_applied > 0
            && eda_api::eco_report(&result).contains("split net")
            && result.netlist.cell_count() > flow.netlist.cell_count()
        {
            Ok(())
        } else {
            Err(eda_api::eco_report(&result))
        }
    });
    c.add("api_fanout_repair_with_physical_placement", || {
        let flow = eda_api::run_flow(
            r#"module m(input a,b,c0,c1,c2,c3,c4,c5,output [5:0] y);
                assign y[0]=a&b&c0;
                assign y[1]=a&b&c1;
                assign y[2]=a&b&c2;
                assign y[3]=a&b&c3;
                assign y[4]=a&b&c4;
                assign y[5]=a&b&c5;
               endmodule"#,
            None,
        )?;
        let result = eda_api::repair_fanout_with_placement(
            &flow.netlist,
            &flow.placement,
            &eda_api::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "api_physical_fanout_fix".into(),
            },
        )?;
        let placement = result.physical_placement.as_ref().ok_or("no placement")?;
        eda_pnr::check_legal(&result.netlist, placement)?;
        let routing = eda_pnr::route(&result.netlist, placement)?;
        let drc = eda_pnr::drc(&result.netlist, placement, &routing)?;
        if drc.has_errors() {
            Err(eda_pnr::drc_report(&drc))
        } else {
            Ok(())
        }
    });
    c.add("api_fanout_repair_in_hierarchy", || {
        let nl = hierarchical_high_fanout_synth();
        let result = eda_api::repair_fanout_in_hierarchy(
            &nl,
            &eda_api::FanoutRepairOptions {
                max_fanout: 2,
                patch_name: "api_u1_fanout".into(),
            },
            "top.u1.**",
        )?;
        let inserted = (0..result.netlist.cell_count())
            .filter(|&cell| result.netlist.cell(cell).name.contains("eco_buf"))
            .count();
        if result.operations_applied == 1
            && inserted > 0
            && result.hierarchy_scope.as_deref() == Some("top.u1.**")
            && eda_api::eco_report(&result).contains("Hierarchy scope: top.u1.**")
        {
            Ok(())
        } else {
            Err(eda_api::eco_report(&result))
        }
    });
}
