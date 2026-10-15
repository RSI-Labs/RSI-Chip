//! Timing waiver, path, and MMMC reporting helpers.

pub(crate) fn timing_waiver_report(
    netlist: &eda_core::netlist::Netlist,
    constrained: &eda_api::ConstrainedTiming,
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    if waivers.is_some() && audit.is_some() {
        return Err("choose either --waivers or --waiver-audit, not both".into());
    }
    let mut store = eda_api::timing_violation_store(&constrained.clock, netlist, constrained)?;
    if let Some(waivers) = waivers {
        for waiver in waivers {
            eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
    }
    if let Some(entries) = audit {
        eda_api::restore_timing_waiver_audit(&mut store, entries)?;
    }
    let mut out = eda_api::timing_violation_report(&store, None);
    if audit.is_some() {
        out.push_str(&eda_api::timing_waiver_audit_report(store.audit()));
        out.push_str(&eda_api::timing_waiver_audit_hierarchy_report(
            store.audit(),
        )?);
    }
    Ok(out)
}

pub(crate) fn optimization_waivers(
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<Option<Vec<eda_api::TimingViolationWaiver>>, String> {
    if let Some(waivers) = waivers {
        Ok(Some(waivers.to_vec()))
    } else if let Some(entries) = audit {
        Ok(Some(eda_api::active_timing_waivers_from_audit(entries)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn write_waiver_exports(
    waiver_rules: Option<&[eda_api::TimingViolationWaiver]>,
    waiver_audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
    waiver_output: Option<&str>,
    audit_output: Option<&str>,
) -> Result<(), String> {
    if waiver_output.is_none() && audit_output.is_none() {
        return Ok(());
    }
    if waiver_rules.is_some() && waiver_audit.is_some() {
        return Err("choose either --waivers or --waiver-audit for waiver export".into());
    }
    if waiver_rules.is_none() && waiver_audit.is_none() {
        return Err("waiver export requires --waivers or --waiver-audit".into());
    }
    if let Some(path) = waiver_output {
        let waivers = if let Some(rules) = waiver_rules {
            rules.to_vec()
        } else {
            eda_api::active_timing_waivers_from_audit(
                waiver_audit.ok_or("waiver export source is missing")?,
            )?
        };
        let text = eda_api::write_timing_violation_waivers(&waivers)?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    if let Some(path) = audit_output {
        let text = if let Some(rules) = waiver_rules {
            let mut store = eda_api::TimingViolationStore::new("export");
            for waiver in rules {
                eda_api::add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            eda_api::write_timing_waiver_audit(store.audit())?
        } else {
            eda_api::write_timing_waiver_audit(
                waiver_audit.ok_or("waiver export source is missing")?,
            )?
        };
        std::fs::write(path, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn constrained_sta_report(
    netlist: &eda_core::netlist::Netlist,
    sdc: &str,
) -> Result<String, String> {
    let constraints = eda_api::parse_timing_constraints(sdc)?;
    let timing = eda_sta::analyze_with_constraints(netlist, &constraints)?;
    let constrained = eda_sta::analyze_constrained(netlist, timing, &constraints)?;
    Ok(eda_sta::constrained_report(&constrained))
}

pub(crate) fn multicorner_timing_waiver_report<'a>(
    _result: &eda_api::MultiCornerTimingResult,
    scenarios: &[eda_api::TimingScenario<'a>],
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    if waivers.is_some() && audit.is_some() {
        return Err("choose either --waivers or --waiver-audit, not both".into());
    }
    let mut registry = eda_api::timing_violation_registry(scenarios.iter().copied())?;
    if let Some(waivers) = waivers {
        for waiver in waivers {
            eda_api::add_timing_violation_registry_waiver(&mut registry, waiver.clone())?;
        }
    }
    if let Some(entries) = audit {
        eda_api::restore_timing_waiver_registry_audit(&mut registry, entries)?;
    }
    let mut out = eda_api::timing_violation_registry_report(&registry, None);
    if audit.is_some() {
        out.push_str(&eda_api::timing_waiver_audit_report(registry.audit()));
        out.push_str(&eda_api::timing_waiver_audit_hierarchy_report(
            registry.audit(),
        )?);
    }
    Ok(out)
}

pub(crate) fn sta_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::timing_paths(netlist, timing, path_count);
        out = eda_api::timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

pub(crate) fn sta_hold_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_hold_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_hold_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_hold_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::hold_timing_paths(netlist, timing, 0.0, path_count);
        out = eda_api::hold_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_hold_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

pub(crate) fn sta_reset_paths_report(
    netlist: &eda_core::netlist::Netlist,
    timing: &eda_api::StaResult,
    constrained: Option<&eda_api::ConstrainedTiming>,
    path_count: usize,
) -> Result<String, String> {
    let mut out;
    if let Some(constrained) = constrained {
        let paths = eda_api::constrained_reset_timing_paths(netlist, constrained, path_count);
        out = eda_api::constrained_reset_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_constrained_reset_timing_paths(netlist, &paths)?,
            ));
        }
    } else {
        let paths = eda_api::reset_timing_paths(netlist, timing, path_count);
        out = eda_api::reset_timing_path_report(netlist, &paths);
        if !netlist.hierarchy.is_empty() {
            out.push_str(&eda_api::hierarchical_timing_path_report(
                &eda_api::hierarchical_reset_timing_paths(netlist, &paths)?,
            ));
        }
    }
    Ok(out)
}

pub(crate) fn multicorner_sta_report(
    netlist: &eda_core::netlist::Netlist,
    source: &str,
    setup_path_count: Option<usize>,
    hold_path_count: Option<usize>,
    reset_path_count: Option<usize>,
    waivers: Option<&[eda_api::TimingViolationWaiver]>,
    audit: Option<&[eda_api::TimingWaiverAuditEntry]>,
) -> Result<String, String> {
    let scenarios = eda_api::parse_timing_scenarios(source)?;
    let result = eda_api::analyze_timing_scenarios_from_definitions(netlist, &scenarios)?;
    let mut report = eda_api::multi_corner_timing_report(&result);
    let views = scenarios
        .iter()
        .map(|scenario| {
            eda_api::TimingScenario::new(scenario.name.as_str(), netlist, &scenario.constraints)
        })
        .collect::<Vec<_>>();
    if waivers.is_some() || audit.is_some() {
        report.push_str(&multicorner_timing_waiver_report(
            &result, &views, waivers, audit,
        )?);
    }
    if let Some(path_count) = setup_path_count {
        let paths =
            eda_api::multi_corner_setup_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_setup_timing_path_report(&paths));
    }
    if let Some(path_count) = hold_path_count {
        let paths =
            eda_api::multi_corner_hold_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_hold_timing_path_report(&paths));
    }
    if let Some(path_count) = reset_path_count {
        let paths =
            eda_api::multi_corner_reset_timing_paths(&result, views.iter().copied(), path_count)?;
        report.push_str(&eda_api::multi_corner_reset_timing_path_report(&paths));
    }
    Ok(report)
}
