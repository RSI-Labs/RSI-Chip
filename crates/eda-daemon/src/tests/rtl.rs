use super::*;

#[test]
fn rpc_rtl_flow_returns_implementation_artifacts() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let request = RpcRequest::new(4, "rtlFlow").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["equivalent"], Json::Bool(true));
    let timing = fields["timing"].as_object().unwrap();
    assert!(timing["criticalPathDelayNs"]
        .as_number()
        .is_some_and(|value| value.is_finite() && *value >= 0.0));
    let physical = fields["physical"].as_object().unwrap();
    assert!(physical["placedCells"]
        .as_number()
        .is_some_and(|value| *value >= 1.0));
    assert!(physical["drcErrors"]
        .as_number()
        .is_some_and(|value| *value == 0.0));
    assert!(fields["verilog"].as_str().unwrap().contains("module"));
    assert!(fields["lef"].as_str().unwrap().contains("VERSION"));
}

#[test]
fn rpc_rtl_equivalence_returns_structured_formal_result() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(6, "rtlEquiv").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["equivalent"], Json::Bool(true));
    assert_eq!(fields["counterexample"], Json::Object(BTreeMap::new()));
    match &fields["notes"] {
        Json::Array(notes) => assert!(notes
            .iter()
            .any(|note| { matches!(note, Json::String(text) if text.contains("localized")) })),
        other => panic!("notes should be an array, got {other:?}"),
    }
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_equiv(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL equivalence cancelled");
}

#[test]
fn rpc_rtl_equivalence_views_compares_two_sources() {
    let source_a = "module top(input a, b, output y); assign y = a & b; endmodule";
    let equivalent_b = "module top(input a, b, output y); assign y = ~(~a | ~b); endmodule";
    let request = RpcRequest::new(66, "rtlEquivViews").with_params(Json::object([
        ("sourceA", Json::string(source_a)),
        ("sourceB", Json::string(equivalent_b)),
        ("topA", Json::string("top")),
        ("topB", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["equivalent"], Json::Bool(true));
    assert_eq!(fields["counterexample"], Json::Object(BTreeMap::new()));
    assert!(fields["viewA"].as_object().is_some());
    assert!(fields["viewB"].as_object().is_some());

    let changed_b = "module top(input a, b, output y); assign y = a | b; endmodule";
    let request = RpcRequest::new(67, "rtlEquivViews").with_params(Json::object([
        ("sourceA", Json::string(source_a)),
        ("sourceB", Json::string(changed_b)),
        ("topA", Json::string("top")),
        ("topB", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["equivalent"], Json::Bool(false));
    assert!(!fields["counterexample"].as_object().unwrap().is_empty());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_equiv_views(&request.params, &cancellation).unwrap_err(),
        "RTL view equivalence cancelled"
    );
}

#[test]
fn rpc_rtl_lvs_returns_structured_match_and_mismatch() {
    let reference = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(61, "rtlLvs").with_params(Json::object([
        ("reference", Json::string(reference)),
        ("layout", Json::string(reference)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["matches"], Json::Bool(true));
    match &fields["cellMatches"] {
        Json::Array(matches) => assert!(!matches.is_empty()),
        other => panic!("cell matches should be an array, got {other:?}"),
    }
    assert!(fields["report"].as_str().unwrap().contains("MATCH"));

    let changed = "module top(input a, b, output y); assign y = a | b; endmodule";
    let request = RpcRequest::new(62, "rtlLvs").with_params(Json::object([
        ("reference", Json::string(reference)),
        ("layout", Json::string(changed)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["matches"], Json::Bool(false));
    assert!(fields["report"].as_str().unwrap().contains("MISMATCH"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_lvs(&request.params, &cancellation).unwrap_err(),
        "RTL LVS cancelled"
    );
}

#[test]
fn rpc_rtl_power_intent_returns_structured_check() {
    let source = "module m(input a, sleep, output y); assign y = a & sleep; endmodule";
    let upf = r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {vdd}
connect_supply_net vss -ports {vss}
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
"#;
    let request = RpcRequest::new(63, "rtlPowerIntent").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("m")),
        ("upf", Json::string(upf)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["domainCount"], Json::from(1_u64));
    assert_eq!(fields["supplyNetCount"], Json::from(2_u64));
    assert_eq!(fields["hasErrors"], Json::Bool(false));
    assert!(fields["report"].as_str().unwrap().contains("UPF"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_power_intent(&request.params, &cancellation).unwrap_err(),
        "RTL power-intent check cancelled"
    );
}

#[test]
fn rpc_rtl_safety_returns_structured_k_induction_result() {
    let source = r#"
        module m(input clk, d, rst, output reg q, output bad);
            always @(posedge clk or posedge rst)
                if (rst) q <= 1'b0; else q <= d;
            assign bad = q & rst;
        endmodule
    "#;
    let request = RpcRequest::new(64, "rtlSafety").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("m")),
        ("badNet", Json::string("bad")),
        ("depth", Json::Number(8.0)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["status"], Json::string("proved"));
    assert_eq!(fields["inductive"], Json::Bool(true));
    assert_eq!(fields["hasCounterexample"], Json::Bool(false));
    assert_eq!(fields["failedInvariant"], Json::Null);
    assert!(fields["depthChecked"].as_number().unwrap() > &0.0);
    assert!(fields["report"].as_str().unwrap().contains("induction"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_safety(&request.params, &cancellation).unwrap_err(),
        "RTL safety proof cancelled"
    );
}

#[test]
fn rpc_rtl_physical_timing_returns_spef_and_sdf() {
    let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
    let request = RpcRequest::new(65, "rtlPhysicalTiming").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "physicalTimingOptions",
            Json::object([
                ("tracksPerChannel", Json::Number(64.0)),
                ("iterations", Json::Number(1.0)),
                ("placementPasses", Json::Number(2.0)),
                ("routingPasses", Json::Number(2.0)),
                ("wireResistanceOhmPerSite", Json::Number(0.05)),
                ("wireCapacitancePfPerSite", Json::Number(0.002)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert!(fields["baselineDelayNs"]
        .as_number()
        .is_some_and(|value| value.is_finite() && *value > 0.0));
    assert!(fields["bestDelayNs"]
        .as_number()
        .is_some_and(|value| value.is_finite() && *value > 0.0));
    assert!(fields["wirelength"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(fields["rcNets"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert_eq!(fields["drcErrors"], Json::from(0_u64));
    assert!(fields["spef"].as_str().unwrap().contains("*SPEF"));
    assert!(fields["sdf"].as_str().unwrap().contains("(SDF"));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("Physical timing flow"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_physical_timing(&request.params, &cancellation).unwrap_err(),
        "RTL physical timing analysis cancelled"
    );
}

#[test]
fn rpc_rtl_post_route_power_ir_returns_multi_scenario_results() {
    let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
    let scenarios = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_scenario_property post_route.voltage_v 0.90
set_scenario_property post_route.vdd_ohm_per_site 0.05
set_scenario_property post_route.gnd_ohm_per_site 0.06
set_scenario_property post_route.max_total_drop_v 0.04
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property post_route.voltage_v 1.10
set_scenario_property post_route.vdd_ohm_per_site 0.04
set_scenario_property post_route.gnd_ohm_per_site 0.05
set_scenario_property post_route.max_total_drop_v 0.05
endscenario
"#;
    let request = RpcRequest::new(74, "rtlPostRoutePowerIr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        (
            "physicalTimingOptions",
            Json::object([
                ("tracksPerChannel", Json::Number(64.0)),
                ("iterations", Json::Number(1.0)),
                ("placementPasses", Json::Number(2.0)),
                ("routingPasses", Json::Number(2.0)),
            ]),
        ),
        (
            "vectors",
            Json::Array(vec![
                Json::object([
                    ("timeNs", Json::Number(0.0)),
                    (
                        "inputs",
                        Json::object([
                            ("a", Json::string("0")),
                            ("b", Json::string("0")),
                            ("c", Json::string("0")),
                        ]),
                    ),
                ]),
                Json::object([
                    ("timeNs", Json::Number(1.0)),
                    (
                        "inputs",
                        Json::object([
                            ("a", Json::string("1")),
                            ("b", Json::string("1")),
                            ("c", Json::string("1")),
                        ]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(2_u64));
    assert_eq!(fields["vectorCount"], Json::from(2_u64));
    assert!(fields["worstPowerScenario"].as_str().is_some());
    assert!(fields["worstDropScenario"].as_str().is_some());
    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    assert_eq!(corners.len(), 2);
    for corner in corners {
        let corner = corner.as_object().unwrap();
        assert!(corner["voltageV"].as_number().unwrap() > &0.0);
        assert!(corner["totalPowerUw"].as_number().unwrap() > &0.0);
        assert!(corner["dynamicPowerUw"].as_number().unwrap() > &0.0);
        assert!(corner["staticPowerUw"].as_number().unwrap() > &0.0);
        assert!(corner["parasiticDynamicPowerUw"].as_number().unwrap() >= &0.0);
        assert!(corner["effectiveCapacitancePf"].as_number().unwrap() > &0.0);
        assert!(corner["effectiveCapacitanceNets"].as_number().unwrap() > &0.0);
        assert!(corner["totalCurrentA"].as_number().unwrap() > &0.0);
        assert!(corner["worstDropV"].as_number().unwrap() >= &0.0);
        assert!(corner["report"]
            .as_str()
            .unwrap()
            .contains("Post-route power/IR"));
    }
    let slow = corners[0].as_object().unwrap();
    let fast = corners[1].as_object().unwrap();
    assert_eq!(slow["scenario"], Json::string("slow"));
    assert_eq!(fast["scenario"], Json::string("fast"));
    assert_eq!(slow["voltageV"], Json::Number(0.9));
    assert_eq!(fast["voltageV"], Json::Number(1.1));
    assert!(fields["physicalTimingReport"]
        .as_str()
        .unwrap()
        .contains("Physical timing flow"));
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_post_route_power_ir(&request.params, &cancellation).unwrap_err(),
        "RTL post-route power/IR analysis cancelled"
    );
}

#[test]
fn rpc_rtl_post_route_power_mode_ir_returns_mode_matrix() {
    let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
    let scenarios = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_scenario_property post_route.voltage_v 0.90
set_scenario_property post_route.vdd_ohm_per_site 0.05
set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property post_route.voltage_v 1.10
set_scenario_property post_route.vdd_ohm_per_site 0.04
set_scenario_property post_route.gnd_ohm_per_site 0.04
endscenario
"#;
    let upf = r#"
upf_version 2.1
set_design_top top
create_power_domain TOP -elements {top}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 0.025
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#;
    let request = RpcRequest::new(75, "rtlPostRoutePowerModeIr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        ("upf", Json::string(upf)),
        (
            "physicalTimingOptions",
            Json::object([
                ("tracksPerChannel", Json::Number(64.0)),
                ("iterations", Json::Number(1.0)),
                ("placementPasses", Json::Number(2.0)),
                ("routingPasses", Json::Number(2.0)),
                ("wireResistanceOhmPerSite", Json::Number(0.05)),
                ("wireCapacitancePfPerSite", Json::Number(0.002)),
            ]),
        ),
        (
            "powerModeOptions",
            Json::object([
                ("multilayerPowerMesh", Json::Bool(true)),
                ("rows", Json::Number(2.0)),
                ("columns", Json::Number(2.0)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(2_u64));
    assert!(fields["worstPowerScenario"].as_str().is_some());
    assert!(fields["worstPowerMode"].as_str().is_some());
    assert!(fields["worstDropScenario"].as_str().is_some());
    assert!(fields["worstDropMode"].as_str().is_some());
    assert!(fields["worstEmLifetimeScenario"].as_str().is_some());
    assert!(fields["worstEmLifetimeMode"].as_str().is_some());
    assert_eq!(fields["worstEmLifetimeHours"], Json::Null);

    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    assert_eq!(corners.len(), 2);
    for corner in corners {
        let corner = corner.as_object().unwrap();
        assert_eq!(corner["modeCount"], Json::from(2_u64));
        assert!(corner["worstPowerMode"].as_str().is_some());
        assert!(corner["worstDropMode"].as_str().is_some());
        let Json::Array(modes) = &corner["modes"] else {
            panic!("modes should be an array");
        };
        assert_eq!(modes.len(), 2);
        let active = modes[0].as_object().unwrap();
        let idle = modes[1].as_object().unwrap();
        assert_eq!(active["powerMode"], Json::string("active"));
        assert_eq!(idle["powerMode"], Json::string("idle"));
        assert!(active["totalPowerUw"].as_number().unwrap() > &0.0);
        assert!(idle["totalPowerUw"].as_number().unwrap() > &0.0);
        assert!(
            active["totalPowerUw"].as_number().unwrap() > idle["totalPowerUw"].as_number().unwrap()
        );
        assert!(active["worstDropV"].as_number().unwrap() >= &0.0);
        assert!(idle["worstDropV"].as_number().unwrap() >= &0.0);
        assert_eq!(active["worstEmLifetimeHours"], Json::Null);
        assert_eq!(idle["worstEmLifetimeHours"], Json::Null);
        assert_eq!(active["worstEmLifetimeUnbounded"], Json::Bool(true));
        assert_eq!(idle["worstEmLifetimeUnbounded"], Json::Bool(true));
        assert!(corner["report"]
            .as_str()
            .unwrap()
            .contains("UPF power-mode IR report"));
    }
    assert!(fields["physicalTimingReport"]
        .as_str()
        .unwrap()
        .contains("Physical timing flow"));
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_post_route_power_mode_ir(&request.params, &cancellation).unwrap_err(),
        "RTL post-route power-mode IR analysis cancelled"
    );
}

#[test]
fn rpc_rtl_post_route_power_ir_accepts_vcd_activity() {
    let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
    let scenarios = r#"
scenario nominal
create_clock -name clk -period 4.0 [get_ports a]
set_scenario_property post_route.voltage_v 1.00
set_scenario_property post_route.vdd_ohm_per_site 0.05
set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
"#;
    let vcd = r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # c $end
$var wire 1 $ y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
0$
#10
1!
1"
0#
1$
#20
0!
1"
1#
1$
"#;
    let request = RpcRequest::new(80, "rtlPostRoutePowerIr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        ("vcd", Json::string(vcd)),
        (
            "physicalTimingOptions",
            Json::object([
                ("tracksPerChannel", Json::Number(64.0)),
                ("iterations", Json::Number(1.0)),
                ("placementPasses", Json::Number(2.0)),
                ("routingPasses", Json::Number(2.0)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(1_u64));
    assert_eq!(fields["vectorCount"], Json::from(3_u64));
    assert_eq!(fields["stimulus"], Json::string("vcd"));
    assert_eq!(fields["worstPowerScenario"], Json::string("nominal"));
    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    let corner = corners[0].as_object().unwrap();
    assert!(corner["dynamicPowerUw"]
        .as_number()
        .is_some_and(|v| *v > 0.0));
    assert!(corner["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));

    let ambiguous = Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        ("vcd", Json::string(vcd)),
        ("vectors", Json::Array(Vec::new())),
    ]);
    assert_eq!(
        rtl_post_route_power_ir(&ambiguous, &FlowCancellation::new()).unwrap_err(),
        "RPC parameters 'vcd' and 'vectors' are mutually exclusive"
    );

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_post_route_power_ir(&request.params, &cancellation).unwrap_err(),
        "RTL post-route power/IR analysis cancelled"
    );
}

#[test]
fn rpc_rtl_post_route_power_mode_ir_accepts_vcd_activity() {
    let source = "module top(input a, b, c, output y); assign y = (a & b) | c; endmodule";
    let scenarios = r#"
scenario nominal
create_clock -name clk -period 4.0 [get_ports a]
set_scenario_property post_route.voltage_v 1.00
set_scenario_property post_route.vdd_ohm_per_site 0.05
set_scenario_property post_route.gnd_ohm_per_site 0.05
endscenario
"#;
    let upf = r#"
upf_version 2.1
set_design_top top
create_power_domain TOP -elements {top}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 0.025
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#;
    let vcd = r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " b $end
$var wire 1 # c $end
$var wire 1 $ y $end
$upscope $end
$enddefinitions $end
#0
0!
0"
0#
0$
#10
1!
1"
0#
1$
#20
0!
1"
1#
1$
"#;
    let request = RpcRequest::new(81, "rtlPostRoutePowerModeIr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        ("upf", Json::string(upf)),
        ("vcd", Json::string(vcd)),
        (
            "physicalTimingOptions",
            Json::object([
                ("tracksPerChannel", Json::Number(64.0)),
                ("iterations", Json::Number(1.0)),
                ("placementPasses", Json::Number(2.0)),
                ("routingPasses", Json::Number(2.0)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(1_u64));
    assert_eq!(fields["vectorCount"], Json::from(3_u64));
    assert_eq!(fields["stimulus"], Json::string("vcd"));
    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    let corner = corners[0].as_object().unwrap();
    assert_eq!(corner["modeCount"], Json::from(2_u64));
    let Json::Array(modes) = &corner["modes"] else {
        panic!("modes should be an array");
    };
    let active = modes[0].as_object().unwrap();
    let idle = modes[1].as_object().unwrap();
    assert!(active["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));
    assert!(idle["totalPowerUw"].as_number().is_some_and(|v| *v > 0.0));
    assert!(
        active["totalPowerUw"].as_number().unwrap() > idle["totalPowerUw"].as_number().unwrap()
    );

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_post_route_power_mode_ir(&request.params, &cancellation).unwrap_err(),
        "RTL post-route power-mode IR analysis cancelled"
    );
}

#[test]
fn rpc_rtl_sta_returns_structured_timing() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(8, "rtlSta").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert!(fields["criticalPathDelayNs"]
        .as_number()
        .is_some_and(|value| value.is_finite() && *value >= 0.0));
    assert_eq!(fields["caseAnalysisNets"], Json::Object(BTreeMap::new()));
    assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
        stats
            .get("libCells")
            .and_then(Json::as_number)
            .is_some_and(|value| *value > 0.0)
    }));

    let strict = RpcRequest::new(120, "rtlPnr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "drcOptions",
            Json::object([("maxAntennaRatio", Json::Number(0.001))]),
        ),
    ]));
    let strict_response = dispatch(&strict).result.unwrap();
    let strict_fields = strict_response.as_object().unwrap();
    let strict_drc = strict_fields["drc"].as_object().unwrap();
    assert_eq!(strict_drc["errors"], Json::from(0_u64));
    assert!(strict_drc["warnings"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    let Json::Array(strict_violations) = &strict_drc["violations"] else {
        panic!("strict PnR DRC violations should be an array");
    };
    assert!(strict_violations
        .iter()
        .any(|violation| { violation.as_object().unwrap()["code"] == Json::string("ANT-001") }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_sta(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL timing analysis cancelled");
}

#[test]
fn rpc_rtl_mmmc_timing_returns_structured_scenarios() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let scenarios = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_scenario_property voltage_v 0.90
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property voltage_v 1.10
endscenario
"#;
    let request = RpcRequest::new(68, "rtlMmmcTiming").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(2_u64));
    assert_eq!(fields["worstScenario"], Json::string("fast"));
    assert!(fields["worstSetupSlackNs"].as_number().is_some());
    assert_eq!(fields["worstHoldSlackNs"], Json::Null);
    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    assert_eq!(corners.len(), 2);
    for corner in corners {
        let corner = corner.as_object().unwrap();
        assert_eq!(corner["clock"], Json::string("clk"));
        assert!(corner["periodNs"].as_number().unwrap() > &0.0);
        assert!(corner["criticalPathDelayNs"].as_number().unwrap() > &0.0);
        assert!(corner["worstSlackNs"].as_number().is_some());
    }
    let properties = fields["properties"].as_object().unwrap();
    let fast = properties["fast"].as_object().unwrap();
    assert_eq!(fast["voltage_v"], Json::string("1.10"));
    assert!(fields["report"].as_str().unwrap().contains("[fast]"));
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_mmmc_timing(&request.params, &cancellation).unwrap_err(),
        "RTL MMMC timing analysis cancelled"
    );
}

#[test]
fn rpc_rtl_mmmc_timing_paths_returns_structured_paths() {
    let source = "module top(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule";
    let scenarios = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports clk]
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports clk]
endscenario
"#;
    let request = RpcRequest::new(73, "rtlMmmcTimingPaths").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
        ("maxPaths", Json::Number(5.0)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(2_u64));
    assert_eq!(fields["maxPaths"], Json::from(5_u64));
    assert_eq!(fields["worstScenario"], Json::string("slow"));

    let Json::Array(setup_paths) = &fields["setupPaths"] else {
        panic!("setup paths should be an array");
    };
    assert!(!setup_paths.is_empty());
    let setup = setup_paths[0].as_object().unwrap();
    assert_eq!(setup["scenario"], Json::string("fast"));
    let endpoint = setup["endpoint"].as_object().unwrap();
    assert_eq!(endpoint["kind"], Json::string("outputPort"));
    assert_eq!(endpoint["name"], Json::string("q"));
    let path = setup["path"].as_object().unwrap();
    assert_eq!(path["kind"], Json::string("sequentialToOutput"));
    let launch = path["launch"].as_object().unwrap();
    assert_eq!(launch["kind"], Json::string("sequentialCell"));
    assert_eq!(launch["name"], Json::string("u1_dff"));
    assert!(matches!(&path["cellNames"], Json::Array(values) if !values.is_empty()));
    assert!(matches!(&path["netNames"], Json::Array(values) if !values.is_empty()));
    assert!(matches!(&path["arcs"], Json::Array(values) if !values.is_empty()));
    let Json::Array(corner_slacks) = &setup["cornerSlacks"] else {
        panic!("corner slacks should be an array");
    };
    assert_eq!(corner_slacks.len(), 2);
    assert!(corner_slacks
        .iter()
        .all(|slack| slack.as_object().unwrap()["slackNs"].as_number().is_some()));

    let Json::Array(hold_paths) = &fields["holdPaths"] else {
        panic!("hold paths should be an array");
    };
    assert!(!hold_paths.is_empty());
    assert!(hold_paths[0].as_object().unwrap().contains_key("holdNs"));
    assert_eq!(fields["resetPaths"], Json::Array(Vec::new()));
    assert!(fields["setupReport"]
        .as_str()
        .unwrap()
        .contains("Multi-corner setup timing path report"));
    assert!(fields["holdReport"]
        .as_str()
        .unwrap()
        .contains("Multi-corner hold timing path report"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_mmmc_timing_paths(&request.params, &cancellation).unwrap_err(),
        "RTL MMMC timing paths cancelled"
    );
}

#[test]
fn rpc_rtl_mmmc_timing_violations_applies_waivers() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let scenarios = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
    let request = RpcRequest::new(76, "rtlMmmcTimingViolations").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("scenarios", Json::string(scenarios)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["scenarioCount"], Json::from(2_u64));
    assert_eq!(fields["activeViolationCount"], Json::from(1_u64));
    assert_eq!(fields["hasActiveViolations"], Json::Bool(true));
    let scenario_reports = fields["scenarios"].as_object().unwrap();
    let slow = scenario_reports["slow"].as_object().unwrap();
    let fast = scenario_reports["fast"].as_object().unwrap();
    assert_eq!(slow["activeViolationCount"], Json::from(0_u64));
    assert_eq!(fast["activeViolationCount"], Json::from(1_u64));
    let Json::Array(active) = &fast["activeViolations"] else {
        panic!("fast active violations should be an array");
    };
    let violation = active[0].as_object().unwrap();
    assert_eq!(violation["scenario"], Json::string("fast"));
    assert_eq!(violation["kind"], Json::string("output"));
    assert_eq!(violation["endpoint"], Json::string("y"));
    assert!(violation["slackNs"].as_number().unwrap() < &0.0);
    let Json::Array(summaries) = &fields["summaries"] else {
        panic!("summaries should be an array");
    };
    assert_eq!(summaries.len(), 1);
    let summary = summaries[0].as_object().unwrap();
    assert_eq!(summary["key"], Json::string("output:y"));
    assert_eq!(summary["worstScenario"], Json::string("fast"));
    assert!(fields["hierarchySummaries"]
        .as_object()
        .unwrap()
        .contains_key("top"));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("Multi-corner timing violation report"));

    let waived_request =
        RpcRequest::new(77, "rtlMmmcTimingViolations").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("scenarios", Json::string(scenarios)),
            (
                "waivers",
                Json::string("waiver \"fast-only\" \"fast\" \"y\" \"-1.0\"\n"),
            ),
            ("actor", Json::string("timing-owner")),
        ]));
    let response = dispatch(&waived_request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["activeViolationCount"], Json::from(0_u64));
    assert_eq!(fields["hasActiveViolations"], Json::Bool(false));
    let scenario_reports = fields["scenarios"].as_object().unwrap();
    let fast = scenario_reports["fast"].as_object().unwrap();
    assert_eq!(fast["activeViolationCount"], Json::from(0_u64));
    assert_eq!(fast["waivedViolationCount"], Json::from(1_u64));
    let waived = fast["waivedViolations"].as_object().unwrap();
    let waived_entry = waived["fast:output:y"].as_object().unwrap();
    assert_eq!(waived_entry["waiver"], Json::string("fast-only"));
    assert!(fast["waiverAuditReport"]
        .as_str()
        .unwrap()
        .contains("by timing-owner"));
    assert_eq!(fields["summaries"], Json::Array(Vec::new()));
    assert_eq!(fields["hierarchySummaries"], Json::Object(BTreeMap::new()));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_mmmc_timing_violations(&request.params, &cancellation).unwrap_err(),
        "RTL MMMC timing violations cancelled"
    );
}

#[test]
fn rpc_rtl_incremental_mmmc_timing_violations_reports_lifecycle() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let old = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
    let new = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
create_clock -name clk -period 0.1 [get_ports a]
endscenario
scenario typical
create_clock -name clk -period 3.0 [get_ports a]
endscenario
"#;
    let request =
        RpcRequest::new(78, "rtlIncrementalMmmcTimingViolations").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("old", Json::string(old)),
            ("new", Json::string(new)),
        ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["oldScenarioCount"], Json::from(2_u64));
    assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
    assert_eq!(fields["activeViolationCount"], Json::from(1_u64));
    let update = fields["update"].as_object().unwrap();
    assert_eq!(update["addedScenarioCount"], Json::from(1_u64));
    assert_eq!(update["removedScenarioCount"], Json::from(0_u64));
    assert_eq!(update["scenarioDiffCount"], Json::from(2_u64));
    let scenario_diffs = update["scenarioDiffs"].as_object().unwrap();
    let fast = scenario_diffs["fast"].as_object().unwrap();
    assert_eq!(fast["degradedCount"], Json::from(1_u64));
    let Json::Array(degraded) = &fast["degraded"] else {
        panic!("fast degradation should be an array");
    };
    let degradation = degraded[0].as_object().unwrap();
    assert_eq!(degradation["id"], Json::string("fast:output:y"));
    let old_slack = degradation["oldSlackNs"].as_number().copied().unwrap();
    let new_slack = degradation["newSlackNs"].as_number().copied().unwrap();
    assert!((old_slack + 0.2).abs() < 1e-12);
    assert!((new_slack + 0.3).abs() < 1e-12);
    let cross_corner = update["crossCorner"].as_object().unwrap();
    assert_eq!(cross_corner["degradedCount"], Json::from(1_u64));
    let Json::Array(cross_degraded) = &cross_corner["degraded"] else {
        panic!("cross-corner degradation should be an array");
    };
    let cross_change = cross_degraded[0].as_object().unwrap();
    assert_eq!(cross_change["key"], Json::string("output:y"));
    let scenarios = fields["scenarios"].as_object().unwrap();
    assert!(scenarios.contains_key("typical"));
    let final_fast = scenarios["fast"].as_object().unwrap();
    assert_eq!(final_fast["activeViolationCount"], Json::from(1_u64));
    assert!(fields["baselineReport"]
        .as_str()
        .unwrap()
        .contains("generation 1"));
    assert!(fields["updateReport"]
        .as_str()
        .unwrap()
        .contains("cross-corner: added=0, removed=0, improved=0, degraded=1"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_incremental_mmmc_timing_violations(&request.params, &cancellation).unwrap_err(),
        "RTL incremental MMMC timing violations cancelled"
    );
}

#[test]
fn rpc_rtl_incremental_mmmc_timing_violations_preserves_waivers() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let old = r#"
scenario fast
create_clock -name clk -period 0.2 [get_ports a]
endscenario
"#;
    let new = r#"
scenario fast
create_clock -name clk -period 0.1 [get_ports a]
endscenario
"#;
    let request =
        RpcRequest::new(79, "rtlIncrementalMmmcTimingViolations").with_params(Json::object([
            ("source", Json::string(source)),
            ("top", Json::string("top")),
            ("old", Json::string(old)),
            ("new", Json::string(new)),
            (
                "waivers",
                Json::string("waiver \"fast-only\" \"fast\" \"y\" \"-1.0\"\n"),
            ),
            ("actor", Json::string("timing-owner")),
        ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["activeViolationCount"], Json::from(0_u64));
    assert_eq!(fields["hasActiveViolations"], Json::Bool(false));
    let final_scenarios = fields["scenarios"].as_object().unwrap();
    let fast = final_scenarios["fast"].as_object().unwrap();
    assert_eq!(fast["activeViolationCount"], Json::from(0_u64));
    assert_eq!(fast["waivedViolationCount"], Json::from(1_u64));
    assert!(fast["waiverAuditReport"]
        .as_str()
        .unwrap()
        .contains("by timing-owner"));
    let update = fields["update"].as_object().unwrap();
    let scenario_diffs = update["scenarioDiffs"].as_object().unwrap();
    let fast_diff = scenario_diffs["fast"].as_object().unwrap();
    assert_eq!(fast_diff["degradedCount"], Json::from(1_u64));
    let cross_corner = update["crossCorner"].as_object().unwrap();
    assert_eq!(cross_corner["degradedCount"], Json::from(0_u64));
    assert!(fields["updateReport"]
        .as_str()
        .unwrap()
        .contains("waived violations: 1"));
}

#[test]
fn rpc_timing_scenario_diff_returns_structured_changes() {
    let old = r#"
scenario removed
create_clock -name clk -period 5.0 [get_ports a]
endscenario
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_input_delay 0.2 -clock clk [get_ports a]
set_scenario_property voltage_v 0.90
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property voltage_v 1.00
endscenario
"#;
    let new = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_input_delay 0.8 -clock clk [get_ports a]
set_scenario_property voltage_v 0.90
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property voltage_v 1.10
endscenario
scenario ss
create_clock -name clk -period 1.0 [get_ports a]
endscenario
"#;
    let request = RpcRequest::new(69, "timingScenarioDiff").with_params(Json::object([
        ("old", Json::string(old)),
        ("new", Json::string(new)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["oldScenarioCount"], Json::from(3_u64));
    assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
    assert_eq!(fields["added"], Json::Array(vec![Json::string("ss")]));
    assert_eq!(
        fields["removed"],
        Json::Array(vec![Json::string("removed")])
    );
    assert_eq!(
        fields["changed"],
        Json::Array(vec![Json::string("fast"), Json::string("slow"),])
    );
    assert_eq!(
        fields["timingChanged"],
        Json::Array(vec![Json::string("slow")])
    );

    let constraints = fields["constraints"].as_object().unwrap();
    let slow = constraints["slow"].as_object().unwrap();
    assert_eq!(
        slow["changedInputPorts"],
        Json::Array(vec![Json::string("a")])
    );
    assert_eq!(slow["baseRecomputeRequired"], Json::Bool(true));
    assert_eq!(slow["constrainedRecomputeRequired"], Json::Bool(true));
    assert!(!constraints.contains_key("fast"));

    let properties = fields["properties"].as_object().unwrap();
    let fast = properties["fast"].as_object().unwrap();
    let changed = fast["changed"].as_object().unwrap();
    let voltage = changed["voltage_v"].as_object().unwrap();
    assert_eq!(voltage["old"], Json::string("1.00"));
    assert_eq!(voltage["new"], Json::string("1.10"));

    let report = fields["report"].as_str().unwrap();
    assert!(report.contains("added [ss]"), "{report}");
    assert!(report.contains("removed [removed]"), "{report}");
    assert!(report.contains("timing changed [slow]"), "{report}");

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        timing_scenario_diff(&request.params, &cancellation).unwrap_err(),
        "timing scenario diff cancelled"
    );
}

#[test]
fn rpc_rtl_signal_quality_reports_structured_violations() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(71, "rtlSignalQuality").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "qualityOptions",
            Json::object([
                ("inputTransitionNs", Json::Number(0.05)),
                ("maxTransitionNs", Json::Number(10.0)),
                ("maxCapacitancePf", Json::Number(0.1)),
                ("maxFanout", Json::Number(16.0)),
                (
                    "scopedMaxTransitionNs",
                    Json::Array(vec![Json::object([
                        ("kind", Json::string("port")),
                        ("name", Json::string("y")),
                        ("limit", Json::Number(10.0)),
                    ])]),
                ),
                (
                    "inputTransitions",
                    Json::Array(vec![Json::object([
                        ("kind", Json::string("port")),
                        ("name", Json::string("a")),
                        ("limit", Json::Number(0.05)),
                    ])]),
                ),
                (
                    "extraCapacitancePf",
                    Json::object([("y", Json::Number(0.2))]),
                ),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["hasViolations"], Json::Bool(true));
    assert!(fields["violationCount"].as_number().unwrap() > &0.0);
    assert!(fields["transitionsChecked"].as_number().unwrap() > &0.0);
    assert!(fields["capacitancesChecked"].as_number().unwrap() > &0.0);
    assert!(fields["fanoutsChecked"].as_number().unwrap() > &0.0);
    let Json::Array(violations) = &fields["violations"] else {
        panic!("violations should be an array");
    };
    let capacitance = violations
        .iter()
        .map(|violation| violation.as_object().unwrap())
        .find(|violation| violation["kind"] == Json::string("maxCapacitance"))
        .unwrap();
    assert_eq!(capacitance["netName"], Json::string("y"));
    assert!(capacitance["actual"].as_number().unwrap() > &0.1);
    assert_eq!(capacitance["limit"], Json::Number(0.1));
    assert!(capacitance["message"].as_str().unwrap().contains("y"));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("Signal-quality report"));
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_signal_quality(&request.params, &cancellation).unwrap_err(),
        "RTL signal-quality analysis cancelled"
    );
}

#[test]
fn rpc_rtl_incremental_mmmc_timing_reuses_unchanged_scenarios() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let old = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_input_delay 0.2 -clock clk [get_ports a]
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property voltage_v 1.00
endscenario
"#;
    let new = r#"
scenario slow
create_clock -name clk -period 4.0 [get_ports a]
set_input_delay 0.8 -clock clk [get_ports a]
endscenario
scenario fast
create_clock -name clk -period 2.0 [get_ports a]
set_scenario_property voltage_v 1.10
endscenario
scenario ss
create_clock -name clk -period 1.0 [get_ports a]
endscenario
"#;
    let request = RpcRequest::new(70, "rtlIncrementalMmmcTiming").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("old", Json::string(old)),
        ("new", Json::string(new)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["oldScenarioCount"], Json::from(2_u64));
    assert_eq!(fields["newScenarioCount"], Json::from(3_u64));
    assert_eq!(fields["baselineWorstScenario"], Json::string("fast"));
    assert_eq!(fields["finalWorstScenario"], Json::string("ss"));
    assert_eq!(
        fields["timingChanged"],
        Json::Array(vec![Json::string("slow")])
    );
    assert_eq!(fields["reused"], Json::Array(vec![Json::string("fast")]));

    let updates = fields["updates"].as_object().unwrap();
    assert_eq!(updates.len(), 1);
    let slow = updates["slow"].as_object().unwrap();
    assert_eq!(
        slow["constraintDiff"].as_object().unwrap()["changedInputPorts"],
        Json::Array(vec![Json::string("a")])
    );
    assert!(slow["totalCells"].as_number().unwrap() > &0.0);
    assert!(slow["criticalPathDelayNs"].as_number().is_some());

    let Json::Array(corners) = &fields["corners"] else {
        panic!("corners should be an array");
    };
    assert_eq!(corners.len(), 3);
    assert!(fields["diffReport"]
        .as_str()
        .unwrap()
        .contains("timing changed [slow]"));
    assert!(fields["finalReport"].as_str().unwrap().contains("[ss]"));
    assert!(fields["mappedStats"].as_object().is_some());

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_incremental_mmmc_timing(&request.params, &cancellation).unwrap_err(),
        "RTL incremental MMMC timing cancelled"
    );
}

#[test]
fn rpc_timing_waiver_audit_returns_events_and_active_rules() {
    let audit = r#"
audit 1 added 1 "frontend"
waiver "u1" "fast" "y" "-1.0"
instance "top.u1.**"
end
audit 2 removed 2 "reviewer"
waiver "u1" "fast" "y" "-1.0"
instance "top.u1.**"
end
audit 3 added 2 "frontend"
waiver "u2" "fast" "y" "-1.0"
instance "top.u2"
end
"#;
    let request = RpcRequest::new(72, "timingWaiverAudit")
        .with_params(Json::object([("audit", Json::string(audit))]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["eventCount"], Json::from(3_u64));
    assert_eq!(fields["activeWaiverCount"], Json::from(1_u64));

    let Json::Array(events) = &fields["events"] else {
        panic!("events should be an array");
    };
    assert_eq!(events.len(), 3);
    let first = events[0].as_object().unwrap();
    assert_eq!(first["sequence"], Json::from(1_u64));
    assert_eq!(first["action"], Json::string("added"));
    assert_eq!(first["waiverId"], Json::string("u1"));
    assert_eq!(first["changedBy"], Json::string("frontend"));
    let first_waiver = first["waiver"].as_object().unwrap();
    assert_eq!(first_waiver["scenarioPattern"], Json::string("fast"));
    assert_eq!(first_waiver["endpointPattern"], Json::string("y"));
    assert_eq!(first_waiver["instancePattern"], Json::string("top.u1.**"));
    assert_eq!(first_waiver["slackFloorNs"], Json::Number(-1.0));

    let Json::Array(active) = &fields["activeWaivers"] else {
        panic!("active waivers should be an array");
    };
    assert_eq!(active.len(), 1);
    let active_waiver = active[0].as_object().unwrap();
    assert_eq!(active_waiver["id"], Json::string("u2"));
    assert_eq!(active_waiver["instancePattern"], Json::string("top.u2"));

    let hierarchy = fields["hierarchy"].as_object().unwrap();
    let first_scope = hierarchy["top.u1.**"].as_object().unwrap();
    assert_eq!(first_scope["events"], Json::from(2_u64));
    assert_eq!(first_scope["addedEvents"], Json::from(1_u64));
    assert_eq!(first_scope["removedEvents"], Json::from(1_u64));
    assert_eq!(first_scope["activeWaiverIds"], Json::Array(Vec::new()));
    let second_scope = hierarchy["top.u2"].as_object().unwrap();
    assert_eq!(
        second_scope["activeWaiverIds"],
        Json::Array(vec![Json::string("u2")])
    );

    assert!(fields["report"].as_str().unwrap().contains("3 event(s)"));
    assert!(fields["hierarchyReport"]
        .as_str()
        .unwrap()
        .contains("'top.u1.**': events=2, added=1, removed=1, active=0"));
    assert!(fields["hierarchyReport"]
        .as_str()
        .unwrap()
        .contains("'top.u2': events=1, added=1, removed=0, active=1"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        timing_waiver_audit(&request.params, &cancellation).unwrap_err(),
        "timing waiver audit cancelled"
    );
}

#[test]
fn rpc_rtl_pnr_returns_physical_views() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(9, "rtlPnr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    let placement = fields["placement"].as_object().unwrap();
    assert!(placement["placedCells"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    let drc = fields["drc"].as_object().unwrap();
    assert_eq!(drc["errors"], Json::Number(0.0));
    assert!(fields["lef"].as_str().unwrap().contains("VERSION"));
    assert!(fields["def"].as_str().unwrap().contains("DESIGN"));
    assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
        stats
            .get("libCells")
            .and_then(Json::as_number)
            .is_some_and(|value| *value > 0.0)
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_pnr(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL PnR cancelled");
}

#[test]
fn rpc_rtl_antenna_repair_returns_structured_result() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(118, "rtlAntennaRepair").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "antennaOptions",
            Json::object([
                ("patchName", Json::string("remote_antenna_fix")),
                ("maxDiodesPerNet", Json::from(2_u64)),
                (
                    "routeOptions",
                    Json::object([
                        ("tracksPerChannel", Json::from(8_u64)),
                        ("allowCongestion", Json::Bool(false)),
                    ]),
                ),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    let baseline_drc = fields["baselineDrc"].as_object().unwrap();
    assert_eq!(baseline_drc["errors"], Json::from(0_u64));
    assert_eq!(fields["repairCount"], Json::from(0_u64));
    assert_eq!(fields["diodesInserted"], Json::from(0_u64));
    assert_eq!(fields["antennaWarnings"], Json::from(0_u64));
    assert_eq!(fields["allAntennasRepaired"], Json::Bool(true));
    let placement = fields["placement"].as_object().unwrap();
    assert!(placement["placedCells"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("Antenna repair: 0 diode(s), 0 net(s), status PASS"));

    let invalid = RpcRequest::new(119, "rtlAntennaRepair").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "antennaOptions",
            Json::object([("maxDiodesPerNet", Json::from(0_u64))]),
        ),
    ]));
    assert!(dispatch(&invalid)
        .result
        .unwrap_err()
        .contains("'maxDiodesPerNet' must be a positive usize"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    assert_eq!(
        rtl_antenna_repair(&request.params, &cancellation).unwrap_err(),
        "RTL antenna repair cancelled"
    );
}

#[test]
fn rpc_rtl_power_returns_structured_activity() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(10, "rtlPower").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "vectors",
            Json::Array(vec![
                Json::object([
                    ("timeNs", Json::Number(0.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                    ),
                ]),
                Json::object([
                    ("timeNs", Json::Number(1.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("1")), ("b", Json::string("1"))]),
                    ),
                ]),
            ]),
        ),
        (
            "powerOptions",
            Json::object([("frequencyMhz", Json::Number(250.0))]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["vectors"], Json::Number(2.0));
    assert_eq!(fields["frequencyMhz"], Json::Number(250.0));
    assert!(fields["totalPowerUw"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(!fields["activityByNet"].as_object().unwrap().is_empty());
    assert!(fields["report"].as_str().unwrap().contains("Power report"));
    assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
        stats
            .get("libCells")
            .and_then(Json::as_number)
            .is_some_and(|value| *value > 0.0)
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_power(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL power analysis cancelled");
}

#[test]
fn rpc_rtl_ir_returns_structured_power_grid_result() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(12, "rtlIr").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "vectors",
            Json::Array(vec![
                Json::object([
                    ("timeNs", Json::Number(0.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                    ),
                ]),
                Json::object([
                    ("timeNs", Json::Number(1.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("1")), ("b", Json::string("1"))]),
                    ),
                ]),
            ]),
        ),
        (
            "irOptions",
            Json::object([
                ("voltageV", Json::Number(1.2)),
                ("vddOhmPerSite", Json::Number(0.002)),
                ("gndOhmPerSite", Json::Number(0.002)),
                ("maxTotalDropV", Json::Number(0.05)),
                ("maxRailCurrentA", Json::Number(0.05)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    let power = fields["power"].as_object().unwrap();
    assert_eq!(power["vectors"], Json::Number(2.0));
    assert!(power["totalPowerUw"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    let ir = fields["ir"].as_object().unwrap();
    assert_eq!(ir["voltageV"], Json::Number(1.2));
    assert!(ir["totalCurrentA"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(ir["worstDropV"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(ir["worstRow"].as_number().is_some());
    assert!(matches!(&ir["rows"], Json::Array(rows) if !rows.is_empty()));
    assert!(matches!(&ir["cells"], Json::Array(cells) if !cells.is_empty()));
    assert_eq!(ir["hasErrors"], Json::Bool(false));
    let placement = fields["placement"].as_object().unwrap();
    assert!(placement["placedCells"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(fields["report"].as_str().unwrap().contains("IR report"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_ir(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL IR analysis cancelled");

    let invalid = RpcRequest::new(13, "rtlIr").with_params(Json::object([
        ("source", Json::string(source)),
        (
            "vectors",
            Json::Array(vec![Json::object([
                ("timeNs", Json::Number(0.0)),
                (
                    "inputs",
                    Json::object([("a", Json::string("0")), ("b", Json::string("0"))]),
                ),
            ])]),
        ),
        (
            "irOptions",
            Json::object([("voltageV", Json::Number(-1.0))]),
        ),
    ]));
    let error = dispatch(&invalid).result.unwrap_err();
    assert!(error.contains("voltage"), "{error}");
}

#[test]
fn rpc_rtl_bmc_returns_structured_property_results() {
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
    let request = RpcRequest::new(14, "rtlBmc").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("properties", Json::string(properties)),
        ("depth", Json::Number(3.0)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["depth"], Json::Number(3.0));
    assert_eq!(fields["properties"], Json::Number(2.0));
    assert_eq!(fields["violations"], Json::Number(1.0));
    assert!(matches!(&fields["assumptions"], Json::Array(names) if names.len() == 1));
    let results = match &fields["results"] {
        Json::Array(results) => results,
        other => panic!("expected property array, got {other:?}"),
    };
    assert_eq!(
        results[0].as_object().unwrap()["status"],
        Json::string("boundedPass")
    );
    let violated = results[1].as_object().unwrap();
    assert_eq!(violated["status"], Json::string("violated"));
    assert_eq!(violated["violated"], Json::Bool(true));
    assert!(matches!(
        &violated["inputTrace"],
        Json::Array(frames) if !frames.is_empty()
    ));
    assert!(matches!(
        &violated["netTrace"],
        Json::Array(frames) if !frames.is_empty()
    ));
    assert!(fields["report"]
        .as_str()
        .unwrap()
        .contains("Temporal BMC report"));
    assert!(fields["mappedStats"].as_object().is_some_and(|stats| {
        stats
            .get("seq")
            .and_then(Json::as_number)
            .is_some_and(|value| *value > 0.0)
    }));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_bmc(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL property BMC cancelled");

    let invalid = RpcRequest::new(15, "rtlBmc").with_params(Json::object([
        ("source", Json::string(source)),
        ("properties", Json::string(properties)),
        ("depth", Json::Number(0.0)),
    ]));
    let error = dispatch(&invalid).result.unwrap_err();
    assert_eq!(error, "'depth' must be a positive integer");
}

#[test]
fn rpc_functional_coverage_returns_structured_bins() {
    let vcd = r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 2 # state [1:0] $end
$enddefinitions $end
#0
0!
0"
b00 #
#10
1!
1"
b01 #
#20
0!
0"
b01 #
#30
1!
1"
b11 #
"#;
    let coverage = r#"
covergroup cg @(posedge clk);
  cp_req: coverpoint req {
bins zero = {0};
bins one = {1};
  }
  cp_state: coverpoint state[1:0] {
bins low = {[0:1]};
bins high = {[2:3]};
  }
  x: cross cp_req, cp_state;
endgroup
toggle req;
"#;
    let request = RpcRequest::new(16, "coverage").with_params(Json::object([
        ("vcd", Json::string(vcd)),
        ("coverage", Json::string(coverage)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["samples"], Json::Number(2.0));
    assert!(matches!(&fields["bins"], Json::Array(bins) if bins.len() == 4));
    assert!(matches!(&fields["crosses"], Json::Array(crosses) if crosses.len() == 4));
    assert!(matches!(&fields["toggles"], Json::Array(toggles) if toggles.len() == 1));
    assert!(fields["percentage"]
        .as_number()
        .is_some_and(|value| *value > 0.0));
    assert!(fields["report"]
        .as_str()
        .is_some_and(|report| report.contains("Coverage report")));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = functional_coverage_rpc(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "functional coverage analysis cancelled");

    let invalid = RpcRequest::new(17, "coverage").with_params(Json::object([
        ("vcd", Json::string(vcd)),
        ("coverage", Json::string("toggle;")),
    ]));
    let error = dispatch(&invalid).result.unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn rpc_rtl_dft_returns_structured_scan_chain() {
    let source = r#"
module top(input clk, d, output reg q1, q2);
always @(posedge clk) begin
  q1 <= d;
  q2 <= q1;
end
endmodule
"#;
    let request = RpcRequest::new(18, "rtlDft").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "scanOptions",
            Json::object([
                ("chainName", Json::string("agent_chain")),
                ("scanEnPort", Json::string("test_en")),
                ("scanInPort", Json::string("test_in")),
                ("scanOutPort", Json::string("test_out")),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    let scan = fields["scan"].as_object().unwrap();
    assert_eq!(scan["chainName"], Json::string("agent_chain"));
    assert_eq!(scan["scanEnPort"], Json::string("test_en"));
    assert_eq!(scan["scanInPort"], Json::string("test_in"));
    assert_eq!(scan["scanOutPort"], Json::string("test_out"));
    assert!(matches!(&scan["chain"], Json::Array(chain) if chain.len() == 2));
    let dft = fields["dft"].as_object().unwrap();
    assert_eq!(dft["sequentialCells"], Json::Number(2.0));
    assert_eq!(dft["scanCells"], Json::Number(2.0));
    assert_eq!(dft["scanChains"], Json::Number(1.0));
    assert_eq!(dft["unscannedSequential"], Json::Array(Vec::new()));
    assert!(fields["scannedStats"].as_object().is_some_and(|stats| {
        stats
            .get("cells")
            .and_then(Json::as_number)
            .is_some_and(|value| *value > 2.0)
    }));
    assert!(fields["report"]
        .as_str()
        .is_some_and(|report| report.contains("DFT scan report")));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_dft(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL scan insertion cancelled");

    let invalid = RpcRequest::new(19, "rtlDft").with_params(Json::object([
        ("source", Json::string(source)),
        (
            "scanOptions",
            Json::object([("scanEnPort", Json::Number(1.0))]),
        ),
    ]));
    let error = dispatch(&invalid).result.unwrap_err();
    assert_eq!(error, "scan option 'scanEnPort' must be a string");
}

#[test]
fn rpc_rtl_cdc_returns_structured_crossings() {
    let source = r#"
module top(input clk_a, clk_b, d, output reg a_q, b_q);
always @(posedge clk_a) a_q <= d;
always @(posedge clk_b) b_q <= a_q;
endmodule
"#;
    let request = RpcRequest::new(20, "rtlCdc").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert!(matches!(&fields["clocks"], Json::Array(clocks) if clocks.len() == 2));
    assert!(matches!(&fields["crossings"], Json::Array(crossings) if !crossings.is_empty()));
    let findings = match &fields["findings"] {
        Json::Array(findings) => findings,
        other => panic!("expected CDC findings, got {other:?}"),
    };
    assert!(!findings.is_empty());
    assert_eq!(
        findings[0].as_object().unwrap()["severity"],
        Json::string("error")
    );
    assert_eq!(fields["hasErrors"], Json::Bool(true));
    assert_eq!(fields["clean"], Json::Bool(false));
    assert!(fields["report"].as_str().unwrap().contains("CDC report"));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_cdc(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL CDC analysis cancelled");
}

#[test]
fn rpc_rtl_fault_returns_structured_coverage_and_stil() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(21, "rtlFault").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "patternOptions",
            Json::object([
                ("maxRandomPatterns", Json::Number(32.0)),
                ("exhaustiveInputLimit", Json::Number(8.0)),
                ("randomSeed", Json::Number(123.0)),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert!(matches!(&fields["patterns"], Json::Array(patterns) if patterns.len() == 4));
    assert_eq!(fields["exhaustive"], Json::Bool(true));
    assert_eq!(
        fields["detectedFaults"]
            .as_number()
            .map(|value| *value as usize),
        fields["totalFaults"]
            .as_number()
            .map(|value| *value as usize)
    );
    assert_eq!(fields["coveragePercent"], Json::Number(100.0));
    assert_eq!(fields["undetectedFaults"], Json::Array(Vec::new()));
    assert!(matches!(&fields["detections"], Json::Array(detections) if !detections.is_empty()));
    assert!(fields["stil"].as_str().unwrap().contains("STIL 1.0"));
    assert!(fields["report"]
        .as_str()
        .is_some_and(|report| report.contains("Fault report")));

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_fault(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL fault analysis cancelled");

    let invalid = RpcRequest::new(22, "rtlFault").with_params(Json::object([
        ("source", Json::string(source)),
        (
            "patternOptions",
            Json::object([("exhaustiveInputLimit", Json::Number(65.0))]),
        ),
    ]));
    let error = dispatch(&invalid).result.unwrap_err();
    assert_eq!(
        error,
        "pattern option 'exhaustiveInputLimit' must be an integer from 1 through 64"
    );
}

#[test]
fn rpc_rtl_simulation_returns_timed_vectors() {
    let source = "module top(input a, b, output y); assign y = a & b; endmodule";
    let request = RpcRequest::new(7, "rtlSim").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        (
            "vectors",
            Json::Array(vec![
                Json::object([
                    ("timeNs", Json::Number(0.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("0")), ("b", Json::string("1"))]),
                    ),
                ]),
                Json::object([
                    ("timeNs", Json::Number(2.0)),
                    (
                        "inputs",
                        Json::object([("a", Json::string("'b1")), ("b", Json::string("0x1"))]),
                    ),
                ]),
            ]),
        ),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["finished"], Json::Bool(false));
    assert_eq!(fields["finalTimeNs"], Json::Number(2.0));
    let Json::Array(samples) = &fields["samples"] else {
        panic!("simulation samples are not an array");
    };
    assert_eq!(samples.len(), 2);
    assert_eq!(
        samples[0].as_object().unwrap()["outputs"]
            .as_object()
            .unwrap()["y"],
        Json::string("0")
    );
    assert_eq!(
        samples[1].as_object().unwrap()["outputs"]
            .as_object()
            .unwrap()["y"],
        Json::string("1")
    );

    let cancellation = FlowCancellation::new();
    cancellation.cancel();
    let error = rtl_sim(&request.params, &cancellation).unwrap_err();
    assert_eq!(error, "RTL simulation cancelled");
}

#[test]
fn rpc_pdk_signoff_returns_structured_result() {
    let source = "module top(input a, output y); assign y = ~a; endmodule";
    let manifest = r#"pdk rpc-pdk
version 2.0
corner vendor process TT voltage_v 1.8 temperature_c 25 delay_scale 1
liberty vendor <<END
library (rpc-vendor) {
time_unit : "1ns";
cell (INV) {
    area : 1.2;
    pin (A) { direction : input; capacitance : 0.01; }
    pin (Y) {
        direction : output;
        function : "!A";
        timing () { related_pin : "A"; cell_rise(values("0.35,0.42")); }
    }
}
}
END
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#;
    let request = RpcRequest::new(5, "pdkSignoff").with_params(Json::object([
        ("source", Json::string(source)),
        ("top", Json::string("top")),
        ("pdk", Json::string(manifest)),
    ]));
    let response = dispatch(&request).result.unwrap();
    let fields = response.as_object().unwrap();
    assert_eq!(fields["passed"], Json::Bool(true));
    assert_eq!(fields["pdk"], Json::string("rpc-pdk"));
    let report = fields["report"].as_str().unwrap();
    assert!(report.contains("imported-liberty=true"));
    assert!(report.contains("Overall signoff: PASS"));
}
