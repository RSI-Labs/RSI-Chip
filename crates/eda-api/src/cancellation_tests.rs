use super::*;

#[test]
fn flow_cancellation_token_is_cooperative_and_idempotent() {
    let cancellation = FlowCancellation::new();
    assert!(!cancellation.is_cancelled());
    assert!(cancellation.cancel());
    assert!(cancellation.is_cancelled());
    assert!(cancellation.cancel());
    let error = match run_flow_cancellable(
        "module top(input a, output y); assign y = ~a; endmodule",
        Some("top"),
        &cancellation,
    ) {
        Err(error) => error,
        Ok(_) => panic!("cancelled flow unexpectedly completed"),
    };
    assert_eq!(error, "RTL flow cancelled");
}

#[test]
fn ams_directives_dispatch_cancellably() {
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let operating_point =
        parse_spice("cancelled op\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.op\n.end").unwrap();
    assert_eq!(
        run_ams_analysis_cancellable(
            &operating_point,
            &operating_point.analyses[0],
            &cancellation
        )
        .unwrap_err(),
        "DC/transient solve cancelled"
    );

    let sweep = parse_spice(
        "cancelled dc sweep\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.dc Vin 0 1.8 0.1\n.end",
    )
    .unwrap();
    assert_eq!(
        run_ams_analysis_cancellable(&sweep, &sweep.analyses[0], &cancellation).unwrap_err(),
        "DC sweep cancelled"
    );

    let circuit = parse_spice(
        "cancelled optimizer\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:5000 population=8 iterations=2 seed=17 tol=1e-8\n.end",
    )
    .unwrap();
    let error =
        run_ams_analysis_cancellable(&circuit, &circuit.analyses[0], &cancellation).unwrap_err();
    assert_eq!(error, "optimization analysis cancelled");
}

#[test]
fn rtl_equivalence_is_structured_and_cancellable() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let result = check_rtl_equivalence(source, Some("top")).unwrap();
    assert!(result.equivalent);
    assert!(result.counterexample.is_empty());
    assert!(result.gold_stats.cells > 0);
    assert!(result.mapped_stats.lib_cells > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = match check_rtl_equivalence_cancellable(
        "module top(input a, output y); assign y = ~a; endmodule",
        Some("top"),
        &cancellation,
    ) {
        Err(error) => error,
        Ok(_) => panic!("cancelled equivalence unexpectedly completed"),
    };
    assert_eq!(error, "RTL equivalence cancelled");
}

#[test]
fn rtl_timing_is_structured_and_cancellable() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let result = analyze_rtl_timing(source, Some("top")).unwrap();
    assert!(result.critical_path_delay_ns.is_finite());
    assert!(result.critical_path_delay_ns >= 0.0);
    assert!(result.mapped_stats.lib_cells > 0);
    assert!(result.case_analysis_nets.is_empty());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = analyze_rtl_timing_cancellable(source, Some("top"), &cancellation).unwrap_err();
    assert_eq!(error, "RTL timing analysis cancelled");
}

#[test]
fn rtl_pnr_returns_physical_databases_and_cancels() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let result = implement_rtl_pnr(source, Some("top")).unwrap();
    assert!(!result.placement.cell_pos.is_empty());
    assert!(result.mapped_stats.lib_cells > 0);
    assert!(!result.lef.trim().is_empty());
    assert!(result.lef.contains("VERSION"));
    assert!(!result.def.trim().is_empty());
    assert!(result.def.contains("DESIGN"));
    assert!(!result.drc.has_errors());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = implement_rtl_pnr_cancellable(source, Some("top"), &cancellation).unwrap_err();
    assert_eq!(error, "RTL PnR cancelled");

    let strict = DrcOptions {
        max_antenna_ratio: 0.001,
    };
    let strict_result = implement_rtl_pnr_with_drc_options(source, Some("top"), &strict).unwrap();
    assert!(strict_result.drc.has_warnings());
    assert!(strict_result
        .drc
        .violations
        .iter()
        .any(|violation| violation.code == "ANT-001"));
    assert!(implement_rtl_pnr_with_drc_options(
        source,
        Some("top"),
        &DrcOptions {
            max_antenna_ratio: f64::NAN,
        }
    )
    .is_err());
}

#[test]
fn rtl_antenna_repair_is_structured_and_cancellable() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let options = AntennaRepairOptions::default();
    let result = repair_rtl_antennas(source, Some("top"), &options).unwrap();
    assert!(!result.baseline_drc.has_errors());
    assert_eq!(result.repair.diodes_inserted, 0);
    assert!(result.repair.all_antennas_repaired);
    assert!(result.final_mapped_stats.lib_cells > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        repair_rtl_antennas_cancellable(source, Some("top"), &options, &cancellation).unwrap_err(),
        "RTL antenna repair cancelled"
    );
    assert!(repair_rtl_antennas(
        source,
        Some("top"),
        &AntennaRepairOptions {
            patch_name: " ".into(),
            ..AntennaRepairOptions::default()
        }
    )
    .is_err());
}

#[test]
fn router_cancellation_surfaces_are_typed() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let design = compile(source, Some("top")).unwrap();
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())
            .unwrap()
            .netlist;
    let placement = eda_pnr::place(&netlist).unwrap();
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        route_with_options_cancellable(
            &netlist,
            &placement,
            &RouteOptions::default(),
            &cancellation
        )
        .unwrap_err(),
        "global routing cancelled"
    );
    assert_eq!(
        route_maze_with_options_cancellable(
            &netlist,
            &placement,
            &RouteOptions::default(),
            &cancellation
        )
        .unwrap_err(),
        "maze routing cancelled"
    );
    assert_eq!(
        route_maze_ripgup_cancellable(
            &netlist,
            &placement,
            &RouteOptions::default(),
            4,
            &cancellation
        )
        .unwrap_err(),
        "rip-up routing cancelled"
    );
    assert_eq!(
        route_maze_timing_ripgup_cancellable(
            &netlist,
            &placement,
            &RouteOptions::default(),
            &TimingDrivenRoutingOptions::default(),
            4,
            &cancellation
        )
        .unwrap_err(),
        "rip-up routing cancelled"
    );
}

#[test]
fn physical_timing_cancellation_reaches_stage_helpers() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let design = compile(source, Some("top")).unwrap();
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())
            .unwrap()
            .netlist;
    let placement = eda_pnr::place(&netlist).unwrap();
    let routing =
        eda_pnr::route_with_options(&netlist, &placement, &RouteOptions::default()).unwrap();
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        evaluate_physical_timing_iteration_cancellable(
            usize::MAX,
            &netlist,
            placement,
            routing,
            &InterconnectModel::default(),
            None,
            &cancellation,
        )
        .unwrap_err(),
        "physical timing analysis cancelled"
    );
    assert_eq!(
        run_physical_timing_flow_cancellable(
            &netlist,
            &PhysicalTimingFlowOptions::default(),
            &cancellation
        )
        .unwrap_err(),
        "physical timing analysis cancelled"
    );
}

#[test]
fn multi_corner_physical_timing_cancellation_is_structured() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let design = compile(source, Some("top")).unwrap();
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())
            .unwrap()
            .netlist;
    let placement = eda_pnr::place(&netlist).unwrap();
    let routing =
        eda_pnr::route_with_options(&netlist, &placement, &RouteOptions::default()).unwrap();
    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        run_multi_corner_physical_timing_flow_cancellable(
            &MultiCornerPhysicalTimingFlowOptions::default(),
            &cancellation
        )
        .unwrap_err(),
        "multi-corner physical timing analysis cancelled"
    );
    assert_eq!(
        evaluate_multi_corner_physical_timing_iteration_cancellable(
            usize::MAX,
            &[],
            &netlist,
            placement,
            routing,
            &InterconnectModel::default(),
            &cancellation,
        )
        .unwrap_err(),
        "multi-corner physical timing analysis cancelled"
    );
}

#[test]
fn rtl_power_uses_mapped_activity_and_cancels() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let vectors = vec![
        RtlSimulationVector {
            time_ns: 0,
            inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "0".into())]),
        },
        RtlSimulationVector {
            time_ns: 1,
            inputs: BTreeMap::from([("a".into(), "1".into()), ("b".into(), "1".into())]),
        },
    ];
    let result =
        analyze_rtl_power(source, Some("top"), &vectors, &PowerOptions::default()).unwrap();
    assert_eq!(result.report.vectors, 2);
    assert!(result.report.total_power_uw > 0.0);
    assert!(result.mapped_stats.lib_cells > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = analyze_rtl_power_cancellable(
        source,
        Some("top"),
        &vectors,
        &PowerOptions::default(),
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(error, "RTL power analysis cancelled");
}

#[test]
fn rtl_ir_uses_mapped_placement_power_and_cancels() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let vectors = vec![
        RtlSimulationVector {
            time_ns: 0,
            inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "0".into())]),
        },
        RtlSimulationVector {
            time_ns: 1,
            inputs: BTreeMap::from([("a".into(), "1".into()), ("b".into(), "1".into())]),
        },
    ];
    let ir_options = IrOptions {
        voltage_v: 1.2,
        vdd_ohm_per_site: 0.002,
        gnd_ohm_per_site: 0.002,
        max_total_drop_v: 0.05,
        max_rail_current_a: 0.05,
    };
    let result = analyze_rtl_ir(
        source,
        Some("top"),
        &vectors,
        &PowerOptions::default(),
        &ir_options,
    )
    .unwrap();
    assert_eq!(result.power.vectors, 2);
    assert!(result.power.total_power_uw > 0.0);
    assert!(!result.placement.cell_pos.is_empty());
    assert_eq!(result.ir.cells.len(), result.mapped_stats.cells);
    assert!(result.ir.total_current_a > 0.0);
    assert!(result.ir.worst_drop_v > 0.0);
    assert!(result.ir.worst_row.is_some());
    assert!(result.mapped_stats.lib_cells > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = analyze_rtl_ir_cancellable(
        source,
        Some("top"),
        &vectors,
        &PowerOptions::default(),
        &ir_options,
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(error, "RTL IR analysis cancelled");
}

#[test]
fn rtl_property_bmc_maps_and_checks_temporal_targets() {
    let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
    let properties = r#"
assert property (@(posedge clk) d |-> ##[2:2] q2);
assert property (@(posedge clk) d |-> ##[1:1] q2);
assume property (@(posedge clk) d || !d);
"#;
    let result = check_rtl_properties(source, Some("top"), properties, 3).unwrap();
    assert_eq!(result.results.len(), 2);
    assert!(!result.results[0].violated());
    assert!(result.results[1].violated());
    assert_eq!(result.assumptions.len(), 1);
    assert!(result.report.contains("ASSUMPTION CONSTRAINT applied"));
    assert!(result.report.contains("BOUNDED PASS"));
    assert!(result.report.contains("VIOLATED"));
    assert!(result.mapped_stats.seq > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = check_rtl_properties_cancellable(source, Some("top"), properties, 3, &cancellation)
        .unwrap_err();
    assert_eq!(error, "RTL property BMC cancelled");
    assert_eq!(
        check_rtl_properties(source, Some("top"), "assume property (d);", 2).unwrap_err(),
        "SVA input contains assumptions but no assert or cover targets"
    );
    assert_eq!(
        check_rtl_properties(source, Some("top"), properties, 0).unwrap_err(),
        "RTL property BMC requires a positive depth"
    );
}

#[test]
fn raw_sat_and_property_cancellation_surfaces_are_typed() {
    let mut cnf = Cnf::new();
    let input = cnf.add_var() as i32;
    cnf.add_clause(vec![input]);
    assert!(matches!(solve_cnf(&cnf), SatResult::Sat(_)));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        solve_cnf_cancellable(&cnf, &cancellation).unwrap_err(),
        "SAT solving cancelled"
    );

    let source = "module top(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
    let design = compile(source, Some("top")).unwrap();
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())
            .unwrap()
            .netlist;
    let assertions = parse_sva("assert property (@(posedge clk) d |-> q);").unwrap();
    assert_eq!(
        check_assertions_bounded_cancellable(&netlist, &assertions, 8, &cancellation).unwrap_err(),
        "temporal property BMC cancelled"
    );
    assert_eq!(
        bounded_model_check_cancellable(&netlist, "q", 8, &cancellation).unwrap_err(),
        "bounded model checking cancelled"
    );
    assert_eq!(
        bounded_model_check_expression_cancellable(
            &netlist,
            &BoolExpr::Signal("q".into()),
            8,
            &cancellation
        )
        .unwrap_err(),
        "bounded model checking cancelled"
    );
    assert_eq!(
        prove_safety_cancellable(&netlist, "q", 8, &cancellation).unwrap_err(),
        "k-induction proof cancelled"
    );
    assert_eq!(
        prove_safety_expression_cancellable(
            &netlist,
            &BoolExpr::Signal("q".into()),
            8,
            &cancellation
        )
        .unwrap_err(),
        "k-induction proof cancelled"
    );
    assert_eq!(
        check_netlists_equivalent_cancellable(&netlist, &netlist, &cancellation).unwrap_err(),
        "equivalence checking cancelled"
    );
}

#[test]
fn rtl_scan_insertion_adds_and_audits_one_chain() {
    let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
    let result = insert_rtl_scan(source, Some("top"), &ScanOptions::default()).unwrap();
    assert_eq!(result.scan.chain.len(), 2);
    assert_eq!(result.dft.sequential_cells, 2);
    assert_eq!(result.dft.scan_cells, 2);
    assert_eq!(result.dft.scan_chains, 1);
    assert!(result.dft.unscanned_sequential.is_empty());
    assert!(result.original_stats.seq > 0);
    assert!(result.scanned_stats.cells > result.original_stats.cells);
    assert!(result.report.contains("DFT scan report"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error =
        insert_rtl_scan_cancellable(source, Some("top"), &ScanOptions::default(), &cancellation)
            .unwrap_err();
    assert_eq!(error, "RTL scan insertion cancelled");
}

#[test]
fn rtl_cdc_reports_unsynchronized_crossing_and_cancels() {
    let source = r#"
module top(input clk_a, clk_b, d, output reg a_q, b_q);
always @(posedge clk_a) a_q <= d;
always @(posedge clk_b) b_q <= a_q;
endmodule
"#;
    let result = analyze_rtl_cdc(source, Some("top")).unwrap();
    assert_eq!(result.cdc.clocks.len(), 2);
    assert!(!result.cdc.crossings.is_empty());
    assert!(result.cdc.has_errors());
    assert!(!result.cdc.clean());
    assert!(result.report.contains("CDC report"));
    assert!(result.mapped_stats.seq >= 2);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = analyze_rtl_cdc_cancellable(source, Some("top"), &cancellation).unwrap_err();
    assert_eq!(error, "RTL CDC analysis cancelled");
}

#[test]
fn rtl_fault_analysis_grades_and_exports_stil() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let result = analyze_rtl_faults(source, Some("top"), &PatternOptions::default()).unwrap();
    assert!(result.faults.exhaustive);
    assert_eq!(result.faults.patterns.len(), 4);
    assert_eq!(result.faults.detected_faults, result.faults.total_faults);
    assert!(result.faults.undetected_faults.is_empty());
    assert_eq!(result.faults.coverage_percent, 100.0);
    assert!(result.report.contains("Fault report"));
    assert!(result.stil.contains("STIL 1.0"));
    assert!(result.mapped_stats.lib_cells > 0);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = analyze_rtl_faults_cancellable(
        source,
        Some("top"),
        &PatternOptions::default(),
        &cancellation,
    )
    .unwrap_err();
    assert_eq!(error, "RTL fault analysis cancelled");

    let sequential = r#"
module top(input clk, d, output reg q);
always @(posedge clk) q <= d;
endmodule
"#;
    let error =
        analyze_rtl_faults(sequential, Some("top"), &PatternOptions::default()).unwrap_err();
    assert!(
        error.contains("sequential stuck-at grading is unsupported"),
        "{error}"
    );
}

#[test]
fn rtl_simulation_returns_timed_output_vectors() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let vectors = vec![
        RtlSimulationVector {
            time_ns: 0,
            inputs: BTreeMap::from([("a".into(), "0".into()), ("b".into(), "1".into())]),
        },
        RtlSimulationVector {
            time_ns: 2,
            inputs: BTreeMap::from([("a".into(), "'b1".into()), ("b".into(), "0x1".into())]),
        },
    ];
    let result = simulate_rtl(source, Some("top"), &vectors).unwrap();
    assert_eq!(result.samples.len(), 2);
    assert_eq!(result.samples[0].outputs["y"], "0");
    assert_eq!(result.samples[1].outputs["y"], "1");
    assert_eq!(result.samples[1].time_ns, 2);
    assert!(!result.finished);

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = simulate_rtl_cancellable(source, Some("top"), &vectors, &cancellation).unwrap_err();
    assert_eq!(error, "RTL simulation cancelled");
}

#[test]
fn rtl_simulation_advances_sequential_time() {
    let source = r#"module top(input clk, output reg [1:0] q);
        initial q = 0;
        always @(posedge clk) q <= q + 1;
    endmodule"#;
    let vectors: Vec<_> = [0_u64, 1, 2, 3]
        .into_iter()
        .map(|time_ns| RtlSimulationVector {
            time_ns,
            inputs: BTreeMap::from([("clk".into(), (time_ns % 2).to_string())]),
        })
        .collect();
    let result = simulate_rtl(source, Some("top"), &vectors).unwrap();
    let outputs: Vec<_> = result
        .samples
        .iter()
        .map(|sample| sample.outputs["q"].as_str())
        .collect();
    assert_eq!(outputs, vec!["00", "01", "01", "10"]);
    assert_eq!(result.final_time_ns, 3);
}
