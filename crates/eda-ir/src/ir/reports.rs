//! Human-readable IR, mesh, UPF, and power-mode reports.

use super::multilayer::multilayer_power_mesh_report;
use super::types::*;

pub fn report(result: &IrResult) -> String {
    let mut out = format!(
        "IR report: total power {:.6} uW, current {:.6} A, worst drop {:.6} V on row {}\n",
        result.total_power_uw,
        result.total_current_a,
        result.worst_drop_v,
        result
            .worst_row
            .map(|row| row.to_string())
            .unwrap_or_else(|| "-".into())
    );
    for row in &result.rows {
        out.push_str(&format!(
            "  ROW {} I={:.6}A VDD={:.6}V GND={:.6}V total={:.6}V EMmargin={:.6}A\n",
            row.row,
            row.total_current_a,
            row.vdd_worst_drop_v,
            row.gnd_worst_drop_v,
            row.total_drop_v,
            row.em_margin
        ));
    }
    for finding in &result.findings {
        let severity = match finding.severity {
            IrSeverity::Warning => "WARN",
            IrSeverity::Error => "ERROR",
        };
        out.push_str(&format!(
            "  {} [{}] {}\n",
            severity, finding.code, finding.message
        ));
    }
    out
}

/// Render a finite-difference power-mesh result.
pub fn report_mesh(result: &PowerMeshResult) -> String {
    let mut out = format!(
        "Power-mesh IR report: total power {:.6} uW, current {:.6} A, worst drop {:.6} V at {:?}, worst segment {:.6} A, iterations {}, final delta {:.3e} V\n",
        result.total_power_uw,
        result.total_current_a,
        result.worst_drop_v,
        result.worst_node,
        result.worst_segment_current_a,
        result.iterations,
        result.final_delta_v
    );
    for node in &result.nodes {
        if node.injected_current_a == 0.0 && node.total_drop_v == 0.0 {
            continue;
        }
        out.push_str(&format!(
            "  NODE ({},{}) I={:.9}A VDD={:.9}V GND={:.9}V total_drop={:.9}V\n",
            node.row,
            node.column,
            node.injected_current_a,
            node.vdd_v,
            node.gnd_v,
            node.total_drop_v
        ));
    }
    for cell in &result.cells {
        out.push_str(&format!(
            "  CELL '{}' node=({},{}) I={:.9}A local_drop={:.9}V\n",
            cell.name, cell.mesh_row, cell.mesh_column, cell.current_a, cell.local_total_drop_v
        ));
    }
    for segment in &result.segments {
        if segment.current_a == 0.0 {
            continue;
        }
        out.push_str(&format!(
            "  SEGMENT {:?} {:?} ({},{})-({},{}) I={:.9}A R={:.6}ohm drop={:.9}V\n",
            segment.network,
            segment.orientation,
            segment.from_row,
            segment.from_column,
            segment.to_row,
            segment.to_column,
            segment.current_a,
            segment.resistance_ohm,
            segment.voltage_drop_v
        ));
    }
    for finding in &result.findings {
        let severity = match finding.severity {
            IrSeverity::Warning => "WARN",
            IrSeverity::Error => "ERROR",
        };
        out.push_str(&format!(
            "  {} [{}] {}\n",
            severity, finding.code, finding.message
        ));
    }
    out
}

/// Render a UPF-aware IR/EM report.
pub fn report_upf(result: &UpfIrResult) -> String {
    let mut out = format!(
        "UPF IR report: mode '{}', total power {:.6} uW, current {:.6} A, switch leakage {:.6} A / {:.6} uW, strategy current {:.6} A / {:.6} uW, worst drop {:.6} V in domain '{}' on row {}\n",
        result.power_mode.as_deref().unwrap_or("default"),
        result.total_power_uw,
        result.total_current_a,
        result.switch_leakage_current_a,
        result.switch_leakage_power_uw,
        result.strategy_current_a,
        result.strategy_power_uw,
        result.worst_drop_v,
        result
            .worst_power_domain
            .as_deref()
            .unwrap_or("DEFAULT"),
        result
            .worst_row
            .map(|row| row.to_string())
            .unwrap_or_else(|| "-".into())
    );
    for domain in &result.domains {
        if domain.cell_count == 0
            && domain.strategy_current_a == 0.0
            && domain.switch_leakage_current_a == 0.0
        {
            continue;
        }
        out.push_str(&format!(
            "  DOMAIN '{}' powered={} cells={} power={:.6}uW I={:.6}A switch_leakage={:.6}A/{:.6}uW strategy={:.6}A/{:.6}uW worst_drop={:.6}V worst_row={} worst_rail={:.6}A\n",
            domain.name,
            domain.powered,
            domain.cell_count,
            domain.total_power_uw,
            domain.total_current_a,
            domain.switch_leakage_current_a,
            domain.switch_leakage_power_uw,
            domain.strategy_current_a,
            domain.strategy_power_uw,
            domain.worst_drop_v,
            domain
                .worst_row
                .map(|row| row.to_string())
                .unwrap_or_else(|| "-".into()),
            domain.worst_rail_current_a
        ));
    }
    for row in &result.rows {
        out.push_str(&format!(
            "  ROW {} I={:.6}A worst_total={:.6}V conservative_EM_margin={:.6}A\n",
            row.row, row.total_current_a, row.total_drop_v, row.em_margin
        ));
    }
    for switch in &result.switches {
        out.push_str(&format!(
            "  SWITCH '{}' domain='{}' enabled={} modeled={} R={:.6}ohm I={:.6}A off_leakage={:.9}A drop={:.6}V\n",
            switch.name,
            switch.power_domain,
            switch.enabled,
            switch.modeled,
            switch.resistance_ohm,
            switch.current_a,
            switch.off_leakage_a,
            switch.drop_v
        ));
    }
    for strategy in &result.strategies {
        out.push_str(&format!(
            "  STRATEGY kind='{}' name='{}' domain='{}' I={:.9}A P={:.6}uW\n",
            strategy.kind,
            strategy.name,
            strategy.power_domain,
            strategy.current_a,
            strategy.power_uw
        ));
    }
    for finding in &result.findings {
        let severity = match finding.severity {
            IrSeverity::Warning => "WARN",
            IrSeverity::Error => "ERROR",
        };
        out.push_str(&format!(
            "  {} [{}] {}\n",
            severity, finding.code, finding.message
        ));
    }
    out
}

/// Render all declared UPF power-mode IR/EM results.
pub fn report_upf_power_modes(result: &UpfPowerModeIrResult) -> String {
    let worst_power = &result.modes[result.worst_power_mode];
    let worst_drop = &result.modes[result.worst_drop_mode];
    let worst_em_lifetime = &result.modes[result.worst_em_lifetime_mode];
    let worst_em_label = worst_em_lifetime
        .worst_em_lifetime_hours
        .map_or("<unavailable>".to_string(), |_| {
            worst_em_lifetime.power_mode.clone()
        });
    let mut out = format!(
        "UPF power-mode IR report: {} mode(s), worst power '{}', worst drop '{}', worst EM '{}', error findings {}\n",
        result.modes.len(),
        worst_power.power_mode,
        worst_drop.power_mode,
        worst_em_label,
        result.error_findings
    );
    for mode in &result.modes {
        let em = mode.worst_em_lifetime_hours.map_or_else(
            || "<unavailable>".to_string(),
            |lifetime| {
                if lifetime.is_infinite() {
                    "<unbounded>".to_string()
                } else {
                    format!("{lifetime:.3e} h")
                }
            },
        );
        out.push_str(&format!(
            "  power mode '{}', coupled_multilayer_mesh={}, worst EM {}\n",
            mode.power_mode, mode.coupled_multilayer_mesh, em
        ));
        for line in report_upf(&mode.result).lines() {
            out.push_str(&format!("  {}\n", line));
        }
        for domain in &mode.domain_multilayer_meshes {
            let mesh = if domain.mesh.is_some() {
                "present"
            } else {
                "not-solved"
            };
            let em = domain.worst_em_lifetime_hours.map_or_else(
                || "<unavailable>".to_string(),
                |lifetime| {
                    if lifetime.is_infinite() {
                        "<unbounded>".to_string()
                    } else {
                        format!("{lifetime:.3e} h")
                    }
                },
            );
            out.push_str(&format!(
                "  DOMAIN-MESH '{}' powered={} V={:.6} I={:.9} A worst_drop={:.9} V worst_EM={} mesh={}\n",
                domain.power_domain,
                domain.powered,
                domain.voltage_v,
                domain.total_current_a,
                domain.worst_drop_v,
                em,
                mesh
            ));
        }
        if let Some(mesh) = &mode.power_mesh {
            for line in report_mesh(mesh).lines() {
                out.push_str(&format!("  {}\n", line));
            }
        }
        if let Some(mesh) = &mode.multilayer_power_mesh {
            for line in multilayer_power_mesh_report(mesh).lines() {
                out.push_str(&format!("  {}\n", line));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    use eda_core::netlist::Netlist;
    use eda_core::netlist::{Cell, CellKind};
    use eda_pnr::Placement;
    use eda_power::PowerReport;

    #[test]
    fn validates_options() {
        let nl = Netlist::new("empty");
        let placement = Placement {
            rows: 0,
            cell_pos: Vec::new(),
            cell_width: Vec::new(),
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 1.0,
            vectors: 1,
            intervals: 1,
            dynamic_power_uw: 0.0,
            static_power_uw: 0.0,
            total_power_uw: 0.0,
            activity_by_net: Default::default(),
            cells: Vec::new(),
        };
        let bad = IrOptions {
            voltage_v: 0.0,
            ..IrOptions::default()
        };
        assert!(analyze(&nl, &placement, &power, &bad).is_err());
    }

    #[test]
    fn upf_domains_use_domain_specific_supplies() {
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let y1 = nl.add_net("y1");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y1,
        });
        let y2 = nl.add_net("y2");
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y2,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![
                eda_pnr::Pos { row: 0, site: 0 },
                eda_pnr::Pos { row: 0, site: 10 },
            ],
            cell_width: vec![2, 2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 0.0,
            static_power_uw: 0.0,
            total_power_uw: 2000.0,
            activity_by_net: Default::default(),
            cells: vec![
                eda_power::CellPower {
                    cell: 0,
                    name: "u1".into(),
                    width_sites: 2,
                    activity: 1.0,
                    dynamic_power_uw: 1000.0,
                },
                eda_power::CellPower {
                    cell: 1,
                    name: "u2".into(),
                    width_sites: 2,
                    activity: 1.0,
                    dynamic_power_uw: 1000.0,
                },
            ],
        };
        let upf = eda_upf::parse_upf(
            r#"
create_power_domain A -elements {u1}
create_power_domain B -elements {u2}
create_supply_net vdda -domain A
create_supply_net vssa -domain A
create_supply_net vddb -domain B
create_supply_net vssb -domain B
set_domain_supply_net A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net B -primary_power_net vddb -primary_ground_net vssb
create_power_switch sw_a -domain A -input_supply_port vdda -output_supply_port vdda
set_power_switch_on_resistance sw_a 10.0
"#,
        )
        .unwrap();
        let low_drop = DomainIrOptions::from(IrOptions::default());
        let high_drop = DomainIrOptions {
            vdd_ohm_per_site: 0.1,
            gnd_ohm_per_site: 0.1,
            ..low_drop
        };
        let result = analyze_upf(
            &nl,
            &placement,
            &power,
            &upf,
            &UpfIrOptions {
                domains: [("A".to_string(), high_drop), ("B".to_string(), low_drop)]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.cells[0].power_domain, "A");
        assert_eq!(result.cells[1].power_domain, "B");
        assert!((result.total_power_uw - 2000.0).abs() < 1e-12);
        assert!((result.total_current_a - 0.002).abs() < 1e-12);
        assert_eq!(result.worst_power_domain.as_deref(), Some("A"));
        assert_eq!(result.switches.len(), 1);
        assert_eq!(result.switches[0].name, "sw_a");
        assert!((result.switches[0].current_a - 0.001).abs() < 1e-12);
        assert!((result.switches[0].drop_v - 0.01).abs() < 1e-12);
        let a_drop = result
            .domain_rows
            .iter()
            .find(|row| row.power_domain == "A")
            .unwrap()
            .total_drop_v;
        let b_drop = result
            .domain_rows
            .iter()
            .find(|row| row.power_domain == "B")
            .unwrap()
            .total_drop_v;
        assert!(a_drop > b_drop);
        assert!(
            a_drop > b_drop
                && (result
                    .domains
                    .iter()
                    .find(|domain| domain.name == "A")
                    .unwrap()
                    .switch_drop_v
                    - 0.01)
                    .abs()
                    < 1e-12
                && report_upf(&result).contains("DOMAIN 'A'")
                && report_upf(&result).contains("SWITCH 'sw_a'")
        );
    }

    #[test]
    fn solves_centered_power_mesh_drop() {
        let mut nl = Netlist::new("mesh");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        let placement = Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: vec![2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 1000.0,
            static_power_uw: 0.0,
            total_power_uw: 1000.0,
            activity_by_net: Default::default(),
            cells: vec![eda_power::CellPower {
                cell: 0,
                name: "u".into(),
                width_sites: 2,
                activity: 1.0,
                dynamic_power_uw: 1000.0,
            }],
        };
        let options = PowerMeshOptions {
            rows: 3,
            columns: 3,
            vdd_horizontal_ohm_per_segment: 1.0,
            vdd_vertical_ohm_per_segment: 1.0,
            gnd_horizontal_ohm_per_segment: 1.0,
            gnd_vertical_ohm_per_segment: 1.0,
            max_node_current_a: 0.01,
            ..PowerMeshOptions::default()
        };
        let result = analyze_mesh(&nl, &placement, &power, &options).unwrap();
        assert_eq!(result.worst_node, Some((1, 1)));
        assert!((result.total_current_a - 0.001).abs() < 1e-12);
        assert!((result.worst_drop_v - 0.0005).abs() < 1e-12);
        let node = &result.nodes[4];
        assert!((node.vdd_v - 0.99975).abs() < 1e-12);
        assert!((node.gnd_v - 0.00025).abs() < 1e-12);
        assert!((node.injected_current_a - 0.001).abs() < 1e-12);
        assert_eq!(result.cells[0].mesh_row, 1);
        assert_eq!(result.cells[0].mesh_column, 1);
        assert_eq!(result.segments.len(), 24);
        assert!((result.worst_segment_current_a - 0.00025).abs() < 1e-12);
        assert!(!result.has_errors());
        assert!(report_mesh(&result).contains("Power-mesh IR report"));

        let bad = PowerMeshOptions {
            rows: 0,
            ..options.clone()
        };
        assert!(analyze_mesh(&nl, &placement, &power, &bad).is_err());

        let corner_tap = PowerMeshOptions {
            use_boundary_taps: false,
            taps: vec![PowerMeshTap::both(0, 0)],
            ..options
        };
        let corner = analyze_mesh(&nl, &placement, &power, &corner_tap).unwrap();
        assert!(corner.worst_drop_v > result.worst_drop_v);
        assert!(corner
            .segments
            .iter()
            .any(|segment| segment.current_a > 0.0));

        let no_taps = PowerMeshOptions {
            use_boundary_taps: false,
            taps: Vec::new(),
            ..options
        };
        assert!(analyze_mesh(&nl, &placement, &power, &no_taps).is_err());
    }

    #[test]
    fn upf_strategy_currents_use_supply_domains() {
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![eda_pnr::Pos { row: 0, site: 0 }],
            cell_width: vec![2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 0.0,
            static_power_uw: 0.0,
            total_power_uw: 1000.0,
            activity_by_net: Default::default(),
            cells: vec![eda_power::CellPower {
                cell: 0,
                name: "u".into(),
                width_sites: 2,
                activity: 1.0,
                dynamic_power_uw: 1000.0,
            }],
        };
        let upf = eda_upf::parse_upf(
            r#"
create_power_domain TOP -elements {u}
create_power_domain AUX -elements {}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
create_supply_net vddaux -domain AUX
create_supply_net vssaux -domain AUX
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
set_domain_supply_net AUX -primary_power_net vddaux -primary_ground_net vssaux
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_state sw off
set_isolation iso -domain TOP -signal a -isolation_power_net vddaux
set_isolation_static_current iso 1e-9
set_retention ret -domain TOP -signal y -retention_power_net vddaux
set_retention_static_current ret 2e-9
set_level_shifter shift -domain TOP -input vdd -output vddaux
set_level_shifter_static_current shift 3e-9
"#,
        )
        .unwrap();
        let result = analyze_upf(&nl, &placement, &power, &upf, &UpfIrOptions::default()).unwrap();
        let report = report_upf(&result);
        assert_eq!(result.strategies.len(), 3);
        assert!(result.strategies.iter().all(|strategy| {
            strategy.power_domain == "AUX"
                && (strategy.power_uw - strategy.current_a * 1_000_000.0).abs() < 1e-12
        }));
        assert!((result.strategy_current_a - 6e-9).abs() < 1e-15);
        assert!((result.strategy_power_uw - 0.006).abs() < 1e-9);
        assert!((result.total_current_a - 6e-9).abs() < 1e-15);
        assert!((result.total_power_uw - 0.006).abs() < 1e-9);
        let aux = result
            .domains
            .iter()
            .find(|domain| domain.name == "AUX")
            .unwrap();
        assert!((aux.strategy_current_a - 6e-9).abs() < 1e-15);
        assert!((aux.strategy_power_uw - 0.006).abs() < 1e-9);
        assert!(
            report.contains("strategy current 0.000000 A / 0.006000 uW")
                && report.contains("STRATEGY kind='isolation' name='iso'")
                && report.contains("STRATEGY kind='retention' name='ret'")
                && report.contains("STRATEGY kind='level_shifter' name='shift'"),
            "{report}"
        );
    }

    #[test]
    fn upf_parallel_power_switches_share_current() {
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![eda_pnr::Pos { row: 0, site: 0 }],
            cell_width: vec![2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 0.0,
            static_power_uw: 0.0,
            total_power_uw: 1000.0,
            activity_by_net: Default::default(),
            cells: vec![eda_power::CellPower {
                cell: 0,
                name: "u".into(),
                width_sites: 2,
                activity: 1.0,
                dynamic_power_uw: 1000.0,
            }],
        };
        let upf = eda_upf::parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw0 -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_switch sw1 -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw0 10.0
set_power_switch_on_resistance sw1 10.0
"#,
        )
        .unwrap();
        let result = analyze_upf(&nl, &placement, &power, &upf, &UpfIrOptions::default()).unwrap();
        assert_eq!(result.switches.len(), 2);
        for switch in &result.switches {
            assert!((switch.current_a - 0.0005).abs() < 1e-12);
            assert!((switch.drop_v - 0.005).abs() < 1e-12);
        }
        let domain = result
            .domains
            .iter()
            .find(|domain| domain.name == "TOP")
            .unwrap();
        assert!((domain.switch_drop_v - 0.005).abs() < 1e-12);
    }

    #[test]
    fn upf_power_modes_scale_mesh_current() {
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        let placement = Placement {
            rows: 3,
            cell_pos: vec![eda_pnr::Pos {
                row: 1,
                site: eda_pnr::SITES_PER_ROW / 2 - 1,
            }],
            cell_width: vec![2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 1000.0,
            static_power_uw: 0.0,
            total_power_uw: 1000.0,
            activity_by_net: Default::default(),
            cells: vec![eda_power::CellPower {
                cell: 0,
                name: "u".into(),
                width_sites: 2,
                activity: 1.0,
                dynamic_power_uw: 1000.0,
            }],
        };
        let upf = eda_upf::parse_upf(
            r#"
create_power_domain TOP -elements {u}
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
"#,
        )
        .unwrap();
        let mesh_options = PowerMeshOptions {
            rows: 3,
            columns: 3,
            ..PowerMeshOptions::default()
        };
        let result = analyze_upf_power_modes_with_mesh(
            &nl,
            &placement,
            &power,
            &upf,
            &UpfIrOptions::default(),
            &mesh_options,
        )
        .unwrap();
        let active_mesh = result.modes[0].power_mesh.as_ref().unwrap();
        let idle_mesh = result.modes[1].power_mesh.as_ref().unwrap();
        assert!((active_mesh.total_current_a - 0.001).abs() < 1e-12);
        assert!((idle_mesh.total_current_a - 0.00025).abs() < 1e-12);
        assert!(active_mesh.worst_drop_v > idle_mesh.worst_drop_v);
        assert!(report_upf_power_modes(&result).contains("Power-mesh IR report"));
    }

    #[test]
    fn upf_off_power_switch_excludes_domain_current() {
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: y,
        });
        let placement = Placement {
            rows: 1,
            cell_pos: vec![eda_pnr::Pos { row: 0, site: 0 }],
            cell_width: vec![2],
            blockages: Vec::new(),
        };
        let power = PowerReport {
            frequency_mhz: 100.0,
            vectors: 2,
            intervals: 1,
            dynamic_power_uw: 1000.0,
            static_power_uw: 0.0,
            total_power_uw: 1000.0,
            activity_by_net: Default::default(),
            cells: vec![eda_power::CellPower {
                cell: 0,
                name: "u".into(),
                width_sites: 2,
                activity: 1.0,
                dynamic_power_uw: 1000.0,
            }],
        };
        let upf = eda_upf::parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_state sw off
set_power_switch_off_leakage sw 2e-9
"#,
        )
        .unwrap();
        let result = analyze_upf(&nl, &placement, &power, &upf, &UpfIrOptions::default()).unwrap();
        let report = report_upf(&result);
        assert!((result.total_current_a - 2e-9).abs() < 1e-15);
        assert!((result.total_power_uw - 0.002).abs() < 1e-9);
        assert!((result.switch_leakage_current_a - 2e-9).abs() < 1e-15);
        assert!((result.switch_leakage_power_uw - 0.002).abs() < 1e-9);
        assert!(!result.cells[0].powered);
        assert_eq!(result.cells[0].current_ma, 0.0);
        assert!(!result.switches[0].enabled);
        assert!(!result.switches[0].modeled);
        assert_eq!(result.switches[0].current_a, 0.0);
        assert!((result.switches[0].off_leakage_a - 2e-9).abs() < 1e-15);
        let domain = result
            .domains
            .iter()
            .find(|domain| domain.name == "TOP")
            .unwrap();
        assert!((domain.switch_leakage_current_a - 2e-9).abs() < 1e-15);
        assert!((domain.switch_leakage_power_uw - 0.002).abs() < 1e-9);
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.code == "IR-POWER-OFF"));
        assert!(
            report.contains("powered=false")
                && report.contains("enabled=false")
                && report.contains("switch leakage 0.000000 A / 0.002000 uW")
                && report.contains("IR-POWER-OFF"),
            "{report}"
        );
    }
}
