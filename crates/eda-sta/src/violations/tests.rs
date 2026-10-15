use super::*;

use crate::{analyze_with_sdc, Netlist, TimingTopologyDiff};
use std::collections::BTreeSet;

use eda_core::library::CellLibrary;

use eda_core::netlist::{Cell, CellKind, Dir, NetlistHierarchyInstance, NetlistHierarchyPort};

use std::sync::Arc;

fn two_flop_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());

    let mut nl = Netlist::new("min-pulse");

    nl.set_library(lib.clone());

    let clk = nl.add_net("clk");

    let d = nl.add_net("d");

    let q1 = nl.add_net("q1");

    let q2 = nl.add_net("q2");

    let dff = lib.find("DFF").unwrap();

    nl.add_cell(Cell {
        name: "launch".into(),

        kind: CellKind::Lib(dff),

        inputs: vec![clk, d],

        output: q1,
    });

    nl.add_cell(Cell {
        name: "capture".into(),

        kind: CellKind::Lib(dff),

        inputs: vec![clk, q1],

        output: q2,
    });

    nl.add_port("clk", Dir::In, vec![clk]);

    nl.add_port("d", Dir::In, vec![d]);

    nl.add_port("q2", Dir::Out, vec![q2]);

    nl
}

fn and_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());

    let mut nl = Netlist::new("delay-limits");

    nl.set_library(lib.clone());

    let a = nl.add_net("a");

    let b = nl.add_net("b");

    let y = nl.add_net("y");

    let z = nl.add_net("z");

    nl.add_cell(Cell {
        name: "u1".into(),

        kind: CellKind::Lib(lib.find("AND2").unwrap()),

        inputs: vec![a, b],

        output: y,
    });

    nl.add_cell(Cell {
        name: "u2".into(),

        kind: CellKind::Lib(lib.find("OR2").unwrap()),

        inputs: vec![a, b],

        output: z,
    });

    nl.add_port("a", Dir::In, vec![a]);

    nl.add_port("b", Dir::In, vec![b]);

    nl.add_port("y", Dir::Out, vec![y]);

    nl.add_port("z", Dir::Out, vec![z]);

    nl
}

#[test]

fn tracks_explicit_max_and_min_delay_violations() {
    let nl = and_netlist();

    let constraints = crate::parse_sdc(
        r#"

        create_clock -name clk -period 3.0 [get_ports clk]

        set_max_delay 0.8 -from [get_ports a] -to [get_ports y]

        set_min_delay 1.2 -from [get_ports a] -to [get_ports y]

    "#,
    )
    .unwrap();

    let timing = analyze_with_sdc(&nl, &constraints).unwrap();

    assert!((timing.max_delay_slack + 0.2).abs() < 1e-12);

    assert!((timing.min_delay_slack + 0.2).abs() < 1e-12);

    assert_eq!(
        timing.net_slacks["y"].min_delay_ns,
        Some(timing.min_delay_slack)
    );

    let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();

    assert_eq!(store.len(), 2);

    assert_eq!(
        store.violations()["sdc:max_delay:y"].kind,
        TimingViolationKind::MaxDelay
    );

    assert_eq!(
        store.violations()["sdc:min_delay:y"].kind,
        TimingViolationKind::MinDelay
    );

    let waiver = TimingViolationWaiver::new("max-only", "sdc", "y", -1.0)
        .unwrap()
        .with_kind(TimingViolationKind::MaxDelay);

    store.add_waiver(waiver.clone()).unwrap();

    assert_eq!(store.len(), 1);

    assert_eq!(store.waived_len(), 1);

    assert!(store.active_violations().contains_key("sdc:min_delay:y"));

    let serialized = write_timing_violation_waivers(&[waiver]).unwrap();

    let parsed = parse_timing_violation_waivers(&serialized).unwrap();

    assert_eq!(parsed[0].kind, Some(TimingViolationKind::MaxDelay));
}

#[test]

fn registry_from_stores_preserves_scenario_local_waivers() {
    let nl = and_netlist();

    let constraints = crate::parse_sdc(
        r#"

        create_clock -name clk -period 0.2 [get_ports a]

        set_output_delay 0.1 -clock clk [get_ports y]

    "#,
    )
    .unwrap();

    let timing = analyze_with_sdc(&nl, &constraints).unwrap();

    let mut slow = TimingViolationStore::from_timing("slow", &nl, &timing).unwrap();

    let fast = TimingViolationStore::from_timing("fast", &nl, &timing).unwrap();

    let waiver = TimingViolationWaiver::new("slow-only", "slow", "*", -10.0)
        .unwrap()
        .with_kind(TimingViolationKind::Output);

    slow.add_waiver(waiver).unwrap();

    let mut registry = timing_violation_registry_from_stores([
        ("slow".to_string(), slow),
        ("fast".to_string(), fast),
    ])
    .unwrap();

    assert_eq!(registry.stores()["slow"].len(), 0);

    assert_eq!(registry.len(), registry.stores()["fast"].len());

    assert_eq!(registry.summaries()["output:y"].worst_scenario, "fast");

    assert!(registry.stores()["slow"]
        .waivers()
        .contains_key("slow-only"));

    assert_eq!(registry.stores()["fast"].waived_len(), 0);

    assert!(registry.waivers().is_empty());

    let relaxed = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports a]

        set_output_delay 0.1 -clock clk [get_ports y]

    "#,
    )
    .unwrap();

    let relaxed_timing = analyze_with_sdc(&nl, &relaxed).unwrap();

    let mut relaxed_slow = TimingViolationStore::from_timing("slow", &nl, &relaxed_timing).unwrap();

    relaxed_slow
        .add_waiver(
            TimingViolationWaiver::new("slow-only", "slow", "*", -10.0)
                .unwrap()
                .with_kind(TimingViolationKind::Output),
        )
        .unwrap();

    let relaxed_fast = TimingViolationStore::from_timing("fast", &nl, &relaxed_timing).unwrap();

    let update = registry
        .replace_stores([
            ("slow".to_string(), relaxed_slow),
            ("fast".to_string(), relaxed_fast),
        ])
        .unwrap();

    assert_eq!(registry.generation(), 2);

    assert!(update.scenario_diffs.contains_key("slow"));

    assert!(registry.stores()["slow"]
        .waivers()
        .contains_key("slow-only"));
}

#[test]

fn dirty_object_update_preserves_untouched_violations() {
    let nl = and_netlist();

    let old = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports clk]

        set_max_delay 0.5 -from [get_ports a] -to [get_ports y]

        set_max_delay 0.5 -from [get_ports a] -to [get_ports z]

    "#,
    )
    .unwrap();

    let new = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports clk]

        set_max_delay 1.5 -from [get_ports a] -to [get_ports y]

        set_max_delay 0.5 -from [get_ports a] -to [get_ports z]

    "#,
    )
    .unwrap();

    let old_timing = analyze_with_sdc(&nl, &old).unwrap();

    let new_timing = analyze_with_sdc(&nl, &new).unwrap();

    let mut store = TimingViolationStore::from_timing("sdc", &nl, &old_timing).unwrap();

    assert_eq!(store.len(), 2);

    let preserved_slack = store.violations()["sdc:max_delay:z"].slack_ns;

    let mut dirty = TimingViolationDirtyObjects::default();

    dirty.add_net("y");

    let diff = store.update_dirty(&nl, &new_timing, &dirty).unwrap();

    assert_eq!(diff.removed.len(), 1);

    assert!(diff.removed.contains_key("sdc:max_delay:y"));

    assert!(store.violations().contains_key("sdc:max_delay:z"));

    assert!(!store.violations().contains_key("sdc:max_delay:y"));

    assert_eq!(
        store.violations()["sdc:max_delay:z"].slack_ns,
        preserved_slack
    );

    assert_eq!(store.generation, 2);

    let no_op = store
        .update_dirty(&nl, &new_timing, &TimingViolationDirtyObjects::default())
        .unwrap();

    assert!(no_op.is_empty());

    assert_eq!(store.generation, 2);
}

#[test]

fn derives_dirty_objects_from_constraint_and_topology_diffs() {
    let old = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports clk]

        set_output_delay 0.1 -clock clk [get_ports y]

    "#,
    )
    .unwrap();

    let changed_endpoint = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports clk]

        set_output_delay 0.3 -clock clk [get_ports y]

    "#,
    )
    .unwrap();

    let endpoint_dirty = timing_violation_dirty_objects_from_constraints(&old, &changed_endpoint);

    assert_eq!(endpoint_dirty.nets, BTreeSet::from(["y".to_string()]));

    assert!(endpoint_dirty.clocks.is_empty());

    assert!(endpoint_dirty.cells.is_empty());

    let changed_clock = crate::parse_sdc(
        r#"

        create_clock -name clk -period 5.0 [get_ports clk]

        set_output_delay 0.1 -clock clk [get_ports y]

    "#,
    )
    .unwrap();

    let clock_dirty = timing_violation_dirty_objects_from_constraints(&old, &changed_clock);

    assert_eq!(clock_dirty.clocks, BTreeSet::from(["clk".to_string()]));

    let broad = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports clk]

        set_output_delay 0.1 -clock clk [get_ports y]

        set_max_delay 1.0 -to [get_ports y]

    "#,
    )
    .unwrap();

    let broad_dirty = timing_violation_dirty_objects_from_constraints(&old, &broad);

    assert_eq!(broad_dirty.clocks, BTreeSet::from(["clk".to_string()]));

    let topology = TimingTopologyDiff {
        changed_cells: BTreeSet::from(["u1".to_string()]),

        removed_nets: BTreeSet::from(["old".to_string()]),

        changed_ports: BTreeSet::from(["y".to_string()]),

        ..Default::default()
    };

    let topology_dirty = timing_violation_dirty_objects_from_topology(&topology);

    assert_eq!(topology_dirty.cells, BTreeSet::from(["u1".to_string()]));

    assert_eq!(
        topology_dirty.nets,
        BTreeSet::from(["old".to_string(), "y".to_string()])
    );
}

#[test]

fn tracks_and_waives_min_pulse_violations() {
    let nl = two_flop_netlist();

    let constraints = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 -fall 0.1 [get_ports clk]

        set_input_delay 1.0 -clock clk [get_ports d]

    "#,
    )
    .unwrap();

    let timing = analyze_with_sdc(&nl, &constraints).unwrap();

    assert_eq!(timing.min_pulse_width_checks.len(), 4);

    assert!((timing.min_pulse_width_slack + 0.15).abs() < 1e-12);

    let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();

    assert_eq!(store.len(), 2);

    assert_eq!(
        store.violations()["sdc:min_pulse_high:launch"].kind,
        TimingViolationKind::MinPulseHigh
    );

    assert_eq!(
        store.violations()["sdc:min_pulse_high:capture"].endpoint,
        "capture"
    );

    let waiver = TimingViolationWaiver::new("capture-pulse", "sdc", "capture", -1.0)
        .unwrap()
        .with_kind(TimingViolationKind::MinPulseHigh);

    store.add_waiver(waiver.clone()).unwrap();

    assert_eq!(store.len(), 1);

    assert_eq!(store.waived_len(), 1);

    assert!(store
        .active_violations()
        .contains_key("sdc:min_pulse_high:launch"));

    let serialized = write_timing_violation_waivers(&[waiver]).unwrap();

    let parsed = parse_timing_violation_waivers(&serialized).unwrap();

    assert_eq!(parsed[0].kind, Some(TimingViolationKind::MinPulseHigh));

    assert!(timing_violation_report(&store, None).contains("min_pulse_high"));
}

#[test]

fn waives_by_hierarchy_instance_namespace() {
    let mut nl = and_netlist();

    let y = nl.net_by_name("y").unwrap();

    let z = nl.net_by_name("z").unwrap();

    nl.hierarchy = vec![
        NetlistHierarchyInstance {
            path: "top.u1.u_leaf".into(),

            module: "leaf".into(),

            parent: Some("top.u1".into()),

            depth: 2,

            nets: vec![y],

            ports: vec![NetlistHierarchyPort {
                name: "y".into(),

                direction: Dir::Out,

                local_nets: vec![y],

                nets: vec![y],

                width: 1,
            }],
        },
        NetlistHierarchyInstance {
            path: "top.u1".into(),

            module: "sub".into(),

            parent: Some("top".into()),

            depth: 1,

            nets: vec![z],

            ports: vec![NetlistHierarchyPort {
                name: "z".into(),

                direction: Dir::Out,

                local_nets: vec![z],

                nets: vec![z],

                width: 1,
            }],
        },
    ];

    let constraints = crate::parse_sdc(
        r#"

        create_clock -name clk -period 4.0 [get_ports a]

        set_max_delay 0.1 -from [get_ports a] -to [get_ports y]

        set_max_delay 0.1 -from [get_ports a] -to [get_ports z]

    "#,
    )
    .unwrap();

    let timing = analyze_with_sdc(&nl, &constraints).unwrap();

    let mut store = TimingViolationStore::from_timing("sdc", &nl, &timing).unwrap();

    assert_eq!(store.len(), 2);

    assert_eq!(
        store.violations()["sdc:max_delay:y"].hierarchy.as_deref(),
        Some("top.u1.u_leaf")
    );

    assert_eq!(
        store.violations()["sdc:max_delay:z"].hierarchy.as_deref(),
        Some("top.u1")
    );

    let waiver = TimingViolationWaiver::new("leaf-only", "sdc", "*", -100.0)
        .unwrap()
        .with_instance_pattern("top.u1.u_leaf");

    store.add_waiver(waiver.clone()).unwrap();

    assert_eq!(store.len(), 1);

    assert_eq!(store.waived_len(), 1);

    assert!(store.active_violations().contains_key("sdc:max_delay:z"));

    assert!(!store.active_violations().contains_key("sdc:max_delay:y"));

    let serialized = write_timing_violation_waivers(&[waiver]).unwrap();

    let parsed = parse_timing_violation_waivers(&serialized).unwrap();

    assert_eq!(parsed[0].instance_pattern.as_deref(), Some("top.u1.u_leaf"));

    let report = timing_violation_report(&store, None);

    assert!(report.contains("hierarchy: top.u1"), "{report}");
}
