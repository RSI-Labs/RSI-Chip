//! UPF-aware IR, domain meshes, power modes, and coupled mesh analysis.

use super::mesh::analyze_mesh_currents;
use super::multilayer::analyze_multilayer_mesh_currents;
use super::types::*;
use super::validation::validate_domain_options;
use eda_core::netlist::Netlist;
use eda_pnr::{Placement, SITES_PER_ROW};
use eda_power::PowerReport;
use eda_upf::Upf;
use std::collections::{BTreeMap, BTreeSet};

fn upf_supply_domain<'a>(
    upf: &'a Upf,
    supply: Option<&str>,
    fallback: &'a str,
) -> Result<&'a str, String> {
    match supply {
        Some(net) => upf
            .supply_nets
            .get(net)
            .map(|supply| supply.domain.as_str())
            .ok_or_else(|| format!("UPF strategy references unknown supply net '{}'", net)),
        None => Ok(fallback),
    }
}

fn apply_upf_strategy_current(
    result: &mut UpfIrResult,
    domain_aggregates: &mut BTreeMap<String, UpfIrDomain>,
    domain_options: &BTreeMap<String, DomainIrOptions>,
    kind: &'static str,
    name: &str,
    power_domain: &str,
    current_a: f64,
) -> Result<(), String> {
    let options = domain_options.get(power_domain).ok_or_else(|| {
        format!(
            "UPF {} '{}' uses unknown IR domain '{}'",
            kind, name, power_domain
        )
    })?;
    let power_uw = current_a * options.voltage_v * 1_000_000.0;
    let aggregate = domain_aggregates
        .get_mut(power_domain)
        .ok_or_else(|| format!("missing IR aggregate for domain '{}'", power_domain))?;
    aggregate.strategy_current_a += current_a;
    aggregate.strategy_power_uw += power_uw;
    aggregate.total_current_a += current_a;
    aggregate.total_power_uw += power_uw;
    result.strategy_current_a += current_a;
    result.strategy_power_uw += power_uw;
    result.total_current_a += current_a;
    result.total_power_uw += power_uw;
    result.strategies.push(UpfIrStrategy {
        kind,
        name: name.to_string(),
        power_domain: power_domain.to_string(),
        current_a,
        power_uw,
    });
    Ok(())
}

/// Analyze placement-aware IR/EM with UPF power-domain-specific supplies.
///
/// Domain elements may name flat cells explicitly. An element equal to the
/// netlist module name is treated as the inclusive top domain. Cells without
/// a more specific domain use `options.default`.
pub fn analyze_upf(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    upf: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfIrResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("UPF IR analysis placement/netlist cell count mismatch".into());
    }
    if power.cells.len() != nl.cell_count() {
        return Err("UPF IR analysis power/netlist cell count mismatch".into());
    }

    let default_options = options
        .default
        .unwrap_or_else(|| DomainIrOptions::from(IrOptions::default()));
    validate_domain_options("DEFAULT", &default_options)?;
    let mut domain_options = BTreeMap::new();
    for name in upf.domains.keys() {
        let domain = options
            .domains
            .get(name)
            .copied()
            .unwrap_or(default_options);
        validate_domain_options(name, &domain)?;
        domain_options.insert(name.clone(), domain);
    }
    domain_options
        .entry("DEFAULT".to_string())
        .or_insert(default_options);

    let mut exact_domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut top_domains: BTreeSet<String> = BTreeSet::new();
    for domain in upf.domains.values() {
        for element in &domain.elements {
            if element == &nl.name {
                top_domains.insert(domain.name.clone());
            } else {
                exact_domains
                    .entry(element.clone())
                    .or_default()
                    .insert(domain.name.clone());
            }
        }
    }

    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    let mut cell_dynamic_power = vec![0.0_f64; nl.cell_count()];
    for report_cell in &power.cells {
        if report_cell.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                report_cell.cell
            ));
        }
        let leakage_share = power.static_power_uw * placement.cell_width[report_cell.cell] as f64
            / total_sites as f64;
        cell_dynamic_power[report_cell.cell] = report_cell.dynamic_power_uw;
        cell_power[report_cell.cell] = report_cell.dynamic_power_uw + leakage_share;
    }

    let mut cell_domains = Vec::with_capacity(nl.cell_count());
    for cell_index in 0..nl.cell_count() {
        let cell_name = nl.cell(cell_index).name.as_str();
        let exact = exact_domains.get(cell_name);
        let domain = if let Some(domains) = exact.filter(|domains| !domains.is_empty()) {
            if domains.len() > 1 {
                return Err(format!(
                    "cell '{}' is assigned to multiple UPF domains: {}",
                    cell_name,
                    domains.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            domains.iter().next().cloned().unwrap()
        } else if top_domains.len() == 1 {
            top_domains.iter().next().cloned().unwrap()
        } else if top_domains.len() > 1 {
            return Err(format!(
                "cell '{}' is covered by multiple top-level UPF domains: {}",
                cell_name,
                top_domains.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        } else {
            "DEFAULT".to_string()
        };
        if !domain_options.contains_key(&domain) {
            return Err(format!(
                "cell '{}' references unknown UPF domain '{}'",
                cell_name, domain
            ));
        }
        cell_domains.push(domain);
    }

    let mut switches_by_domain: BTreeMap<String, Vec<&eda_upf::PowerSwitch>> = BTreeMap::new();
    let active_mode = upf
        .active_power_mode
        .as_ref()
        .and_then(|name| upf.power_modes.get(name));
    if let Some(mode) = active_mode {
        for switch_name in mode.switch_states.keys() {
            if !upf.switches.contains_key(switch_name) {
                return Err(format!(
                    "active power mode '{}' references unknown switch '{}'",
                    mode.name, switch_name
                ));
            }
        }
        for switch_name in upf.switches.keys() {
            if !mode.switch_states.contains_key(switch_name) {
                return Err(format!(
                    "active power mode '{}' does not define state for switch '{}'",
                    mode.name, switch_name
                ));
            }
        }
        for (domain_name, factor) in &mode.domain_activity {
            if !factor.is_finite() || *factor < 0.0 {
                return Err(format!(
                    "active power mode '{}' has invalid activity factor {} for domain '{}'",
                    mode.name, factor, domain_name
                ));
            }
            if !domain_options.contains_key(domain_name) {
                return Err(format!(
                    "active power mode '{}' references unknown activity domain '{}'",
                    mode.name, domain_name
                ));
            }
        }
        for (key, current) in &mode.strategy_currents {
            if !current.is_finite() || *current < 0.0 {
                return Err(format!(
                    "active power mode '{}' has invalid strategy current {} for '{}'",
                    mode.name, current, key
                ));
            }
            let Some((kind, name)) = key.split_once(':') else {
                return Err(format!(
                    "active power mode '{}' has malformed strategy key '{}'",
                    mode.name, key
                ));
            };
            let exists = match kind {
                "isolation" => upf.isolations.contains_key(name),
                "retention" => upf.retentions.contains_key(name),
                "level_shifter" => upf.level_shifters.contains_key(name),
                _ => false,
            };
            if !exists {
                return Err(format!(
                    "active power mode '{}' references unknown strategy '{}'",
                    mode.name, key
                ));
            }
        }
    }
    let switch_state = |switch: &eda_upf::PowerSwitch| -> bool {
        active_mode
            .and_then(|mode| mode.switch_states.get(&switch.name).copied())
            .or(switch.state)
            .unwrap_or(true)
    };
    let strategy_current = |kind: &str, name: &str, base: Option<f64>| -> Option<f64> {
        let key = eda_upf::power_mode_strategy_key(kind, name);
        active_mode
            .and_then(|mode| mode.strategy_currents.get(&key).copied())
            .or(base)
    };
    for switch in upf.switches.values() {
        if !domain_options.contains_key(&switch.domain) {
            return Err(format!(
                "power switch '{}' references unknown IR domain '{}'",
                switch.name, switch.domain
            ));
        }
        if let Some(resistance) = switch.on_resistance_ohm {
            if !resistance.is_finite() || resistance <= 0.0 {
                return Err(format!(
                    "power switch '{}' has invalid on resistance {}",
                    switch.name, resistance
                ));
            }
        }
        if let Some(leakage) = switch.off_leakage_a {
            if !leakage.is_finite() || leakage < 0.0 {
                return Err(format!(
                    "power switch '{}' has invalid off-state leakage {}",
                    switch.name, leakage
                ));
            }
        }
        switches_by_domain
            .entry(switch.domain.clone())
            .or_default()
            .push(switch);
    }
    let domain_powered: BTreeMap<String, bool> = domain_options
        .keys()
        .map(|name| {
            let powered = switches_by_domain
                .get(name)
                .map(|switches| switches.iter().any(|switch| switch_state(switch)))
                .unwrap_or(true);
            (name.clone(), powered)
        })
        .collect();
    for (cell_index, domain) in cell_domains.iter().enumerate() {
        let activity = active_mode
            .and_then(|mode| mode.domain_activity.get(domain).copied())
            .unwrap_or(1.0);
        let leakage = cell_power[cell_index] - cell_dynamic_power[cell_index];
        cell_power[cell_index] = cell_dynamic_power[cell_index] * activity + leakage;
        if !domain_powered[domain] {
            cell_power[cell_index] = 0.0;
        }
    }

    let mut result = UpfIrResult {
        power_mode: upf.active_power_mode.clone(),
        ..Default::default()
    };
    let mut domain_aggregates: BTreeMap<String, UpfIrDomain> = domain_options
        .keys()
        .map(|name| {
            (
                name.clone(),
                UpfIrDomain {
                    name: name.clone(),
                    powered: domain_powered[name],
                    voltage_v: domain_options[name].voltage_v,
                    ..Default::default()
                },
            )
        })
        .collect();

    for strategy in upf.isolations.values() {
        if let Some(current) =
            strategy_current("isolation", &strategy.name, strategy.static_current_a)
        {
            if !current.is_finite() || current < 0.0 {
                return Err(format!(
                    "isolation '{}' has invalid static current {}",
                    strategy.name, current
                ));
            }
            let domain = upf_supply_domain(
                upf,
                strategy.isolation_power_net.as_deref(),
                &strategy.domain,
            )?;
            apply_upf_strategy_current(
                &mut result,
                &mut domain_aggregates,
                &domain_options,
                "isolation",
                &strategy.name,
                domain,
                current,
            )?;
        }
    }
    for strategy in upf.retentions.values() {
        if let Some(current) =
            strategy_current("retention", &strategy.name, strategy.static_current_a)
        {
            if !current.is_finite() || current < 0.0 {
                return Err(format!(
                    "retention '{}' has invalid static current {}",
                    strategy.name, current
                ));
            }
            let domain = upf_supply_domain(
                upf,
                strategy.retention_power_net.as_deref(),
                &strategy.domain,
            )?;
            apply_upf_strategy_current(
                &mut result,
                &mut domain_aggregates,
                &domain_options,
                "retention",
                &strategy.name,
                domain,
                current,
            )?;
        }
    }
    for strategy in upf.level_shifters.values() {
        if let Some(current) =
            strategy_current("level_shifter", &strategy.name, strategy.static_current_a)
        {
            if !current.is_finite() || current < 0.0 {
                return Err(format!(
                    "level shifter '{}' has invalid static current {}",
                    strategy.name, current
                ));
            }
            let domain = upf_supply_domain(upf, Some(&strategy.output_supply), &strategy.domain)?;
            apply_upf_strategy_current(
                &mut result,
                &mut domain_aggregates,
                &domain_options,
                "level_shifter",
                &strategy.name,
                domain,
                current,
            )?;
        }
    }

    for row in 0..placement.rows.max(1) {
        let mut row_current = 0.0_f64;
        let mut row_vdd_drop = 0.0_f64;
        let mut row_gnd_drop = 0.0_f64;
        let mut row_total_drop = 0.0_f64;
        let mut row_em_margin = f64::INFINITY;

        for (domain_name, domain) in &domain_options {
            let mut domain_row_current = 0.0_f64;
            let mut vdd_drop = 0.0_f64;
            let mut gnd_drop = 0.0_f64;
            let mut domain_cell_count = 0_usize;

            for cell_index in 0..nl.cell_count() {
                if placement.cell_pos[cell_index].row != row
                    || cell_domains[cell_index] != *domain_name
                {
                    continue;
                }
                let start = placement.cell_pos[cell_index].site;
                let width = placement.cell_width[cell_index].max(1);
                let end = (start + width).min(SITES_PER_ROW);
                let current_a = cell_power[cell_index] / domain.voltage_v / 1_000_000.0;
                domain_row_current += current_a;

                let center = (start + end) as f64 / 2.0;
                let length = SITES_PER_ROW as f64;
                let position = center.clamp(0.0, length);
                let factor = position * (length - position) / length;
                let local_vdd = current_a * domain.vdd_ohm_per_site * factor;
                let local_gnd = current_a * domain.gnd_ohm_per_site * factor;
                vdd_drop += local_vdd;
                gnd_drop += local_gnd;

                result.cells.push(UpfIrCell {
                    cell: cell_index,
                    name: nl.cell(cell_index).name.clone(),
                    power_domain: domain_name.clone(),
                    row,
                    site_start: start,
                    site_end: end,
                    voltage_v: domain.voltage_v,
                    powered: domain_powered[domain_name],
                    current_ma: current_a * 1000.0,
                    local_drop_v: local_vdd + local_gnd,
                });
                let aggregate = domain_aggregates
                    .get_mut(domain_name)
                    .ok_or_else(|| format!("missing IR aggregate for domain '{}'", domain_name))?;
                aggregate.cell_count += 1;
                aggregate.total_power_uw += cell_power[cell_index];
                aggregate.total_current_a += current_a;
                domain_cell_count += 1;
            }

            let total_drop = vdd_drop + gnd_drop;
            let em_margin = domain.max_rail_current_a - domain_row_current;
            row_current += domain_row_current;
            row_vdd_drop = row_vdd_drop.max(vdd_drop);
            row_gnd_drop = row_gnd_drop.max(gnd_drop);
            row_total_drop = row_total_drop.max(total_drop);
            row_em_margin = row_em_margin.min(em_margin);

            let aggregate = domain_aggregates
                .get_mut(domain_name)
                .ok_or_else(|| format!("missing IR aggregate for domain '{}'", domain_name))?;
            if total_drop > aggregate.worst_drop_v {
                aggregate.worst_drop_v = total_drop;
                aggregate.worst_row = Some(row);
            }
            aggregate.worst_rail_current_a = aggregate.worst_rail_current_a.max(domain_row_current);
            let _ = domain_cell_count;

            result.domain_rows.push(UpfIrDomainRow {
                power_domain: domain_name.clone(),
                row,
                total_current_a: domain_row_current,
                vdd_worst_drop_v: vdd_drop,
                gnd_worst_drop_v: gnd_drop,
                total_drop_v: total_drop,
                em_margin_a: em_margin,
            });
            if domain_row_current > domain.max_rail_current_a {
                result.findings.push(IrFinding {
                    code: "EM-CURRENT".into(),
                    severity: IrSeverity::Error,
                    row: Some(row),
                    message: format!(
                        "domain '{}' row {} current {:.6} A exceeds EM limit {:.6} A",
                        domain_name, row, domain_row_current, domain.max_rail_current_a
                    ),
                });
            } else if em_margin < domain.max_rail_current_a * 0.1 {
                result.findings.push(IrFinding {
                    code: "EM-MARGIN".into(),
                    severity: IrSeverity::Warning,
                    row: Some(row),
                    message: format!(
                        "domain '{}' row {} has less than 10% EM margin ({:.6} A remaining)",
                        domain_name, row, em_margin
                    ),
                });
            }
        }

        result.rows.push(IrRow {
            row,
            total_current_a: row_current,
            vdd_worst_drop_v: row_vdd_drop,
            gnd_worst_drop_v: row_gnd_drop,
            total_drop_v: row_total_drop,
            em_margin: row_em_margin,
        });
    }

    result.total_power_uw = domain_aggregates
        .values()
        .map(|domain| domain.total_power_uw)
        .sum();
    result.total_current_a = domain_aggregates
        .values()
        .map(|domain| domain.total_current_a)
        .sum();

    let mut switch_drops = BTreeMap::new();
    for switch in upf.switches.values() {
        let enabled = switch_state(switch);
        let resistance = switch.on_resistance_ohm.unwrap_or(0.0);
        let modeled = enabled && switch.on_resistance_ohm.is_some();
        let off_leakage = if enabled {
            0.0
        } else {
            switch.off_leakage_a.unwrap_or(0.0)
        };
        let (switch_current, drop) = if modeled {
            let parallel_resistance = upf
                .switches
                .values()
                .filter(|other| {
                    other.domain == switch.domain
                        && switch_state(other)
                        && other.on_resistance_ohm.is_some()
                })
                .map(|other| 1.0 / other.on_resistance_ohm.unwrap())
                .sum::<f64>()
                .recip();
            let domain_current = domain_aggregates
                .get(&switch.domain)
                .map(|domain| domain.total_current_a)
                .unwrap_or(0.0);
            let switch_current = domain_current * parallel_resistance / resistance;
            let drop = switch_current * resistance;
            switch_drops.insert(switch.domain.clone(), drop);
            (switch_current, drop)
        } else {
            (0.0, 0.0)
        };
        result.switches.push(UpfIrSwitch {
            name: switch.name.clone(),
            power_domain: switch.domain.clone(),
            enabled,
            modeled,
            resistance_ohm: resistance,
            current_a: switch_current,
            off_leakage_a: off_leakage,
            drop_v: drop,
        });
    }
    for switch_result in &result.switches {
        if switch_result.off_leakage_a == 0.0 {
            continue;
        }
        let voltage = domain_options
            .get(&switch_result.power_domain)
            .map(|domain| domain.voltage_v)
            .ok_or_else(|| {
                format!(
                    "missing IR options for domain '{}'",
                    switch_result.power_domain
                )
            })?;
        let leakage_power_uw = switch_result.off_leakage_a * voltage * 1_000_000.0;
        let aggregate = domain_aggregates
            .get_mut(&switch_result.power_domain)
            .ok_or_else(|| {
                format!(
                    "missing IR aggregate for domain '{}'",
                    switch_result.power_domain
                )
            })?;
        aggregate.switch_leakage_current_a += switch_result.off_leakage_a;
        aggregate.switch_leakage_power_uw += leakage_power_uw;
        aggregate.total_current_a += switch_result.off_leakage_a;
        aggregate.total_power_uw += leakage_power_uw;
        result.switch_leakage_current_a += switch_result.off_leakage_a;
        result.switch_leakage_power_uw += leakage_power_uw;
        result.total_current_a += switch_result.off_leakage_a;
        result.total_power_uw += leakage_power_uw;
    }
    for (domain_name, powered) in &domain_powered {
        if !powered
            && domain_aggregates
                .get(domain_name)
                .map(|domain| domain.cell_count > 0)
                .unwrap_or(false)
        {
            result.findings.push(IrFinding {
                code: "IR-POWER-OFF".into(),
                severity: IrSeverity::Warning,
                row: None,
                message: format!(
                    "power domain '{}' is switched off; its cell current is excluded from static IR",
                    domain_name
                ),
            });
        }
    }
    for switch in &result.switches {
        if switch.enabled && !switch.modeled {
            result.findings.push(IrFinding {
                code: "IR-SWITCH-MODEL".into(),
                severity: IrSeverity::Warning,
                row: None,
                message: format!(
                    "power switch '{}' is on but has no on-resistance model",
                    switch.name
                ),
            });
        }
    }
    for (domain_name, drop) in &switch_drops {
        for domain_row in &mut result.domain_rows {
            if &domain_row.power_domain == domain_name {
                domain_row.vdd_worst_drop_v += drop;
                domain_row.total_drop_v += drop;
            }
        }
        let aggregate = domain_aggregates
            .get_mut(domain_name)
            .ok_or_else(|| format!("missing IR aggregate for domain '{}'", domain_name))?;
        aggregate.switch_drop_v = *drop;
        aggregate.worst_drop_v += *drop;
    }

    let mut rebuilt_rows = BTreeMap::new();
    for domain_row in &result.domain_rows {
        let row = rebuilt_rows.entry(domain_row.row).or_insert(IrRow {
            row: domain_row.row,
            total_current_a: 0.0,
            vdd_worst_drop_v: 0.0,
            gnd_worst_drop_v: 0.0,
            total_drop_v: 0.0,
            em_margin: f64::INFINITY,
        });
        row.total_current_a += domain_row.total_current_a;
        row.vdd_worst_drop_v = row.vdd_worst_drop_v.max(domain_row.vdd_worst_drop_v);
        row.gnd_worst_drop_v = row.gnd_worst_drop_v.max(domain_row.gnd_worst_drop_v);
        row.total_drop_v = row.total_drop_v.max(domain_row.total_drop_v);
        row.em_margin = row.em_margin.min(domain_row.em_margin_a);
    }
    result.rows = rebuilt_rows.into_values().collect();

    for domain_row in &result.domain_rows {
        let domain = domain_options
            .get(&domain_row.power_domain)
            .ok_or_else(|| {
                format!(
                    "missing IR options for domain '{}'",
                    domain_row.power_domain
                )
            })?;
        if domain_row.total_drop_v > domain.max_total_drop_v {
            result.findings.push(IrFinding {
                code: "IR-DROP".into(),
                severity: IrSeverity::Error,
                row: Some(domain_row.row),
                message: format!(
                    "domain '{}' row {} worst IR drop {:.6} V exceeds {:.6} V",
                    domain_row.power_domain,
                    domain_row.row,
                    domain_row.total_drop_v,
                    domain.max_total_drop_v
                ),
            });
        }
        if domain_row.total_drop_v > result.worst_drop_v {
            result.worst_drop_v = domain_row.total_drop_v;
            result.worst_power_domain = Some(domain_row.power_domain.clone());
            result.worst_row = Some(domain_row.row);
        }
    }
    result.domains = domain_aggregates.into_values().collect();
    result.cells.sort_by_key(|cell| cell.cell);
    result.findings.sort_by(|left, right| {
        (&left.severity, &left.code, &left.message).cmp(&(
            &right.severity,
            &right.code,
            &right.message,
        ))
    });
    Ok(result)
}

fn finish_upf_power_modes(modes: Vec<UpfPowerModeIr>) -> UpfPowerModeIrResult {
    let mut worst_power_mode = 0;
    let mut worst_drop_mode = 0;
    let mut worst_em_lifetime_mode = 0;
    let mut error_findings = 0;
    for (index, mode) in modes.iter().enumerate() {
        if mode.result.total_power_uw > modes[worst_power_mode].result.total_power_uw {
            worst_power_mode = index;
        }
        let mesh_drop = mode
            .power_mesh
            .as_ref()
            .map(|mesh| mesh.worst_drop_v)
            .or_else(|| {
                mode.multilayer_power_mesh
                    .as_ref()
                    .map(|mesh| mesh.worst_drop_v)
            });
        let drop = mesh_drop.map_or(mode.result.worst_drop_v, |mesh| {
            mode.result.worst_drop_v.max(mesh)
        });
        let domain_drop = mode
            .domain_multilayer_meshes
            .iter()
            .map(|domain| domain.worst_drop_v)
            .fold(0.0_f64, f64::max);
        let drop = drop.max(domain_drop);
        if drop > modes[worst_drop_mode].result.worst_drop_v {
            worst_drop_mode = index;
        }
        match (
            modes[worst_em_lifetime_mode].worst_em_lifetime_hours,
            mode.worst_em_lifetime_hours,
        ) {
            (Some(current), Some(candidate)) if candidate < current => {
                worst_em_lifetime_mode = index;
            }
            (None, Some(_)) => worst_em_lifetime_mode = index,
            _ => {}
        }
        error_findings += mode
            .result
            .findings
            .iter()
            .filter(|finding| finding.severity == IrSeverity::Error)
            .count();
        error_findings += mode
            .power_mesh
            .as_ref()
            .map(|mesh| {
                mesh.findings
                    .iter()
                    .filter(|finding| finding.severity == IrSeverity::Error)
                    .count()
            })
            .unwrap_or(0);
        error_findings += mode
            .domain_multilayer_meshes
            .iter()
            .filter_map(|domain| domain.mesh.as_ref())
            .map(|mesh| {
                mesh.findings
                    .iter()
                    .filter(|finding| finding.severity == IrSeverity::Error)
                    .count()
            })
            .sum::<usize>();
        error_findings += mode
            .multilayer_power_mesh
            .as_ref()
            .map(|mesh| {
                mesh.findings
                    .iter()
                    .filter(|finding| finding.severity == IrSeverity::Error)
                    .count()
            })
            .unwrap_or(0);
    }
    UpfPowerModeIrResult {
        modes,
        worst_power_mode,
        worst_drop_mode,
        worst_em_lifetime_mode,
        error_findings,
    }
}

/// Build independent multilayer mesh views for every UPF power domain.
///
/// Cell currents are injected only for the named domain, and the copied mesh
/// options use that domain's supply voltage. Strategy and switch-leakage
/// currents have no physical coordinates yet and remain in the UPF IR summary
/// but are not injected into these domain meshes.
pub fn analyze_upf_domain_multilayer_meshes(
    nl: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
) -> Result<Vec<UpfDomainMultilayerMeshResult>, String> {
    let mut out = Vec::with_capacity(result.domains.len());
    for domain in &result.domains {
        let mut cell_current = vec![0.0_f64; nl.cell_count()];
        let mut total_current_a = 0.0_f64;
        let mut total_power_uw = 0.0_f64;
        for cell in result
            .cells
            .iter()
            .filter(|cell| cell.power_domain == domain.name && cell.powered)
        {
            if cell.cell >= cell_current.len() {
                return Err(format!(
                    "UPF IR cell index {} is outside netlist cell count {}",
                    cell.cell,
                    cell_current.len()
                ));
            }
            let current_a = cell.current_ma / 1_000.0;
            cell_current[cell.cell] = current_a;
            total_current_a += current_a;
            total_power_uw += current_a * domain.voltage_v * 1_000_000.0;
        }

        let mesh = if domain.powered && domain.cell_count > 0 {
            let mut domain_options = options.clone();
            domain_options.voltage_v = domain.voltage_v;
            domain_options.power_domains.clear();
            domain_options.cell_power_domains.clear();
            Some(analyze_multilayer_mesh_currents(
                nl,
                placement,
                &cell_current,
                total_power_uw,
                &domain_options,
            )?)
        } else {
            None
        };
        out.push(UpfDomainMultilayerMeshResult {
            power_domain: domain.name.clone(),
            powered: domain.powered,
            voltage_v: domain.voltage_v,
            total_current_a,
            worst_drop_v: mesh.as_ref().map_or(0.0, |mesh| mesh.worst_drop_v),
            worst_em_lifetime_hours: mesh.as_ref().map(|mesh| mesh.worst_em_lifetime_hours),
            mesh,
        });
    }
    Ok(out)
}

/// Solve a shared multilayer mesh with multiple UPF supply-domain voltages.
///
/// Unlike [`analyze_upf_domain_multilayer_meshes`], this is one coupled nodal
/// solve: all domains share the same resistive metal and via network, while
/// their VDD taps may be held at different domain voltages.
pub fn analyze_upf_coupled_multilayer_mesh(
    nl: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
    domain_supplies: &[MultilayerPowerDomainSupply],
) -> Result<MultilayerPowerMeshResult, String> {
    let result_domains = result
        .domains
        .iter()
        .filter(|domain| domain.cell_count > 0)
        .map(|domain| domain.name.as_str())
        .collect::<BTreeSet<_>>();
    let supplied_domains = domain_supplies
        .iter()
        .map(|domain| domain.name.as_str())
        .collect::<BTreeSet<_>>();
    if result_domains != supplied_domains {
        return Err("UPF coupled mesh supply domains do not exactly match UPF IR domains".into());
    }

    let mut coupled_options = options.clone();
    coupled_options.power_domains = domain_supplies.to_vec();
    coupled_options.cell_power_domains = result
        .cells
        .iter()
        .map(|cell| cell.power_domain.clone())
        .collect::<Vec<_>>();
    let cell_current = result
        .cells
        .iter()
        .map(|cell| cell.current_ma / 1_000.0)
        .collect::<Vec<_>>();
    let total_power_uw = result
        .cells
        .iter()
        .map(|cell| cell.current_ma / 1_000.0 * cell.voltage_v * 1_000_000.0)
        .sum::<f64>();
    analyze_multilayer_mesh_currents(
        nl,
        placement,
        &cell_current,
        total_power_uw,
        &coupled_options,
    )
}

/// Analyze every declared UPF power mode with the same physical implementation.
pub fn analyze_upf_power_modes(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    upf: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfPowerModeIrResult, String> {
    if upf.power_modes.is_empty() {
        return Err("UPF power-mode analysis has no declared power modes".into());
    }
    let mut modes = Vec::with_capacity(upf.power_modes.len());
    for mode_name in upf.power_modes.keys() {
        let mut mode_view = upf.clone();
        mode_view.active_power_mode = Some(mode_name.clone());
        let result = analyze_upf(nl, placement, power, &mode_view, options)?;
        modes.push(UpfPowerModeIr {
            power_mode: mode_name.clone(),
            result,
            power_mesh: None,
            multilayer_power_mesh: None,
            coupled_multilayer_mesh: false,
            worst_em_lifetime_hours: None,
            domain_multilayer_meshes: Vec::new(),
        });
    }

    Ok(finish_upf_power_modes(modes))
}

/// Analyze every UPF power mode and a mode-aware VDD/GND power mesh.
///
/// Mesh injections are derived from the UPF result cells, so domain voltage,
/// switch state, and per-domain mode activity are respected. Strategy cells
/// do not yet have physical coordinates and are therefore reported in the row
/// IR result but not injected into the mesh.
pub fn analyze_upf_power_modes_with_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    upf: &Upf,
    options: &UpfIrOptions,
    mesh_options: &PowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    if upf.power_modes.is_empty() {
        return Err("UPF power-mode analysis has no declared power modes".into());
    }
    let mut modes = Vec::with_capacity(upf.power_modes.len());
    for mode_name in upf.power_modes.keys() {
        let mut mode_view = upf.clone();
        mode_view.active_power_mode = Some(mode_name.clone());
        let result = analyze_upf(nl, placement, power, &mode_view, options)?;
        let cell_current = result
            .cells
            .iter()
            .map(|cell| cell.current_ma / 1_000.0)
            .collect::<Vec<_>>();
        let cell_power = result
            .cells
            .iter()
            .map(|cell| cell.current_ma / 1_000.0 * cell.voltage_v * 1_000_000.0)
            .sum::<f64>();
        let power_mesh =
            analyze_mesh_currents(nl, placement, &cell_current, cell_power, mesh_options)?;
        modes.push(UpfPowerModeIr {
            power_mode: mode_name.clone(),
            result,
            power_mesh: Some(power_mesh),
            multilayer_power_mesh: None,
            coupled_multilayer_mesh: false,
            worst_em_lifetime_hours: None,
            domain_multilayer_meshes: Vec::new(),
        });
    }

    Ok(finish_upf_power_modes(modes))
}

/// Analyze every UPF power mode and a mode-aware multilayer VDD/GND mesh.
///
/// Cell-layer mapping, layer resistances, and via resistances come from the
/// supplied options. Mode-dependent UPF currents are injected on each cell's
/// assigned layer.
pub fn analyze_upf_power_modes_with_multilayer_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    upf: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    if upf.power_modes.is_empty() {
        return Err("UPF power-mode analysis has no declared power modes".into());
    }
    let mut modes = Vec::with_capacity(upf.power_modes.len());
    for mode_name in upf.power_modes.keys() {
        let mut mode_view = upf.clone();
        mode_view.active_power_mode = Some(mode_name.clone());
        let result = analyze_upf(nl, placement, power, &mode_view, options)?;
        let cell_current = result
            .cells
            .iter()
            .map(|cell| cell.current_ma / 1_000.0)
            .collect::<Vec<_>>();
        let cell_power = result
            .cells
            .iter()
            .map(|cell| cell.current_ma / 1_000.0 * cell.voltage_v * 1_000_000.0)
            .sum::<f64>();
        let multilayer_power_mesh = analyze_multilayer_mesh_currents(
            nl,
            placement,
            &cell_current,
            cell_power,
            mesh_options,
        )?;
        let worst_em_lifetime_hours = Some(multilayer_power_mesh.worst_em_lifetime_hours);
        let domain_multilayer_meshes =
            analyze_upf_domain_multilayer_meshes(nl, placement, &result, mesh_options)?;
        let worst_em_lifetime_hours = domain_multilayer_meshes
            .iter()
            .filter_map(|domain| domain.worst_em_lifetime_hours)
            .fold(worst_em_lifetime_hours, |current, candidate| {
                Some(current.unwrap_or(f64::INFINITY).min(candidate))
            });
        modes.push(UpfPowerModeIr {
            power_mode: mode_name.clone(),
            result,
            power_mesh: None,
            multilayer_power_mesh: Some(multilayer_power_mesh),
            coupled_multilayer_mesh: false,
            worst_em_lifetime_hours,
            domain_multilayer_meshes,
        });
    }

    Ok(finish_upf_power_modes(modes))
}

/// Analyze every UPF power mode with one shared multi-supply multilayer mesh.
///
/// Supply taps and voltages come from
/// [`MultilayerPowerMeshOptions::power_domains`]. Unlike
/// [`analyze_upf_power_modes_with_multilayer_mesh`], this routine performs one
/// coupled nodal solve for all domains rather than adding independent
/// per-domain mesh views.
pub fn analyze_upf_power_modes_with_coupled_multilayer_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    upf: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    if upf.power_modes.is_empty() {
        return Err("UPF power-mode analysis has no declared power modes".into());
    }
    let domain_supplies = mesh_options.power_domains.clone();
    let mut modes = Vec::with_capacity(upf.power_modes.len());
    for mode_name in upf.power_modes.keys() {
        let mut mode_view = upf.clone();
        mode_view.active_power_mode = Some(mode_name.clone());
        let result = analyze_upf(nl, placement, power, &mode_view, options)?;
        let multilayer_power_mesh = analyze_upf_coupled_multilayer_mesh(
            nl,
            placement,
            &result,
            mesh_options,
            &domain_supplies,
        )?;
        let worst_em_lifetime_hours = Some(multilayer_power_mesh.worst_em_lifetime_hours);
        modes.push(UpfPowerModeIr {
            power_mode: mode_name.clone(),
            result,
            power_mesh: None,
            multilayer_power_mesh: Some(multilayer_power_mesh),
            coupled_multilayer_mesh: true,
            worst_em_lifetime_hours,
            domain_multilayer_meshes: Vec::new(),
        });
    }
    Ok(finish_upf_power_modes(modes))
}
