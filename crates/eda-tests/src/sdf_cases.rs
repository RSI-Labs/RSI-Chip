//! SDF parsing, writing, and timing back-annotation cases.

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

fn and() -> Netlist {
    synth("module m(input a, b, output y); assign y = a & b; endmodule")
}

pub fn register(c: &mut CaseList) {
    c.add("sdf_parse_header_and_arcs", || {
        let text = r#"
(DELAYFILE
  (SDFVERSION "3.0")
  (DESIGN "m")
  (TIMESCALE 1ns)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (1.1::1.2::1.3))
      (IOPATH B Y (1.4))
    ))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        if sdf.design != "m" || sdf.iopaths.len() != 2 {
            Err(format!("design={} paths={}", sdf.design, sdf.iopaths.len()))
        } else if (sdf.iopaths[0].delay.rise - 1.1).abs() > 1e-12
            || (sdf.iopaths[0].delay.fall - 1.2).abs() > 1e-12
            || (sdf.iopaths[0].delay.transition - 1.3).abs() > 1e-12
        {
            Err(format!("{:?}", sdf.iopaths[0].delay))
        } else {
            Ok(())
        }
    });

    c.add("sdf_timescale_ps_to_ns", || {
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "x") (TIMESCALE 1ps)
  (CELL (CELLTYPE "AND2") (INSTANCE *)
    (DELAY (ABSOLUTE (IOPATH A Y (1000::1100))))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        if (sdf.iopaths[0].delay.rise - 1.0).abs() > 1e-12 {
            Err(format!("{:?}", sdf.iopaths[0].delay))
        } else {
            Ok(())
        }
    });

    c.add("sdf_write_parse_round_trip", || {
        let nl = and();
        let text = eda_sdf::write_sdf(&nl)?;
        let sdf = eda_sdf::parse_sdf(&text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let base = eda_sta::analyze(&nl)?;
        let after = eda_sta::analyze(&annotated)?;
        if (base.critical_path_delay - after.critical_path_delay).abs() > 1e-9 {
            Err(format!(
                "nominal {} changed to {}",
                base.critical_path_delay, after.critical_path_delay
            ))
        } else {
            Ok(())
        }
    });

    c.add("sdf_iopath_back_annotation_updates_sta", || {
        let nl = and();
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "AND2") (INSTANCE *)
    (DELAY (ABSOLUTE
      (IOPATH a y (2.2::2.2))
      (IOPATH b y (3.3::3.3))
    ))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let timing = eda_sta::analyze(&annotated)?;
        if (timing.critical_path_delay - 3.3).abs() > 1e-9 {
            Err(format!("critical {}", timing.critical_path_delay))
        } else {
            Ok(())
        }
    });

    c.add("sdf_setup_annotation_updates_reg2reg", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFF") (INSTANCE *)
    (TIMINGCHECK (SETUP D CLK (0.9::0.9)))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let timing = eda_sta::analyze(&annotated)?;
        if (timing.in2reg_critical - 0.9).abs() > 1e-9 {
            Err(format!("in2reg {}", timing.in2reg_critical))
        } else {
            Ok(())
        }
    });

    c.add("sdf_hold_annotation_updates_sta", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFF") (INSTANCE *)
    (TIMINGCHECK (HOLD D CLK (0.15::0.17)))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        let before_timing = eda_sta::analyze(&nl)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let after_timing = eda_sta::analyze(&annotated)?;
        let before = eda_sta::worst_hold_slack(&nl, &before_timing, 0.0);
        let after = eda_sta::worst_hold_slack(&annotated, &after_timing, 0.0);
        if sdf.timing_checks.len() == 1
            && sdf.timing_checks[0].kind == eda_sdf::TimingCheckKind::Hold
            && (before + 0.2).abs() < 1e-9
            && (after + 0.17).abs() < 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "checks={:?} before={} after={}",
                sdf.timing_checks, before, after
            ))
        }
    });

    c.add("sdf_width_annotation_updates_min_pulse", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFF") (INSTANCE *)
    (TIMINGCHECK (WIDTH CLK (0.38::0.40)))
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 -fall 0.3 [get_ports clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&annotated, &constraints)?;
        if sdf.timing_checks.len() == 1
            && sdf.timing_checks[0].kind == eda_sdf::TimingCheckKind::Width
            && (timing.min_pulse_high_slack + 0.1).abs() < 1e-9
            && (timing.min_pulse_width_slack + 0.1).abs() < 1e-9
        {
            Ok(())
        } else {
            Err(format!(
                "check={:?} high={} worst={}",
                sdf.timing_checks.first().map(|check| check.kind),
                timing.min_pulse_high_slack,
                timing.min_pulse_width_slack
            ))
        }
    });

    c.add("sdf_recovery_removal_annotations_update_sta", || {
        let nl = synth(
            "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule",
        );
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFFR") (INSTANCE *)
    (TIMINGCHECK
      (RECOVERY RST CLK (0.41::0.43))
      (REMOVAL RST CLK (0.16::0.18))
    )
  )
)
"#;
        let sdf = eda_sdf::parse_sdf(text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&annotated, &constraints)?;
        if (timing.recovery_slack - 2.37).abs() > 1e-9 || (timing.removal_slack - 1.02).abs() > 1e-9
        {
            Err(format!(
                "recovery={} removal={}",
                timing.recovery_slack, timing.removal_slack
            ))
        } else {
            Ok(())
        }
    });

    c.add("sdf_interconnect_annotation", || {
        let nl = synth(
            "module m(input a, b, output y); wire w; assign w = ~a; assign y = w & b; endmodule",
        );
        let target = nl
            .net_by_name("w")
            .or_else(|| nl.net_by_name("y"))
            .map(|id| nl.net(id).name.clone())
            .ok_or("no internal wire")?;
        let text = format!(
            r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (INSTANCE *)
    (DELAY (ABSOLUTE (INTERCONNECT a {target} (1.7::1.7))))
  )
)
"#
        );
        let sdf = eda_sdf::parse_sdf(&text)?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let timing = eda_sta::analyze(&annotated)?;
        if timing.critical_path_delay >= 1.7 {
            Ok(())
        } else {
            Err(format!("critical {}", timing.critical_path_delay))
        }
    });

    c.add("sdf_report_content", || {
        let nl = and();
        let text = eda_sdf::write_sdf(&nl)?;
        let sdf = eda_sdf::parse_sdf(&text)?;
        let report = eda_sdf::report(&sdf);
        if report.contains("SDF report") && report.contains("IOPATH") {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_sdf_parse_and_back_annotate", || {
        let nl = and();
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "AND2") (INSTANCE *)
    (DELAY (ABSOLUTE (IOPATH a y (4.5::4.5)) (IOPATH b y (4.5::4.5))))
  )
)
"#;
        let annotations = eda_api::parse_delay_file(text)?;
        let annotated = eda_api::back_annotate(&nl, &annotations)?;
        let timing = eda_sta::analyze(&annotated)?;
        if (timing.critical_path_delay - 4.5).abs() > 1e-9 {
            Err(format!("critical {}", timing.critical_path_delay))
        } else {
            Ok(())
        }
    });

    c.add("api_write_delay_file_round_trip", || {
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ps)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (100.25::110.5::120.75))
      (INTERCONNECT a b (10::11::12))
    ))
    (TIMINGCHECK
      (SETUP D CLK (50::51::52))
      (HOLD D CLK (10::11::12))
      (WIDTH CLK (20::21::22))
      (RECOVERY RST CLK (30::31::32))
      (REMOVAL RST CLK (5::6::7))
    )
  )
)
"#;
        let original = eda_api::parse_delay_file(text)?;
        let exported = eda_api::write_delay_file(&original)?;
        let reparsed = eda_api::parse_delay_file(&exported)?;
        if original == reparsed {
            Ok(())
        } else {
            Err(format!("original={original:?}\nreparsed={reparsed:?}"))
        }
    });

    c.add("api_physical_timing_flow_exports_best_sdf", || {
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
        let text = eda_api::write_delay_file(&best.sdf)?;
        let reparsed = eda_api::parse_delay_file(&text)?;
        if reparsed != best.sdf {
            Err(format!(
                "best={:?}\nreparsed={:?}\n{}",
                best.sdf, reparsed, text
            ))
        } else if best.sdf.interconnects.is_empty() {
            Err("best physical SDF omitted INTERCONNECT timing".into())
        } else {
            Ok(())
        }
    });

    c.add(
        "api_multi_corner_physical_timing_writes_per_corner_sdf",
        || {
            let flow = eda_api::run_flow(
                "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
                None,
            )?;
            let constraints = eda_api::parse_timing_constraints(
                r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_output_delay 0.3 -clock clk [get_ports y]
        "#,
            )?;
            let scenarios = vec![
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "slow",
                    &flow.netlist,
                    &constraints,
                    Vec::new(),
                ),
                eda_api::MultiCornerPhysicalTimingScenario::new(
                    "fast",
                    &flow.netlist,
                    &constraints,
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
            if eda_api::prepare_multi_corner_delay_file_exports(&result.best().corners, "best.sdf")
                .is_ok()
            {
                return Err("MMMC SDF export accepted a template without {corner}".into());
            }

            let base = std::env::temp_dir().join(format!(
                "eda-api-mmmc-sdf-{}-line-{}.{{corner}}.sdf",
                std::process::id(),
                line!()
            ));
            let template = base
                .to_str()
                .ok_or_else(|| "temporary MMMC SDF path is not UTF-8".to_string())?
                .to_string();
            let exports = eda_api::prepare_multi_corner_delay_file_exports(
                &result.best().corners,
                &template,
            )?;
            let paths = eda_api::write_multi_corner_delay_file_exports(&exports)?;
            if exports.len() != 2
                || exports[0].scenario != "slow"
                || exports[1].scenario != "fast"
                || exports[0].path == exports[1].path
                || paths.len() != 2
            {
                return Err(format!("exports={exports:?} paths={paths:?}"));
            }

            for (export, corner) in exports.iter().zip(result.best().corners.iter()) {
                let text = std::fs::read_to_string(&export.path).map_err(|e| e.to_string())?;
                let reparsed = eda_api::parse_delay_file(&text)?;
                if reparsed != corner.evaluation.sdf {
                    return Err(format!(
                        "scenario '{}' changed on round trip: {:?} != {:?}",
                        export.scenario, reparsed, corner.evaluation.sdf
                    ));
                }
                let _ = std::fs::remove_file(&export.path);
            }
            Ok(())
        },
    );

    c.add("api_flow_contains_nominal_sdf", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        if flow.sdf.contains("DELAYFILE") && flow.sdf.contains("IOPATH") {
            Ok(())
        } else {
            Err("nominal SDF missing".into())
        }
    });

    for (name, text) in [
        ("missing_delayfile", "(DESIGN \"x\")"),
        (
            "no_annotations",
            "(DELAYFILE (DESIGN \"x\") (TIMESCALE 1ns))",
        ),
        (
            "bad_timescale",
            "(DELAYFILE (DESIGN \"x\") (TIMESCALE 1sec) (CELL (DELAY (ABSOLUTE (IOPATH a b (1)))))))",
        ),
        (
            "missing_instance",
            "(DELAYFILE (DESIGN \"x\") (TIMESCALE 1ns) (CELL (INSTANCE ghost) (DELAY (ABSOLUTE (IOPATH a b (1)))))))",
        ),
    ] {
        c.add(format!("sdf_err_{}", name), move || {
            if let Ok(sdf) = eda_sdf::parse_sdf(text) {
                let nl = and();
                if eda_sdf::annotate_sdf(&nl, &sdf).is_ok() {
                    return Err(format!("{} unexpectedly applied", name));
                }
            }
            Ok(())
        });
    }
}
