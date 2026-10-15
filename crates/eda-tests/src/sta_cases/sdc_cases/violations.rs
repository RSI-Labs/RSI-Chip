//! SDC/MMMC child regression domain.

use super::super::*;
use super::through_chain_netlist;

pub(super) fn register_violations(c: &mut CaseList) {
    c.add("sta_violation_store_tracks_lifecycle", || {
        let nl = through_chain_netlist();
        let failing = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let worse = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
        let passing = eda_sta::parse_sdc("create_clock -name clk -period 2.0 [get_ports a]")?;

        let failing_timing = eda_sta::analyze_with_sdc(&nl, &failing)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &failing_timing)?;
        let violation_id = "func:output:y".to_string();
        let worse_timing = eda_sta::analyze_with_sdc(&nl, &worse)?;
        let degraded = store.update(&nl, &worse_timing)?;
        let passing_timing = eda_sta::analyze_with_sdc(&nl, &passing)?;
        let removed = store.update(&nl, &passing_timing)?;
        if store.generation == 3
            && store.is_empty()
            && degraded.degraded.contains_key(&violation_id)
            && (degraded.degraded[&violation_id].old_slack_ns + 0.2).abs() < 1e-12
            && (degraded.degraded[&violation_id].new_slack_ns + 0.7).abs() < 1e-12
            && removed.removed.contains_key(&violation_id)
            && eda_sta::timing_violation_report(&store, Some(&removed))
                .contains("lifecycle: added=0, removed=1")
        {
            Ok(())
        } else {
            Err("violation lifecycle mismatch".into())
        }
    });

    c.add("sta_violation_store_survives_eco_with_stable_id", || {
        let old = through_chain_netlist();
        let mut new = old.clone();
        let eco_net = new.add_net("eco_n1");
        let target = new
            .cells()
            .iter()
            .position(|cell| cell.name == "u2")
            .ok_or("missing u2")?;
        let mut patched = new.cell(target).clone();
        patched.inputs[0] = eco_net;
        new.replace_cell(target, patched);
        let library = new.library().ok_or("missing library")?.clone();
        new.add_cell(Cell {
            name: "eco_buf".into(),
            kind: CellKind::Lib(library.find("BUF").unwrap()),
            inputs: vec![new.net_by_name("n0").ok_or("missing n0")?],
            output: eco_net,
        });
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let old_timing = eda_sta::analyze_with_sdc(&old, &constraints)?;
        let new_timing = eda_sta::analyze_with_sdc(&new, &constraints)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("eco", &old, &old_timing)?;
        let diff = store.update(&new, &new_timing)?;
        let id = "eco:output:y".to_string();
        if store.violations().contains_key(&id)
            && diff.degraded.contains_key(&id)
            && (new_timing.worst_slack - old_timing.worst_slack).abs() > 1e-12
            && store.violations()[&id]
                .cells
                .iter()
                .any(|cell| cell == "eco_buf")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, Some(&diff)))
        }
    });

    c.add("sta_violation_store_tracks_hold_and_reset_checks", || {
        let src: &'static str = "module m(input clk, d, rst, output reg q);
                always @(posedge clk or posedge rst)
                    if (rst) q <= 1'b0; else q <= d;
             endmodule";
        let nl = synth_cached(src);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.3 -hold [get_clocks clk]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
        let store = eda_sta::TimingViolationStore::from_timing("checks", nl, &timing)?;
        let hold_id = "checks:hold:u1_dffr".to_string();
        let removal_id = "checks:removal:u1_dffr".to_string();
        if store.violations().contains_key(&hold_id)
            && store.violations()[&hold_id].kind == eda_sta::TimingViolationKind::Hold
            && (store.violations()[&hold_id].slack_ns + 0.5).abs() < 1e-12
            && store.violations().contains_key(&removal_id)
            && store.violations()[&removal_id].kind == eda_sta::TimingViolationKind::Removal
            && (store.violations()[&removal_id].slack_ns + 0.45).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, None))
        }
    });

    c.add(
        "sta_multicorner_violation_registry_aggregates_worst_corner",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
            let fast_fixed =
                eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let slow_view = eda_sta::TimingScenario::new("slow", &nl, &slow);
            let fast_view = eda_sta::TimingScenario::new("fast", &nl, &fast);
            let mut registry =
                eda_sta::TimingViolationRegistry::from_scenarios([slow_view, fast_view])?;
            let key = "output:y".to_string();
            let initial_summary = registry
                .summaries()
                .get(&key)
                .ok_or("missing summary")?
                .clone();
            let updated = registry.replace_scenarios([
                slow_view,
                eda_sta::TimingScenario::new("fast", &nl, &fast_fixed),
            ])?;
            if registry.generation() == 2
                && registry.stores().len() == 2
                && registry.stores()["fast"].is_empty()
                && initial_summary.worst_scenario == "fast"
                && (initial_summary.worst_slack_ns + 0.7).abs() < 1e-12
                && initial_summary.scenarios.len() == 1
                && updated.cross_corner.removed.contains_key(&key)
                && updated.scenario_diffs["fast"]
                    .removed
                    .contains_key("fast:output:y")
                && eda_sta::timing_violation_registry_report(&registry, Some(&updated))
                    .contains("cross-corner: added=0, removed=1")
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(
                    &registry,
                    Some(&updated),
                ))
            }
        },
    );

    c.add(
        "sta_multicorner_registry_distributes_violations_by_hierarchy",
        || {
            let src: &'static str = "module inv(input a, output y); assign y = ~a; endmodule
                module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
                module top(input a, output y1, y2);
                    sub u1(.a(a), .y(y1));
                    sub u2(.a(a), .y(y2));
                endmodule";
            let nl = synth_cached(src);
            let slow = eda_sta::parse_sdc(
                r#"
                create_clock -name clk -period 2.0 [get_ports a]
                set_output_delay 0.2 -clock clk [get_ports y1]
                set_output_delay 0.2 -clock clk [get_ports y2]
            "#,
            )?;
            let fast = eda_sta::parse_sdc(
                r#"
                create_clock -name clk -period 0.1 [get_ports a]
                set_output_delay 0.2 -clock clk [get_ports y1]
                set_output_delay 0.2 -clock clk [get_ports y2]
            "#,
            )?;
            let registry = eda_sta::TimingViolationRegistry::from_scenarios([
                eda_sta::TimingScenario::new("slow", nl, &slow),
                eda_sta::TimingScenario::new("fast", nl, &fast),
            ])?;
            let first = registry
                .summaries()
                .get("output:y1")
                .ok_or("missing y1 cross-corner summary")?;
            let second = registry
                .summaries()
                .get("output:y2")
                .ok_or("missing y2 cross-corner summary")?;
            let first_hierarchy = Some("top.u1.u_leaf".to_string());
            let second_hierarchy = Some("top.u2.u_leaf".to_string());
            let first_owner = first
                .hierarchy_summary
                .get(&first_hierarchy)
                .ok_or("missing y1 hierarchy summary")?;
            let second_owner = second
                .hierarchy_summary
                .get(&second_hierarchy)
                .ok_or("missing y2 hierarchy summary")?;
            let registry_hierarchies = registry.hierarchy_summaries();
            if registry.stores()["slow"].is_empty()
                && registry.stores()["fast"].len() == 2
                && first.scenarios.len() == 1
                && second.scenarios.len() == 1
                && first_owner.worst_scenario == "fast"
                && second_owner.worst_scenario == "fast"
                && first_owner.violations == 1
                && second_owner.violations == 1
                && first_owner
                    .kinds
                    .contains_key(&eda_sta::TimingViolationKind::Output)
                && registry_hierarchies.len() == 2
                && registry_hierarchies.contains_key(&first_hierarchy)
                && registry_hierarchies.contains_key(&second_hierarchy)
            {
                let report = eda_sta::timing_violation_registry_report(&registry, None);
                if report.contains("hierarchy distribution:")
                    && report.contains("'top.u1.u_leaf': violations=1")
                    && report.contains("'top.u2.u_leaf': violations=1")
                {
                    Ok(())
                } else {
                    Err(report)
                }
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );

    c.add("sta_timing_violation_waivers_filter_and_reappear", || {
        let nl = through_chain_netlist();
        let failing = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let worse = eda_sta::parse_sdc("create_clock -name clk -period 0.5 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &failing)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver = eda_sta::TimingViolationWaiver::new("small-output", "func", "y", -0.21)?
            .with_kind(eda_sta::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("accepted early mode");
        store.add_waiver(waiver)?;
        let waived_ok = store.is_empty()
            && store.violations().len() == 1
            && store.waived_len() == 1
            && store.waived_violations().get("func:output:y") == Some(&"small-output".to_string());

        let worse_timing = eda_sta::analyze_with_sdc(&nl, &worse)?;
        store.update(&nl, &worse_timing)?;
        let threshold_ok = store.len() == 1
            && store.waived_len() == 0
            && (store.violations()["func:output:y"].slack_ns + 0.7).abs() < 1e-12;

        store.remove_waiver("small-output")?;
        let temporary = eda_sta::TimingViolationWaiver::new("temporary-output", "func", "y", -1.0)?
            .expires_after(3);
        store.add_waiver(temporary)?;
        store.update(&nl, &worse_timing)?;
        let temporary_valid = store.is_empty() && store.waived_len() == 1;
        store.update(&nl, &worse_timing)?;
        let temporary_expired = store.len() == 1 && store.waived_len() == 0;
        store.remove_waiver("temporary-output")?;
        if waived_ok
            && threshold_ok
            && temporary_valid
            && temporary_expired
            && store.len() == 1
            && store.waivers().is_empty()
            && eda_sta::timing_violation_report(&store, None).contains("waived violations: 0")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&store, None))
        }
    });

    c.add(
        "sta_multicorner_violation_waiver_applies_by_scenario",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
            let views = [
                eda_sta::TimingScenario::new("slow", &nl, &slow),
                eda_sta::TimingScenario::new("fast", &nl, &fast),
            ];
            let mut registry = eda_sta::TimingViolationRegistry::from_scenarios(views)?;
            let waiver = eda_sta::TimingViolationWaiver::new("fast-accepted", "fast", "y", -0.21)?
                .with_owner("frontend");
            registry.add_waiver(waiver)?;
            let waived_ok = registry.is_empty()
                && registry.stores()["fast"].violations().len() == 1
                && registry.stores()["fast"].waived_len() == 1
                && registry.summaries().is_empty();
            registry.remove_waiver("fast-accepted")?;
            if waived_ok
                && registry.len() == 1
                && registry.summaries().contains_key("output:y")
                && registry.waivers().is_empty()
                && eda_sta::timing_violation_registry_report(&registry, None)
                    .contains("waived violations: 0")
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );

    c.add("sta_timing_waivers_serialize_and_parse_round_trip", || {
        let waivers = vec![
            eda_sta::TimingViolationWaiver::new("all", "func*", "**/y", -0.21)?
                .with_kind(eda_sta::TimingViolationKind::Output)
                .with_owner("timing\\owner")
                .with_note("reviewed\nlater")
                .expires_after(12),
            eda_sta::TimingViolationWaiver::new("hold-any", "*", "top/**", -0.05)?
                .with_kind(eda_sta::TimingViolationKind::Hold),
        ];
        let text = eda_sta::write_timing_violation_waivers(&waivers)?;
        let parsed = eda_sta::parse_timing_violation_waivers(&text)?;
        let malformed = eda_sta::parse_timing_violation_waivers(
            "waiver same func y -1\nwaiver same func y -2\n",
        );
        if parsed == waivers
            && text.contains("waiver \"all\" \"func*\" \"**/y\" \"-0.21\"")
            && text.contains("owner \"timing\\\\owner\"")
            && text.contains("note \"reviewed\\nlater\"")
            && matches!(malformed, Err(error) if error.contains("duplicate timing violation waiver"))
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("sta_timing_waiver_audit_records_actor_and_snapshot", || {
        let nl = through_chain_netlist();
        let constraints = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let mut store = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver =
            eda_sta::TimingViolationWaiver::new("audited", "func", "y", -1.0)?.with_owner("timing");
        store.add_waiver_with_actor(waiver.clone(), Some("engineer".into()))?;
        store.remove_waiver_with_actor("audited", Some("reviewer".into()))?;
        let audit = store.audit();
        if audit.len() == 2
            && audit[0].sequence == 1
            && audit[0].action == eda_sta::TimingWaiverAuditAction::Added
            && audit[0].changed_by.as_deref() == Some("engineer")
            && audit[0].waiver == waiver
            && audit[1].sequence == 2
            && audit[1].action == eda_sta::TimingWaiverAuditAction::Removed
            && audit[1].changed_by.as_deref() == Some("reviewer")
            && audit[1].waiver == waiver
            && eda_sta::timing_waiver_audit_report(audit)
                .contains("#1 added 'audited' at generation 1 by engineer")
        {
            Ok(())
        } else {
            Err(eda_sta::timing_waiver_audit_report(audit))
        }
    });

    c.add("sta_timing_waiver_audit_groups_by_hierarchy_scope", || {
        let nl = through_chain_netlist();
        let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let mut registry =
            eda_sta::TimingViolationRegistry::from_scenarios([eda_sta::TimingScenario::new(
                "fast", &nl, &fast,
            )])?;
        let first = eda_sta::TimingViolationWaiver::new("u1", "fast", "y", -1.0)?
            .with_instance_pattern("top.u1.**");
        let second = eda_sta::TimingViolationWaiver::new("u2", "fast", "y", -1.0)?
            .with_instance_pattern("top.u2");
        let unscoped =
            eda_sta::TimingViolationWaiver::new("any", "fast", "y", -1.0)?.with_owner("timing");
        registry.add_waiver_with_actor(first, Some("frontend".into()))?;
        registry.add_waiver_with_actor(second, Some("frontend".into()))?;
        registry.add_waiver_with_actor(unscoped, Some("lead".into()))?;
        registry.remove_waiver_with_actor("u1", Some("reviewer".into()))?;
        let summaries = eda_sta::timing_waiver_audit_hierarchy_summaries(registry.audit())?;
        let first_key = Some("top.u1.**".to_string());
        let second_key = Some("top.u2".to_string());
        let first_summary = summaries
            .get(&first_key)
            .ok_or("missing u1 audit hierarchy summary")?;
        let second_summary = summaries
            .get(&second_key)
            .ok_or("missing u2 audit hierarchy summary")?;
        let unscoped_summary = summaries
            .get(&None)
            .ok_or("missing unscoped audit hierarchy summary")?;
        let report = eda_sta::timing_waiver_audit_hierarchy_report(registry.audit())?;
        if summaries.len() == 3
            && registry.audit().len() == 4
            && first_summary.events == 2
            && first_summary.added_events == 1
            && first_summary.removed_events == 1
            && first_summary.active_waiver_ids.is_empty()
            && first_summary.latest_generation == 4
            && second_summary.events == 1
            && second_summary.active_waiver_ids.contains("u2")
            && unscoped_summary.active_waiver_ids.contains("any")
            && report.contains("'top.u1.**': events=2, added=1, removed=1, active=0")
            && report.contains("'top.u2': events=1, added=1, removed=0, active=1")
            && report.contains("'(unscoped)': events=1, added=1, removed=0, active=1")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_multicorner_waiver_registry_keeps_global_audit", || {
        let nl = through_chain_netlist();
        let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
        let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let mut registry = eda_sta::TimingViolationRegistry::from_scenarios([
            eda_sta::TimingScenario::new("slow", &nl, &slow),
            eda_sta::TimingScenario::new("fast", &nl, &fast),
        ])?;
        let waiver =
            eda_sta::TimingViolationWaiver::new("global", "*", "y", -1.0)?.with_owner("timing");
        registry.add_waiver_with_actor(waiver.clone(), Some("lead".into()))?;
        registry.remove_waiver_with_actor("global", Some("lead".into()))?;
        let audit = registry.audit();
        if audit.len() == 2
            && audit[0].action == eda_sta::TimingWaiverAuditAction::Added
            && audit[1].action == eda_sta::TimingWaiverAuditAction::Removed
            && audit
                .iter()
                .all(|entry| entry.changed_by.as_deref() == Some("lead") && entry.waiver == waiver)
        {
            Ok(())
        } else {
            Err(eda_sta::timing_waiver_audit_report(audit))
        }
    });

    c.add("sta_timing_waiver_audit_serializes_and_parses_round_trip", || {
        let waiver = eda_sta::TimingViolationWaiver::new("restore", "func*", "**/y", -0.21)?
            .with_kind(eda_sta::TimingViolationKind::Output)
            .with_owner("timing")
            .with_note("approved\nsession")
            .expires_after(9);
        let removed = eda_sta::TimingViolationWaiver::new("old", "func", "y", -1.0)?;
        let entries = vec![
            eda_sta::TimingWaiverAuditEntry {
                sequence: 1,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "old".into(),
                generation: 1,
                changed_by: Some("engineer".into()),
                waiver: removed,
            },
            eda_sta::TimingWaiverAuditEntry {
                sequence: 2,
                action: eda_sta::TimingWaiverAuditAction::Removed,
                waiver_id: "old".into(),
                generation: 1,
                changed_by: None,
                waiver: eda_sta::TimingViolationWaiver::new("old", "func", "y", -1.0)?,
            },
            eda_sta::TimingWaiverAuditEntry {
                sequence: 3,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "restore".into(),
                generation: 2,
                changed_by: Some("reviewer".into()),
                waiver,
            },
        ];
        let text = eda_sta::write_timing_waiver_audit(&entries)?;
        let parsed = eda_sta::parse_timing_waiver_audit(&text)?;
        let malformed = eda_sta::parse_timing_waiver_audit(
            "audit 2 added 1 \"x\"\nend\n",
        );
        if parsed == entries
            && text.contains("audit 1 added 1 \"engineer\"")
            && text.contains("audit 2 removed 1 \"\"")
            && text.contains("audit 3 added 2 \"reviewer\"")
            && matches!(malformed, Err(error) if error.contains("audit block must contain exactly one waiver"))
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("sta_timing_waiver_audit_restores_store_state", || {
        let nl = through_chain_netlist();
        let constraints =
            eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let source = eda_sta::TimingViolationStore::from_timing("func", &nl, &timing)?;
        let waiver = eda_sta::TimingViolationWaiver::new("restore", "func", "y", -1.0)?
            .with_owner("timing");
        let entries = vec![eda_sta::TimingWaiverAuditEntry {
            sequence: 1,
            action: eda_sta::TimingWaiverAuditAction::Added,
            waiver_id: "restore".into(),
            generation: 1,
            changed_by: Some("engineer".into()),
            waiver,
        }];
        let mut restored = source.clone();
        restored.restore_waiver_audit(&entries)?;
        let future = vec![eda_sta::TimingWaiverAuditEntry {
            sequence: 1,
            action: eda_sta::TimingWaiverAuditAction::Added,
            waiver_id: "future".into(),
            generation: 2,
            changed_by: None,
            waiver: eda_sta::TimingViolationWaiver::new("future", "func", "y", -1.0)?,
        }];
        if restored.generation == source.generation
            && restored.audit() == entries.as_slice()
            && restored.waivers().len() == 1
            && restored.is_empty()
            && restored.violations().len() == 1
            && matches!(restored.restore_waiver_audit(&future), Err(error) if error.contains("newer than store generation"))
        {
            Ok(())
        } else {
            Err(eda_sta::timing_violation_report(&restored, None))
        }
    });

    c.add(
        "sta_multicorner_waiver_audit_restores_registry_state",
        || {
            let nl = through_chain_netlist();
            let slow = eda_sta::parse_sdc("create_clock -name clk -period 4.0 [get_ports a]")?;
            let fast = eda_sta::parse_sdc("create_clock -name clk -period 1.0 [get_ports a]")?;
            let mut registry = eda_sta::TimingViolationRegistry::from_scenarios([
                eda_sta::TimingScenario::new("slow", &nl, &slow),
                eda_sta::TimingScenario::new("fast", &nl, &fast),
            ])?;
            let waiver = eda_sta::TimingViolationWaiver::new("fast", "fast", "y", -0.21)?;
            let entries = vec![eda_sta::TimingWaiverAuditEntry {
                sequence: 1,
                action: eda_sta::TimingWaiverAuditAction::Added,
                waiver_id: "fast".into(),
                generation: registry.generation(),
                changed_by: Some("lead".into()),
                waiver,
            }];
            registry.restore_waiver_audit(&entries)?;
            if registry.audit() == entries.as_slice()
                && registry.waivers().len() == 1
                && registry.is_empty()
                && registry.stores()["fast"].audit() == entries.as_slice()
                && registry.stores()["slow"].audit() == entries.as_slice()
                && registry.summaries().is_empty()
            {
                Ok(())
            } else {
                Err(eda_sta::timing_violation_registry_report(&registry, None))
            }
        },
    );
}
