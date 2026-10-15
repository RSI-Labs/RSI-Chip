//! Power-grid IR-drop and electromigration regression cases.

use crate::CaseList;
use eda_core::netlist::Netlist;
use eda_power::{InputVector, PowerOptions};
use std::collections::HashMap;

fn synth() -> Netlist {
    let design = eda_hdl::compile(
        "module m(input a, b, output y); assign y = a & b; endmodule",
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

fn vector(a: u64, b: u64) -> InputVector {
    HashMap::from([
        ("a".to_string(), eda_core::logic::LogicWord::from_u64(2, a)),
        ("b".to_string(), eda_core::logic::LogicWord::from_u64(2, b)),
    ])
}

fn power(nl: &Netlist) -> eda_power::PowerReport {
    eda_power::analyze(
        nl,
        &[vector(0, 0), vector(1, 1), vector(0, 0), vector(1, 1)],
        &PowerOptions::default(),
    )
    .unwrap()
}

fn two_cell_synth() -> Netlist {
    let design = eda_hdl::compile(
        "module m(input a, b, c, d, output y, z); assign y = a & b; assign z = c | d; endmodule",
        None,
    )
    .expect("compile two cells");
    eda_synth::synthesize(
        &design,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .expect("synthesize two cells")
    .netlist
}

pub fn register(c: &mut CaseList) {
    c.add("ir_clean_default_grid", || {
        let nl = synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let report = power(&nl);
        let result = eda_ir::analyze(&nl, &placement, &report, &eda_ir::IrOptions::default())?;
        if result.has_errors()
            || result.rows.len() != placement.rows
            || result.cells.len() != nl.cell_count()
        {
            Err(eda_ir::report(&result))
        } else {
            Ok(())
        }
    });

    c.add("ir_multilayer_mesh_solves_via_current", || {
        let nl = synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let mut taps = Vec::new();
        for row in 0..3 {
            for column in 0..3 {
                if row == 0 || row == 2 || column == 0 || column == 2 {
                    taps.push(eda_ir::PowerMeshLayerTap {
                        layer: 0,
                        row,
                        column,
                        vdd: true,
                        gnd: true,
                    });
                }
            }
        }
        let options = eda_ir::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            voltage_v: 1.0,
            layers: vec![
                eda_ir::PowerMeshLayer::new("M8", 0.1, 0.1, 0.1, 0.1),
                eda_ir::PowerMeshLayer::new("M7", 1.0, 1.0, 1.0, 1.0),
            ],
            vias: vec![eda_ir::PowerMeshVia::new(1, 1, 0, 1, 0.2, 0.2)],
            cell_layers: vec![1],
            use_boundary_taps: false,
            taps,
            max_total_drop_v: 0.05,
            max_node_current_a: 0.002,
            max_segment_current_a: 0.002,
            em: Default::default(),
            power_domains: Vec::new(),
            cell_power_domains: Vec::new(),
            max_iterations: 20_000,
            tolerance_v: 1e-10,
        };
        let result =
            eda_ir::analyze_multilayer_mesh_currents(&nl, &placement, &[0.001], 1_000.0, &options)?;
        let report = eda_ir::multilayer_power_mesh_report(&result);
        let via_currents: Vec<_> = result
            .segments
            .iter()
            .filter(|segment| segment.orientation == eda_ir::PowerMeshSegmentOrientation::Via)
            .map(|segment| segment.current_a)
            .collect();
        let mut invalid_options = options.clone();
        invalid_options.vias[0].from_layer = invalid_options.vias[0].to_layer;
        if result.nodes.len() != 18
            || result.cells.len() != 1
            || result.cells[0].layer != 1
            || result.cells[0].mesh_row != 1
            || result.cells[0].mesh_column != 1
            || via_currents.len() != 2
            || via_currents.iter().any(|current| *current <= 0.0)
            || result.worst_drop_v <= 0.0
            || result.has_errors()
            || !report.contains("Multilayer power-mesh IR report:")
            || !report.contains("via Vdd")
            || !matches!(
                eda_ir::analyze_multilayer_mesh_currents(
                    &nl,
                    &placement,
                    &[0.001],
                    1_000.0,
                    &invalid_options
                ),
                Err(error) if error.contains("must connect distinct layers")
            )
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_multilayer_power_mesh", || {
        let nl = synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let mut taps = Vec::new();
        for row in 0..3_usize {
            for column in 0..3_usize {
                if row == 0 || row == 2 || column == 0 || column == 2 {
                    taps.push(eda_api::PowerMeshLayerTap {
                        layer: 0,
                        row,
                        column,
                        vdd: true,
                        gnd: true,
                    });
                }
            }
        }
        let options = eda_api::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            layers: vec![
                eda_api::PowerMeshLayer::new("top", 0.1, 0.1, 0.1, 0.1),
                eda_api::PowerMeshLayer::new("bottom", 1.0, 1.0, 1.0, 1.0),
            ],
            vias: vec![eda_api::PowerMeshVia::new(1, 1, 0, 1, 0.2, 0.2)],
            cell_layers: vec![1],
            use_boundary_taps: false,
            taps,
            max_total_drop_v: 0.05,
            max_node_current_a: 0.002,
            max_segment_current_a: 0.002,
            max_iterations: 20_000,
            tolerance_v: 1e-10,
            ..Default::default()
        };
        let result = eda_api::multilayer_power_grid_mesh_currents(
            &nl,
            &placement,
            &[0.001],
            1_000.0,
            &options,
        )?;
        let report = eda_api::multilayer_power_mesh_report(&result);
        if result.nodes.len() == 18
            && result.cells[0].layer == 1
            && result.segments.iter().any(|segment| {
                segment.orientation == eda_api::PowerMeshSegmentOrientation::Via
                    && segment.current_a > 0.0
            })
            && report.contains("Multilayer power-mesh IR report:")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("api_multilayer_power_mesh_em_lifetime", || {
        let em = eda_api::PowerMeshEmOptions::default();
        let reference = eda_api::estimate_multilayer_mesh_em_lifetime(
            em.reference_current_density_a_per_um2,
            &em,
        )?;
        let doubled = eda_api::estimate_multilayer_mesh_em_lifetime(
            em.reference_current_density_a_per_um2 * 2.0,
            &em,
        )?;
        let unpowered = eda_api::estimate_multilayer_mesh_em_lifetime(0.0, &em)?;
        let relative_reference =
            (reference - em.reference_lifetime_hours).abs() / em.reference_lifetime_hours;
        let relative_doubled = (doubled * 4.0 - reference).abs() / reference;
        if relative_reference > 1e-9
            || relative_doubled > 1e-9
            || !unpowered.is_infinite()
            || eda_api::estimate_multilayer_mesh_em_lifetime(-1.0, &em).is_ok()
        {
            return Err(format!(
                "reference={reference} doubled={doubled} unpowered={unpowered} relative_reference={relative_reference} relative_doubled={relative_doubled}"
            ));
        }

        let nl = synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos { row: 1, site: eda_pnr::SITES_PER_ROW / 2 - 1 }],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let mut taps = Vec::new();
        for row in 0..3_usize {
            for column in 0..3_usize {
                if row == 0 || row == 2 || column == 0 || column == 2 {
                    taps.push(eda_api::PowerMeshLayerTap {
                        layer: 0,
                        row,
                        column,
                        vdd: true,
                        gnd: true,
                    });
                }
            }
        }
        let options = eda_api::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            layers: vec![
                eda_api::PowerMeshLayer::new("top", 0.1, 0.1, 0.1, 0.1),
                eda_api::PowerMeshLayer::new("bottom", 1.0, 1.0, 1.0, 1.0),
            ],
            vias: vec![eda_api::PowerMeshVia::new(1, 1, 0, 1, 0.2, 0.2).with_area(1e-9)],
            cell_layers: vec![1],
            use_boundary_taps: false,
            taps,
            ..Default::default()
        };
        let result = eda_api::multilayer_power_grid_mesh_currents(
            &nl,
            &placement,
            &[0.001],
            1_000.0,
            &options,
        )?;
        let report = eda_api::multilayer_power_mesh_report(&result);
        let em_violation = result
            .findings
            .iter()
            .find(|finding| finding.code == "MULTILAYER-MESH-EM-LIFETIME")
            .ok_or("missing EM lifetime finding")?;
        if result.segments.iter().any(|segment| {
            segment.orientation == eda_api::PowerMeshSegmentOrientation::Via
                && segment.current_a > 0.0
                && segment.current_density_a_per_um2 > 0.0
                && segment.em_lifetime_hours < options.em.min_lifetime_hours
        }) && result.worst_em_lifetime_hours < options.em.min_lifetime_hours
            && report.contains("worst EM")
            && em_violation.message.contains("current density")
        {
            Ok(())
        } else {
            Err(format!("{result:?}\n{report}"))
        }
    });

    c.add("api_coupled_multilayer_power_domains", || {
        let nl = two_cell_synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![
                eda_pnr::Pos {
                    row: 1,
                    site: eda_pnr::SITES_PER_ROW / 2 - 1,
                },
                eda_pnr::Pos { row: 1, site: 25 },
            ],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let mut options = eda_api::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            use_boundary_taps: false,
            cell_layers: vec![1, 1],
            ..Default::default()
        };
        for row in 0..3_usize {
            for column in 0..3_usize {
                options
                    .vias
                    .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
            }
        }
        options.taps = vec![
            eda_api::PowerMeshLayerTap {
                layer: 0,
                row: 2,
                column: 0,
                vdd: false,
                gnd: true,
            },
            eda_api::PowerMeshLayerTap {
                layer: 0,
                row: 2,
                column: 2,
                vdd: false,
                gnd: true,
            },
        ];
        options.power_domains = vec![
            eda_api::MultilayerPowerDomainSupply::new(
                "A",
                0.8,
                vec![eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 0,
                    vdd: true,
                    gnd: false,
                }],
            ),
            eda_api::MultilayerPowerDomainSupply::new(
                "B",
                0.65,
                vec![eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 2,
                    vdd: true,
                    gnd: false,
                }],
            ),
        ];
        options.cell_power_domains = vec!["A".into(), "B".into()];
        let result = eda_api::multilayer_power_grid_mesh_currents(
            &nl,
            &placement,
            &[0.002, 0.001],
            2_250.0,
            &options,
        )?;
        let report = eda_api::multilayer_power_mesh_report(&result);
        let node = |layer, row, column| {
            result
                .nodes
                .iter()
                .find(|node| node.layer == layer && node.row == row && node.column == column)
                .ok_or("missing coupled mesh node")
        };
        let supply_a = node(0, 0, 0)?;
        let supply_b = node(0, 0, 2)?;
        let domain_a = result
            .power_domains
            .iter()
            .find(|domain| domain.name == "A")
            .ok_or("missing coupled domain A")?;
        let domain_b = result
            .power_domains
            .iter()
            .find(|domain| domain.name == "B")
            .ok_or("missing coupled domain B")?;
        if (supply_a.vdd_v - 0.8).abs() > 1e-12
            || (supply_b.vdd_v - 0.65).abs() > 1e-12
            || result.cells.len() != 2
            || result.cells.iter().any(|cell| cell.power_domain.is_none())
            || (domain_a.total_current_a - 0.002).abs() > 1e-12
            || (domain_b.total_current_a - 0.001).abs() > 1e-12
            || !result
                .segments
                .iter()
                .any(|segment| segment.current_a > 0.0)
            || !report.contains("domain 'A' V=0.800000")
            || !report.contains("domain 'B' V=0.650000")
        {
            Err(format!("{result:?}\n{report}"))
        } else {
            let mut conflicting = options.clone();
            conflicting.power_domains[1]
                .taps
                .push(eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 0,
                    vdd: true,
                    gnd: false,
                });
            match eda_api::multilayer_power_grid_mesh_currents(
                &nl,
                &placement,
                &[0.002, 0.001],
                2_250.0,
                &conflicting,
            ) {
                Err(error) if error.contains("conflicting VDD fixed voltages") => Ok(()),
                Err(error) => Err(error),
                Ok(result) => Err(format!(
                    "conflicting domain taps unexpectedly solved: {result:?}"
                )),
            }
        }
    });

    c.add("api_multilayer_power_domain_manifest_round_trip", || {
        let supplies = vec![
            eda_api::MultilayerPowerDomainSupply::new(
                "DOMAIN_A",
                0.8,
                vec![
                    eda_api::PowerMeshLayerTap {
                        layer: 0,
                        row: 0,
                        column: 0,
                        vdd: true,
                        gnd: false,
                    },
                    eda_api::PowerMeshLayerTap {
                        layer: 0,
                        row: 2,
                        column: 0,
                        vdd: false,
                        gnd: true,
                    },
                ],
            ),
            eda_api::MultilayerPowerDomainSupply::new(
                "DOMAIN_B",
                0.65,
                vec![eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 2,
                    vdd: true,
                    gnd: true,
                }],
            ),
        ];
        let text = eda_api::write_multilayer_power_domain_supplies(&supplies)?;
        let parsed = eda_api::parse_multilayer_power_domain_supplies(&text)?;
        let upf_options =
            eda_api::multilayer_power_domain_upf_options(&parsed, &eda_api::IrOptions::default())?;
        let tap_outside_error = matches!(
            eda_api::parse_multilayer_power_domain_supplies("tap 0 0 0 vdd\n"),
            Err(error) if error.contains("tap appears outside a supply block")
        );
        let missing_vdd_error = matches!(
            eda_api::parse_multilayer_power_domain_supplies("supply A 0.8\ntap 0 0 0 gnd\nend\n"),
            Err(error) if error.contains("requires at least one VDD tap")
        );
        if parsed != supplies
            || !text.contains("supply DOMAIN_A 0.8")
            || !text.contains("tap 0 0 0 vdd")
            || !text.contains("tap 0 0 2 both")
            || (upf_options.domains["DOMAIN_A"].voltage_v - 0.8).abs() > 1e-12
            || (upf_options.domains["DOMAIN_B"].voltage_v - 0.65).abs() > 1e-12
            || !tap_outside_error
            || !missing_vdd_error
        {
            Err(format!("{text}\n{parsed:?}\n{upf_options:?}"))
        } else {
            Ok(())
        }
    });

    c.add("ir_total_power_and_current_consistent", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions::default(),
        )?;
        let expected_current = result.total_power_uw / result.voltage_v / 1_000_000.0;
        if (result.total_current_a - expected_current).abs() > 1e-12 {
            Err(format!(
                "current {} expected {}",
                result.total_current_a, expected_current
            ))
        } else {
            Ok(())
        }
    });

    c.add("ir_drop_scales_with_resistance", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let low = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions {
                vdd_ohm_per_site: 0.01,
                gnd_ohm_per_site: 0.01,
                ..eda_ir::IrOptions::default()
            },
        )?;
        let high = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions {
                vdd_ohm_per_site: 0.02,
                gnd_ohm_per_site: 0.02,
                ..eda_ir::IrOptions::default()
            },
        )?;
        if (high.worst_drop_v - 2.0 * low.worst_drop_v).abs() > 1e-12 {
            Err(format!(
                "drop did not scale: {} vs {}",
                high.worst_drop_v, low.worst_drop_v
            ))
        } else {
            Ok(())
        }
    });

    c.add("ir_vdd_and_gnd_components_sum", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions::default(),
        )?;
        for row in &result.rows {
            let sum = row.vdd_worst_drop_v + row.gnd_worst_drop_v;
            if (sum - row.total_drop_v).abs() > 1e-12 {
                return Err(format!("row {} components do not sum", row.row));
            }
        }
        Ok(())
    });

    c.add("ir_excessive_drop_is_reported", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions {
                max_total_drop_v: 1e-12,
                ..eda_ir::IrOptions::default()
            },
        )?;
        if result.has_errors()
            && result
                .findings
                .iter()
                .any(|finding| finding.code == "IR-DROP")
        {
            Ok(())
        } else {
            Err(eda_ir::report(&result))
        }
    });

    c.add("ir_em_current_violation_is_reported", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions {
                max_rail_current_a: 1e-12,
                ..eda_ir::IrOptions::default()
            },
        )?;
        if result
            .findings
            .iter()
            .any(|finding| finding.code == "EM-CURRENT")
        {
            Ok(())
        } else {
            Err(eda_ir::report(&result))
        }
    });

    c.add("ir_leakage_distributed_by_width", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let mut power_report = power(&nl);
        power_report.static_power_uw = 100.0;
        power_report.total_power_uw = power_report.dynamic_power_uw + power_report.static_power_uw;
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions::default(),
        )?;
        if (result.total_power_uw - power_report.total_power_uw).abs() > 1e-12 {
            Err(format!(
                "total {} expected {}",
                result.total_power_uw, power_report.total_power_uw
            ))
        } else {
            Ok(())
        }
    });

    c.add("ir_report_content", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        let result = eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions::default(),
        )?;
        let text = eda_ir::report(&result);
        if text.contains("IR report") && text.contains("ROW 0") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_power_grid_ir", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let power_report = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let result = eda_api::power_grid_ir(
            &flow.netlist,
            &flow.placement,
            &power_report,
            &eda_api::IrOptions::default(),
        )?;
        if !result.has_errors() && eda_api::ir_report(&result).contains("IR report") {
            Ok(())
        } else {
            Err(eda_api::ir_report(&result))
        }
    });

    c.add("api_power_grid_mesh", || {
        let nl = synth();
        let placed = eda_pnr::place(&nl)?;
        let placement = eda_pnr::Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: placed.cell_width.clone(),
            blockages: Vec::new(),
        };
        let power_report = power(&nl);
        let base = eda_api::PowerMeshOptions {
            rows: 3,
            columns: 3,
            ..eda_api::PowerMeshOptions::default()
        };
        let low = eda_api::power_grid_mesh(&nl, &placement, &power_report, &base)?;
        let high_options = eda_api::PowerMeshOptions {
            vdd_horizontal_ohm_per_segment: 1.0,
            vdd_vertical_ohm_per_segment: 1.0,
            gnd_horizontal_ohm_per_segment: 1.0,
            gnd_vertical_ohm_per_segment: 1.0,
            ..base.clone()
        };
        let high = eda_api::power_grid_mesh(&nl, &placement, &power_report, &high_options)?;
        let corner_options = eda_api::PowerMeshOptions {
            use_boundary_taps: false,
            taps: vec![eda_api::PowerMeshTap::both(0, 0)],
            ..high_options.clone()
        };
        let corner = eda_api::power_grid_mesh(&nl, &placement, &power_report, &corner_options)?;
        let expected_current = power_report.total_power_uw / 1_000_000.0;
        let report = eda_api::power_mesh_report(&high);
        if low.nodes.len() != 9
            || high.nodes.len() != 9
            || low.cells.len() != nl.cell_count()
            || high.cells.len() != nl.cell_count()
            || high.segments.len() != 24
            || corner.worst_drop_v <= high.worst_drop_v
            || corner.worst_segment_current_a <= 0.0
            || (low.total_current_a - expected_current).abs() > 1e-12
            || (high.total_current_a - expected_current).abs() > 1e-12
            || high.worst_drop_v <= low.worst_drop_v
            || !report.contains("Power-mesh IR report")
            || !report.contains("NODE (")
            || !report.contains("SEGMENT Vdd Horizontal")
        {
            Err(format!("low={low:?}\nhigh={high:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_post_route_power_mesh", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let physical = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let vectors = eda_api::default_power_vectors(&physical.best().annotated_netlist);
        let options = eda_api::PostRoutePowerIrOptions {
            power_mesh: Some(eda_api::PowerMeshOptions {
                rows: 3,
                columns: 3,
                use_boundary_taps: false,
                taps: vec![eda_api::PowerMeshTap::both(0, 0)],
                ..Default::default()
            }),
            ..Default::default()
        };
        let result =
            eda_api::analyze_physical_timing_power_ir(physical.best(), &vectors, &options)?;
        let mesh = result
            .power_mesh
            .as_ref()
            .ok_or("post-route power mesh result missing")?;
        let report = eda_api::post_route_power_ir_report(&result);
        let expected_current = result.power.total_power_uw / 1_000_000.0;
        if mesh.nodes.len() != 9
            || mesh.segments.len() != 24
            || mesh.cells.len() != physical.best().annotated_netlist.cell_count()
            || (mesh.total_current_a - expected_current).abs() > 1e-12
            || !report.contains("Post-route power/IR:")
            || !report.contains("Power-mesh IR report:")
            || !report.contains("SEGMENT Vdd Horizontal")
        {
            Err(format!("{result:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_post_route_power_ir_with_upf", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let physical = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let intent = eda_api::parse_upf(
            r#"
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
create_power_switch top_sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance top_sw 100.0
"#,
        )?;
        let definitions = eda_api::parse_timing_scenarios(
            r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_scenario_property post_route.voltage_v 0.80
    set_scenario_property post_route.gnd_ohm_per_site 0.04
    set_scenario_property post_route.domain.TOP.voltage_v 0.65
    set_scenario_property post_route.domain.TOP.vdd_ohm_per_site 0.07
endscenario
"#,
        )?;
        let power_scenarios =
            eda_api::post_route_power_scenarios_from_timing_scenarios(&definitions)?;
        let vectors = eda_api::default_power_vectors(&physical.best().annotated_netlist);
        let options = power_scenarios[0].options.clone();
        let result = eda_api::analyze_physical_timing_power_ir_with_upf(
            physical.best(),
            &vectors,
            &options,
            &intent,
        )?;
        let upf_result = result
            .upf_ir
            .as_ref()
            .ok_or("post-route UPF IR result missing")?;
        let report = eda_api::post_route_power_ir_report(&result);
        if upf_result.cells.len() != physical.best().annotated_netlist.cell_count()
            || upf_result
                .cells
                .iter()
                .any(|cell| cell.power_domain != "TOP")
            || (upf_result.total_power_uw - result.power.total_power_uw).abs() > 1e-12
            || upf_result
                .cells
                .iter()
                .any(|cell| (cell.voltage_v - 0.65).abs() > 1e-12)
            || upf_result.switches.len() != 1
            || upf_result.switches[0].name != "top_sw"
            || (upf_result.switches[0].current_a - upf_result.total_current_a).abs() > 1e-12
            || (upf_result.switches[0].drop_v - upf_result.total_current_a * 100.0).abs() > 1e-12
            || !report.contains("UPF IR report")
            || !report.contains("DOMAIN 'TOP'")
            || !report.contains("SWITCH 'top_sw'")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_upf_off_power_switch_excludes_current", || {
        let flow = eda_api::run_flow(
            "module m(input a, output y); assign y = ~a; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_state sw off
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let result = eda_api::power_grid_ir_upf(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let report = eda_api::upf_ir_report(&result);
        if result.total_power_uw != 0.0
            || result.total_current_a != 0.0
            || result.cells.iter().any(|cell| cell.powered)
            || result.switches.iter().any(|switch| switch.enabled)
            || !result
                .findings
                .iter()
                .any(|finding| finding.code == "IR-POWER-OFF")
            || !report.contains("powered=false")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_upf_power_switch_off_leakage_current", || {
        let flow = eda_api::run_flow(
            "module m(input a, output y); assign y = ~a; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_state sw off
set_power_switch_off_leakage sw 1e-9
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let result = eda_api::power_grid_ir_upf(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let report = eda_api::upf_ir_report(&result);
        if (result.switch_leakage_current_a - 1e-9).abs() > 1e-15
            || (result.switch_leakage_power_uw - 0.001).abs() > 1e-9
            || (result.total_current_a - 1e-9).abs() > 1e-15
            || (result.total_power_uw - 0.001).abs() > 1e-9
            || result.cells.iter().any(|cell| cell.powered)
            || result.switches.iter().any(|switch| switch.enabled)
            || !report.contains("off_leakage=0.000000001A")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_upf_strategy_static_currents", || {
        let flow = eda_api::run_flow(
            "module m(input a, output y); assign y = ~a; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
set_isolation iso -domain TOP -signal a -isolation_power_net vdd
set_isolation_static_current iso 1e-9
set_retention ret -domain TOP -signal y -retention_power_net vdd
set_retention_static_current ret 2e-9
set_level_shifter shift -domain TOP -input vdd -output vdd
set_level_shifter_static_current shift 3e-9
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let result = eda_api::power_grid_ir_upf(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let report = eda_api::upf_ir_report(&result);
        if result.strategies.len() != 3
            || result
                .strategies
                .iter()
                .any(|strategy| strategy.power_domain != "TOP")
            || (result.strategy_current_a - 6e-9).abs() > 1e-15
            || (result.strategy_power_uw - 0.006).abs() > 1e-9
            || (result.total_current_a - (power.total_power_uw / 1_000_000.0 + 6e-9)).abs() > 1e-12
            || (result.total_power_uw - power.total_power_uw - 0.006).abs() > 1e-9
            || !report.contains("strategy current 0.000000 A / 0.006000 uW")
            || !report.contains("STRATEGY kind='isolation' name='iso'")
            || !report.contains("STRATEGY kind='retention' name='ret'")
            || !report.contains("STRATEGY kind='level_shifter' name='shift'")
        {
            Err(report)
        } else {
            Ok(())
        }
    });

    c.add("api_upf_mode_strategy_currents", || {
        let flow = eda_api::run_flow(
            "module m(input a, output y); assign y = ~a; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_isolation iso -domain TOP -signal a -isolation_power_net vdd
set_isolation_static_current iso 1e-9
set_retention ret -domain TOP -signal y -retention_power_net vdd
set_retention_static_current ret 2e-9
set_level_shifter shift -domain TOP -input vdd -output vdd
set_level_shifter_static_current shift 3e-9
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_strategy_current active isolation iso 3e-9
create_power_mode sleep
set_power_mode_switch sleep sw off
set_power_mode_strategy_current sleep isolation iso 0
set_power_mode_strategy_current sleep level_shifter shift 4e-9
set_active_power_mode active
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let modes = eda_api::power_grid_ir_upf_power_modes(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let active = &modes.modes[0];
        let sleep = &modes.modes[1];
        let active_iso = active
            .result
            .strategies
            .iter()
            .find(|strategy| strategy.kind == "isolation")
            .ok_or("active mode omitted isolation")?;
        let sleep_iso = sleep
            .result
            .strategies
            .iter()
            .find(|strategy| strategy.kind == "isolation")
            .ok_or("sleep mode omitted isolation")?;
        let sleep_shift = sleep
            .result
            .strategies
            .iter()
            .find(|strategy| strategy.kind == "level_shifter")
            .ok_or("sleep mode omitted level shifter")?;
        if (active_iso.current_a - 3e-9).abs() > 1e-15
            || (sleep_iso.current_a - 0.0).abs() > 1e-15
            || (sleep_shift.current_a - 4e-9).abs() > 1e-15
            || (active.result.strategy_current_a - 8e-9).abs() > 1e-15
            || (sleep.result.strategy_current_a - 6e-9).abs() > 1e-15
            || (active.result.strategy_power_uw - 0.008).abs() > 1e-9
            || (sleep.result.strategy_power_uw - 0.006).abs() > 1e-9
            || !report.contains("power mode 'active'")
            || !report.contains("power mode 'sleep'")
        {
            Err(format!("{modes:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_upf_mode_domain_activity_scales_dynamic_power", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let modes = eda_api::power_grid_ir_upf_power_modes(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let expected_idle = power.total_power_uw - 0.75 * power.dynamic_power_uw;
        let active = &modes.modes[0];
        let idle = &modes.modes[1];
        if (active.result.total_power_uw - power.total_power_uw).abs() > 1e-9
            || (idle.result.total_power_uw - expected_idle).abs() > 1e-9
            || idle.result.total_power_uw >= active.result.total_power_uw
            || (active.result.total_current_a - power.total_power_uw / 1_000_000.0).abs() > 1e-12
        {
            Err(format!(
                "power={:?}\nactive={:?}\nidle={:?}",
                power, active.result, idle.result
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_upf_power_mode_ir_analysis", || {
        let flow = eda_api::run_flow(
            "module m(input a, output y); assign y = ~a; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_state sw on
set_power_switch_off_leakage sw 1e-9
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
set_active_power_mode active
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let single = eda_api::power_grid_ir_upf(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let modes = eda_api::power_grid_ir_upf_power_modes(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let active = &modes.modes[0];
        let sleep = &modes.modes[1];
        if single.power_mode.as_deref() != Some("active")
            || active.result.power_mode.as_deref() != Some("active")
            || sleep.result.power_mode.as_deref() != Some("sleep")
            || active.result.total_current_a <= sleep.result.total_current_a
            || !active.result.switches[0].enabled
            || sleep.result.switches[0].enabled
            || modes.worst_power_mode != 0
            || modes.worst_drop_mode != 0
            || !report.contains("power mode 'active'")
            || !report.contains("power mode 'sleep'")
            || !report.contains("worst power 'active'")
        {
            Err(format!("{single:?}\n{modes:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_post_route_upf_power_mode_mesh", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let physical = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#,
        )?;
        let vectors = eda_api::default_power_vectors(&physical.best().annotated_netlist);
        let options = eda_api::PostRoutePowerIrOptions {
            power_mesh: Some(eda_api::PowerMeshOptions {
                rows: 3,
                columns: 3,
                use_boundary_taps: false,
                taps: vec![eda_api::PowerMeshTap::both(0, 0)],
                ..Default::default()
            }),
            ..Default::default()
        };
        let modes = eda_api::analyze_physical_timing_power_ir_power_modes_with_upf(
            physical.best(),
            &vectors,
            &options,
            &intent,
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let active_mesh = modes.modes[0]
            .power_mesh
            .as_ref()
            .ok_or("active mode omitted power mesh")?;
        let idle_mesh = modes.modes[1]
            .power_mesh
            .as_ref()
            .ok_or("idle mode omitted power mesh")?;
        if active_mesh.total_current_a <= idle_mesh.total_current_a
            || active_mesh.worst_drop_v <= idle_mesh.worst_drop_v
            || active_mesh.cells.len() != physical.best().annotated_netlist.cell_count()
            || idle_mesh.cells.len() != physical.best().annotated_netlist.cell_count()
            || !report.contains("power mode 'active'")
            || !report.contains("power mode 'idle'")
            || !report.contains("Power-mesh IR report:")
        {
            Err(format!("{modes:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_upf_power_modes_with_multilayer_mesh", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#,
        )?;
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let modes = eda_api::power_grid_ir_upf_power_modes_with_multilayer_mesh(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &eda_api::UpfIrOptions::default(),
            &{
                let mut mesh_options = eda_api::MultilayerPowerMeshOptions {
                    rows: 3,
                    columns: 3,
                    ..Default::default()
                };
                mesh_options.use_boundary_taps = false;
                mesh_options.cell_layers = vec![1; flow.netlist.cell_count()];
                for row in 0..3_usize {
                    for column in 0..3_usize {
                        if row == 0 || row == 2 || column == 0 || column == 2 {
                            mesh_options.taps.push(eda_api::PowerMeshLayerTap {
                                layer: 0,
                                row,
                                column,
                                vdd: true,
                                gnd: true,
                            });
                        }
                        mesh_options
                            .vias
                            .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
                    }
                }
                mesh_options
            },
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let active = modes.modes[0]
            .multilayer_power_mesh
            .as_ref()
            .ok_or("active mode omitted multilayer mesh")?;
        let idle = modes.modes[1]
            .multilayer_power_mesh
            .as_ref()
            .ok_or("idle mode omitted multilayer mesh")?;
        if active.total_current_a <= idle.total_current_a
            || active.nodes.len() != idle.nodes.len()
            || modes.modes.iter().any(|mode| mode.power_mesh.is_some())
            || modes.modes[0].worst_em_lifetime_hours.is_none()
            || modes.modes[1].worst_em_lifetime_hours.is_none()
            || modes.modes[0].worst_em_lifetime_hours > modes.modes[1].worst_em_lifetime_hours
            || modes.worst_em_lifetime_mode != 0
            || !report.contains("Multilayer power-mesh IR report:")
            || !report.contains("worst EM 'active'")
        {
            Err(format!("{modes:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_upf_domain_multilayer_meshes", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, d, output y, z); assign y = a & b; assign z = c | d; endmodule",
            None,
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode on
set_active_power_mode on
"#,
        )?;
        let domain_options = |voltage_v| eda_api::DomainIrOptions {
            voltage_v,
            vdd_ohm_per_site: 0.01,
            gnd_ohm_per_site: 0.01,
            max_total_drop_v: 0.05,
            max_rail_current_a: 0.05,
        };
        let ir_options = eda_api::UpfIrOptions {
            default: Some(domain_options(1.0)),
            domains: std::collections::BTreeMap::from([
                ("DOMAIN_A".to_string(), domain_options(0.8)),
                ("DOMAIN_B".to_string(), domain_options(0.65)),
            ]),
        };
        let power = eda_api::quick_power(&flow.netlist, &eda_api::PowerOptions::default())?;
        let result = eda_api::power_grid_ir_upf(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &ir_options,
        )?;
        let mut mesh_options = eda_api::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            ..Default::default()
        };
        mesh_options.use_boundary_taps = false;
        mesh_options.cell_layers = vec![1; flow.netlist.cell_count()];
        for row in 0..3_usize {
            for column in 0..3_usize {
                if row == 0 || row == 2 || column == 0 || column == 2 {
                    mesh_options.taps.push(eda_api::PowerMeshLayerTap {
                        layer: 0,
                        row,
                        column,
                        vdd: true,
                        gnd: true,
                    });
                }
                mesh_options
                    .vias
                    .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
            }
        }

        let domains = eda_api::upf_domain_multilayer_meshes(
            &flow.netlist,
            &flow.placement,
            &result,
            &mesh_options,
        )?;
        let domain_a = domains
            .iter()
            .find(|domain| domain.power_domain == "DOMAIN_A")
            .ok_or("missing DOMAIN_A mesh")?;
        let domain_b = domains
            .iter()
            .find(|domain| domain.power_domain == "DOMAIN_B")
            .ok_or("missing DOMAIN_B mesh")?;
        let expected_a = result
            .cells
            .iter()
            .filter(|cell| cell.power_domain == "DOMAIN_A")
            .map(|cell| cell.current_ma / 1_000.0)
            .sum::<f64>();
        let expected_b = result
            .cells
            .iter()
            .filter(|cell| cell.power_domain == "DOMAIN_B")
            .map(|cell| cell.current_ma / 1_000.0)
            .sum::<f64>();
        if result.cells.len() != flow.netlist.cell_count()
            || result
                .cells
                .iter()
        .any(|cell| !matches!(cell.power_domain.as_str(), "DOMAIN_A" | "DOMAIN_B"))
            || (domain_a.voltage_v - 0.8).abs() > 1e-12
            || (domain_b.voltage_v - 0.65).abs() > 1e-12
            || domain_a.mesh.is_none()
            || domain_b.mesh.is_none()
            || (domain_a.total_current_a - expected_a).abs() > 1e-15
            || (domain_b.total_current_a - expected_b).abs() > 1e-15
            || domain_a.worst_em_lifetime_hours.is_none()
            || domain_b.worst_em_lifetime_hours.is_none()
        {
            return Err(format!("{result:?}\n{domains:?}"));
        }

        let modes = eda_api::power_grid_ir_upf_power_modes_with_multilayer_mesh(
            &flow.netlist,
            &flow.placement,
            &power,
            &intent,
            &ir_options,
            &mesh_options,
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let mode = &modes.modes[0];
        if mode.domain_multilayer_meshes.len() < 2
            || !mode
                .domain_multilayer_meshes
                .iter()
                .any(|domain| domain.power_domain == "DOMAIN_A" && domain.mesh.is_some())
            || !mode
                .domain_multilayer_meshes
                .iter()
                .any(|domain| domain.power_domain == "DOMAIN_B" && domain.mesh.is_some())
            || !report.contains("DOMAIN-MESH 'DOMAIN_A'")
            || !report.contains("DOMAIN-MESH 'DOMAIN_B'")
        {
            Err(format!("{modes:?}\n{report}"))
        } else {
            let mut coupled_options = mesh_options.clone();
            coupled_options.taps.clear();
            let coupled = eda_api::upf_coupled_multilayer_mesh(
                &flow.netlist,
                &flow.placement,
                &result,
                &coupled_options,
                &[
                    eda_api::MultilayerPowerDomainSupply::new(
                        "DOMAIN_A",
                        0.8,
                        vec![
                            eda_api::PowerMeshLayerTap {
                                layer: 0,
                                row: 0,
                                column: 0,
                                vdd: true,
                                gnd: false,
                            },
                            eda_api::PowerMeshLayerTap {
                                layer: 0,
                                row: 2,
                                column: 0,
                                vdd: false,
                                gnd: true,
                            },
                        ],
                    ),
                    eda_api::MultilayerPowerDomainSupply::new(
                        "DOMAIN_B",
                        0.65,
                        vec![
                            eda_api::PowerMeshLayerTap {
                                layer: 0,
                                row: 0,
                                column: 2,
                                vdd: true,
                                gnd: false,
                            },
                            eda_api::PowerMeshLayerTap {
                                layer: 0,
                                row: 2,
                                column: 2,
                                vdd: false,
                                gnd: true,
                            },
                        ],
                    ),
                ],
            )?;
            let coupled_report = eda_api::multilayer_power_mesh_report(&coupled);
            if coupled.power_domains.len() == 2
                && coupled
                    .cells
                    .iter()
                    .all(|cell| cell.power_domain.is_some())
                && coupled_report.contains("domain 'DOMAIN_A' V=0.800000")
                && coupled_report.contains("domain 'DOMAIN_B' V=0.650000")
            {
                Ok(())
            } else {
                Err(format!("{coupled:?}\n{coupled_report}"))
            }
        }
    });

    c.add("api_post_route_upf_power_mode_multilayer_mesh", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let physical = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
create_power_mode idle
set_power_mode_switch idle sw on
set_power_mode_domain_activity idle TOP 0.25
set_active_power_mode active
"#,
        )?;
        let vectors = eda_api::default_power_vectors(&physical.best().annotated_netlist);
        let options = eda_api::PostRoutePowerIrOptions {
            multilayer_power_mesh: Some(eda_api::MultilayerPowerMeshOptions {
                rows: 3,
                columns: 3,
                ..Default::default()
            }),
            ..Default::default()
        };
        let single =
            eda_api::analyze_physical_timing_power_ir(physical.best(), &vectors, &options)?;
        let modes = eda_api::analyze_physical_timing_power_ir_power_modes_with_upf(
            physical.best(),
            &vectors,
            &options,
            &intent,
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let active = modes.modes[0]
            .multilayer_power_mesh
            .as_ref()
            .ok_or("active mode omitted post-route multilayer mesh")?;
        let idle = modes.modes[1]
            .multilayer_power_mesh
            .as_ref()
            .ok_or("idle mode omitted post-route multilayer mesh")?;
        if single.multilayer_power_mesh.is_none()
            || active.total_current_a <= idle.total_current_a
            || active.nodes.len() != idle.nodes.len()
            || modes.modes[0].worst_em_lifetime_hours.is_none()
            || modes.modes[1].worst_em_lifetime_hours.is_none()
            || modes.modes[0].worst_em_lifetime_hours > modes.modes[1].worst_em_lifetime_hours
            || modes.worst_em_lifetime_mode != 0
            || !report.contains("Multilayer power-mesh IR report:")
            || !report.contains("worst EM 'active'")
            || !eda_api::post_route_power_ir_report(&single)
                .contains("Multilayer power-mesh IR report:")
        {
            Err(format!("{single:?}\n{modes:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("api_post_route_upf_coupled_multilayer_mesh", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output z); wire n; assign n = a & b; assign z = c | n; endmodule",
            None,
        )?;
        let physical = eda_api::run_physical_timing_flow(
            &flow.netlist,
            &eda_api::PhysicalTimingFlowOptions {
                iterations: 1,
                ..Default::default()
            },
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode on
set_active_power_mode on
"#,
        )?;
        let domain_options = |voltage_v| eda_api::DomainIrOptions {
            voltage_v,
            vdd_ohm_per_site: 0.01,
            gnd_ohm_per_site: 0.01,
            max_total_drop_v: 0.05,
            max_rail_current_a: 0.05,
        };
        let mut mesh_options = eda_api::MultilayerPowerMeshOptions {
            rows: 3,
            columns: 3,
            use_boundary_taps: false,
            cell_layers: vec![1; flow.netlist.cell_count()],
            ..Default::default()
        };
        for row in 0..3_usize {
            for column in 0..3_usize {
                mesh_options
                    .vias
                    .push(eda_api::PowerMeshVia::new(row, column, 0, 1, 0.02, 0.02));
            }
        }
        mesh_options.taps = vec![
            eda_api::PowerMeshLayerTap {
                layer: 0,
                row: 2,
                column: 0,
                vdd: false,
                gnd: true,
            },
            eda_api::PowerMeshLayerTap {
                layer: 0,
                row: 2,
                column: 2,
                vdd: false,
                gnd: true,
            },
        ];
        mesh_options.power_domains = vec![
            eda_api::MultilayerPowerDomainSupply::new(
                "DOMAIN_A",
                0.8,
                vec![eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 0,
                    vdd: true,
                    gnd: false,
                }],
            ),
            eda_api::MultilayerPowerDomainSupply::new(
                "DOMAIN_B",
                0.65,
                vec![eda_api::PowerMeshLayerTap {
                    layer: 0,
                    row: 0,
                    column: 2,
                    vdd: true,
                    gnd: false,
                }],
            ),
        ];
        let options = eda_api::PostRoutePowerIrOptions {
            upf: eda_api::UpfIrOptions {
                default: Some(domain_options(1.0)),
                domains: std::collections::BTreeMap::from([
                    ("DOMAIN_A".to_string(), domain_options(0.8)),
                    ("DOMAIN_B".to_string(), domain_options(0.65)),
                ]),
            },
            multilayer_power_mesh: Some(mesh_options),
            ..Default::default()
        };
        let vectors = eda_api::default_power_vectors(&physical.best().annotated_netlist);
        let single = eda_api::analyze_physical_timing_power_ir_with_upf(
            physical.best(),
            &vectors,
            &options,
            &intent,
        )?;
        let modes = eda_api::analyze_physical_timing_power_ir_power_modes_with_upf(
            physical.best(),
            &vectors,
            &options,
            &intent,
        )?;
        let report = eda_api::upf_power_mode_ir_report(&modes);
        let single_mesh = single
            .multilayer_power_mesh
            .as_ref()
            .ok_or("post-route result omitted coupled mesh")?;
        let mode_mesh = modes.modes[0]
            .multilayer_power_mesh
            .as_ref()
            .ok_or("power mode omitted coupled mesh")?;
        if single_mesh.power_domains.len() != 2
            || single_mesh
                .cells
                .iter()
                .any(|cell| cell.power_domain.is_none())
            || !modes.modes[0].coupled_multilayer_mesh
            || !modes.modes[0].domain_multilayer_meshes.is_empty()
            || mode_mesh.power_domains.len() != 2
            || !report.contains("Multilayer power-mesh IR report:")
            || !report.contains("coupled_multilayer_mesh=true")
            || !report.contains("domain 'DOMAIN_A' V=0.800000")
            || !report.contains("domain 'DOMAIN_B' V=0.650000")
            || !eda_api::post_route_power_ir_report(&single)
                .contains("domain 'DOMAIN_A' V=0.800000")
        {
            Err(format!("{single:?}\n{modes:?}\n{report}"))
        } else {
            match eda_api::analyze_physical_timing_power_ir(
                physical.best(),
                &vectors,
                &options,
            ) {
                Err(error) if error.contains("require UPF power intent") => Ok(()),
                Err(error) => Err(error),
                Ok(result) => Err(format!(
                    "coupled domains unexpectedly solved without UPF: {result:?}"
                )),
            }
        }
    });

    c.add("api_multicorner_upf_power_mode_ir_analysis", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
            None,
        )?;
        let constraints = eda_api::parse_timing_constraints(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
        "#,
        )?;
        let physical_scenarios = vec![
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
        let physical = eda_api::run_multi_corner_physical_timing_flow(
            &eda_api::MultiCornerPhysicalTimingFlowOptions {
                scenarios: physical_scenarios,
                iterations: 1,
                ..Default::default()
            },
        )?;
        let intent = eda_api::parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
"#,
        )?;
        let power_scenarios = vec![
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "slow",
                eda_api::PostRoutePowerIrOptions {
                    multilayer_power_mesh: Some(eda_api::MultilayerPowerMeshOptions {
                        rows: 3,
                        columns: 3,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )?,
            eda_api::MultiCornerPostRoutePowerIrScenario::new(
                "fast",
                eda_api::PostRoutePowerIrOptions {
                    multilayer_power_mesh: Some(eda_api::MultilayerPowerMeshOptions {
                        rows: 3,
                        columns: 3,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )?,
        ];
        let Some(reference) = physical.best().corners.first() else {
            return Err("MMMC physical flow omitted reference corner".into());
        };
        let vectors = eda_api::default_power_vectors(&reference.evaluation.annotated_netlist);
        let result = eda_api::analyze_multi_corner_physical_timing_power_ir_power_modes_with_upf(
            physical.best(),
            &vectors,
            &power_scenarios,
            &intent,
        )?;
        let report = eda_api::multi_corner_post_route_power_mode_ir_report(&result);
        if result.corners.len() != 2
            || result
                .corners
                .iter()
                .any(|corner| corner.modes.modes.len() != 2)
            || result.corners[0].scenario != "slow"
            || result.corners[1].scenario != "fast"
            || result.corners.iter().any(|corner| {
                corner.modes.modes.iter().any(|mode| {
                    mode.result
                        .switches
                        .iter()
                        .any(|switch| switch.enabled != (mode.power_mode == "active"))
                })
            })
            || !report.contains("Multi-corner UPF power-mode IR report:")
            || !report.contains("scenario 'slow'")
            || !report.contains("scenario 'fast'")
            || !report.contains("power mode 'active'")
            || !report.contains("power mode 'sleep'")
            || result.corners.iter().any(|corner| {
                corner
                    .modes
                    .modes
                    .iter()
                    .any(|mode| mode.worst_em_lifetime_hours.is_none())
            })
            || result.worst_em_lifetime_mode != 0
            || !report.contains("worst EM 'slow' / 'active'")
        {
            Err(format!("{result:?}\n{report}"))
        } else {
            Ok(())
        }
    });

    c.add("ir_rejects_invalid_voltage", || {
        let nl = synth();
        let placement = eda_pnr::place(&nl)?;
        let power_report = power(&nl);
        if eda_ir::analyze(
            &nl,
            &placement,
            &power_report,
            &eda_ir::IrOptions {
                voltage_v: 0.0,
                ..eda_ir::IrOptions::default()
            },
        )
        .is_ok()
        {
            Err("zero voltage accepted".into())
        } else {
            Ok(())
        }
    });
}
