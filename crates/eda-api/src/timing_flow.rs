//! Placement/routing, interconnect extraction, physical timing, and optimization APIs.

use super::integration::{endpoint_name, insert_hold_buffer};
use super::*;

pub(crate) fn evaluate_physical_timing_iteration_cancellable(
    index: usize,
    netlist: &Netlist,
    placement: Placement,
    routing: Routing,
    model: &InterconnectModel,
    constraints: Option<&TimingConstraints>,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let parasitics = extract_interconnect_parasitics(netlist, &routing, model)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let parasitic_report = parasitic_analysis(netlist, &parasitics)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let sdf_text = parasitics_to_sdf(netlist, &parasitics)?;
    let sdf = parse_delay_file(&sdf_text)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let annotated_netlist = back_annotate(netlist, &sdf)?;
    let timing = eda_sta::analyze(&annotated_netlist)?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let constrained_timing = constraints
        .map(|constraints| constrained_timing(&annotated_netlist, constraints))
        .transpose()?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let drc = physical_verification(netlist, &placement, &routing)?;
    let antenna_warnings = drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    if drc.has_errors() {
        return Err(format!(
            "physical timing iteration {} has DRC errors: {:?}",
            index, drc.violations
        ));
    }
    Ok(PhysicalTimingIteration {
        index,
        placement,
        routing,
        parasitics,
        parasitic_report,
        sdf,
        annotated_netlist,
        timing,
        constrained_timing,
        drc,
        antenna_repairs: Vec::new(),
        antenna_warnings,
        hold_repairs: Vec::new(),
        hold_repair_applied: false,
        all_active_hold_repaired: true,
    })
}

pub(crate) fn repair_physical_timing_antennas_cancellable(
    iteration: PhysicalTimingIteration,
    index: usize,
    options: &PhysicalTimingFlowOptions,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    if !options.repair_antennas || iteration.antenna_warnings == 0 {
        return Ok(iteration);
    }
    let patch_name = if index == usize::MAX {
        "physical_antenna_baseline".to_string()
    } else {
        format!("physical_antenna_{index}")
    };
    let repaired = repair_antennas(
        &iteration.annotated_netlist,
        &iteration.placement,
        &iteration.routing,
        &AntennaRepairOptions {
            patch_name,
            max_diodes_per_net: options.antenna_max_diodes_per_net,
            route: options.route.clone(),
        },
    )?;
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let mut evaluated = evaluate_physical_timing_iteration_cancellable(
        index,
        &repaired.netlist,
        repaired.physical_placement,
        repaired.routing,
        &options.interconnect,
        options.constraints.as_ref(),
        cancellation,
    )?;
    evaluated.antenna_repairs = repaired.repairs;
    evaluated.antenna_warnings = repaired.antenna_warnings;
    Ok(evaluated)
}

fn repair_physical_timing_hold_cancellable(
    iteration: PhysicalTimingIteration,
    options: &PhysicalTimingFlowOptions,
    patch_name: Option<&str>,
    cancellation: &FlowCancellation,
) -> Result<(PhysicalTimingIteration, HoldRepairResult), String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let hold = options
        .hold_repair
        .as_ref()
        .ok_or_else(|| "physical timing hold repair is disabled".to_string())?;
    let constraints = options
        .constraints
        .as_ref()
        .ok_or_else(|| "physical timing hold repair requires SDC constraints".to_string())?;
    let mut current = iteration;
    let antenna_repairs = current.antenna_repairs.clone();
    let patch_name = patch_name.unwrap_or(hold.patch_name.as_str());
    let mut old_slacks = std::collections::BTreeMap::new();
    let mut inserted = std::collections::BTreeMap::new();

    loop {
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let constrained = current
            .constrained_timing
            .clone()
            .ok_or_else(|| "physical timing hold repair requires constrained timing".to_string())?;
        let mut store =
            timing_violation_store(&constrained.clock, &current.annotated_netlist, &constrained)?;
        for waiver in &options.waivers {
            add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
        for waiver in &hold.waivers {
            add_timing_violation_waiver(&mut store, waiver.clone())?;
        }
        let waived = store.waived_violations().clone();
        let mut repaired_this_round = false;
        for path in constrained_hold_timing_paths(&current.annotated_netlist, &constrained, 0) {
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            let TimingPathEndpoint::SequentialCell(endpoint) = &path.path.path.endpoint else {
                continue;
            };
            if path.slack_ns >= 0.0
                || !path.slack_ns.is_finite()
                || waived.contains_key(&format!("{}:hold:{}", constrained.clock, endpoint))
            {
                continue;
            }
            old_slacks.entry(endpoint.clone()).or_insert(path.slack_ns);
            let count = inserted.get(endpoint).copied().unwrap_or(0);
            if count >= hold.max_buffers_per_endpoint {
                continue;
            }
            let mut netlist = current.annotated_netlist.clone();
            insert_hold_buffer(&mut netlist, endpoint, patch_name, count)?;
            let placement = extend_physical_placement(&netlist, &current.placement)?;
            let routing = eda_pnr::route_with_options_cancellable(
                &netlist,
                &placement,
                &options.route,
                cancellation,
            )?;
            current = evaluate_physical_timing_iteration_cancellable(
                current.index,
                &netlist,
                placement,
                routing,
                &options.interconnect,
                Some(constraints),
                cancellation,
            )?;
            current.antenna_repairs = antenna_repairs.clone();
            inserted.insert(endpoint.clone(), count + 1);
            repaired_this_round = true;
        }
        if !repaired_this_round {
            break;
        }
    }

    let final_timing = current
        .constrained_timing
        .as_ref()
        .ok_or_else(|| "physical timing hold repair lost constrained timing".to_string())?;
    let final_paths = constrained_hold_timing_paths(&current.annotated_netlist, final_timing, 0);
    let mut repairs = Vec::new();
    for (endpoint, old_slack) in old_slacks {
        let buffers = inserted.get(&endpoint).copied().unwrap_or(0);
        let new_slack = final_paths
            .iter()
            .find(|path| {
                matches!(
                    &path.path.path.endpoint,
                    TimingPathEndpoint::SequentialCell(name) if *name == endpoint
                )
            })
            .map(|path| path.slack_ns)
            .unwrap_or(f64::INFINITY);
        repairs.push(HoldRepair {
            endpoint: endpoint.clone(),
            endpoint_pin: 1,
            old_slack_ns: old_slack,
            new_slack_ns: new_slack,
            buffers_inserted: buffers,
        });
    }

    let mut store = timing_violation_store(
        &final_timing.clock,
        &current.annotated_netlist,
        final_timing,
    )?;
    for waiver in &options.waivers {
        add_timing_violation_waiver(&mut store, waiver.clone())?;
    }
    for waiver in &hold.waivers {
        add_timing_violation_waiver(&mut store, waiver.clone())?;
    }
    let all_active_repaired = !final_paths.into_iter().any(|path| {
        path.slack_ns < 0.0
            && path.slack_ns.is_finite()
            && !store.waived_violations().contains_key(&format!(
                "{}:hold:{}",
                final_timing.clock,
                endpoint_name(&path.path.path.endpoint)
            ))
    });
    let buffers_inserted = inserted.values().sum();
    let result = HoldRepairResult {
        netlist: current.annotated_netlist.clone(),
        physical_placement: Some(current.placement.clone()),
        repairs,
        buffers_inserted,
        all_active_violations_repaired: all_active_repaired,
    };
    current.antenna_repairs = antenna_repairs;
    Ok((current, result))
}

struct MultiCornerAntennaRepairState {
    scenario_netlists: Vec<Netlist>,
    placement: Placement,
    routing: Routing,
    repairs: Vec<AntennaRepair>,
    antenna_warnings: usize,
}

fn propagate_common_antenna_repair(
    original_owner: &Netlist,
    repaired_owner: &Netlist,
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
) -> Result<Vec<Netlist>, String> {
    let original_names = original_owner
        .cells()
        .iter()
        .map(|cell| cell.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut out = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        let mut view = scenario.netlist.clone();
        let target_diode = view
            .library()
            .and_then(|library| library.find("DIODE"))
            .ok_or_else(|| {
                format!(
                    "MMMC antenna repair scenario '{}' has no library DIODE cell",
                    scenario.name
                )
            })?;
        for repaired_cell in repaired_owner.cells() {
            if original_names.contains(repaired_cell.name.as_str()) {
                continue;
            }
            if (0..view.cell_count()).any(|cell| view.cell(cell).name == repaired_cell.name) {
                return Err(format!(
                    "MMMC antenna repair cell '{}' already exists in scenario '{}'",
                    repaired_cell.name, scenario.name
                ));
            }
            let source_kind = repaired_owner
                .library()
                .and_then(|library| {
                    let eda_core::netlist::CellKind::Lib(id) = &repaired_cell.kind else {
                        return None;
                    };
                    Some(library.get(*id).name.clone())
                })
                .unwrap_or_else(|| "GENERIC".to_string());
            if source_kind != "DIODE" {
                return Err(format!(
                    "MMMC antenna repair attempted to propagate non-DIODE cell '{}'",
                    repaired_cell.name
                ));
            }
            let inputs = repaired_cell
                .inputs
                .iter()
                .map(|net| {
                    let name = repaired_owner.net(*net).name.clone();
                    view.net_by_name(&name).ok_or_else(|| {
                        format!(
                            "net '{}' is missing from scenario '{}'",
                            name, scenario.name
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let output_name = repaired_owner.net(repaired_cell.output).name.clone();
            let output = match view.net_by_name(&output_name) {
                Some(net) => net,
                None => view.add_net(output_name),
            };
            view.add_cell(eda_core::netlist::Cell {
                name: repaired_cell.name.clone(),
                kind: eda_core::netlist::CellKind::Lib(target_diode),
                inputs,
                output,
            });
        }
        out.push(view);
    }
    Ok(out)
}

fn repair_multicorner_antennas(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    geometry_owner: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
    index: usize,
) -> Result<Option<MultiCornerAntennaRepairState>, String> {
    if !options.repair_antennas {
        return Ok(None);
    }
    let drc = physical_verification(geometry_owner, placement, routing)?;
    let warnings = drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    if warnings == 0 {
        return Ok(None);
    }
    let patch_name = if index == usize::MAX {
        "mmmc_antenna_baseline".to_string()
    } else {
        format!("mmmc_antenna_{index}")
    };
    let repaired = repair_antennas(
        geometry_owner,
        placement,
        routing,
        &AntennaRepairOptions {
            patch_name,
            max_diodes_per_net: options.antenna_max_diodes_per_net,
            route: options.route.clone(),
        },
    )?;
    let scenario_netlists =
        propagate_common_antenna_repair(geometry_owner, &repaired.netlist, scenarios)?;
    Ok(Some(MultiCornerAntennaRepairState {
        scenario_netlists,
        placement: repaired.physical_placement,
        routing: repaired.routing,
        repairs: repaired.repairs,
        antenna_warnings: repaired.antenna_warnings,
    }))
}

type NameIdMap = std::collections::HashMap<String, usize>;

fn physical_name_maps(nl: &Netlist, scenario: &str) -> Result<(NameIdMap, NameIdMap), String> {
    let mut cells = NameIdMap::new();
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        if cell.name.trim().is_empty() {
            return Err(format!(
                "physical timing scenario '{}' has an empty cell name",
                scenario
            ));
        }
        if cells.insert(cell.name.clone(), cell_id).is_some() {
            return Err(format!(
                "physical timing scenario '{}' has duplicate cell '{}'",
                scenario, cell.name
            ));
        }
    }
    let mut nets = NameIdMap::new();
    for (net_id, net) in nl.nets().iter().enumerate() {
        if net.name.trim().is_empty() {
            return Err(format!(
                "physical timing scenario '{}' has an empty net name",
                scenario
            ));
        }
        if nets.insert(net.name.clone(), net_id).is_some() {
            return Err(format!(
                "physical timing scenario '{}' has duplicate net '{}'",
                scenario, net.name
            ));
        }
    }
    Ok((cells, nets))
}

fn library_cell_identity(nl: &Netlist, cell_id: usize, scenario: &str) -> Result<String, String> {
    let cell = nl.cell(cell_id);
    let eda_core::netlist::CellKind::Lib(library_cell) = &cell.kind else {
        return Ok(format!("{:?}", cell.kind));
    };
    let library = nl.library().ok_or_else(|| {
        format!(
            "physical timing scenario '{}' has library cell '{}' without a library",
            scenario, cell.name
        )
    })?;
    let definition = library.get(*library_cell);
    Ok(format!(
        "{}|{}|{}",
        definition.name,
        definition.inputs.join(","),
        definition.output
    ))
}

fn check_common_physical_topology(
    reference: &Netlist,
    reference_placement: &Placement,
    other: &Netlist,
    other_placement: &Placement,
    scenario: &str,
) -> Result<(), String> {
    let (reference_cells, reference_nets) = physical_name_maps(reference, "reference")?;
    let (other_cells, other_nets) = physical_name_maps(other, scenario)?;
    if reference_cells.len() != other_cells.len() || reference_nets.len() != other_nets.len() {
        return Err(format!(
            "physical timing scenario '{}' topology differs: {} cells / {} nets versus {} / {}",
            scenario,
            reference_cells.len(),
            reference_nets.len(),
            other_cells.len(),
            other_nets.len()
        ));
    }
    if reference_cells
        .keys()
        .any(|name| !other_cells.contains_key(name))
        || reference_nets
            .keys()
            .any(|name| !other_nets.contains_key(name))
    {
        return Err(format!(
            "physical timing scenario '{}' has different stable cell/net names",
            scenario
        ));
    }
    if reference.ports.len() != other.ports.len() {
        return Err(format!(
            "physical timing scenario '{}' has a different port count",
            scenario
        ));
    }
    for (reference_port, other_port) in reference.ports.iter().zip(other.ports.iter()) {
        let reference_bits = reference_port
            .bits
            .iter()
            .map(|net| reference.net(*net).name.as_str())
            .collect::<Vec<_>>();
        let other_bits = other_port
            .bits
            .iter()
            .map(|net| other.net(*net).name.as_str())
            .collect::<Vec<_>>();
        if reference_port.name != other_port.name
            || reference_port.dir != other_port.dir
            || reference_bits != other_bits
        {
            return Err(format!(
                "physical timing scenario '{}' port '{}' differs",
                scenario, reference_port.name
            ));
        }
    }
    for (name, reference_cell_id) in &reference_cells {
        let other_cell_id = other_cells[name];
        let reference_cell = reference.cell(*reference_cell_id);
        let other_cell = other.cell(other_cell_id);
        if reference_placement.cell_width[*reference_cell_id]
            != other_placement.cell_width[other_cell_id]
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' width differs: {} versus {} sites",
                scenario,
                name,
                reference_placement.cell_width[*reference_cell_id],
                other_placement.cell_width[other_cell_id]
            ));
        }
        if library_cell_identity(reference, *reference_cell_id, "reference")?
            != library_cell_identity(other, other_cell_id, scenario)?
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' has a different physical cell type",
                scenario, name
            ));
        }
        if reference.net(reference_cell.output).name != other.net(other_cell.output).name
            || reference_cell
                .inputs
                .iter()
                .map(|net| reference.net(*net).name.as_str())
                .collect::<Vec<_>>()
                != other_cell
                    .inputs
                    .iter()
                    .map(|net| other.net(*net).name.as_str())
                    .collect::<Vec<_>>()
        {
            return Err(format!(
                "physical timing scenario '{}' cell '{}' connectivity differs",
                scenario, name
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_common_physical_topology(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
) -> Result<(), String> {
    let reference = scenarios[0].netlist;
    let reference_placement = eda_pnr::place(reference)?;
    for scenario in scenarios.iter().skip(1) {
        let placement = eda_pnr::place(scenario.netlist)?;
        check_common_physical_topology(
            reference,
            &reference_placement,
            scenario.netlist,
            &placement,
            &scenario.name,
        )?;
    }
    Ok(())
}

pub(crate) fn map_placement_by_names(
    owner: &Netlist,
    placement: &Placement,
    target: &Netlist,
    scenario: &str,
) -> Result<Placement, String> {
    let (owner_cells, _) = physical_name_maps(owner, "geometry-owner")?;
    let (target_cells, _) = physical_name_maps(target, scenario)?;
    let mut cell_pos = Vec::with_capacity(target.cell_count());
    let mut cell_width = Vec::with_capacity(target.cell_count());
    for target_cell_id in 0..target.cell_count() {
        let name = target.cell(target_cell_id).name.as_str();
        let owner_cell_id = *owner_cells
            .get(name)
            .ok_or_else(|| format!("cell '{}' is missing from common geometry owner", name))?;
        let position = placement.cell_pos[owner_cell_id];
        cell_pos.push(position);
        cell_width.push(placement.cell_width[owner_cell_id]);
    }
    let _ = target_cells;
    Ok(Placement {
        rows: placement.rows,
        cell_pos,
        cell_width,
        blockages: placement.blockages.clone(),
    })
}

pub(crate) fn map_routing_by_names(
    owner: &Netlist,
    routing: &Routing,
    target: &Netlist,
    scenario: &str,
) -> Result<Routing, String> {
    let (_, owner_nets) = physical_name_maps(owner, "geometry-owner")?;
    let (_, target_nets) = physical_name_maps(target, scenario)?;
    let mut mapped = routing.clone();
    for segment in &mut mapped.segments {
        let name = owner.net(segment.net).name.clone();
        segment.net = *target_nets
            .get(&name)
            .ok_or_else(|| format!("net '{}' is missing from physical scenario", name))?;
    }
    for via in &mut mapped.via_locations {
        let name = owner.net(via.net).name.clone();
        via.net = *target_nets
            .get(&name)
            .ok_or_else(|| format!("net '{}' is missing from physical scenario", name))?;
    }
    let _ = owner_nets;
    Ok(mapped)
}

fn constrained_setup_slack(timing: &ConstrainedTiming) -> f64 {
    timing
        .reg2reg_slack
        .min(timing.in2reg_slack)
        .min(timing.output_slack)
        .min(timing.max_delay_slack)
}

fn constrained_hold_slack(timing: &ConstrainedTiming) -> f64 {
    timing
        .reg2reg_hold_slack
        .min(timing.in2reg_hold_slack)
        .min(timing.min_delay_slack)
}

fn worst_corner_index(
    corners: &[MultiCornerPhysicalTimingCorner],
    slack: impl Fn(&ConstrainedTiming) -> f64,
) -> usize {
    let mut worst = 0_usize;
    for (index, corner) in corners.iter().enumerate().skip(1) {
        let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
            continue;
        };
        let Some(previous) = corners[worst]
            .evaluation
            .constrained_timing
            .as_ref()
            .map(&slack)
        else {
            worst = index;
            continue;
        };
        if slack(timing).total_cmp(&previous) == std::cmp::Ordering::Less {
            worst = index;
        }
    }
    worst
}

fn objective_timing_after_waivers(
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
) -> Result<ConstrainedTiming, String> {
    let mut objective = timing.clone();
    for violation_id in store.waived_violations().keys() {
        let Some(violation) = store.violations().get(violation_id.as_str()) else {
            return Err(format!(
                "waived timing violation '{}' is missing from its store",
                violation_id
            ));
        };
        if matches!(
            violation.kind,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow
        ) {
            // Min-pulse checks are clock-waveform constraints, not routed
            // data-path endpoint slacks, so there is no data-net objective to
            // remove.
            continue;
        }
        let Some(net_name) = violation.nets.last() else {
            return Err(format!(
                "waived timing violation '{}' has no endpoint net",
                violation_id
            ));
        };
        let name = net_name.clone();
        let Some(slack) = objective.net_slacks.get_mut(&name) else {
            return Err(format!(
                "waived timing endpoint net '{}' has no constrained slack",
                name
            ));
        };
        match violation.kind {
            TimingViolationKind::Setup => slack.setup_ns = None,
            TimingViolationKind::Hold => slack.hold_ns = None,
            TimingViolationKind::Recovery => slack.recovery_ns = None,
            TimingViolationKind::Removal => slack.removal_ns = None,
            TimingViolationKind::Output => slack.output_ns = None,
            TimingViolationKind::MaxDelay => slack.max_delay_ns = None,
            TimingViolationKind::MinDelay => slack.min_delay_ns = None,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow => continue,
        }
    }
    Ok(objective)
}

fn active_late_slack(
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
) -> Result<Option<f64>, String> {
    let objective = objective_timing_after_waivers(timing, store)?;
    Ok(objective
        .net_slacks
        .values()
        .filter_map(|slack| slack.late_slack_ns())
        .filter(|slack| slack.is_finite())
        .reduce(f64::min))
}

fn optimization_corner(iteration: &MultiCornerPhysicalTimingIteration) -> Result<usize, String> {
    let mut selected = None;
    for (index, corner) in iteration.corners.iter().enumerate() {
        let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
            continue;
        };
        let Some(slack) = active_late_slack(timing, &corner.violations)? else {
            continue;
        };
        if selected
            .map(|(_, previous_slack)| slack.total_cmp(&previous_slack) == std::cmp::Ordering::Less)
            .unwrap_or(true)
        {
            selected = Some((index, slack));
        }
    }
    Ok(selected
        .map(|(index, _)| index)
        .unwrap_or(iteration.worst_slack_corner))
}

pub(crate) fn evaluate_multi_corner_physical_timing_iteration_cancellable(
    index: usize,
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    geometry_owner: &Netlist,
    placement: Placement,
    routing: Routing,
    model: &InterconnectModel,
    cancellation: &FlowCancellation,
) -> Result<MultiCornerPhysicalTimingIteration, String> {
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }
    let parasitics = extract_interconnect_parasitics(geometry_owner, &routing, model)?;
    let mut corners = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let mapped_placement =
            map_placement_by_names(geometry_owner, &placement, scenario.netlist, &scenario.name)?;
        let mapped_routing =
            map_routing_by_names(geometry_owner, &routing, scenario.netlist, &scenario.name)?;
        let parasitic_report = parasitic_analysis(scenario.netlist, &parasitics)?;
        let sdf_text = parasitics_to_sdf(scenario.netlist, &parasitics)?;
        let sdf = parse_delay_file(&sdf_text)?;
        let annotated_netlist = back_annotate(scenario.netlist, &sdf)?;
        let timing = eda_sta::analyze(&annotated_netlist)?;
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let constrained_timing = constrained_timing(&annotated_netlist, scenario.constraints)?;
        let drc = physical_verification(&annotated_netlist, &mapped_placement, &mapped_routing)?;
        let antenna_warnings = drc
            .violations
            .iter()
            .filter(|violation| violation.code == "ANT-001")
            .count();
        if drc.has_errors() {
            return Err(format!(
                "multi-corner physical timing iteration {} scenario '{}' has DRC errors: {:?}",
                index, scenario.name, drc.violations
            ));
        }
        let mut violations =
            timing_violation_store(&scenario.name, &annotated_netlist, &constrained_timing)?;
        for waiver in &scenario.waivers {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            add_timing_violation_waiver(&mut violations, waiver.clone())?;
        }
        corners.push(MultiCornerPhysicalTimingCorner {
            scenario: scenario.name.clone(),
            evaluation: PhysicalTimingIteration {
                index,
                placement: mapped_placement,
                routing: mapped_routing,
                parasitics: parasitics.clone(),
                parasitic_report,
                sdf,
                annotated_netlist,
                timing,
                constrained_timing: Some(constrained_timing),
                drc,
                antenna_repairs: Vec::new(),
                antenna_warnings,
                hold_repairs: Vec::new(),
                hold_repair_applied: false,
                all_active_hold_repaired: true,
            },
            violations,
        });
    }
    let worst_slack_corner = worst_corner_index(&corners, |timing| timing.worst_slack);
    let worst_setup_corner = worst_corner_index(&corners, constrained_setup_slack);
    let worst_hold_corner = worst_corner_index(&corners, constrained_hold_slack);
    let registry = timing_violation_registry_from_stores(
        corners
            .iter()
            .map(|corner| (corner.scenario.clone(), corner.violations.clone())),
    )?;
    let Some(first) = corners.first() else {
        return Err("multi-corner physical timing flow requires a scenario".into());
    };
    let antenna_warnings = first.evaluation.antenna_warnings;
    Ok(MultiCornerPhysicalTimingIteration {
        index,
        placement: first.evaluation.placement.clone(),
        routing: first.evaluation.routing.clone(),
        corners,
        registry,
        registry_update: None,
        antenna_repairs: Vec::new(),
        antenna_warnings,
        hold_repairs: Vec::new(),
        hold_repair_applied: false,
        all_active_hold_repaired: true,
        worst_slack_corner,
        worst_setup_corner,
        worst_hold_corner,
        optimization_corner: None,
    })
}

/// Run one common physical implementation across named timing scenarios.
///
/// The flow places and routes the first scenario, extracts one name-keyed RC
/// view, back-annotates that RC separately into every scenario, and performs
/// constrained STA in each view. Optimization uses one common placement and
/// routing. Scenario netlists may differ only in timing views (Liberty arcs
/// or SDF annotations); physical cell names, widths, connectivity, and port
/// topology must be stable. Hold-critical fast-corner repair is reported but
/// is not yet a separate hold-aware placement/routing objective.
pub fn run_multi_corner_physical_timing_flow(
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
) -> Result<MultiCornerPhysicalTimingFlowResult, String> {
    run_multi_corner_physical_timing_flow_cancellable(options, &FlowCancellation::new())
}

/// Run MMMC physical timing with cooperative major-stage/router checks.
pub fn run_multi_corner_physical_timing_flow_cancellable(
    options: &MultiCornerPhysicalTimingFlowOptions<'_>,
    cancellation: &FlowCancellation,
) -> Result<MultiCornerPhysicalTimingFlowResult, String> {
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }
    options.validate()?;
    validate_common_physical_topology(&options.scenarios)?;

    fn scenarios_for_local_netlists<'a>(
        source: &'a [MultiCornerPhysicalTimingScenario<'a>],
        netlists: &'a [Netlist],
    ) -> Vec<MultiCornerPhysicalTimingScenario<'a>> {
        source
            .iter()
            .zip(netlists)
            .map(|(scenario, netlist)| MultiCornerPhysicalTimingScenario {
                name: scenario.name.clone(),
                netlist,
                constraints: scenario.constraints,
                waivers: scenario.waivers.clone(),
            })
            .collect()
    }

    let mut physical_netlists = Vec::new();
    let clock_tree = if let Some(cts) = options.timing_driven_cts {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let reference = options.scenarios[0].netlist;
        let seed_placement = match &options.initial_placement {
            Some(placement) => {
                eda_pnr::check_legal(reference, placement)?;
                placement.clone()
            }
            None => eda_pnr::place(reference)?,
        };
        let reference_cts =
            timing_driven_clock_tree_synthesis_with_placement(reference, &seed_placement, &cts)?;
        let reference_placement = reference_cts
            .cts
            .physical_placement
            .clone()
            .ok_or_else(|| "timing-driven MMMC CTS produced no placement".to_string())?;
        let branch_factor = reference_cts.selected_branch_factor;
        for scenario in &options.scenarios {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            let view_cts = clock_tree_synthesis_with_placement(
                scenario.netlist,
                &seed_placement,
                &CtsOptions {
                    branch_factor,
                    wire_delay_ns_per_unit: cts.wire_delay_ns_per_unit,
                },
            )?;
            if view_cts.physical_placement.as_ref() != Some(&reference_placement) {
                return Err(format!(
                    "MMMC timing-driven CTS produced different physical topology in scenario '{}'",
                    scenario.name
                ));
            }
            physical_netlists.push(view_cts.netlist);
        }
        Some(reference_cts)
    } else {
        for scenario in &options.scenarios {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            if let Some(placement) = &options.initial_placement {
                eda_pnr::check_legal(scenario.netlist, placement)?;
            }
            physical_netlists.push(scenario.netlist.clone());
        }
        None
    };
    if cancellation.is_cancelled() {
        return Err("multi-corner physical timing analysis cancelled".into());
    }

    let mut effective_scenarios =
        scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
    validate_common_physical_topology(&effective_scenarios)?;
    let reference = &physical_netlists[0];
    let mut baseline_placement = match &clock_tree {
        Some(cts) => cts
            .cts
            .physical_placement
            .clone()
            .ok_or_else(|| "timing-driven MMMC CTS produced no placement".to_string())?,
        None => match &options.initial_placement {
            Some(placement) => placement.clone(),
            None => eda_pnr::place(reference)?,
        },
    };
    let mut baseline_routing = eda_pnr::route_with_options_cancellable(
        reference,
        &baseline_placement,
        &options.route,
        cancellation,
    )?;
    let mut baseline_antenna_repairs = Vec::new();
    if let Some(repaired) = repair_multicorner_antennas(
        &effective_scenarios,
        reference,
        &baseline_placement,
        &baseline_routing,
        options,
        usize::MAX,
    )? {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        physical_netlists = repaired.scenario_netlists;
        effective_scenarios = scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
        validate_common_physical_topology(&effective_scenarios)?;
        baseline_placement = repaired.placement;
        baseline_routing = repaired.routing;
        baseline_antenna_repairs = repaired.repairs;
    }
    let hold_repair_result = if let Some(hold_options) = &options.hold_repair {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let repaired = repair_multicorner_hold_violations(
            &effective_scenarios,
            &baseline_placement,
            hold_options,
        )?;
        physical_netlists = repaired
            .scenarios
            .iter()
            .map(|scenario| scenario.netlist.clone())
            .collect();
        effective_scenarios = scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
        validate_common_physical_topology(&effective_scenarios)?;
        baseline_placement = repaired.physical_placement.clone();
        baseline_routing = repaired.routing.clone();
        Some(repaired)
    } else {
        None
    };
    let mut baseline = evaluate_multi_corner_physical_timing_iteration_cancellable(
        usize::MAX,
        &effective_scenarios,
        &physical_netlists[0],
        baseline_placement,
        baseline_routing,
        &options.interconnect,
        cancellation,
    )?;
    baseline.antenna_repairs = baseline_antenna_repairs;
    baseline.antenna_warnings = baseline.antenna_warnings.max(baseline.antenna_warnings);
    if let Some(hold) = &hold_repair_result {
        baseline.hold_repairs = hold.repairs.clone();
        baseline.hold_repair_applied = true;
        baseline.all_active_hold_repaired = hold.all_active_violations_repaired;
    }

    let mut optimized = Vec::new();
    let mut persistent_registry = baseline.registry.clone();
    let mut current = baseline.clone();
    for index in 0..options.iterations {
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let objective_index = optimization_corner(&current)?;
        let corner = &current.corners[objective_index];
        let scenario = &effective_scenarios[objective_index];
        let constrained = corner
            .evaluation
            .constrained_timing
            .as_ref()
            .ok_or_else(|| {
                format!(
                    "physical timing scenario '{}' has no constrained timing",
                    scenario.name
                )
            })?;
        let placement = if scenario.waivers.is_empty() {
            timing_driven_place_from_constraints(
                &corner.evaluation.annotated_netlist,
                constrained,
                options.placement_passes,
            )?
            .placement
        } else {
            timing_driven_place_from_waiver_aware_timing(
                &corner.evaluation.annotated_netlist,
                constrained,
                &corner.violations,
                options.placement_passes,
            )?
            .placement
        };
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }
        let routing = if scenario.waivers.is_empty() {
            route_maze_timing_ripgup_from_constraints_cancellable(
                &corner.evaluation.annotated_netlist,
                &placement,
                &options.route,
                constrained,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        } else {
            route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
                &corner.evaluation.annotated_netlist,
                &placement,
                &options.route,
                constrained,
                &corner.violations,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        };
        if cancellation.is_cancelled() {
            return Err("multi-corner physical timing analysis cancelled".into());
        }

        let mut antenna_repairs = Vec::new();
        let mut antenna_warnings = 0;
        let (geometry_owner, placement, routing) = if let Some(repaired) =
            repair_multicorner_antennas(
                &effective_scenarios,
                &corner.evaluation.annotated_netlist,
                &placement,
                &routing,
                options,
                index,
            )? {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            physical_netlists = repaired.scenario_netlists;
            effective_scenarios =
                scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
            validate_common_physical_topology(&effective_scenarios)?;
            antenna_repairs = repaired.repairs;
            antenna_warnings = repaired.antenna_warnings;
            (&physical_netlists[0], repaired.placement, repaired.routing)
        } else {
            (&corner.evaluation.annotated_netlist, placement, routing)
        };
        let mut hold_repairs = Vec::new();
        let mut hold_repair_applied = false;
        let mut all_active_hold_repaired = true;
        let (geometry_owner, placement, routing) = if let Some(hold_options) = &options.hold_repair
        {
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            let mut iteration_options = hold_options.clone();
            iteration_options.hold.patch_name = format!("mmmc_physical_hold_{index}");
            let repaired = repair_multicorner_hold_violations(
                &effective_scenarios,
                &placement,
                &iteration_options,
            )?;
            if cancellation.is_cancelled() {
                return Err("multi-corner physical timing analysis cancelled".into());
            }
            physical_netlists = repaired
                .scenarios
                .iter()
                .map(|scenario| scenario.netlist.clone())
                .collect();
            effective_scenarios =
                scenarios_for_local_netlists(&options.scenarios, &physical_netlists);
            validate_common_physical_topology(&effective_scenarios)?;
            hold_repairs = repaired.repairs;
            hold_repair_applied = true;
            all_active_hold_repaired = repaired.all_active_violations_repaired;
            (
                &physical_netlists[0],
                repaired.physical_placement,
                repaired.routing,
            )
        } else {
            (geometry_owner, placement, routing)
        };
        let mut iteration = evaluate_multi_corner_physical_timing_iteration_cancellable(
            index,
            &effective_scenarios,
            geometry_owner,
            placement,
            routing,
            &options.interconnect,
            cancellation,
        )?;
        iteration.optimization_corner = Some(objective_index);
        iteration.antenna_repairs = antenna_repairs;
        let _ = antenna_warnings;
        iteration.hold_repairs = hold_repairs;
        iteration.hold_repair_applied = hold_repair_applied;
        iteration.all_active_hold_repaired = all_active_hold_repaired;
        let registry_update = persistent_registry.replace_stores(
            iteration
                .corners
                .iter()
                .map(|corner| (corner.scenario.clone(), corner.violations.clone())),
        )?;
        iteration.registry = persistent_registry.clone();
        iteration.registry_update = Some(registry_update);
        current = iteration.clone();
        optimized.push(iteration);
    }

    let best_index = optimized
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            let left_slack = left.corners[left.worst_slack_corner]
                .evaluation
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(f64::NEG_INFINITY);
            let right_slack = right.corners[right.worst_slack_corner]
                .evaluation
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(f64::NEG_INFINITY);
            right_slack
                .total_cmp(&left_slack)
                .then(left.routing.wirelength().cmp(&right.routing.wirelength()))
                .then(left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    baseline.optimization_corner = None;
    Ok(MultiCornerPhysicalTimingFlowResult {
        baseline,
        optimized,
        best_index,
        clock_tree,
        hold_repair: hold_repair_result,
    })
}

pub fn multi_corner_physical_timing_flow_report(
    result: &MultiCornerPhysicalTimingFlowResult,
) -> String {
    let best = result.best();
    let corner_slack = |iteration: &MultiCornerPhysicalTimingIteration| -> f64 {
        iteration.corners[iteration.worst_slack_corner]
            .evaluation
            .constrained_timing
            .as_ref()
            .map(|timing| timing.worst_slack)
            .unwrap_or(f64::NAN)
    };
    let iteration_name = |index: usize| -> String {
        if index == usize::MAX {
            "baseline".to_string()
        } else {
            index.to_string()
        }
    };
    let mut out = format!(
        "Multi-corner physical timing flow: baseline worst slack {:.6} ns, best worst slack {:.6} ns, best iteration {}, scenarios {}\n",
        corner_slack(&result.baseline),
        corner_slack(best),
        iteration_name(best.index),
        result.baseline.corners.len()
    );
    if let Some(clock_tree) = &result.clock_tree {
        out.push_str(&timing_driven_cts_report(clock_tree));
    }
    if let Some(hold_repair) = &result.hold_repair {
        out.push_str(&multi_corner_hold_repair_report(hold_repair));
    }
    for iteration in std::iter::once(&result.baseline).chain(result.optimized.iter()) {
        let Some(reference) = iteration.corners.first() else {
            continue;
        };
        out.push_str(&format!(
            "  iteration {}: common HPWL {:.3} sites, wirelength {}, RC nets {}, worst slack scenario '{}', DRC errors {}, antenna diodes {}, antenna warnings {}, hold buffers {}, hold status {}, active violations {}, cross-corner checks {}\n",
            iteration_name(iteration.index),
            eda_pnr::hpwl(
                &reference.evaluation.annotated_netlist,
                &iteration.placement
            ),
            iteration.routing.wirelength(),
            reference.evaluation.parasitic_report.net_count,
            iteration.corners[iteration.worst_slack_corner].scenario,
            reference.evaluation.drc.error_count(),
            iteration
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>(),
            iteration.antenna_warnings,
            iteration
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>(),
            if iteration.hold_repair_applied {
                if iteration.all_active_hold_repaired {
                    "PASS"
                } else {
                    "LIMITED"
                }
            } else {
                "NONE"
            },
            iteration.registry.len(),
            iteration.registry.summaries().len()
        ));
        if let Some(update) = &iteration.registry_update {
            out.push_str(&format!(
                "    registry lifecycle: scenarios +{}/-{}, violations added={}, removed={}, improved={}, degraded={}, unchanged={}\n",
                update.added_scenarios.len(),
                update.removed_scenarios.len(),
                update.cross_corner.added.len(),
                update.cross_corner.removed.len(),
                update.cross_corner.improved.len(),
                update.cross_corner.degraded.len(),
                update.cross_corner.unchanged.len()
            ));
        }
        for (corner_index, corner) in iteration.corners.iter().enumerate() {
            let Some(timing) = corner.evaluation.constrained_timing.as_ref() else {
                continue;
            };
            let worst_marker = |index: usize| {
                if index == corner_index {
                    " *"
                } else {
                    ""
                }
            };
            out.push_str(&format!(
                "    scenario '{}'{}: delay {:.6} ns, worst slack {:.6} ns, setup {:.6} ns, hold {:.6} ns, waived violations {}{}\n",
                corner.scenario,
                worst_marker(iteration.worst_slack_corner),
                corner.evaluation.timing.critical_path_delay,
                timing.worst_slack,
                constrained_setup_slack(timing),
                constrained_hold_slack(timing),
                corner.violations.waived_violations().len(),
                if iteration.optimization_corner == Some(corner_index) {
                    ", optimization objective"
                } else {
                    ""
                }
            ));
        }
    }
    out
}

/// Run placement → routing → RC extraction → SDF back-annotation → STA.
///
/// The baseline uses deterministic global routing. Each requested iteration
/// feeds the previous RC-annotated STA arrivals into timing-driven placement
/// and timing-prioritized rip-up/reroute. All candidates are evaluated with
/// the same interconnect model, and the lowest critical-path delay wins.
pub fn run_physical_timing_flow(
    netlist: &Netlist,
    options: &PhysicalTimingFlowOptions,
) -> Result<PhysicalTimingFlowResult, String> {
    run_physical_timing_flow_cancellable(netlist, options, &FlowCancellation::new())
}

/// Run the physical timing loop with cooperative stage/router checks.
pub fn run_physical_timing_flow_cancellable(
    netlist: &Netlist,
    options: &PhysicalTimingFlowOptions,
    cancellation: &FlowCancellation,
) -> Result<PhysicalTimingFlowResult, String> {
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    options.validate()?;
    let clock_tree = match options.timing_driven_cts {
        Some(cts) => {
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            let seed_placement = match &options.initial_placement {
                Some(placement) => {
                    eda_pnr::check_legal(netlist, placement)?;
                    placement.clone()
                }
                None => eda_pnr::place(netlist)?,
            };
            let synthesized =
                timing_driven_clock_tree_synthesis_with_placement(netlist, &seed_placement, &cts)?;
            if cancellation.is_cancelled() {
                return Err("physical timing analysis cancelled".into());
            }
            Some(synthesized)
        }
        None => {
            if let Some(placement) = &options.initial_placement {
                eda_pnr::check_legal(netlist, placement)?;
            }
            None
        }
    };
    if cancellation.is_cancelled() {
        return Err("physical timing analysis cancelled".into());
    }
    let effective_netlist = clock_tree
        .as_ref()
        .and_then(|result| {
            result
                .cts
                .physical_placement
                .as_ref()
                .map(|_| &result.cts.netlist)
        })
        .unwrap_or(netlist);
    if options.repair_antennas
        && effective_netlist
            .library()
            .and_then(|library| library.find("DIODE"))
            .is_none()
    {
        return Err("antenna-aware physical timing flow requires a library DIODE cell".into());
    }
    let baseline_placement = match clock_tree
        .as_ref()
        .and_then(|result| result.cts.physical_placement.clone())
    {
        Some(placement) => placement,
        None => match &options.initial_placement {
            Some(placement) => placement.clone(),
            None => eda_pnr::place(effective_netlist)?,
        },
    };
    let baseline_routing = eda_pnr::route_with_options_cancellable(
        effective_netlist,
        &baseline_placement,
        &options.route,
        cancellation,
    )?;
    let antenna_baseline = repair_physical_timing_antennas_cancellable(
        evaluate_physical_timing_iteration_cancellable(
            usize::MAX,
            effective_netlist,
            baseline_placement,
            baseline_routing,
            &options.interconnect,
            options.constraints.as_ref(),
            cancellation,
        )?,
        usize::MAX,
        options,
        cancellation,
    )?;
    let (baseline, hold_repair) = if options.hold_repair.is_some() {
        let (mut iteration, repair) =
            repair_physical_timing_hold_cancellable(antenna_baseline, options, None, cancellation)?;
        iteration.hold_repairs = repair.repairs.clone();
        iteration.hold_repair_applied = true;
        iteration.all_active_hold_repaired = repair.all_active_violations_repaired;
        (iteration, Some(repair))
    } else {
        (antenna_baseline, None::<HoldRepairResult>)
    };

    let mut optimized = Vec::new();
    let mut current = baseline.clone();
    for index in 0..options.iterations {
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let placement = if let Some(constrained) = &current.constrained_timing {
            if options.waivers.is_empty() {
                timing_driven_place_from_constraints(
                    &current.annotated_netlist,
                    constrained,
                    options.placement_passes,
                )?
                .placement
            } else {
                let mut violation_store = timing_violation_store(
                    &constrained.clock,
                    &current.annotated_netlist,
                    constrained,
                )?;
                for waiver in &options.waivers {
                    if cancellation.is_cancelled() {
                        return Err("physical timing analysis cancelled".into());
                    }
                    add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
                }
                timing_driven_place_from_waiver_aware_timing(
                    &current.annotated_netlist,
                    constrained,
                    &violation_store,
                    options.placement_passes,
                )?
                .placement
            }
        } else {
            timing_driven_place_from_timing(
                &current.annotated_netlist,
                &current.timing,
                options.placement_passes,
            )?
            .placement
        };
        if cancellation.is_cancelled() {
            return Err("physical timing analysis cancelled".into());
        }
        let routing = if let Some(constrained) = &current.constrained_timing {
            if options.waivers.is_empty() {
                route_maze_timing_ripgup_from_constraints_cancellable(
                    &current.annotated_netlist,
                    &placement,
                    &options.route,
                    constrained,
                    options.routing_passes,
                    cancellation,
                )?
                .ripup
                .routing
            } else {
                let mut violation_store = timing_violation_store(
                    &constrained.clock,
                    &current.annotated_netlist,
                    constrained,
                )?;
                for waiver in &options.waivers {
                    if cancellation.is_cancelled() {
                        return Err("physical timing analysis cancelled".into());
                    }
                    add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
                }
                route_maze_timing_ripgup_from_waiver_aware_timing_cancellable(
                    &current.annotated_netlist,
                    &placement,
                    &options.route,
                    constrained,
                    &violation_store,
                    options.routing_passes,
                    cancellation,
                )?
                .ripup
                .routing
            }
        } else {
            route_maze_timing_ripgup_from_timing_cancellable(
                &current.annotated_netlist,
                &placement,
                &options.route,
                &current.timing,
                options.routing_passes,
                cancellation,
            )?
            .ripup
            .routing
        };
        let antenna_iteration = repair_physical_timing_antennas_cancellable(
            evaluate_physical_timing_iteration_cancellable(
                index,
                &current.annotated_netlist,
                placement,
                routing,
                &options.interconnect,
                options.constraints.as_ref(),
                cancellation,
            )?,
            index,
            options,
            cancellation,
        )?;
        let (mut iteration, hold_result) = if options.hold_repair.is_some() {
            let patch_name = format!("physical_hold_{index}");
            let (iteration, result) = repair_physical_timing_hold_cancellable(
                antenna_iteration,
                options,
                Some(&patch_name),
                cancellation,
            )?;
            (iteration, Some(result))
        } else {
            (antenna_iteration, None)
        };
        if let Some(result) = hold_result {
            iteration.hold_repairs = result.repairs;
            iteration.hold_repair_applied = true;
            iteration.all_active_hold_repaired = result.all_active_violations_repaired;
        }
        current = iteration.clone();
        optimized.push(iteration);
    }

    let best_index = optimized
        .iter()
        .enumerate()
        .min_by(|(left_index, left), (right_index, right)| {
            let left_quality = left
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(-left.timing.critical_path_delay);
            let right_quality = right
                .constrained_timing
                .as_ref()
                .map(|timing| timing.worst_slack)
                .unwrap_or(-right.timing.critical_path_delay);
            right_quality
                .total_cmp(&left_quality)
                .then(left.routing.wirelength().cmp(&right.routing.wirelength()))
                .then(left_index.cmp(right_index))
        })
        .map(|(index, _)| index)
        .unwrap_or(0);
    Ok(PhysicalTimingFlowResult {
        baseline,
        optimized,
        best_index,
        clock_tree,
        hold_repair,
    })
}

pub fn physical_timing_flow_report(result: &PhysicalTimingFlowResult) -> String {
    let best = result.best();
    let slack_text = |iteration: &PhysicalTimingIteration| -> String {
        iteration
            .constrained_timing
            .as_ref()
            .map(|timing| format!("{:.6}", timing.worst_slack))
            .unwrap_or_else(|| "n/a".into())
    };
    let mut out = format!(
        "Physical timing flow: baseline delay {:.6} ns, baseline slack {}, best delay {:.6} ns, best slack {}, best iteration {}\n",
        result.baseline.timing.critical_path_delay,
        slack_text(&result.baseline),
        best.timing.critical_path_delay,
        slack_text(best),
        if best.index == usize::MAX {
            "baseline".to_string()
        } else {
            best.index.to_string()
        }
    );
    if let Some(clock_tree) = &result.clock_tree {
        out.push_str(&timing_driven_cts_report(clock_tree));
    }
    if let Some(hold_repair) = &result.hold_repair {
        out.push_str(&hold_repair_report(hold_repair));
    }
    for iteration in std::iter::once(&result.baseline).chain(result.optimized.iter()) {
        out.push_str(&format!(
            "  iteration {}: HPWL {:.3} sites, wirelength {}, RC nets {}, delay {:.6} ns, DRC errors {}, antenna diodes {}, antenna warnings {}, hold buffers {}, hold status {}\n",
            if iteration.index == usize::MAX {
                "baseline".to_string()
            } else {
                iteration.index.to_string()
            },
            eda_pnr::hpwl(&iteration.annotated_netlist, &iteration.placement),
            iteration.routing.wirelength(),
            iteration.parasitic_report.net_count,
            iteration.timing.critical_path_delay,
            iteration.drc.error_count(),
            iteration
                .antenna_repairs
                .iter()
                .map(|repair| repair.diodes_inserted)
                .sum::<usize>(),
            iteration.antenna_warnings,
            iteration
                .hold_repairs
                .iter()
                .map(|repair| repair.buffers_inserted)
                .sum::<usize>(),
            if iteration.hold_repair_applied {
                if iteration.all_active_hold_repaired {
                    "PASS"
                } else {
                    "LIMITED"
                }
            } else {
                "NONE"
            }
        ));
        out.push_str(&format!(
            "    constrained slack: {}\n",
            slack_text(iteration)
        ));
    }
    out
}

/// Place a netlist using caller-supplied timing/criticality weights.
pub fn timing_driven_place(
    nl: &Netlist,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    eda_pnr::place_timing_driven(nl, options)
}

/// Improve an existing legal placement using a weighted-HPWL objective.
pub fn optimize_timing_driven_placement(
    nl: &Netlist,
    placement: Placement,
    options: &TimingDrivenPlacementOptions,
) -> Result<TimingDrivenPlacementResult, String> {
    eda_pnr::optimize_placement_timing_driven(nl, placement, options)
}

/// Derive late-path criticality weights from STA and run timing-driven placement.
pub fn timing_driven_place_from_timing(
    nl: &Netlist,
    timing: &StaResult,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
    let mut max_arrival = 0.0_f64;
    for net in 0..nl.net_count() {
        if let Some(arrival) = timing.net_arrival(net) {
            max_arrival = max_arrival.max(arrival);
        }
    }
    let mut net_weights = std::collections::HashMap::new();
    if max_arrival > 0.0 {
        for net in 0..nl.net_count() {
            if let Some(arrival) = timing.net_arrival(net) {
                net_weights.insert(net, 1.0 + 9.0 * (arrival / max_arrival));
            }
        }
    }
    let options = TimingDrivenPlacementOptions {
        net_weights,
        default_net_weight: 1.0,
        max_passes,
    };
    eda_pnr::place_timing_driven(nl, &options)
}

/// Derive net weights from constrained late-path slack.
///
/// The most critical endpoint net receives weight `1 + gain`; the least
/// critical receives weight 1. Weights are propagated through the endpoint's
/// combinational fan-in cone so the complete late path is prioritized.
pub fn timing_weights_from_constrained_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    gain: f64,
) -> Result<std::collections::HashMap<usize, f64>, String> {
    if !gain.is_finite() || gain < 0.0 {
        return Err("timing criticality gain must be finite and non-negative".into());
    }

    let endpoint_slacks = timing
        .net_slacks
        .iter()
        .filter_map(|(name, slack)| {
            slack
                .late_slack_ns()
                .filter(|value| value.is_finite())
                .map(|value| (name.as_str(), value))
        })
        .collect::<Vec<_>>();
    let mut weights = std::collections::HashMap::new();
    if endpoint_slacks.is_empty() {
        return Ok(weights);
    }
    let min_slack = endpoint_slacks
        .iter()
        .map(|(_, slack)| *slack)
        .fold(f64::INFINITY, f64::min);
    let max_slack = endpoint_slacks
        .iter()
        .map(|(_, slack)| *slack)
        .fold(f64::NEG_INFINITY, f64::max);
    let range = max_slack - min_slack;

    for (name, slack) in endpoint_slacks {
        let criticality = if range.abs() < 1e-12 {
            1.0
        } else {
            (max_slack - slack) / range
        };
        let weight = 1.0 + gain * criticality;
        let Some(endpoint) = nl.net_by_name(name) else {
            return Err(format!("constrained slack net '{}' not found", name));
        };
        let mut stack = vec![endpoint];
        let mut seen = std::collections::HashSet::new();
        while let Some(net) = stack.pop() {
            if !seen.insert(net) {
                continue;
            }
            weights
                .entry(net)
                .and_modify(|old| *old = (*old).max(weight))
                .or_insert(weight);
            for &driver in &nl.net(net).drivers {
                let cell = nl.cell(driver);
                if nl.cell_is_sequential(&cell.kind) {
                    continue;
                }
                stack.extend(cell.inputs.iter().copied());
            }
        }
    }
    Ok(weights)
}

/// Derive timing weights while excluding checks covered by active waivers.
///
/// Raw violations remain available in `store`; only their endpoint slack is
/// removed from the optimizer objective. This lets an implementation keep
/// auditable timing history without spending physical-optimization effort on
/// accepted exceptions.
pub fn timing_weights_from_waiver_aware_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    gain: f64,
) -> Result<std::collections::HashMap<usize, f64>, String> {
    let mut objective_timing = timing.clone();
    for violation_id in store.waived_violations().keys() {
        let Some(violation) = store.violations().get(violation_id.as_str()) else {
            return Err(format!(
                "waived timing violation '{}' is missing from its store",
                violation_id
            ));
        };
        if matches!(
            violation.kind,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow
        ) {
            continue;
        }
        let Some(net_name) = violation.nets.last() else {
            return Err(format!(
                "waived timing violation '{}' has no endpoint net",
                violation_id
            ));
        };
        let name = net_name.clone();
        let Some(slack) = objective_timing.net_slacks.get_mut(&name) else {
            return Err(format!(
                "waived timing endpoint net '{}' has no constrained slack",
                name
            ));
        };
        match violation.kind {
            TimingViolationKind::Setup => slack.setup_ns = None,
            TimingViolationKind::Hold => slack.hold_ns = None,
            TimingViolationKind::Recovery => slack.recovery_ns = None,
            TimingViolationKind::Removal => slack.removal_ns = None,
            TimingViolationKind::Output => slack.output_ns = None,
            TimingViolationKind::MaxDelay => slack.max_delay_ns = None,
            TimingViolationKind::MinDelay => slack.min_delay_ns = None,
            TimingViolationKind::MinPulseHigh | TimingViolationKind::MinPulseLow => continue,
        }
    }
    timing_weights_from_constrained_timing(nl, &objective_timing, gain)
}

/// Slack-aware timing-driven placement.
pub fn timing_driven_place_from_constraints(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
    let net_weights = timing_weights_from_constrained_timing(nl, timing, 9.0)?;
    eda_pnr::place_timing_driven(
        nl,
        &TimingDrivenPlacementOptions {
            net_weights,
            default_net_weight: 1.0,
            max_passes,
        },
    )
}

/// Waiver-aware slack-driven placement.
pub fn timing_driven_place_from_waiver_aware_timing(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    store: &TimingViolationStore,
    max_passes: usize,
) -> Result<TimingDrivenPlacementResult, String> {
    let net_weights = timing_weights_from_waiver_aware_timing(nl, timing, store, 9.0)?;
    eda_pnr::place_timing_driven(
        nl,
        &TimingDrivenPlacementOptions {
            net_weights,
            default_net_weight: 1.0,
            max_passes,
        },
    )
}
