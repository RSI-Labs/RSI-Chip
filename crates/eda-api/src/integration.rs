//! Waveform, coverage, DFT, ECO, hold repair, CTS, UPF, and power-grid facade APIs.

use super::*;

pub fn parse_vcd(source: &str) -> Result<Vcd, String> {
    eda_waveform::parse_vcd(source)
}

/// Parse SVA-style assertions through the unified API.
pub fn parse_assertions(source: &str) -> Result<Vec<Assertion>, String> {
    eda_assert::parse_assertions(source)
}

/// Check assertions against a VCD through the unified API.
pub fn check_assertions(
    waveform: &Vcd,
    assertions: &[Assertion],
) -> Result<Vec<AssertionResult>, String> {
    eda_assert::check_vcd(waveform, assertions)
}

/// Parse a functional-coverage plan through the unified API.
pub fn parse_covergroups(source: &str) -> Result<CoveragePlan, String> {
    eda_coverage::parse_coverage(source)
}

/// Check functional and toggle coverage against a VCD.
pub fn functional_coverage(waveform: &Vcd, plan: &CoveragePlan) -> Result<CoverageReport, String> {
    eda_coverage::check_coverage(waveform, plan)
}

/// Infer clock domains and report clock-domain crossings.
pub fn clock_domain_crossing(netlist: &Netlist) -> Result<CdcResult, String> {
    eda_cdc::analyze(netlist)
}

/// Insert a scan chain into a mapped sequential netlist.
pub fn insert_scan(netlist: &Netlist, options: &ScanOptions) -> Result<ScanChainResult, String> {
    eda_dft::insert_scan_chain(netlist, options)
}

/// Analyze scan coverage and unscanned sequential cells.
pub fn dft_analysis(netlist: &Netlist) -> Result<DftReport, String> {
    eda_dft::analyze(netlist)
}

/// Generate deterministic ATPG patterns for a combinational netlist.
pub fn generate_patterns(
    netlist: &Netlist,
    options: &PatternOptions,
) -> Result<Vec<Pattern>, String> {
    eda_fault::automatic_patterns(netlist, options)
}

/// Generate patterns and grade single stuck-at net faults.
pub fn fault_coverage(
    netlist: &Netlist,
    options: &PatternOptions,
) -> Result<FaultCoverageResult, String> {
    eda_fault::grade_automatic(netlist, options)
}

/// Export generated fault patterns as the compact STIL subset.
pub fn patterns_to_stil(
    netlist: &Netlist,
    patterns: &[Pattern],
    name: &str,
) -> Result<String, String> {
    eda_fault::write_stil(netlist, patterns, name)
}

/// Read back the compact STIL subset emitted by [`patterns_to_stil`].
pub fn parse_stil_patterns(source: &str) -> Result<StilPatternSet, String> {
    eda_fault::parse_stil(source)
}

/// Parse an ECO patch through the unified API.
pub fn parse_eco_patch(source: &str) -> Result<EcoPatch, String> {
    eda_eco::parse_eco(source)
}

/// Apply a structural ECO patch to a mapped netlist.
pub fn apply_eco_patch(netlist: &Netlist, patch: &EcoPatch) -> Result<EcoResult, String> {
    eda_eco::apply_eco(netlist, patch)
}

/// Apply an ECO patch within one stable hierarchy-instance namespace.
pub fn apply_eco_patch_in_hierarchy(
    netlist: &Netlist,
    patch: &EcoPatch,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::apply_eco_in_hierarchy(netlist, patch, hierarchy_pattern)
}

/// Automatically repair high-fanout nets with functionally transparent buffers.
pub fn repair_fanout(
    netlist: &Netlist,
    options: &FanoutRepairOptions,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout(netlist, options)
}

/// Automatically repair high fanout and legalize inserted ECO buffers.
pub fn repair_fanout_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &FanoutRepairOptions,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_with_placement(netlist, placement, options)
}

/// Automatically repair high-fanout nets in one hierarchy namespace.
pub fn repair_fanout_in_hierarchy(
    netlist: &Netlist,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_in_hierarchy(netlist, options, hierarchy_pattern)
}

/// Repair and legalize high-fanout nets in one hierarchy namespace.
pub fn repair_fanout_with_placement_in_hierarchy(
    netlist: &Netlist,
    placement: &eda_pnr::Placement,
    options: &FanoutRepairOptions,
    hierarchy_pattern: &str,
) -> Result<EcoResult, String> {
    eda_eco::repair_fanout_with_placement_in_hierarchy(
        netlist,
        placement,
        options,
        hierarchy_pattern,
    )
}

/// Options for automatic hold-violation repair.
#[derive(Clone, Debug)]
pub struct HoldRepairOptions {
    pub patch_name: String,
    pub max_buffers_per_endpoint: usize,
    /// Hold checks covered by these active waivers are not repaired. Raw
    /// timing remains available to the caller.
    pub waivers: Vec<TimingViolationWaiver>,
}

impl Default for HoldRepairOptions {
    fn default() -> Self {
        Self {
            patch_name: "hold_repair".into(),
            max_buffers_per_endpoint: 4,
            waivers: Vec::new(),
        }
    }
}

impl HoldRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.patch_name.trim().is_empty() {
            return Err("hold repair patch name cannot be empty".into());
        }
        if self.max_buffers_per_endpoint == 0 {
            return Err("hold repair requires a positive buffer limit".into());
        }
        for waiver in &self.waivers {
            let mut enabled = TimingViolationStore::new("");
            enabled.add_waiver(waiver.clone()).map_err(|error| {
                format!("invalid hold repair waiver '{}': {}", waiver.id, error)
            })?;
        }
        Ok(())
    }
}

/// One repaired hold endpoint.
#[derive(Clone, Debug)]
pub struct HoldRepair {
    pub endpoint: String,
    pub endpoint_pin: usize,
    pub old_slack_ns: f64,
    pub new_slack_ns: f64,
    pub buffers_inserted: usize,
}

/// Result of transparent buffer insertion on negative hold-slack endpoints.
#[derive(Clone, Debug)]
pub struct HoldRepairResult {
    pub netlist: Netlist,
    pub physical_placement: Option<Placement>,
    pub repairs: Vec<HoldRepair>,
    pub buffers_inserted: usize,
    pub all_active_violations_repaired: bool,
}

/// Render hold-repair actions and post-repair status.
pub fn hold_repair_report(result: &HoldRepairResult) -> String {
    let mut out = format!(
        "Hold repair: {} buffer(s), {} endpoint(s), status {}\n",
        result.buffers_inserted,
        result.repairs.len(),
        if result.all_active_violations_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  {} pin {}: {} buffer(s), slack {:.6} -> {:.6} ns\n",
            repair.endpoint,
            repair.endpoint_pin,
            repair.buffers_inserted,
            repair.old_slack_ns,
            repair.new_slack_ns
        ));
    }
    out
}

/// Options for common multi-corner hold repair.
#[derive(Clone, Debug, Default)]
pub struct MultiCornerHoldRepairOptions {
    pub hold: HoldRepairOptions,
    pub route: RouteOptions,
    pub interconnect: InterconnectModel,
}

impl MultiCornerHoldRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        self.hold.validate()?;
        self.route.validate()?;
        self.interconnect.validate()
    }
}

/// Per-endpoint hold repair evidence across every timing scenario.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepair {
    pub endpoint: String,
    pub endpoint_pin: usize,
    pub old_slack_ns_by_scenario: std::collections::BTreeMap<String, f64>,
    pub new_slack_ns_by_scenario: std::collections::BTreeMap<String, f64>,
    pub buffers_inserted: usize,
}

/// Final repaired state for one timing scenario.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepairScenario {
    pub scenario: String,
    pub netlist: Netlist,
    pub timing: ConstrainedTiming,
    pub all_active_violations_repaired: bool,
}

/// Result of common buffer insertion across multi-corner timing views.
#[derive(Clone, Debug)]
pub struct MultiCornerHoldRepairResult {
    pub scenarios: Vec<MultiCornerHoldRepairScenario>,
    pub physical_placement: Placement,
    pub routing: Routing,
    pub repairs: Vec<MultiCornerHoldRepair>,
    pub buffers_inserted: usize,
    pub all_active_violations_repaired: bool,
}

/// Repair active hold violations in every timing scenario with one common,
/// stable-name buffer topology.
///
/// After every common buffer round the first physical view is rerouted. RC is
/// extracted from that common route, converted to SDF, and back-annotated
/// separately into every scenario before hold constraints are re-evaluated.
pub fn repair_multicorner_hold_violations(
    scenarios: &[MultiCornerPhysicalTimingScenario<'_>],
    placement: &Placement,
    options: &MultiCornerHoldRepairOptions,
) -> Result<MultiCornerHoldRepairResult, String> {
    options.validate()?;
    validate_common_physical_topology(scenarios)?;
    let mut netlists = scenarios
        .iter()
        .map(|scenario| scenario.netlist.clone())
        .collect::<Vec<_>>();
    for scenario in scenarios {
        eda_pnr::check_legal(scenario.netlist, placement)?;
    }

    let mut current_placement = placement.clone();
    let mut current_routing =
        eda_pnr::route_with_options(&netlists[0], &current_placement, &options.route)?;
    let mut inserted = std::collections::BTreeMap::<String, usize>::new();
    let mut old_slacks = std::collections::BTreeMap::<(String, String), f64>::new();

    fn local_scenarios<'a>(
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

    fn evaluate_views(
        source: &[MultiCornerPhysicalTimingScenario<'_>],
        netlists: &[Netlist],
        placement: &Placement,
        routing: &Routing,
        model: &InterconnectModel,
        common_waivers: &[TimingViolationWaiver],
    ) -> Result<Vec<(Netlist, ConstrainedTiming, TimingViolationStore)>, String> {
        let mut out = Vec::with_capacity(source.len());
        for (scenario, netlist) in source.iter().zip(netlists) {
            let mapped_placement =
                map_placement_by_names(&netlists[0], placement, netlist, &scenario.name)?;
            let mapped_routing =
                map_routing_by_names(&netlists[0], routing, netlist, &scenario.name)?;
            let parasitics = extract_interconnect_parasitics(netlist, &mapped_routing, model)?;
            let sdf_text = parasitics_to_sdf(netlist, &parasitics)?;
            let sdf = parse_delay_file(&sdf_text)?;
            let annotated = back_annotate(netlist, &sdf)?;
            let timing = constrained_timing(&annotated, scenario.constraints)?;
            let mut store = timing_violation_store(&scenario.name, &annotated, &timing)?;
            for waiver in &scenario.waivers {
                add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            for waiver in common_waivers {
                add_timing_violation_waiver(&mut store, waiver.clone())?;
            }
            let drc = physical_verification(&annotated, &mapped_placement, &mapped_routing)?;
            if drc.has_errors() {
                return Err(format!(
                    "multi-corner hold repair scenario '{}' has DRC errors: {}",
                    scenario.name,
                    eda_pnr::drc_report(&drc)
                ));
            }
            out.push((annotated, timing, store));
        }
        Ok(out)
    }

    loop {
        let views = local_scenarios(scenarios, &netlists);
        let states = evaluate_views(
            &views,
            &netlists,
            &current_placement,
            &current_routing,
            &options.interconnect,
            &options.hold.waivers,
        )?;
        let mut endpoints = std::collections::BTreeSet::new();
        for (scenario, state) in scenarios.iter().zip(&states) {
            let (_, timing, store) = state;
            for path in constrained_hold_timing_paths(&state.0, timing, 0) {
                let TimingPathEndpoint::SequentialCell(endpoint) = &path.path.path.endpoint else {
                    continue;
                };
                if path.slack_ns >= 0.0
                    || !path.slack_ns.is_finite()
                    || store
                        .waived_violations()
                        .contains_key(&format!("{}:hold:{}", timing.clock, endpoint))
                {
                    continue;
                }
                old_slacks
                    .entry((scenario.name.clone(), endpoint.clone()))
                    .or_insert(path.slack_ns);
                endpoints.insert(endpoint.clone());
            }
        }

        let mut inserted_this_round = false;
        for endpoint in endpoints {
            let count = inserted.get(&endpoint).copied().unwrap_or(0);
            if count >= options.hold.max_buffers_per_endpoint {
                continue;
            }
            for netlist in &mut netlists {
                insert_hold_buffer(netlist, &endpoint, &options.hold.patch_name, count)?;
            }
            inserted.insert(endpoint, count + 1);
            inserted_this_round = true;
        }
        if !inserted_this_round {
            break;
        }

        let views = local_scenarios(scenarios, &netlists);
        validate_common_physical_topology(&views)?;
        let mut placements = Vec::with_capacity(netlists.len());
        for netlist in &netlists {
            placements.push(extend_physical_placement(netlist, &current_placement)?);
        }
        if placements.iter().any(|extended| *extended != placements[0]) {
            return Err(
                "multi-corner hold repair produced inconsistent physical placements".into(),
            );
        }
        current_placement = placements.remove(0);
        current_routing =
            eda_pnr::route_with_options(&netlists[0], &current_placement, &options.route)?;
    }

    let views = local_scenarios(scenarios, &netlists);
    let final_states = evaluate_views(
        &views,
        &netlists,
        &current_placement,
        &current_routing,
        &options.interconnect,
        &options.hold.waivers,
    )?;
    let mut repaired_scenarios = Vec::new();
    let mut all_repaired = true;
    for ((scenario, state), netlist) in scenarios.iter().zip(&final_states).zip(&netlists) {
        let (annotated, timing, store) = state;
        let scenario_repaired = !constrained_hold_timing_paths(annotated, timing, 0)
            .into_iter()
            .any(|path| {
                path.slack_ns < 0.0
                    && path.slack_ns.is_finite()
                    && !store.waived_violations().contains_key(&format!(
                        "{}:hold:{}",
                        timing.clock,
                        endpoint_name(&path.path.path.endpoint)
                    ))
            });
        all_repaired &= scenario_repaired;
        repaired_scenarios.push(MultiCornerHoldRepairScenario {
            scenario: scenario.name.clone(),
            netlist: netlist.clone(),
            timing: timing.clone(),
            all_active_violations_repaired: scenario_repaired,
        });
    }

    let endpoints = inserted.keys().cloned().collect::<Vec<_>>();
    let mut repairs = Vec::new();
    for endpoint in endpoints {
        let mut old_by_scenario = std::collections::BTreeMap::new();
        let mut new_by_scenario = std::collections::BTreeMap::new();
        for (scenario, state) in scenarios.iter().zip(&final_states) {
            let old = old_slacks
                .get(&(scenario.name.clone(), endpoint.clone()))
                .copied();
            if let Some(old) = old {
                old_by_scenario.insert(scenario.name.clone(), old);
            }
            let new = constrained_hold_timing_paths(&state.0, &state.1, 0)
                .into_iter()
                .find(|path| {
                    matches!(
                        &path.path.path.endpoint,
                        TimingPathEndpoint::SequentialCell(name) if *name == endpoint
                    )
                })
                .map(|path| path.slack_ns)
                .unwrap_or(f64::INFINITY);
            new_by_scenario.insert(scenario.name.clone(), new);
        }
        repairs.push(MultiCornerHoldRepair {
            endpoint: endpoint.clone(),
            endpoint_pin: 1,
            old_slack_ns_by_scenario: old_by_scenario,
            new_slack_ns_by_scenario: new_by_scenario,
            buffers_inserted: inserted.get(&endpoint).copied().unwrap_or(0),
        });
    }
    let buffers_inserted = inserted.values().sum();
    Ok(MultiCornerHoldRepairResult {
        scenarios: repaired_scenarios,
        physical_placement: current_placement,
        routing: current_routing,
        repairs,
        buffers_inserted,
        all_active_violations_repaired: all_repaired,
    })
}

/// Render common multi-corner hold-repair actions and post-repair status.
pub fn multi_corner_hold_repair_report(result: &MultiCornerHoldRepairResult) -> String {
    let mut out = format!(
        "Multi-corner hold repair: {} common buffer(s), {} endpoint(s), status {}\n",
        result.buffers_inserted,
        result.repairs.len(),
        if result.all_active_violations_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  {} pin {}: {} common buffer(s)\n",
            repair.endpoint, repair.endpoint_pin, repair.buffers_inserted
        ));
        for scenario in repair.old_slack_ns_by_scenario.keys() {
            out.push_str(&format!(
                "    scenario '{}': slack {:.6} -> {:.6} ns\n",
                scenario,
                repair.old_slack_ns_by_scenario[scenario],
                repair
                    .new_slack_ns_by_scenario
                    .get(scenario)
                    .copied()
                    .unwrap_or(f64::INFINITY)
            ));
        }
    }
    out
}

/// Repair unwaived hold violations by inserting transparent buffers before
/// sequential data pins.
///
/// Repair is iterative: after each buffer round the constrained timing is
/// recomputed. This avoids over-buffering endpoints that need only one delay
/// element while still supporting endpoints that require several buffers.
pub fn repair_hold_violations(
    netlist: &Netlist,
    constraints: &TimingConstraints,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    repair_hold_violations_impl(netlist, constraints, None, options)
}

/// Repair hold violations and legalize every inserted physical buffer.
pub fn repair_hold_violations_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    constraints: &TimingConstraints,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    eda_pnr::check_legal(netlist, placement)?;
    repair_hold_violations_impl(netlist, constraints, Some(placement), options)
}

fn repair_hold_violations_impl(
    netlist: &Netlist,
    constraints: &TimingConstraints,
    placement: Option<&Placement>,
    options: &HoldRepairOptions,
) -> Result<HoldRepairResult, String> {
    options.validate()?;
    let mut out = netlist.clone();
    let mut old_slacks = std::collections::BTreeMap::new();
    let mut inserted = std::collections::BTreeMap::new();
    let mut current = constrained_timing(&out, constraints)?;

    loop {
        let mut violation_store = timing_violation_store(&current.clock, &out, &current)?;
        for waiver in &options.waivers {
            add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
        }
        let waived = violation_store.waived_violations().clone();
        let mut repaired_this_round = false;
        for path in constrained_hold_timing_paths(&out, &current, 0) {
            let endpoint = match &path.path.path.endpoint {
                TimingPathEndpoint::SequentialCell(name) => name.clone(),
                TimingPathEndpoint::OutputPort(_) => continue,
            };
            if path.slack_ns >= 0.0
                || !path.slack_ns.is_finite()
                || waived.contains_key(&format!("{}:hold:{}", current.clock, endpoint))
            {
                continue;
            }
            old_slacks.entry(endpoint.clone()).or_insert(path.slack_ns);
            let count = *inserted.get(&endpoint).unwrap_or(&0);
            if count >= options.max_buffers_per_endpoint {
                continue;
            }
            insert_hold_buffer(&mut out, &endpoint, &options.patch_name, count)?;
            inserted.insert(endpoint, count + 1);
            repaired_this_round = true;
        }
        if !repaired_this_round {
            break;
        }
        current = constrained_timing(&out, constraints)?;
    }

    let final_paths = constrained_hold_timing_paths(&out, &current, 0);
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

    let mut violation_store = timing_violation_store(&current.clock, &out, &current)?;
    for waiver in &options.waivers {
        add_timing_violation_waiver(&mut violation_store, waiver.clone())?;
    }
    let active_hold_negative = constrained_hold_timing_paths(&out, &current, 0)
        .into_iter()
        .any(|path| {
            path.slack_ns < 0.0
                && path.slack_ns.is_finite()
                && !violation_store.waived_violations().contains_key(&format!(
                    "{}:hold:{}",
                    current.clock,
                    endpoint_name(&path.path.path.endpoint)
                ))
        });
    let physical_placement = match placement {
        Some(original) if !repairs.is_empty() => Some(eda_pnr::extend_placement(&out, original)?),
        Some(original) => Some(original.clone()),
        None => None,
    };
    let buffers_inserted = inserted.values().sum();
    Ok(HoldRepairResult {
        netlist: out,
        physical_placement,
        repairs,
        buffers_inserted,
        all_active_violations_repaired: !active_hold_negative,
    })
}

pub(crate) fn insert_hold_buffer(
    netlist: &mut Netlist,
    endpoint: &str,
    patch_name: &str,
    index: usize,
) -> Result<(), String> {
    let endpoint_id = (0..netlist.cell_count())
        .find(|&cell| netlist.cell(cell).name == endpoint)
        .ok_or_else(|| format!("hold endpoint cell '{}' not found", endpoint))?;
    let pin = 1;
    let Some(&source) = netlist.cell(endpoint_id).inputs.get(pin) else {
        return Err(format!("hold endpoint '{}' has no data pin", endpoint));
    };
    let safe_endpoint = endpoint
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let safe_patch = patch_name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let cell_name = format!("{}_{}_{}", safe_patch, safe_endpoint, index);
    let net_name = format!("{}_net", cell_name);
    if netlist.net_by_name(&net_name).is_some()
        || (0..netlist.cell_count()).any(|cell| netlist.cell(cell).name == cell_name)
    {
        return Err(format!("hold repair name '{}' is already used", cell_name));
    }
    let new_net = netlist.add_net(net_name);
    let kind = hold_buffer_kind(netlist);
    netlist.add_cell(eda_core::netlist::Cell {
        name: cell_name,
        kind,
        inputs: vec![source],
        output: new_net,
    });
    let mut endpoint_cell = netlist.cell(endpoint_id).clone();
    endpoint_cell.inputs[pin] = new_net;
    netlist.replace_cell(endpoint_id, endpoint_cell);
    Ok(())
}

fn hold_buffer_kind(netlist: &Netlist) -> eda_core::netlist::CellKind {
    netlist
        .library()
        .and_then(|library| library.find("BUF"))
        .map(eda_core::netlist::CellKind::Lib)
        .unwrap_or(eda_core::netlist::CellKind::Buf)
}

pub(crate) fn endpoint_name(endpoint: &TimingPathEndpoint) -> String {
    match endpoint {
        TimingPathEndpoint::SequentialCell(name) => name.clone(),
        TimingPathEndpoint::OutputPort(name) => name.clone(),
    }
}

/// Extend a legal placement prefix to cover newly appended physical cells.
pub fn extend_physical_placement(
    netlist: &Netlist,
    placement: &Placement,
) -> Result<Placement, String> {
    eda_pnr::extend_placement(netlist, placement)
}

/// Parse a UPF power-intent subset through the unified API.
pub fn parse_power_intent(source: &str) -> Result<Upf, String> {
    eda_upf::parse_upf(source)
}

/// Check UPF power intent against an elaborated design.
pub fn power_intent_check(design: &Design, upf: &Upf) -> UpfResult {
    eda_upf::check(design, upf)
}

/// Synthesize balanced clock trees for every inferred clock domain.
pub fn clock_tree_synthesis(
    netlist: &Netlist,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    eda_cts::synthesize(netlist, options)
}

/// Placement-aware clock-tree synthesis through the unified API.
pub fn clock_tree_synthesis_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &CtsOptions,
) -> Result<ClockTreeResult, String> {
    eda_cts::synthesize_with_placement(netlist, placement, options)
}

/// Search placement-aware CTS implementations against a skew target.
pub fn timing_driven_clock_tree_synthesis_with_placement(
    netlist: &Netlist,
    placement: &Placement,
    options: &TimingDrivenCtsOptions,
) -> Result<TimingDrivenCtsResult, String> {
    eda_cts::synthesize_timing_driven_with_placement(netlist, placement, options)
}

/// Static power-grid IR-drop and electromigration analysis.
pub fn power_grid_ir(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &IrOptions,
) -> Result<IrResult, String> {
    eda_ir::analyze(netlist, placement, power, options)
}

/// Finite-difference VDD/GND power-mesh IR analysis.
pub fn power_grid_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    eda_ir::analyze_mesh(netlist, placement, power, options)
}

/// Layer/via-aware finite-difference VDD/GND power-mesh IR analysis.
pub fn multilayer_power_grid_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_multilayer_mesh(netlist, placement, power, options)
}

/// Layer/via-aware mesh analysis from explicit cell currents.
pub fn multilayer_power_grid_mesh_currents(
    netlist: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_multilayer_mesh_currents(
        netlist,
        placement,
        cell_current_a,
        total_power_uw,
        options,
    )
}

/// Render a layer/via-aware power-mesh report.
pub fn multilayer_power_mesh_report(result: &MultilayerPowerMeshResult) -> String {
    eda_ir::multilayer_power_mesh_report(result)
}

/// Serialize multilayer power-domain supply/tap definitions.
pub fn write_multilayer_power_domain_supplies(
    supplies: &[MultilayerPowerDomainSupply],
) -> Result<String, String> {
    let mut names = std::collections::BTreeSet::new();
    let mut out = String::new();
    for supply in supplies {
        if supply.name.trim().is_empty() {
            return Err("power-domain supply name cannot be empty".into());
        }
        if !names.insert(supply.name.as_str()) {
            return Err(format!("duplicate power-domain supply '{}'", supply.name));
        }
        if !supply.voltage_v.is_finite() || supply.voltage_v <= 0.0 {
            return Err(format!(
                "power-domain supply '{}' has invalid voltage {}",
                supply.name, supply.voltage_v
            ));
        }
        if supply.taps.is_empty() {
            return Err(format!(
                "power-domain supply '{}' requires at least one tap",
                supply.name
            ));
        }
        out.push_str(&format!("supply {} {:?}\n", supply.name, supply.voltage_v));
        for tap in &supply.taps {
            if !tap.vdd && !tap.gnd {
                return Err(format!(
                    "power-domain supply '{}' has a tap with neither VDD nor GND enabled",
                    supply.name
                ));
            }
            let network = match (tap.vdd, tap.gnd) {
                (true, true) => "both",
                (true, false) => "vdd",
                (false, true) => "gnd",
                (false, false) => unreachable!(),
            };
            out.push_str(&format!(
                "tap {} {} {} {}\n",
                tap.layer, tap.row, tap.column, network
            ));
        }
        out.push_str("end\n");
    }
    Ok(out)
}

/// Parse the power-domain supply/tap format emitted by
/// [`write_multilayer_power_domain_supplies`].
pub fn parse_multilayer_power_domain_supplies(
    source: &str,
) -> Result<Vec<MultilayerPowerDomainSupply>, String> {
    struct PartialSupply {
        name: String,
        voltage_v: f64,
        taps: Vec<PowerMeshLayerTap>,
    }

    let mut supplies = Vec::new();
    let mut current: Option<PartialSupply> = None;
    let mut names = std::collections::BTreeSet::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let line_number = index + 1;
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        match tokens.first().copied() {
            Some("supply") => {
                if current.is_some() {
                    return Err(format!(
                        "line {}: previous power-domain supply block is not closed",
                        line_number
                    ));
                }
                if tokens.len() != 3 {
                    return Err(format!(
                        "line {}: supply requires a name and voltage",
                        line_number
                    ));
                }
                let name = tokens[1].to_string();
                if name.trim().is_empty() {
                    return Err(format!("line {}: supply name cannot be empty", line_number));
                }
                if !names.insert(name.clone()) {
                    return Err(format!("line {}: duplicate supply '{}'", line_number, name));
                }
                let voltage_v = tokens[2].parse::<f64>().map_err(|_| {
                    format!(
                        "line {}: invalid supply voltage '{}'",
                        line_number, tokens[2]
                    )
                })?;
                if !voltage_v.is_finite() || voltage_v <= 0.0 {
                    return Err(format!(
                        "line {}: supply '{}' voltage must be positive and finite",
                        line_number, name
                    ));
                }
                current = Some(PartialSupply {
                    name,
                    voltage_v,
                    taps: Vec::new(),
                });
            }
            Some("tap") => {
                let Some(supply) = current.as_mut() else {
                    return Err(format!(
                        "line {}: tap appears outside a supply block",
                        line_number
                    ));
                };
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: tap requires layer, row, column, and network",
                        line_number
                    ));
                }
                let parse_coordinate = |token: &str, name: &str| {
                    token.parse::<usize>().map_err(|_| {
                        format!("line {}: invalid tap {} '{}'", line_number, name, token)
                    })
                };
                let layer = parse_coordinate(tokens[1], "layer")?;
                let row = parse_coordinate(tokens[2], "row")?;
                let column = parse_coordinate(tokens[3], "column")?;
                let (vdd, gnd) = match tokens[4] {
                    "vdd" => (true, false),
                    "gnd" => (false, true),
                    "both" => (true, true),
                    other => {
                        return Err(format!(
                            "line {}: unknown tap network '{}'; expected vdd, gnd, or both",
                            line_number, other
                        ))
                    }
                };
                supply.taps.push(PowerMeshLayerTap {
                    layer,
                    row,
                    column,
                    vdd,
                    gnd,
                });
            }
            Some("end") => {
                let Some(supply) = current.take() else {
                    return Err(format!(
                        "line {}: end appears outside a supply block",
                        line_number
                    ));
                };
                if tokens.len() != 1 {
                    return Err(format!("line {}: malformed supply end", line_number));
                }
                if supply.taps.is_empty() {
                    return Err(format!(
                        "power-domain supply '{}' requires at least one tap",
                        supply.name
                    ));
                }
                if !supply.taps.iter().any(|tap| tap.vdd) {
                    return Err(format!(
                        "power-domain supply '{}' requires at least one VDD tap",
                        supply.name
                    ));
                }
                supplies.push(MultilayerPowerDomainSupply {
                    name: supply.name,
                    voltage_v: supply.voltage_v,
                    taps: supply.taps,
                });
            }
            Some(other) => {
                return Err(format!(
                    "line {}: unknown power-domain supply token '{}'",
                    line_number, other
                ));
            }
            None => {}
        }
    }
    if current.is_some() {
        return Err("unterminated power-domain supply block".into());
    }
    if supplies.is_empty() {
        return Err("power-domain supply file contains no supplies".into());
    }
    Ok(supplies)
}

/// Build UPF IR domain options from multilayer supply-domain definitions.
pub fn multilayer_power_domain_upf_options(
    supplies: &[MultilayerPowerDomainSupply],
    fallback: &IrOptions,
) -> Result<UpfIrOptions, String> {
    let mut options = UpfIrOptions {
        default: Some(DomainIrOptions::from(fallback.clone())),
        domains: std::collections::BTreeMap::new(),
    };
    for supply in supplies {
        if supply.name.trim().is_empty() {
            return Err("power-domain supply name cannot be empty".into());
        }
        if !supply.voltage_v.is_finite() || supply.voltage_v <= 0.0 {
            return Err(format!(
                "power-domain supply '{}' has invalid voltage {}",
                supply.name, supply.voltage_v
            ));
        }
        options.domains.insert(
            supply.name.clone(),
            DomainIrOptions {
                voltage_v: supply.voltage_v,
                ..DomainIrOptions::from(fallback.clone())
            },
        );
    }
    Ok(options)
}

/// UPF-aware static power-grid IR-drop and electromigration analysis.
pub fn power_grid_ir_upf(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfIrResult, String> {
    eda_ir::analyze_upf(netlist, placement, power, power_intent, options)
}

/// Analyze every declared UPF power mode through the unified API.
pub fn power_grid_ir_upf_power_modes(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes(netlist, placement, power, power_intent, options)
}

/// Analyze every UPF power mode with a multilayer power mesh.
pub fn power_grid_ir_upf_power_modes_with_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
        netlist,
        placement,
        power,
        power_intent,
        options,
        mesh_options,
    )
}

/// Analyze all UPF power modes with one shared multi-supply mesh.
pub fn power_grid_ir_upf_power_modes_with_coupled_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    power_intent: &Upf,
    options: &UpfIrOptions,
    mesh_options: &MultilayerPowerMeshOptions,
) -> Result<UpfPowerModeIrResult, String> {
    eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
        netlist,
        placement,
        power,
        power_intent,
        options,
        mesh_options,
    )
}

/// Build domain-decomposed multilayer power meshes from a UPF IR result.
pub fn upf_domain_multilayer_meshes(
    netlist: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
) -> Result<Vec<UpfDomainMultilayerMeshResult>, String> {
    eda_ir::analyze_upf_domain_multilayer_meshes(netlist, placement, result, options)
}

/// Solve one shared multilayer mesh with multiple UPF supply-domain voltages.
pub fn upf_coupled_multilayer_mesh(
    netlist: &Netlist,
    placement: &Placement,
    result: &UpfIrResult,
    options: &MultilayerPowerMeshOptions,
    domain_supplies: &[MultilayerPowerDomainSupply],
) -> Result<MultilayerPowerMeshResult, String> {
    eda_ir::analyze_upf_coupled_multilayer_mesh(
        netlist,
        placement,
        result,
        options,
        domain_supplies,
    )
}

/// Estimate power from a gate-level VCD through the unified API.
pub fn power_from_waveform(
    nl: &Netlist,
    waveform: &Vcd,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_waveform(nl, waveform, opts)
}
