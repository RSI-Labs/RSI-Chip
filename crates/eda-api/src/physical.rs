//! SDF/SPEF, physical verification, antenna repair, power, and post-route IR APIs.

use super::*;

pub fn parse_delay_file(source: &str) -> Result<Sdf, String> {
    eda_sdf::parse_sdf(source)
}

/// Serialize an in-memory SDF through the unified API.
///
/// Unlike [`write_sdf`], this preserves parsed or extracted `INTERCONNECT`
/// timing rather than regenerating nominal library delays.
pub fn write_delay_file(sdf: &Sdf) -> Result<String, String> {
    eda_sdf::write_delay_file(sdf)
}

/// One prepared per-corner SDF output from an MMMC physical implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCornerDelayFileExport {
    pub scenario: String,
    pub path: PathBuf,
    pub text: String,
}

/// Serialize every timing view of an MMMC physical implementation without
/// writing files.
///
/// The template must contain `{corner}` exactly as a placeholder for the
/// stable scenario name, for example `timing/best.{corner}.sdf`.
pub fn prepare_multi_corner_delay_file_exports(
    corners: &[MultiCornerPhysicalTimingCorner],
    path_template: &str,
) -> Result<Vec<MultiCornerDelayFileExport>, String> {
    if corners.is_empty() {
        return Err("MMMC SDF export has no timing corner".into());
    }
    if !path_template.contains("{corner}") {
        return Err(
            "MMMC SDF export template requires a '{corner}' placeholder, for example best.{corner}.sdf"
                .into(),
        );
    }

    let mut scenarios = Vec::new();
    let mut out = Vec::new();
    for corner in corners {
        let scenario = &corner.scenario;
        if scenario.is_empty()
            || scenario == "."
            || scenario == ".."
            || scenario.trim() != scenario
            || scenario.contains('/')
            || scenario.contains('\\')
            || scenario.contains('\0')
        {
            return Err(format!(
                "MMMC SDF export requires safe scenario names; got '{}'",
                corner.scenario
            ));
        }
        if scenarios.iter().any(|name: &String| name == scenario) {
            return Err(format!("duplicate MMMC SDF export scenario '{}'", scenario));
        }
        scenarios.push(scenario.clone());

        let path = PathBuf::from(path_template.replace("{corner}", scenario));
        let text = write_delay_file(&corner.evaluation.sdf)?;
        if out.iter().any(|export: &MultiCornerDelayFileExport| {
            export.path == path || export.scenario == scenario.as_str()
        }) {
            return Err(format!(
                "MMMC SDF template '{}' does not produce unique files",
                path_template
            ));
        }
        out.push(MultiCornerDelayFileExport {
            scenario: scenario.clone(),
            path,
            text,
        });
    }
    Ok(out)
}

/// Write prepared MMMC SDF outputs and return their paths.
pub fn write_multi_corner_delay_file_exports(
    exports: &[MultiCornerDelayFileExport],
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for export in exports {
        if paths.iter().any(|path: &PathBuf| path == &export.path) {
            return Err(format!(
                "duplicate MMMC SDF output path '{}'",
                export.path.display()
            ));
        }
        paths.push(export.path.clone());
    }
    for export in exports {
        std::fs::write(&export.path, &export.text).map_err(|error| {
            format!(
                "cannot write SDF for corner '{}' to {}: {}",
                export.scenario,
                export.path.display(),
                error
            )
        })?;
    }
    Ok(paths)
}

/// Apply SDF timing annotations to a mapped netlist.
pub fn back_annotate(netlist: &Netlist, annotations: &Sdf) -> Result<Netlist, String> {
    eda_sdf::annotate_sdf(netlist, annotations)
}

/// Parse a SPEF parasitic-extraction file through the unified API.
pub fn parse_parasitics(source: &str) -> Result<Spef, String> {
    eda_spef::parse_spef(source)
}

/// Analyze SPEF parasitics against a mapped netlist.
pub fn parasitic_analysis(netlist: &Netlist, parasitics: &Spef) -> Result<ParasiticReport, String> {
    eda_spef::analyze(netlist, parasitics)
}

/// Convert SPEF RC estimates into SDF INTERCONNECT annotations.
pub fn parasitics_to_sdf(netlist: &Netlist, parasitics: &Spef) -> Result<String, String> {
    eda_spef::write_sdf(netlist, parasitics)
}

/// Physical verification through the unified API: placement legality,
/// routing legality, opens, dangling wires, and conservative antenna checks.
pub fn physical_verification(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
) -> Result<DrcResult, String> {
    eda_pnr::drc(nl, placement, routing)
}

/// Physical verification with a foundry-specific maximum antenna ratio.
pub fn physical_verification_with_options(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &DrcOptions,
) -> Result<DrcResult, String> {
    eda_pnr::drc_with_options(nl, placement, routing, options)
}

/// Options for automatic antenna-ratio repair.
#[derive(Clone, Debug)]
pub struct AntennaRepairOptions {
    pub patch_name: String,
    pub max_diodes_per_net: usize,
    pub route: RouteOptions,
}

impl Default for AntennaRepairOptions {
    fn default() -> Self {
        Self {
            patch_name: "antenna_repair".into(),
            max_diodes_per_net: 4,
            route: RouteOptions::default(),
        }
    }
}

impl AntennaRepairOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.patch_name.trim().is_empty() {
            return Err("antenna repair patch name cannot be empty".into());
        }
        if self.max_diodes_per_net == 0 {
            return Err("antenna repair requires a positive diode limit".into());
        }
        self.route.validate()
    }
}

/// One repaired antenna-violating net.
#[derive(Clone, Debug)]
pub struct AntennaRepair {
    pub net: String,
    pub old_ratio: f64,
    pub new_ratio: f64,
    pub diodes_inserted: usize,
}

/// Result of antenna diode insertion and post-repair routing.
#[derive(Clone, Debug)]
pub struct AntennaRepairResult {
    pub netlist: Netlist,
    pub physical_placement: Placement,
    pub routing: Routing,
    pub repairs: Vec<AntennaRepair>,
    pub diodes_inserted: usize,
    pub antenna_warnings: usize,
    pub all_antennas_repaired: bool,
}

/// End-to-end RTL physical-implementation antenna repair result.
#[derive(Clone, Debug)]
pub struct RtlAntennaRepairResult {
    pub baseline_drc: DrcResult,
    pub repair: AntennaRepairResult,
    pub final_mapped_stats: NetlistStats,
}

/// Compile RTL, place/route it, verify antenna ratios, and insert physical DIODEs.
pub fn repair_rtl_antennas(
    src: &str,
    top: Option<&str>,
    options: &AntennaRepairOptions,
) -> Result<RtlAntennaRepairResult, String> {
    repair_rtl_antennas_cancellable(src, top, options, &FlowCancellation::new())
}

pub fn repair_rtl_antennas_cancellable(
    src: &str,
    top: Option<&str>,
    options: &AntennaRepairOptions,
    cancellation: &FlowCancellation,
) -> Result<RtlAntennaRepairResult, String> {
    options.validate()?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let design = compile(src, top)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let netlist =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &CellLibrary::std())?
            .netlist;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let placement = eda_pnr::place(&netlist)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let routing = eda_pnr::route_maze_with_options_cancellable(
        &netlist,
        &placement,
        &options.route,
        cancellation,
    )?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let baseline_drc = physical_verification(&netlist, &placement, &routing)?;
    if baseline_drc.has_errors() {
        return Err(format!(
            "RTL antenna repair requires an error-free route: {}",
            eda_pnr::drc_report(&baseline_drc)
        ));
    }
    let repair = repair_antennas(&netlist, &placement, &routing, options)?;
    if cancellation.is_cancelled() {
        return Err("RTL antenna repair cancelled".into());
    }
    let final_mapped_stats = repair.netlist.stats();
    Ok(RtlAntennaRepairResult {
        baseline_drc,
        repair,
        final_mapped_stats,
    })
}

/// Repair conservative antenna-ratio warnings with physical DIODE loads.
///
/// The repair loop reroutes the complete netlist after every diode-insertion
/// round. Diodes are placed in legal free sites and connected by the detailed
/// maze router; they are not silently counted as logical fanout without a
/// physical path.
pub fn repair_antennas(
    netlist: &Netlist,
    placement: &Placement,
    routing: &Routing,
    options: &AntennaRepairOptions,
) -> Result<AntennaRepairResult, String> {
    options.validate()?;
    let initial_drc = physical_verification(netlist, placement, routing)?;
    if initial_drc.has_errors() {
        return Err(format!(
            "antenna repair requires an error-free route: {}",
            eda_pnr::drc_report(&initial_drc)
        ));
    }

    let mut out = netlist.clone();
    let mut current_placement = placement.clone();
    let mut current_routing = routing.clone();
    let mut inserted = std::collections::BTreeMap::<usize, usize>::new();
    let mut old_ratios = std::collections::BTreeMap::<usize, f64>::new();
    for violation in &initial_drc.violations {
        if violation.code == "ANT-001" {
            if let Some(net) = violation.net {
                old_ratios.insert(
                    net,
                    antenna_ratio(&out, &current_placement, &current_routing, net),
                );
            }
        }
    }

    let mut final_drc = initial_drc;
    loop {
        let violating_nets = final_drc
            .violations
            .iter()
            .filter(|violation| violation.code == "ANT-001")
            .filter_map(|violation| violation.net)
            .collect::<Vec<_>>();
        if violating_nets.is_empty() {
            break;
        }

        let mut inserted_this_round = false;
        for net in violating_nets {
            let count = inserted.get(&net).copied().unwrap_or(0);
            if count >= options.max_diodes_per_net {
                continue;
            }
            insert_antenna_diode(&mut out, net, &options.patch_name, count)?;
            inserted.insert(net, count + 1);
            inserted_this_round = true;
        }
        if !inserted_this_round {
            break;
        }

        current_placement = extend_physical_placement(&out, &current_placement)?;
        current_routing =
            eda_pnr::route_maze_with_options(&out, &current_placement, &options.route)?;
        final_drc = physical_verification(&out, &current_placement, &current_routing)?;
        if final_drc.has_errors() {
            return Err(format!(
                "antenna repair created DRC errors: {}",
                eda_pnr::drc_report(&final_drc)
            ));
        }
    }

    let mut repairs = Vec::new();
    for (net, old_ratio) in old_ratios {
        repairs.push(AntennaRepair {
            net: out.net(net).name.clone(),
            old_ratio,
            new_ratio: antenna_ratio(&out, &current_placement, &current_routing, net),
            diodes_inserted: inserted.get(&net).copied().unwrap_or(0),
        });
    }
    let antenna_warnings = final_drc
        .violations
        .iter()
        .filter(|violation| violation.code == "ANT-001")
        .count();
    let diodes_inserted = inserted.values().sum();
    Ok(AntennaRepairResult {
        netlist: out,
        physical_placement: current_placement,
        routing: current_routing,
        repairs,
        diodes_inserted,
        antenna_warnings,
        all_antennas_repaired: antenna_warnings == 0,
    })
}

fn antenna_ratio(nl: &Netlist, placement: &Placement, routing: &Routing, net: usize) -> f64 {
    let wirelength = routing
        .segments
        .iter()
        .filter(|segment| segment.net == net)
        .map(|segment| segment.length())
        .sum::<usize>();
    let mut pins = std::collections::BTreeSet::new();
    let physical_net = nl.net(net);
    let push_pin = |cell: usize, pins: &mut std::collections::BTreeSet<(usize, usize)>| {
        if let Some(position) = placement.cell_pos.get(cell) {
            let width = placement.cell_width.get(cell).copied().unwrap_or(1);
            let x = (position.site as f64 + width as f64 / 2.0).round() as usize;
            pins.insert((x, position.row));
        }
    };
    for driver in &physical_net.drivers {
        push_pin(*driver, &mut pins);
    }
    for sink in &physical_net.sinks {
        push_pin(*sink, &mut pins);
    }
    wirelength as f64 / pins.len().max(1) as f64
}

fn insert_antenna_diode(
    netlist: &mut Netlist,
    net: usize,
    patch_name: &str,
    index: usize,
) -> Result<(), String> {
    let diode = netlist
        .library()
        .and_then(|library| library.find("DIODE"))
        .ok_or_else(|| "antenna repair requires a library cell named 'DIODE'".to_string())?;
    let safe_net = netlist
        .net(net)
        .name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let safe_patch = patch_name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>();
    let cell_name = format!("{}_{}_{}", safe_patch, safe_net, index);
    let output_name = format!("{}_net", cell_name);
    if netlist.net_by_name(&output_name).is_some()
        || (0..netlist.cell_count()).any(|cell| netlist.cell(cell).name == cell_name)
    {
        return Err(format!(
            "antenna repair name '{}' is already used",
            cell_name
        ));
    }
    let output = netlist.add_net(output_name);
    netlist.add_cell(eda_core::netlist::Cell {
        name: cell_name,
        kind: eda_core::netlist::CellKind::Lib(diode),
        inputs: vec![net],
        output,
    });
    Ok(())
}

/// Render antenna repair actions and post-repair status.
pub fn antenna_repair_report(result: &AntennaRepairResult) -> String {
    let mut out = format!(
        "Antenna repair: {} diode(s), {} net(s), status {}\n",
        result.diodes_inserted,
        result.repairs.len(),
        if result.all_antennas_repaired {
            "PASS"
        } else {
            "LIMITED"
        }
    );
    for repair in &result.repairs {
        out.push_str(&format!(
            "  net '{}': {} diode(s), ratio {:.3} -> {:.3}\n",
            repair.net, repair.diodes_inserted, repair.old_ratio, repair.new_ratio
        ));
    }
    out
}

/// Structural layout-versus-schematic check through the unified API.
pub fn layout_vs_schematic(reference: &Netlist, layout: &Netlist) -> Result<LvsResult, String> {
    eda_lvs::check_lvs(reference, layout)
}

/// Export the physical view of a placed/routed netlist as LEF/DEF text.
pub fn export_physical(
    nl: &Netlist,
    placement: &Placement,
    routing: &Routing,
) -> Result<(String, String), String> {
    Ok((
        eda_pnr::write_lef(nl, placement)?,
        eda_pnr::write_def(nl, placement, routing)?,
    ))
}

/// Parse the compact DEF subset through the unified API.
pub fn parse_physical_def(source: &str) -> Result<eda_pnr::Def, String> {
    eda_pnr::parse_def(source)
}

/// Copy placed-component coordinates from a DEF view into AMS element
/// locations for spatial-mismatch Monte Carlo.
///
/// Components are matched to SPICE elements by case-insensitive stable name.
/// Coordinates use the DEF database-unit scale supplied by the caller. The
/// compact DEF parser currently preserves component coordinates but not its
/// `UNITS DISTANCE MICRONS` statement, so the scale cannot be inferred
/// automatically.
pub fn apply_physical_def_locations(
    circuit: &mut eda_ams::SpiceCircuit,
    def: &eda_pnr::Def,
    database_units_per_micron: f64,
) -> Result<usize, String> {
    if !database_units_per_micron.is_finite() || database_units_per_micron <= 0.0 {
        return Err("DEF database units per micron must be finite and positive".into());
    }
    let meters_per_database_unit = 1.0e-6 / database_units_per_micron;
    let mut matched = 0usize;
    for component in &def.components {
        let Some(element) = circuit
            .elements
            .iter()
            .find(|element| element.name.eq_ignore_ascii_case(&component.name))
        else {
            continue;
        };
        let location = eda_ams::ElementLocation {
            x_m: component.x as f64 * meters_per_database_unit,
            y_m: component.y as f64 * meters_per_database_unit,
        };
        circuit.locations.insert(element.name.clone(), location);
        matched += 1;
    }
    if matched == 0 {
        return Err("DEF has no components matching SPICE element names".into());
    }
    circuit.validate()?;
    Ok(matched)
}

/// Parse and reconstruct placement/routing from compact DEF text.
pub fn import_physical_def_text(
    netlist: &Netlist,
    source: &str,
) -> Result<(Placement, Routing), String> {
    let def = parse_physical_def(source)?;
    import_physical_def(netlist, &def)
}

/// Reconstruct placement and routing from a parsed compact DEF subset.
pub fn import_physical_def(
    netlist: &Netlist,
    def: &eda_pnr::Def,
) -> Result<(Placement, Routing), String> {
    eda_pnr::physical_from_def(netlist, def)
}

/// Gate-level switching-power analysis through the unified API.
pub fn analyze_power(
    nl: &Netlist,
    vectors: &[InputVector],
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze(nl, vectors, opts)
}

/// Estimate switching power with explicit per-net switched capacitance.
pub fn analyze_power_with_capacitances(
    nl: &Netlist,
    vectors: &[InputVector],
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_with_capacitances(nl, vectors, capacitance_pf, opts)
}

/// Estimate waveform-driven power with explicit switched capacitance.
pub fn power_from_waveform_with_capacitances(
    nl: &Netlist,
    waveform: &Vcd,
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    eda_power::analyze_waveform_with_capacitances(nl, waveform, capacitance_pf, opts)
}

/// Generate a deterministic zero → all-ones stimulus pair for a quick power
/// estimate when no application-specific VCD/activity vectors are available.
pub fn default_power_vectors(nl: &Netlist) -> Vec<InputVector> {
    let zero: InputVector = nl
        .ports
        .iter()
        .filter(|p| p.dir == eda_core::netlist::Dir::In)
        .map(|p| (p.name.clone(), LogicWord::zero(p.bits.len())))
        .collect();
    let ones: InputVector = nl
        .ports
        .iter()
        .filter(|p| p.dir == eda_core::netlist::Dir::In)
        .map(|p| (p.name.clone(), LogicWord::ones(p.bits.len())))
        .collect();
    vec![zero, ones]
}

/// Quick default-vector power estimate through the unified API.
pub fn quick_power(nl: &Netlist, opts: &PowerOptions) -> Result<PowerReport, String> {
    let vectors = default_power_vectors(nl);
    analyze_power(nl, &vectors, opts)
}

/// Options for post-route power and power-grid integrity analysis.
#[derive(Clone, Debug)]
pub struct PostRoutePowerIrOptions {
    pub power: PowerOptions,
    pub ir: IrOptions,
    /// Domain-specific supplies and rails for UPF-aware IR analysis.
    pub upf: UpfIrOptions,
    /// Optional finite-difference VDD/GND mesh analysis.
    pub power_mesh: Option<PowerMeshOptions>,
    /// Optional layer/via-aware finite-difference mesh analysis.
    pub multilayer_power_mesh: Option<MultilayerPowerMeshOptions>,
    /// Derive `C*V^2` switched-capacitance energy from the IR supply voltage.
    ///
    /// When disabled, `power.capacitive_energy_per_pf_pj` is used unchanged.
    pub voltage_scaled_capacitance: bool,
}

impl Default for PostRoutePowerIrOptions {
    fn default() -> Self {
        Self {
            power: PowerOptions::default(),
            ir: IrOptions::default(),
            upf: UpfIrOptions::default(),
            power_mesh: None,
            multilayer_power_mesh: None,
            voltage_scaled_capacitance: true,
        }
    }
}

/// Post-route switching power and IR/EM result for one physical iteration.
#[derive(Clone, Debug)]
pub struct PostRoutePowerIrResult {
    pub power: PowerReport,
    pub ir: IrResult,
    /// Domain-resolved result when a UPF power intent was supplied.
    pub upf_ir: Option<UpfIrResult>,
    /// Optional post-route finite-difference mesh result.
    pub power_mesh: Option<PowerMeshResult>,
    /// Optional post-route layer/via-aware finite-difference mesh result.
    pub multilayer_power_mesh: Option<MultilayerPowerMeshResult>,
    /// Dynamic-power increase caused by extracted switched capacitance.
    pub parasitic_dynamic_power_uw: f64,
    /// Effective capacitance used per stable net name.
    pub effective_capacitance_pf: BTreeMap<String, f64>,
}

/// One named post-route power/IR operating condition.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrScenario {
    pub name: String,
    pub options: PostRoutePowerIrOptions,
}

impl MultiCornerPostRoutePowerIrScenario {
    pub fn new(name: impl Into<String>, options: PostRoutePowerIrOptions) -> Result<Self, String> {
        let name = name.into();
        if name.trim().is_empty()
            || name.trim() != name
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Err(format!("invalid post-route power scenario name '{}'", name));
        }
        Ok(Self { name, options })
    }
}

/// Convert generic MMMC scenario properties into post-route power options.
///
/// Properties use the `post_route.` prefix. For example:
///
/// ```text
/// scenario slow
///     set_scenario_property post_route.voltage_v 0.90
///     set_scenario_property post_route.vdd_ohm_per_site 0.05
///     create_clock -name clk -period 4.0 [get_ports clk]
/// endscenario
/// ```
pub fn post_route_power_scenarios_from_timing_scenarios(
    definitions: &[TimingScenarioDefinition],
) -> Result<Vec<MultiCornerPostRoutePowerIrScenario>, String> {
    let mut out = Vec::with_capacity(definitions.len());
    for definition in definitions {
        let mut options = PostRoutePowerIrOptions::default();
        for (property, text) in &definition.properties {
            let Some(key) = property.strip_prefix("post_route.") else {
                continue;
            };
            if key.starts_with("domain.") {
                continue;
            }
            let parse_f64 = || -> Result<f64, String> {
                text.parse::<f64>()
                    .map_err(|_| format!("invalid post-route property '{}': '{}'", property, text))
            };
            match key {
                "frequency_mhz" => options.power.frequency_mhz = parse_f64()?,
                "base_cell_energy_pj" => options.power.base_cell_energy_pj = parse_f64()?,
                "energy_per_site_pj" => options.power.energy_per_site_pj = parse_f64()?,
                "capacitive_energy_per_pf_pj" => {
                    options.power.capacitive_energy_per_pf_pj = parse_f64()?
                }
                "leakage_per_site_nw" => options.power.leakage_per_site_nw = parse_f64()?,
                "voltage_scaled_capacitance" => {
                    options.voltage_scaled_capacitance = match text.as_str() {
                        "true" => true,
                        "false" => false,
                        _ => {
                            return Err(format!(
                                "post-route property '{}' must be true or false",
                                property
                            ))
                        }
                    }
                }
                "voltage_v" => options.ir.voltage_v = parse_f64()?,
                "vdd_ohm_per_site" => options.ir.vdd_ohm_per_site = parse_f64()?,
                "gnd_ohm_per_site" => options.ir.gnd_ohm_per_site = parse_f64()?,
                "max_total_drop_v" => options.ir.max_total_drop_v = parse_f64()?,
                "max_rail_current_a" => options.ir.max_rail_current_a = parse_f64()?,
                _ => {
                    return Err(format!(
                        "unknown post-route scenario property '{}'",
                        property
                    ))
                }
            }
        }

        let default_domain = DomainIrOptions::from(options.ir.clone());
        for (property, text) in &definition.properties {
            let Some(key) = property.strip_prefix("post_route.domain.") else {
                continue;
            };
            let Some((domain, field)) = key.rsplit_once('.') else {
                return Err(format!(
                    "invalid post-route domain property '{}'; expected post_route.domain.<name>.<field>",
                    property
                ));
            };
            if domain.is_empty() || field.is_empty() {
                return Err(format!(
                    "invalid post-route domain property '{}'; domain and field names cannot be empty",
                    property
                ));
            }
            let value = text.parse::<f64>().map_err(|_| {
                format!(
                    "invalid post-route domain property '{}': '{}'",
                    property, text
                )
            })?;
            let domain_options = options
                .upf
                .domains
                .entry(domain.to_string())
                .or_insert(default_domain);
            match field {
                "voltage_v" => domain_options.voltage_v = value,
                "vdd_ohm_per_site" => domain_options.vdd_ohm_per_site = value,
                "gnd_ohm_per_site" => domain_options.gnd_ohm_per_site = value,
                "max_total_drop_v" => domain_options.max_total_drop_v = value,
                "max_rail_current_a" => domain_options.max_rail_current_a = value,
                _ => return Err(format!("unknown post-route domain property '{}'", property)),
            }
        }
        if options.upf.default.is_none() {
            options.upf.default = Some(default_domain);
        }
        validate_post_route_power_ir_options(&options).map_err(|error| {
            format!(
                "invalid post-route options for scenario '{}': {}",
                definition.name, error
            )
        })?;
        out.push(MultiCornerPostRoutePowerIrScenario::new(
            definition.name.clone(),
            options,
        )?);
    }
    Ok(out)
}

/// One scenario result in a multi-corner post-route power/IR analysis.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrCorner {
    pub scenario: String,
    pub result: PostRoutePowerIrResult,
}

/// Per-corner post-route power and power-grid results for one implementation.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerIrResult {
    pub corners: Vec<MultiCornerPostRoutePowerIrCorner>,
    pub worst_power_corner: usize,
    pub worst_drop_corner: usize,
    pub error_findings: usize,
}

/// One MMMC scenario's post-route power results for all UPF modes.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerModeIr {
    pub scenario: String,
    pub modes: UpfPowerModeIrResult,
}

/// MMMC × UPF-power-mode post-route IR/EM aggregation.
#[derive(Clone, Debug)]
pub struct MultiCornerPostRoutePowerModeIrResult {
    pub corners: Vec<MultiCornerPostRoutePowerModeIr>,
    pub worst_power_corner: usize,
    pub worst_power_mode: usize,
    pub worst_drop_corner: usize,
    pub worst_drop_mode: usize,
    pub worst_em_lifetime_corner: usize,
    pub worst_em_lifetime_mode: usize,
    pub error_findings: usize,
}

fn effective_post_route_capacitance(iteration: &PhysicalTimingIteration) -> BTreeMap<String, f64> {
    iteration
        .parasitic_report
        .nets
        .iter()
        .map(|net| {
            (
                net.net.clone(),
                net.grounded_cap_pf + 0.5 * net.coupling_cap_pf,
            )
        })
        .collect()
}

fn post_route_power_option_set(options: &PostRoutePowerIrOptions) -> (PowerOptions, PowerOptions) {
    let mut pre_route = options.power.clone();
    pre_route.capacitive_energy_per_pf_pj = 0.0;
    let mut post_route = options.power.clone();
    if options.voltage_scaled_capacitance {
        post_route.capacitive_energy_per_pf_pj = options.ir.voltage_v * options.ir.voltage_v;
    }
    (pre_route, post_route)
}

fn validate_post_route_power_ir_options(options: &PostRoutePowerIrOptions) -> Result<(), String> {
    let power_fields = [
        ("frequency", options.power.frequency_mhz),
        ("base cell energy", options.power.base_cell_energy_pj),
        ("site energy", options.power.energy_per_site_pj),
        (
            "capacitive energy",
            options.power.capacitive_energy_per_pf_pj,
        ),
        ("leakage per site", options.power.leakage_per_site_nw),
    ];
    for (name, value) in power_fields {
        if !value.is_finite() || value < 0.0 {
            return Err(format!("invalid post-route power {} {}", name, value));
        }
    }
    if options.power.frequency_mhz == 0.0 {
        return Err("post-route power frequency must be positive".into());
    }

    let ir_fields = [
        ("voltage", options.ir.voltage_v),
        ("VDD resistance", options.ir.vdd_ohm_per_site),
        ("GND resistance", options.ir.gnd_ohm_per_site),
        ("maximum drop", options.ir.max_total_drop_v),
        ("maximum rail current", options.ir.max_rail_current_a),
    ];
    for (name, value) in ir_fields {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid post-route IR {} {}", name, value));
        }
    }
    for (domain_name, domain) in &options.upf.domains {
        let fields = [
            ("voltage", domain.voltage_v),
            ("VDD resistance", domain.vdd_ohm_per_site),
            ("GND resistance", domain.gnd_ohm_per_site),
            ("maximum drop", domain.max_total_drop_v),
            ("maximum rail current", domain.max_rail_current_a),
        ];
        for (name, value) in fields {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "invalid post-route IR domain '{}' {} {}",
                    domain_name, name, value
                ));
            }
        }
    }
    if let Some(default) = options.upf.default {
        let fields = [
            ("voltage", default.voltage_v),
            ("VDD resistance", default.vdd_ohm_per_site),
            ("GND resistance", default.gnd_ohm_per_site),
            ("maximum drop", default.max_total_drop_v),
            ("maximum rail current", default.max_rail_current_a),
        ];
        for (name, value) in fields {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "invalid post-route default IR domain {} {}",
                    name, value
                ));
            }
        }
    }
    Ok(())
}

fn finish_post_route_power_ir(
    iteration: &PhysicalTimingIteration,
    options: &PostRoutePowerIrOptions,
    power_intent: Option<&Upf>,
    pre_route_dynamic_power_uw: f64,
    power: PowerReport,
    effective_capacitance_pf: BTreeMap<String, f64>,
) -> Result<PostRoutePowerIrResult, String> {
    let ir = eda_ir::analyze(
        &iteration.annotated_netlist,
        &iteration.placement,
        &power,
        &options.ir,
    )?;
    let mut upf_options = options.upf.clone();
    if upf_options.default.is_none() {
        upf_options.default = Some(DomainIrOptions::from(options.ir.clone()));
    }
    let upf_ir = match power_intent {
        Some(upf) => Some(eda_ir::analyze_upf(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            upf,
            &upf_options,
        )?),
        None => None,
    };
    let parasitic_dynamic_power_uw = power.dynamic_power_uw - pre_route_dynamic_power_uw;
    if !parasitic_dynamic_power_uw.is_finite() || parasitic_dynamic_power_uw < 0.0 {
        return Err("post-route parasitic power contribution is invalid".into());
    }
    let power_mesh = match &options.power_mesh {
        Some(mesh_options) => Some(eda_ir::analyze_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            mesh_options,
        )?),
        None => None,
    };
    let multilayer_power_mesh = match &options.multilayer_power_mesh {
        Some(mesh_options) => {
            if !mesh_options.power_domains.is_empty() {
                let Some(upf_result) = &upf_ir else {
                    return Err("coupled multilayer power domains require UPF power intent".into());
                };
                Some(eda_ir::analyze_upf_coupled_multilayer_mesh(
                    &iteration.annotated_netlist,
                    &iteration.placement,
                    upf_result,
                    mesh_options,
                    &mesh_options.power_domains,
                )?)
            } else {
                Some(eda_ir::analyze_multilayer_mesh(
                    &iteration.annotated_netlist,
                    &iteration.placement,
                    &power,
                    mesh_options,
                )?)
            }
        }
        None => None,
    };
    Ok(PostRoutePowerIrResult {
        power,
        ir,
        upf_ir,
        power_mesh,
        multilayer_power_mesh,
        parasitic_dynamic_power_uw,
        effective_capacitance_pf,
    })
}

/// Analyze the final RC-annotated netlist, placement, and power grid.
///
/// Coupling capacitance is conservatively halved for effective switching
/// load. Dynamic cell activity is simulated on the supplied vectors, while
/// leakage and IR current are distributed over the placed cells.
pub fn analyze_physical_timing_power_ir(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
) -> Result<PostRoutePowerIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    if !options.ir.voltage_v.is_finite() || options.ir.voltage_v <= 0.0 {
        return Err("post-route power analysis requires a positive supply voltage".into());
    }

    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route = eda_power::analyze(&iteration.annotated_netlist, vectors, &pre_route_options)?;
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        None,
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze post-route power/IR from gate-level VCD activity.
pub fn analyze_physical_timing_power_ir_from_waveform(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
) -> Result<PostRoutePowerIrResult, String> {
    if !options.ir.voltage_v.is_finite() || options.ir.voltage_v <= 0.0 {
        return Err("post-route power analysis requires a positive supply voltage".into());
    }
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route =
        eda_power::analyze_waveform(&iteration.annotated_netlist, waveform, &pre_route_options)?;
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        None,
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze a post-route physical view with UPF domain-resolved supplies.
pub fn analyze_physical_timing_power_ir_with_upf(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<PostRoutePowerIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route = eda_power::analyze(&iteration.annotated_netlist, vectors, &pre_route_options)?;
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        Some(power_intent),
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

/// Analyze UPF-aware post-route power/IR from gate-level VCD activity.
pub fn analyze_physical_timing_power_ir_from_waveform_with_upf(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<PostRoutePowerIrResult, String> {
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (pre_route_options, post_route_options) = post_route_power_option_set(options);
    let pre_route =
        eda_power::analyze_waveform(&iteration.annotated_netlist, waveform, &pre_route_options)?;
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    finish_post_route_power_ir(
        iteration,
        options,
        Some(power_intent),
        pre_route.dynamic_power_uw,
        power,
        effective_capacitance_pf,
    )
}

fn post_route_upf_options(options: &PostRoutePowerIrOptions) -> UpfIrOptions {
    let mut upf = options.upf.clone();
    if upf.default.is_none() {
        upf.default = Some(DomainIrOptions::from(options.ir.clone()));
    }
    upf
}

/// Analyze every declared UPF power mode on one post-route implementation.
pub fn analyze_physical_timing_power_ir_power_modes_with_upf(
    iteration: &PhysicalTimingIteration,
    vectors: &[InputVector],
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<UpfPowerModeIrResult, String> {
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (_, post_route_options) = post_route_power_option_set(options);
    let power = eda_power::analyze_with_capacitances(
        &iteration.annotated_netlist,
        vectors,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    if let Some(mesh_options) = &options.multilayer_power_mesh {
        if mesh_options.power_domains.is_empty() {
            eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        } else {
            eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        }
    } else if let Some(mesh_options) = &options.power_mesh {
        eda_ir::analyze_upf_power_modes_with_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
            mesh_options,
        )
    } else {
        eda_ir::analyze_upf_power_modes(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
        )
    }
}

/// Analyze all UPF power modes from post-route VCD activity.
pub fn analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
    iteration: &PhysicalTimingIteration,
    waveform: &Vcd,
    options: &PostRoutePowerIrOptions,
    power_intent: &Upf,
) -> Result<UpfPowerModeIrResult, String> {
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let effective_capacitance_pf = effective_post_route_capacitance(iteration);
    let (_, post_route_options) = post_route_power_option_set(options);
    let power = eda_power::analyze_waveform_with_capacitances(
        &iteration.annotated_netlist,
        waveform,
        &effective_capacitance_pf,
        &post_route_options,
    )?;
    if let Some(mesh_options) = &options.multilayer_power_mesh {
        if mesh_options.power_domains.is_empty() {
            eda_ir::analyze_upf_power_modes_with_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        } else {
            eda_ir::analyze_upf_power_modes_with_coupled_multilayer_mesh(
                &iteration.annotated_netlist,
                &iteration.placement,
                &power,
                power_intent,
                &post_route_upf_options(options),
                mesh_options,
            )
        }
    } else if let Some(mesh_options) = &options.power_mesh {
        eda_ir::analyze_upf_power_modes_with_mesh(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
            mesh_options,
        )
    } else {
        eda_ir::analyze_upf_power_modes(
            &iteration.annotated_netlist,
            &iteration.placement,
            &power,
            power_intent,
            &post_route_upf_options(options),
        )
    }
}

fn validate_multi_corner_post_route_power_scenarios(
    corners: &[MultiCornerPhysicalTimingCorner],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<(), String> {
    if corners.is_empty() || scenarios.is_empty() {
        return Err("multi-corner post-route power analysis has no scenario".into());
    }
    if corners.len() != scenarios.len() {
        return Err(format!(
            "multi-corner post-route power scenario count mismatch: {} physical corners, {} power scenarios",
            corners.len(),
            scenarios.len()
        ));
    }
    for (index, (corner, scenario)) in corners.iter().zip(scenarios.iter()).enumerate() {
        if corner.scenario != scenario.name {
            return Err(format!(
                "post-route power scenario {} is '{}', but physical corner is '{}'",
                index, scenario.name, corner.scenario
            ));
        }
    }
    Ok(())
}

fn finish_multi_corner_post_route_power_ir(
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    results: Vec<PostRoutePowerIrResult>,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    let mut corners = Vec::with_capacity(results.len());
    let mut worst_power_corner = 0;
    let mut worst_drop_corner = 0;
    let mut error_findings = 0;

    for (index, (scenario, result)) in scenarios.iter().zip(results.iter()).enumerate() {
        if result.power.total_power_uw
            > results
                .get(worst_power_corner)
                .map(|previous| previous.power.total_power_uw)
                .unwrap_or(f64::NEG_INFINITY)
        {
            worst_power_corner = index;
        }
        if result.ir.worst_drop_v
            > results
                .get(worst_drop_corner)
                .map(|previous| previous.ir.worst_drop_v)
                .unwrap_or(f64::NEG_INFINITY)
        {
            worst_drop_corner = index;
        }
        error_findings += result
            .ir
            .findings
            .iter()
            .filter(|finding| finding.severity == IrSeverity::Error)
            .count();
        corners.push(MultiCornerPostRoutePowerIrCorner {
            scenario: scenario.name.clone(),
            result: result.clone(),
        });
    }
    Ok(MultiCornerPostRoutePowerIrResult {
        corners,
        worst_power_corner,
        worst_drop_corner,
        error_findings,
    })
}

/// Analyze every MMMC timing view with scenario-specific supply and power
/// models while retaining the common post-route physical implementation.
pub fn analyze_multi_corner_physical_timing_power_ir(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir(
            &corner.evaluation,
            vectors,
            &scenario.options,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC timing view from VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_from_waveform(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_from_waveform(
            &corner.evaluation,
            waveform,
            &scenario.options,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC view with UPF domain-resolved post-route supplies.
pub fn analyze_multi_corner_physical_timing_power_ir_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if vectors.is_empty() {
        return Err("post-route power analysis requires at least one input vector".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_with_upf(
            &corner.evaluation,
            vectors,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

/// Analyze every MMMC view with UPF supplies and VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_from_waveform_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    if waveform.time_points().is_empty() {
        return Err("post-route VCD power analysis requires at least one timestamp".into());
    }
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_from_waveform_with_upf(
            &corner.evaluation,
            waveform,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_ir(scenarios, results)
}

fn finish_multi_corner_post_route_power_mode_ir(
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    results: Vec<UpfPowerModeIrResult>,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    if scenarios.len() != results.len() || results.is_empty() {
        return Err("MMMC UPF power-mode result/scenario count mismatch".into());
    }
    let mode_count = results[0].modes.len();
    if mode_count == 0
        || results
            .iter()
            .any(|result| result.modes.len() != mode_count)
    {
        return Err("MMMC UPF power modes are inconsistent between scenarios".into());
    }

    let mut corners = Vec::with_capacity(results.len());
    let mut worst_power_corner = 0;
    let mut worst_power_mode = 0;
    let mut worst_drop_corner = 0;
    let mut worst_drop_mode = 0;
    let mut worst_em_lifetime_corner = 0;
    let mut worst_em_lifetime_mode = 0;
    let mut worst_power = f64::NEG_INFINITY;
    let mut worst_drop = f64::NEG_INFINITY;
    let mut worst_em_lifetime: Option<f64> = None;
    let mut error_findings = 0;

    for (corner_index, (scenario, result)) in scenarios.iter().zip(results.into_iter()).enumerate()
    {
        for (mode_index, mode) in result.modes.iter().enumerate() {
            if mode.result.total_power_uw > worst_power {
                worst_power = mode.result.total_power_uw;
                worst_power_corner = corner_index;
                worst_power_mode = mode_index;
            }
            if mode.result.worst_drop_v > worst_drop {
                worst_drop = mode.result.worst_drop_v;
                worst_drop_corner = corner_index;
                worst_drop_mode = mode_index;
            }
            let update_worst_em = match (worst_em_lifetime, mode.worst_em_lifetime_hours) {
                (Some(current), Some(candidate)) if candidate < current => true,
                (None, Some(_)) => true,
                _ => false,
            };
            if update_worst_em {
                worst_em_lifetime = mode.worst_em_lifetime_hours;
                worst_em_lifetime_corner = corner_index;
                worst_em_lifetime_mode = mode_index;
            }
            error_findings += result.error_findings;
        }
        corners.push(MultiCornerPostRoutePowerModeIr {
            scenario: scenario.name.clone(),
            modes: result,
        });
    }
    Ok(MultiCornerPostRoutePowerModeIrResult {
        corners,
        worst_power_corner,
        worst_power_mode,
        worst_drop_corner,
        worst_drop_mode,
        worst_em_lifetime_corner,
        worst_em_lifetime_mode,
        error_findings,
    })
}

/// Analyze every MMMC timing view and every declared UPF power mode.
pub fn analyze_multi_corner_physical_timing_power_ir_power_modes_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    vectors: &[InputVector],
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(analyze_physical_timing_power_ir_power_modes_with_upf(
            &corner.evaluation,
            vectors,
            &scenario.options,
            power_intent,
        )?);
    }
    finish_multi_corner_post_route_power_mode_ir(scenarios, results)
}

/// Analyze MMMC × UPF power modes from VCD activity.
pub fn analyze_multi_corner_physical_timing_power_ir_power_modes_from_waveform_with_upf(
    iteration: &MultiCornerPhysicalTimingIteration,
    waveform: &Vcd,
    scenarios: &[MultiCornerPostRoutePowerIrScenario],
    power_intent: &Upf,
) -> Result<MultiCornerPostRoutePowerModeIrResult, String> {
    validate_multi_corner_post_route_power_scenarios(&iteration.corners, scenarios)?;
    let mut results = Vec::with_capacity(iteration.corners.len());
    for (corner, scenario) in iteration.corners.iter().zip(scenarios.iter()) {
        results.push(
            analyze_physical_timing_power_ir_power_modes_from_waveform_with_upf(
                &corner.evaluation,
                waveform,
                &scenario.options,
                power_intent,
            )?,
        );
    }
    finish_multi_corner_post_route_power_mode_ir(scenarios, results)
}

/// Render a post-route power/IR summary.
pub fn post_route_power_ir_report(result: &PostRoutePowerIrResult) -> String {
    let effective_cap_pf: f64 = result.effective_capacitance_pf.values().sum();
    let mut out = format!(
        "Post-route power/IR: parasitic dynamic {:.6} uW, effective capacitance {:.6} pF\n{}{}",
        result.parasitic_dynamic_power_uw,
        effective_cap_pf,
        power_report(&result.power),
        ir_report(&result.ir)
    );
    if let Some(upf_result) = &result.upf_ir {
        out.push_str(&upf_ir_report(upf_result));
    }
    if let Some(mesh_result) = &result.power_mesh {
        out.push_str(&power_mesh_report(mesh_result));
    }
    if let Some(mesh_result) = &result.multilayer_power_mesh {
        out.push_str(&multilayer_power_mesh_report(mesh_result));
    }
    out
}

/// Render all scenario-specific post-route power/IR results.
pub fn multi_corner_post_route_power_ir_report(
    result: &MultiCornerPostRoutePowerIrResult,
) -> String {
    let worst_power = &result.corners[result.worst_power_corner];
    let worst_drop = &result.corners[result.worst_drop_corner];
    let mut out = format!(
        "Multi-corner post-route power/IR: {} scenarios, worst power '{}', worst IR drop '{}', error findings {}\n",
        result.corners.len(),
        worst_power.scenario,
        worst_drop.scenario,
        result.error_findings
    );
    for corner in &result.corners {
        out.push_str(&format!("  scenario '{}'\n", corner.scenario));
        out.push_str(&post_route_power_ir_report(&corner.result));
    }
    out
}

/// Render the MMMC × UPF-power-mode result matrix.
pub fn multi_corner_post_route_power_mode_ir_report(
    result: &MultiCornerPostRoutePowerModeIrResult,
) -> String {
    let worst_power_corner = &result.corners[result.worst_power_corner];
    let worst_power = &worst_power_corner.modes.modes[result.worst_power_mode];
    let worst_drop_corner = &result.corners[result.worst_drop_corner];
    let worst_drop = &worst_drop_corner.modes.modes[result.worst_drop_mode];
    let worst_em_corner = &result.corners[result.worst_em_lifetime_corner];
    let worst_em_mode = &worst_em_corner.modes.modes[result.worst_em_lifetime_mode];
    let worst_em_label = worst_em_mode
        .worst_em_lifetime_hours
        .map_or("<unavailable>".to_string(), |_| {
            worst_em_mode.power_mode.clone()
        });
    let mut out = format!(
        "Multi-corner UPF power-mode IR report: {} scenario(s), worst power '{}' / '{}', worst drop '{}' / '{}', worst EM '{}' / '{}', error findings {}\n",
        result.corners.len(),
        worst_power_corner.scenario,
        worst_power.power_mode,
        worst_drop_corner.scenario,
        worst_drop.power_mode,
        worst_em_corner.scenario,
        worst_em_label,
        result.error_findings
    );
    for corner in &result.corners {
        out.push_str(&format!("  scenario '{}'\n", corner.scenario));
        out.push_str(&upf_power_mode_ir_report(&corner.modes));
    }
    out
}
