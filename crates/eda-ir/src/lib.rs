//! # eda-ir — static power-grid IR-drop and electromigration analysis.
//!
//! Cell currents are derived from a switching-power report and distributed on
//! their placed sites. VDD/GND rails are modeled as resistive segments fed
//! from both ends, giving a physically ordered static voltage-drop estimate
//! without requiring a full nodal linear solver.

use eda_core::netlist::Netlist;
use eda_pnr::{Placement, SITES_PER_ROW};
use eda_power::PowerReport;
use eda_upf::Upf;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct IrOptions {
    /// Nominal supply voltage in volts.
    pub voltage_v: f64,
    /// VDD rail resistance in ohms per placement site.
    pub vdd_ohm_per_site: f64,
    /// Ground rail resistance in ohms per placement site.
    pub gnd_ohm_per_site: f64,
    /// Maximum allowed VDD+GND static drop.
    pub max_total_drop_v: f64,
    /// Maximum current carried by either end of a rail.
    pub max_rail_current_a: f64,
}

/// Supply and rail parameters for one UPF power domain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DomainIrOptions {
    pub voltage_v: f64,
    pub vdd_ohm_per_site: f64,
    pub gnd_ohm_per_site: f64,
    pub max_total_drop_v: f64,
    pub max_rail_current_a: f64,
}

impl From<IrOptions> for DomainIrOptions {
    fn from(options: IrOptions) -> Self {
        Self {
            voltage_v: options.voltage_v,
            vdd_ohm_per_site: options.vdd_ohm_per_site,
            gnd_ohm_per_site: options.gnd_ohm_per_site,
            max_total_drop_v: options.max_total_drop_v,
            max_rail_current_a: options.max_rail_current_a,
        }
    }
}

/// UPF-aware IR options with a fallback for unmapped cells.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfIrOptions {
    pub default: Option<DomainIrOptions>,
    pub domains: BTreeMap<String, DomainIrOptions>,
}

impl Default for IrOptions {
    fn default() -> Self {
        IrOptions {
            voltage_v: 1.0,
            vdd_ohm_per_site: 0.01,
            gnd_ohm_per_site: 0.01,
            max_total_drop_v: 0.05,
            max_rail_current_a: 0.05,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrCell {
    pub cell: usize,
    pub name: String,
    pub row: usize,
    pub site_start: usize,
    pub site_end: usize,
    pub current_ma: f64,
    pub local_drop_v: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrRow {
    pub row: usize,
    pub total_current_a: f64,
    pub vdd_worst_drop_v: f64,
    pub gnd_worst_drop_v: f64,
    pub total_drop_v: f64,
    pub em_margin: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IrSeverity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrFinding {
    pub code: String,
    pub severity: IrSeverity,
    pub row: Option<usize>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrResult {
    pub voltage_v: f64,
    pub total_power_uw: f64,
    pub total_current_a: f64,
    pub worst_drop_v: f64,
    pub worst_row: Option<usize>,
    pub rows: Vec<IrRow>,
    pub cells: Vec<IrCell>,
    pub findings: Vec<IrFinding>,
}

impl IrResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == IrSeverity::Error)
    }
}

/// Options for a finite-difference VDD/GND power-mesh solver.
#[derive(Clone, Debug)]
pub struct PowerMeshOptions {
    /// Number of mesh rows.
    pub rows: usize,
    /// Number of mesh columns.
    pub columns: usize,
    pub voltage_v: f64,
    pub vdd_horizontal_ohm_per_segment: f64,
    pub vdd_vertical_ohm_per_segment: f64,
    pub gnd_horizontal_ohm_per_segment: f64,
    pub gnd_vertical_ohm_per_segment: f64,
    pub max_total_drop_v: f64,
    pub max_node_current_a: f64,
    /// Maximum absolute current through one mesh segment.
    pub max_segment_current_a: f64,
    /// Use all perimeter nodes as ideal supply taps.
    pub use_boundary_taps: bool,
    /// Additional explicit VDD/GND tap locations.
    pub taps: Vec<PowerMeshTap>,
    /// Maximum Gauss-Seidel iterations.
    pub max_iterations: usize,
    /// Node-voltage convergence threshold in volts.
    pub tolerance_v: f64,
}

impl Default for PowerMeshOptions {
    fn default() -> Self {
        Self {
            rows: 8,
            columns: 16,
            voltage_v: 1.0,
            vdd_horizontal_ohm_per_segment: 0.02,
            vdd_vertical_ohm_per_segment: 0.02,
            gnd_horizontal_ohm_per_segment: 0.02,
            gnd_vertical_ohm_per_segment: 0.02,
            max_total_drop_v: 0.05,
            max_node_current_a: 0.05,
            max_segment_current_a: 0.05,
            use_boundary_taps: true,
            taps: Vec::new(),
            max_iterations: 10_000,
            tolerance_v: 1e-9,
        }
    }
}

/// One explicit power-mesh supply tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerMeshTap {
    pub row: usize,
    pub column: usize,
    pub vdd: bool,
    pub gnd: bool,
}

impl PowerMeshTap {
    pub fn both(row: usize, column: usize) -> Self {
        Self {
            row,
            column,
            vdd: true,
            gnd: true,
        }
    }
}

/// Which resistive network a mesh segment belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMeshNetwork {
    Vdd,
    Gnd,
}

/// Direction of one resistive mesh segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMeshSegmentOrientation {
    Horizontal,
    Vertical,
    Via,
}

/// One solved power-mesh node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerMeshNode {
    pub row: usize,
    pub column: usize,
    pub vdd_v: f64,
    pub gnd_v: f64,
    pub vdd_drop_v: f64,
    pub gnd_drop_v: f64,
    pub total_drop_v: f64,
    pub injected_current_a: f64,
}

/// One cell's contribution to the power mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerMeshCell {
    pub cell: usize,
    pub name: String,
    pub mesh_row: usize,
    pub mesh_column: usize,
    pub current_a: f64,
    pub local_total_drop_v: f64,
}

/// Current through one horizontal or vertical mesh segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerMeshSegment {
    pub network: PowerMeshNetwork,
    pub orientation: PowerMeshSegmentOrientation,
    pub from_row: usize,
    pub from_column: usize,
    pub to_row: usize,
    pub to_column: usize,
    pub resistance_ohm: f64,
    pub voltage_drop_v: f64,
    pub current_a: f64,
}

/// Finite-difference power-mesh IR result.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerMeshResult {
    pub voltage_v: f64,
    pub total_power_uw: f64,
    pub total_current_a: f64,
    pub worst_drop_v: f64,
    pub worst_node: Option<(usize, usize)>,
    pub worst_segment_current_a: f64,
    pub nodes: Vec<PowerMeshNode>,
    pub cells: Vec<PowerMeshCell>,
    pub segments: Vec<PowerMeshSegment>,
    pub iterations: usize,
    pub final_delta_v: f64,
    pub findings: Vec<IrFinding>,
}

impl PowerMeshResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == IrSeverity::Error)
    }
}

/// One metal layer in a multilayer power mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerMeshLayer {
    pub name: String,
    pub vdd_horizontal_ohm_per_segment: f64,
    pub vdd_vertical_ohm_per_segment: f64,
    pub gnd_horizontal_ohm_per_segment: f64,
    pub gnd_vertical_ohm_per_segment: f64,
    /// Conductor cross-section used for horizontal current density.
    pub horizontal_cross_section_um2: f64,
    /// Conductor cross-section used for vertical current density.
    pub vertical_cross_section_um2: f64,
}

impl PowerMeshLayer {
    pub fn new(
        name: impl Into<String>,
        vdd_horizontal_ohm_per_segment: f64,
        vdd_vertical_ohm_per_segment: f64,
        gnd_horizontal_ohm_per_segment: f64,
        gnd_vertical_ohm_per_segment: f64,
    ) -> Self {
        Self {
            name: name.into(),
            vdd_horizontal_ohm_per_segment,
            vdd_vertical_ohm_per_segment,
            gnd_horizontal_ohm_per_segment,
            gnd_vertical_ohm_per_segment,
            horizontal_cross_section_um2: 1.0,
            vertical_cross_section_um2: 1.0,
        }
    }

    pub fn with_cross_sections(
        mut self,
        horizontal_cross_section_um2: f64,
        vertical_cross_section_um2: f64,
    ) -> Self {
        self.horizontal_cross_section_um2 = horizontal_cross_section_um2;
        self.vertical_cross_section_um2 = vertical_cross_section_um2;
        self
    }
}

/// A vertical inter-layer connection at one mesh location.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerMeshVia {
    pub row: usize,
    pub column: usize,
    pub from_layer: usize,
    pub to_layer: usize,
    pub vdd_resistance_ohm: f64,
    pub gnd_resistance_ohm: f64,
    /// Total via-stack cross-sectional area used for current density.
    pub area_um2: f64,
}

impl PowerMeshVia {
    pub fn new(
        row: usize,
        column: usize,
        from_layer: usize,
        to_layer: usize,
        vdd_resistance_ohm: f64,
        gnd_resistance_ohm: f64,
    ) -> Self {
        Self {
            row,
            column,
            from_layer,
            to_layer,
            vdd_resistance_ohm,
            gnd_resistance_ohm,
            area_um2: 1.0,
        }
    }

    pub fn with_area(mut self, area_um2: f64) -> Self {
        self.area_um2 = area_um2;
        self
    }
}

/// Black-equation-based relative EM lifetime model.
#[derive(Clone, Debug, PartialEq)]
pub struct PowerMeshEmOptions {
    pub enabled: bool,
    pub temperature_k: f64,
    pub reference_temperature_k: f64,
    pub reference_current_density_a_per_um2: f64,
    pub reference_lifetime_hours: f64,
    pub activation_energy_ev: f64,
    pub current_density_exponent: f64,
    pub min_lifetime_hours: f64,
}

impl Default for PowerMeshEmOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            temperature_k: 358.15,
            reference_temperature_k: 358.15,
            reference_current_density_a_per_um2: 0.001,
            reference_lifetime_hours: 1.0e8,
            activation_energy_ev: 0.7,
            current_density_exponent: 2.0,
            min_lifetime_hours: 1.0e7,
        }
    }
}

/// One explicit tap on a specific mesh layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerMeshLayerTap {
    pub layer: usize,
    pub row: usize,
    pub column: usize,
    pub vdd: bool,
    pub gnd: bool,
}

/// Options for a layer/via-aware finite-difference power mesh.
#[derive(Clone, Debug)]
pub struct MultilayerPowerMeshOptions {
    pub rows: usize,
    pub columns: usize,
    pub voltage_v: f64,
    pub layers: Vec<PowerMeshLayer>,
    pub vias: Vec<PowerMeshVia>,
    /// Layer index for each cell; an empty vector assigns every cell to layer 0.
    pub cell_layers: Vec<usize>,
    pub use_boundary_taps: bool,
    pub taps: Vec<PowerMeshLayerTap>,
    pub max_total_drop_v: f64,
    pub max_node_current_a: f64,
    pub max_segment_current_a: f64,
    pub em: PowerMeshEmOptions,
    pub max_iterations: usize,
    pub tolerance_v: f64,
    /// Optional coupled supply domains with distinct VDD tap voltages.
    pub power_domains: Vec<MultilayerPowerDomainSupply>,
    /// Power-domain name for each cell; empty means one global supply.
    pub cell_power_domains: Vec<String>,
}

/// One named supply domain in a coupled multilayer mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MultilayerPowerDomainSupply {
    pub name: String,
    pub voltage_v: f64,
    pub taps: Vec<PowerMeshLayerTap>,
}

impl MultilayerPowerDomainSupply {
    pub fn new(name: impl Into<String>, voltage_v: f64, taps: Vec<PowerMeshLayerTap>) -> Self {
        Self {
            name: name.into(),
            voltage_v,
            taps,
        }
    }
}

impl Default for MultilayerPowerMeshOptions {
    fn default() -> Self {
        Self {
            rows: 8,
            columns: 16,
            voltage_v: 1.0,
            layers: vec![
                PowerMeshLayer::new("M8", 0.02, 0.02, 0.02, 0.02),
                PowerMeshLayer::new("M7", 0.03, 0.03, 0.03, 0.03),
            ],
            vias: Vec::new(),
            cell_layers: Vec::new(),
            use_boundary_taps: true,
            taps: Vec::new(),
            max_total_drop_v: 0.05,
            max_node_current_a: 0.05,
            max_segment_current_a: 0.05,
            em: PowerMeshEmOptions::default(),
            max_iterations: 20_000,
            tolerance_v: 1e-9,
            power_domains: Vec::new(),
            cell_power_domains: Vec::new(),
        }
    }
}

/// One solved node on a named mesh layer.
#[derive(Clone, Debug, PartialEq)]
pub struct MultilayerPowerMeshNode {
    pub layer: usize,
    pub layer_name: String,
    pub row: usize,
    pub column: usize,
    pub vdd_v: f64,
    pub gnd_v: f64,
    pub vdd_drop_v: f64,
    pub gnd_drop_v: f64,
    pub total_drop_v: f64,
    pub injected_current_a: f64,
}

/// One cell's contribution to a multilayer mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MultilayerPowerMeshCell {
    pub cell: usize,
    pub name: String,
    pub layer: usize,
    pub mesh_row: usize,
    pub mesh_column: usize,
    pub current_a: f64,
    pub local_total_drop_v: f64,
    pub power_domain: Option<String>,
    pub supply_voltage_v: Option<f64>,
}

/// One planar or via segment in a multilayer mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MultilayerPowerMeshSegment {
    pub network: PowerMeshNetwork,
    pub orientation: PowerMeshSegmentOrientation,
    pub from_layer: usize,
    pub from_row: usize,
    pub from_column: usize,
    pub to_layer: usize,
    pub to_row: usize,
    pub to_column: usize,
    pub resistance_ohm: f64,
    pub voltage_drop_v: f64,
    pub current_a: f64,
    pub current_density_a_per_um2: f64,
    pub em_lifetime_hours: f64,
}

/// Result of a layer/via-aware power-mesh solve.
#[derive(Clone, Debug, PartialEq)]
pub struct MultilayerPowerMeshResult {
    pub voltage_v: f64,
    pub total_power_uw: f64,
    pub total_current_a: f64,
    pub worst_drop_v: f64,
    pub worst_node: Option<(usize, usize, usize)>,
    pub worst_segment_current_a: f64,
    pub worst_em_lifetime_hours: f64,
    pub nodes: Vec<MultilayerPowerMeshNode>,
    pub cells: Vec<MultilayerPowerMeshCell>,
    pub segments: Vec<MultilayerPowerMeshSegment>,
    pub iterations: usize,
    pub final_delta_v: f64,
    pub findings: Vec<IrFinding>,
    /// Per-domain current and local supply-margin summaries.
    pub power_domains: Vec<MultilayerPowerDomainResult>,
}

/// One supply domain's current and local drop in a coupled mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultilayerPowerDomainResult {
    pub name: String,
    pub voltage_v: f64,
    pub cells: usize,
    pub total_current_a: f64,
    pub worst_local_drop_v: f64,
}

impl MultilayerPowerMeshResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == IrSeverity::Error)
    }
}

/// One cell assigned to a UPF supply domain.
#[derive(Clone, Debug, PartialEq)]
pub struct UpfIrCell {
    pub cell: usize,
    pub name: String,
    pub power_domain: String,
    pub row: usize,
    pub site_start: usize,
    pub site_end: usize,
    pub voltage_v: f64,
    pub powered: bool,
    pub current_ma: f64,
    pub local_drop_v: f64,
}

/// One physical row segment owned by a power domain.
#[derive(Clone, Debug, PartialEq)]
pub struct UpfIrDomainRow {
    pub power_domain: String,
    pub row: usize,
    pub total_current_a: f64,
    pub vdd_worst_drop_v: f64,
    pub gnd_worst_drop_v: f64,
    pub total_drop_v: f64,
    pub em_margin_a: f64,
}

/// Aggregate IR/EM result for one power domain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfIrDomain {
    pub name: String,
    pub powered: bool,
    pub voltage_v: f64,
    pub cell_count: usize,
    pub total_power_uw: f64,
    pub total_current_a: f64,
    pub worst_drop_v: f64,
    pub switch_drop_v: f64,
    pub switch_leakage_current_a: f64,
    pub switch_leakage_power_uw: f64,
    pub strategy_current_a: f64,
    pub strategy_power_uw: f64,
    pub worst_row: Option<usize>,
    pub worst_rail_current_a: f64,
}

/// Current and voltage drop through one UPF power switch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfIrSwitch {
    pub name: String,
    pub power_domain: String,
    pub enabled: bool,
    pub modeled: bool,
    pub resistance_ohm: f64,
    pub current_a: f64,
    pub off_leakage_a: f64,
    pub drop_v: f64,
}

/// Static current consumed by an isolation, retention, or level-shifter strategy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfIrStrategy {
    pub kind: &'static str,
    pub name: String,
    pub power_domain: String,
    pub current_a: f64,
    pub power_uw: f64,
}

/// UPF-aware static IR/EM result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfIrResult {
    pub power_mode: Option<String>,
    pub total_power_uw: f64,
    pub total_current_a: f64,
    pub switch_leakage_current_a: f64,
    pub switch_leakage_power_uw: f64,
    pub strategy_current_a: f64,
    pub strategy_power_uw: f64,
    pub worst_drop_v: f64,
    pub worst_power_domain: Option<String>,
    pub worst_row: Option<usize>,
    pub rows: Vec<IrRow>,
    pub domain_rows: Vec<UpfIrDomainRow>,
    pub switches: Vec<UpfIrSwitch>,
    pub strategies: Vec<UpfIrStrategy>,
    pub cells: Vec<UpfIrCell>,
    pub domains: Vec<UpfIrDomain>,
    pub findings: Vec<IrFinding>,
}

/// One named power-mode IR result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfPowerModeIr {
    pub power_mode: String,
    pub result: UpfIrResult,
    /// Optional mode-aware finite-difference mesh result.
    pub power_mesh: Option<PowerMeshResult>,
    /// Optional mode-aware multilayer finite-difference mesh result.
    pub multilayer_power_mesh: Option<MultilayerPowerMeshResult>,
    /// True when the multilayer result is one coupled multi-supply solve.
    pub coupled_multilayer_mesh: bool,
    /// Minimum finite-or-infinite multilayer EM lifetime for this mode.
    pub worst_em_lifetime_hours: Option<f64>,
    /// Domain-decomposed multilayer meshes for domains with physical cells.
    pub domain_multilayer_meshes: Vec<UpfDomainMultilayerMeshResult>,
}

/// One UPF power domain's independent multilayer mesh view.
///
/// Domains are solved separately because each can have its own supply voltage,
/// switch state, and activity. The results are not a coupled multi-supply
/// nodal solve; they preserve domain-specific voltage and current views.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfDomainMultilayerMeshResult {
    pub power_domain: String,
    pub powered: bool,
    pub voltage_v: f64,
    pub total_current_a: f64,
    pub worst_drop_v: f64,
    pub worst_em_lifetime_hours: Option<f64>,
    pub mesh: Option<MultilayerPowerMeshResult>,
}

/// Aggregated IR/EM results for all declared UPF power modes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfPowerModeIrResult {
    pub modes: Vec<UpfPowerModeIr>,
    pub worst_power_mode: usize,
    pub worst_drop_mode: usize,
    /// Mode with the lowest available multilayer EM lifetime.
    pub worst_em_lifetime_mode: usize,
    pub error_findings: usize,
}

impl UpfIrResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == IrSeverity::Error)
    }
}

pub fn analyze(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &IrOptions,
) -> Result<IrResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("IR analysis placement/netlist cell count mismatch".into());
    }
    if power.cells.len() != nl.cell_count() {
        return Err("IR analysis power/netlist cell count mismatch".into());
    }
    validate_f64("voltage", options.voltage_v, true)?;
    validate_f64("VDD resistance", options.vdd_ohm_per_site, true)?;
    validate_f64("GND resistance", options.gnd_ohm_per_site, true)?;
    validate_f64("maximum drop", options.max_total_drop_v, true)?;
    validate_f64("maximum rail current", options.max_rail_current_a, true)?;

    // Leakage is reported only as a global estimate. Distribute it by cell
    // width, which is also how the power estimator modeled leakage.
    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    for cp in &power.cells {
        if cp.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                cp.cell
            ));
        }
        let leakage_share =
            power.static_power_uw * placement.cell_width[cp.cell] as f64 / total_sites as f64;
        cell_power[cp.cell] = cp.dynamic_power_uw + leakage_share;
    }

    // P[µW] = V[V] * I[µA]; convert µA to A with 1e6.
    let total_power_uw: f64 = cell_power.iter().sum();
    let total_current_a = total_power_uw / options.voltage_v / 1_000_000.0;
    let mut rows = Vec::new();
    let mut cells = Vec::with_capacity(nl.cell_count());
    let mut worst_drop = 0.0_f64;
    let mut worst_row = None;

    for row in 0..placement.rows.max(1) {
        let mut row_current = 0.0_f64;
        let mut vdd_drop = 0.0_f64;
        let mut gnd_drop = 0.0_f64;
        let mut row_cells = Vec::new();
        for (ci, cell_power_uw) in cell_power.iter().enumerate() {
            if placement.cell_pos[ci].row != row {
                continue;
            }
            let start = placement.cell_pos[ci].site;
            let width = placement.cell_width[ci].max(1);
            let end = (start + width).min(SITES_PER_ROW);
            let current_a = cell_power_uw / options.voltage_v / 1_000_000.0;
            row_current += current_a;

            // A current at x on a length-L rail splits between the two feeds:
            // x/L flows from the left and (L-x)/L from the right. The local
            // drop contribution is therefore I*R*x*(L-x)/L. This is exact for
            // a one-dimensional rail with both ends tied to ideal supplies.
            let center = (start + end) as f64 / 2.0;
            let length = SITES_PER_ROW as f64;
            let position = center.clamp(0.0, length);
            let factor = position * (length - position) / length;
            let local_vdd = current_a * options.vdd_ohm_per_site * factor;
            let local_gnd = current_a * options.gnd_ohm_per_site * factor;
            vdd_drop += local_vdd;
            gnd_drop += local_gnd;
            row_cells.push(IrCell {
                cell: ci,
                name: nl.cell(ci).name.clone(),
                row,
                site_start: start,
                site_end: end,
                current_ma: current_a * 1000.0,
                local_drop_v: local_vdd + local_gnd,
            });
        }

        let total_drop = vdd_drop + gnd_drop;
        // Each end carries at most approximately half of the rail current for
        // uniformly distributed load. Use the full current as a conservative
        // EM bound, so thin/degraded rails are not missed.
        let em_current = row_current;
        let em_margin = if options.max_rail_current_a == 0.0 {
            0.0
        } else {
            options.max_rail_current_a - em_current
        };
        if total_drop > worst_drop {
            worst_drop = total_drop;
            worst_row = Some(row);
        }
        row_cells.sort_by_key(|cell| cell.cell);
        cells.extend(row_cells);
        rows.push(IrRow {
            row,
            total_current_a: row_current,
            vdd_worst_drop_v: vdd_drop,
            gnd_worst_drop_v: gnd_drop,
            total_drop_v: total_drop,
            em_margin,
        });
    }

    let mut findings = Vec::new();
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_row,
            message: format!(
                "worst IR drop {:.6} V exceeds {:.6} V",
                worst_drop, options.max_total_drop_v
            ),
        });
    }
    for row in &rows {
        if row.total_current_a > options.max_rail_current_a {
            findings.push(IrFinding {
                code: "EM-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(row.row),
                message: format!(
                    "row {} current {:.6} A exceeds EM limit {:.6} A",
                    row.row, row.total_current_a, options.max_rail_current_a
                ),
            });
        } else if row.em_margin < options.max_rail_current_a * 0.1 {
            findings.push(IrFinding {
                code: "EM-MARGIN".into(),
                severity: IrSeverity::Warning,
                row: Some(row.row),
                message: format!(
                    "row {} has less than 10% EM margin ({:.6} A remaining)",
                    row.row, row.em_margin
                ),
            });
        }
    }

    Ok(IrResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_row,
        rows,
        cells,
        findings,
    })
}

fn validate_power_mesh_options(options: &PowerMeshOptions) -> Result<(), String> {
    if options.rows == 0 || options.columns == 0 {
        return Err("power mesh requires positive row and column counts".into());
    }
    let fields = [
        ("voltage", options.voltage_v),
        (
            "VDD horizontal resistance",
            options.vdd_horizontal_ohm_per_segment,
        ),
        (
            "VDD vertical resistance",
            options.vdd_vertical_ohm_per_segment,
        ),
        (
            "GND horizontal resistance",
            options.gnd_horizontal_ohm_per_segment,
        ),
        (
            "GND vertical resistance",
            options.gnd_vertical_ohm_per_segment,
        ),
        ("maximum drop", options.max_total_drop_v),
        ("maximum node current", options.max_node_current_a),
        ("maximum segment current", options.max_segment_current_a),
        ("tolerance", options.tolerance_v),
    ];
    for (name, value) in fields {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid power mesh {} {}", name, value));
        }
    }
    if options.max_iterations == 0 {
        return Err("power mesh requires a positive iteration limit".into());
    }
    for (index, tap) in options.taps.iter().enumerate() {
        if tap.row >= options.rows || tap.column >= options.columns {
            return Err(format!(
                "power mesh tap {} location ({},{}) is outside {}x{} mesh",
                index, tap.row, tap.column, options.rows, options.columns
            ));
        }
        if !tap.vdd && !tap.gnd {
            return Err(format!("power mesh tap {} must enable VDD or GND", index));
        }
    }
    if !options.use_boundary_taps {
        if !options.taps.iter().any(|tap| tap.vdd) {
            return Err("power mesh requires at least one VDD tap".into());
        }
        if !options.taps.iter().any(|tap| tap.gnd) {
            return Err("power mesh requires at least one GND tap".into());
        }
    }
    Ok(())
}

type PowerMeshSolve = (Vec<Vec<f64>>, usize, f64);

#[allow(clippy::too_many_arguments)]
fn solve_power_mesh_network(
    rows: usize,
    columns: usize,
    injected_current: &[Vec<f64>],
    fixed_voltage_v: f64,
    horizontal_ohm: f64,
    vertical_ohm: f64,
    current_sign: f64,
    taps: &[Vec<bool>],
    max_iterations: usize,
    tolerance_v: f64,
) -> Result<PowerMeshSolve, String> {
    let mut voltage = vec![vec![fixed_voltage_v; columns]; rows];
    let conductance_horizontal = 1.0 / horizontal_ohm;
    let conductance_vertical = 1.0 / vertical_ohm;
    let mut iterations = 0;
    let mut final_delta = f64::INFINITY;

    while iterations < max_iterations {
        let mut max_delta = 0.0_f64;
        for row in 0..rows {
            for column in 0..columns {
                if taps[row][column] {
                    voltage[row][column] = fixed_voltage_v;
                    continue;
                }

                let mut conductance_sum = 0.0;
                let mut neighbor_voltage_sum = 0.0;
                if row > 0 {
                    conductance_sum += conductance_vertical;
                    neighbor_voltage_sum += conductance_vertical * voltage[row - 1][column];
                }
                if row + 1 < rows {
                    conductance_sum += conductance_vertical;
                    neighbor_voltage_sum += conductance_vertical * voltage[row + 1][column];
                }
                if column > 0 {
                    conductance_sum += conductance_horizontal;
                    neighbor_voltage_sum += conductance_horizontal * voltage[row][column - 1];
                }
                if column + 1 < columns {
                    conductance_sum += conductance_horizontal;
                    neighbor_voltage_sum += conductance_horizontal * voltage[row][column + 1];
                }
                let updated = (neighbor_voltage_sum + current_sign * injected_current[row][column])
                    / conductance_sum;
                let delta = (updated - voltage[row][column]).abs();
                max_delta = max_delta.max(delta);
                voltage[row][column] = updated;
            }
        }
        iterations += 1;
        final_delta = max_delta;
        if max_delta <= tolerance_v {
            return Ok((voltage, iterations, final_delta));
        }
    }
    Err(format!(
        "power mesh solver did not converge: final delta {:.6e} V after {} iterations",
        final_delta, iterations
    ))
}

fn power_mesh_tap_mask(options: &PowerMeshOptions, select_vdd: bool) -> Vec<Vec<bool>> {
    let mut taps = vec![vec![false; options.columns]; options.rows];
    if options.use_boundary_taps {
        for (row, tap_row) in taps.iter_mut().enumerate() {
            for (column, tap) in tap_row.iter_mut().enumerate() {
                if row == 0
                    || row + 1 == options.rows
                    || column == 0
                    || column + 1 == options.columns
                {
                    *tap = true;
                }
            }
        }
    }
    for tap in &options.taps {
        if select_vdd {
            taps[tap.row][tap.column] |= tap.vdd;
        } else {
            taps[tap.row][tap.column] |= tap.gnd;
        }
    }
    taps
}

/// Solve a two-network VDD/GND power mesh with configurable supply taps.
///
/// Cell currents are placed on the nearest mesh node. The VDD and GND networks
/// are solved independently using a finite-difference Gauss-Seidel nodal
/// equation. Total local drop is the VDD voltage loss plus local ground bounce.
pub fn analyze_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("power mesh placement/netlist cell count mismatch".into());
    }
    if power.cells.len() != nl.cell_count() {
        return Err("power mesh power/netlist cell count mismatch".into());
    }
    validate_power_mesh_options(options)?;

    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    for report_cell in &power.cells {
        if report_cell.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                report_cell.cell
            ));
        }
        let leakage_share = power.static_power_uw * placement.cell_width[report_cell.cell] as f64
            / total_sites as f64;
        cell_power[report_cell.cell] = report_cell.dynamic_power_uw + leakage_share;
    }
    let cell_current = cell_power
        .iter()
        .map(|power_uw| power_uw / options.voltage_v / 1_000_000.0)
        .collect::<Vec<_>>();
    let total_power_uw: f64 = cell_power.iter().sum();
    analyze_mesh_currents(nl, placement, &cell_current, total_power_uw, options)
}

/// Solve a mesh from explicit per-cell currents.
///
/// This entry point lets UPF-aware callers account for domain-specific supply
/// voltages, switch state, and mode-dependent activity before injection.
#[allow(clippy::needless_range_loop)]
pub fn analyze_mesh_currents(
    nl: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &PowerMeshOptions,
) -> Result<PowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("power mesh placement/netlist cell count mismatch".into());
    }
    if cell_current_a.len() != nl.cell_count() {
        return Err("power mesh current/netlist cell count mismatch".into());
    }
    validate_power_mesh_options(options)?;

    let mut injected_current = vec![vec![0.0_f64; options.columns]; options.rows];
    let mut cells = Vec::with_capacity(nl.cell_count());
    for (cell_index, current_a) in cell_current_a.iter().enumerate() {
        let position = &placement.cell_pos[cell_index];
        if position.row >= placement.rows {
            return Err(format!(
                "cell '{}' row {} is outside placement rows {}",
                nl.cell(cell_index).name,
                position.row,
                placement.rows
            ));
        }
        let physical_row = position.row as f64 + 0.5;
        let mesh_row = ((physical_row * options.rows as f64) / placement.rows.max(1) as f64)
            .floor()
            .clamp(0.0, (options.rows - 1) as f64) as usize;
        let center_site =
            position.site as f64 + placement.cell_width[cell_index].max(1) as f64 / 2.0;
        let normalized_column = (center_site / SITES_PER_ROW as f64).clamp(0.0, 0.999999);
        let mesh_column = (normalized_column * options.columns as f64)
            .floor()
            .clamp(0.0, (options.columns - 1) as f64) as usize;

        let current_a = *current_a;
        if !current_a.is_finite() || current_a < 0.0 {
            return Err(format!(
                "cell '{}' has invalid power-mesh current {}",
                nl.cell(cell_index).name,
                current_a
            ));
        }
        injected_current[mesh_row][mesh_column] += current_a;
        cells.push(PowerMeshCell {
            cell: cell_index,
            name: nl.cell(cell_index).name.clone(),
            mesh_row,
            mesh_column,
            current_a,
            local_total_drop_v: 0.0,
        });
    }

    let vdd_taps = power_mesh_tap_mask(options, true);
    let gnd_taps = power_mesh_tap_mask(options, false);
    let (vdd, vdd_iterations, vdd_delta) = solve_power_mesh_network(
        options.rows,
        options.columns,
        &injected_current,
        options.voltage_v,
        options.vdd_horizontal_ohm_per_segment,
        options.vdd_vertical_ohm_per_segment,
        -1.0,
        &vdd_taps,
        options.max_iterations,
        options.tolerance_v,
    )?;
    let (gnd, gnd_iterations, gnd_delta) = solve_power_mesh_network(
        options.rows,
        options.columns,
        &injected_current,
        0.0,
        options.gnd_horizontal_ohm_per_segment,
        options.gnd_vertical_ohm_per_segment,
        1.0,
        &gnd_taps,
        options.max_iterations,
        options.tolerance_v,
    )?;

    let mut nodes = Vec::with_capacity(options.rows * options.columns);
    let mut worst_drop = 0.0_f64;
    let mut worst_node = None;
    for row in 0..options.rows {
        for column in 0..options.columns {
            let vdd_drop = (options.voltage_v - vdd[row][column]).max(0.0);
            let gnd_drop = gnd[row][column].max(0.0);
            let total_drop = vdd_drop + gnd_drop;
            if total_drop > worst_drop {
                worst_drop = total_drop;
                worst_node = Some((row, column));
            }
            nodes.push(PowerMeshNode {
                row,
                column,
                vdd_v: vdd[row][column],
                gnd_v: gnd[row][column],
                vdd_drop_v: vdd_drop,
                gnd_drop_v: gnd_drop,
                total_drop_v: total_drop,
                injected_current_a: injected_current[row][column],
            });
        }
    }
    for cell in &mut cells {
        let node = &nodes[cell.mesh_row * options.columns + cell.mesh_column];
        cell.local_total_drop_v = node.total_drop_v;
    }

    let mut segments = Vec::new();
    for (network, voltage, horizontal_ohm, vertical_ohm) in [
        (
            PowerMeshNetwork::Vdd,
            &vdd,
            options.vdd_horizontal_ohm_per_segment,
            options.vdd_vertical_ohm_per_segment,
        ),
        (
            PowerMeshNetwork::Gnd,
            &gnd,
            options.gnd_horizontal_ohm_per_segment,
            options.gnd_vertical_ohm_per_segment,
        ),
    ] {
        for row in 0..options.rows {
            for column in 0..options.columns.saturating_sub(1) {
                let voltage_drop = voltage[row][column] - voltage[row][column + 1];
                segments.push(PowerMeshSegment {
                    network,
                    orientation: PowerMeshSegmentOrientation::Horizontal,
                    from_row: row,
                    from_column: column,
                    to_row: row,
                    to_column: column + 1,
                    resistance_ohm: horizontal_ohm,
                    voltage_drop_v: voltage_drop,
                    current_a: (voltage_drop / horizontal_ohm).abs(),
                });
            }
        }
        for row in 0..options.rows.saturating_sub(1) {
            for column in 0..options.columns {
                let voltage_drop = voltage[row][column] - voltage[row + 1][column];
                segments.push(PowerMeshSegment {
                    network,
                    orientation: PowerMeshSegmentOrientation::Vertical,
                    from_row: row,
                    from_column: column,
                    to_row: row + 1,
                    to_column: column,
                    resistance_ohm: vertical_ohm,
                    voltage_drop_v: voltage_drop,
                    current_a: (voltage_drop / vertical_ohm).abs(),
                });
            }
        }
    }

    let total_current_a: f64 = cell_current_a.iter().sum();
    let mut findings = Vec::new();
    let worst_segment_current_a = segments
        .iter()
        .map(|segment| segment.current_a)
        .fold(0.0, f64::max);
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "MESH-IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_node.map(|(row, _)| row),
            message: format!(
                "worst power-mesh drop {:.6} V at {:?} exceeds {:.6} V",
                worst_drop, worst_node, options.max_total_drop_v
            ),
        });
    }
    for node in &nodes {
        if node.injected_current_a > options.max_node_current_a {
            findings.push(IrFinding {
                code: "MESH-NODE-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(node.row),
                message: format!(
                    "mesh node ({},{}) current {:.6} A exceeds {:.6} A",
                    node.row, node.column, node.injected_current_a, options.max_node_current_a
                ),
            });
        }
    }
    for segment in &segments {
        if segment.current_a > options.max_segment_current_a {
            findings.push(IrFinding {
                code: "MESH-SEGMENT-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} mesh segment {:?} ({},{})-({},{}) current {:.6} A exceeds {:.6} A",
                    segment.network,
                    segment.orientation,
                    segment.from_row,
                    segment.from_column,
                    segment.to_row,
                    segment.to_column,
                    segment.current_a,
                    options.max_segment_current_a
                ),
            });
        }
    }

    Ok(PowerMeshResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_node,
        worst_segment_current_a,
        nodes,
        cells,
        segments,
        iterations: vdd_iterations.max(gnd_iterations),
        final_delta_v: vdd_delta.max(gnd_delta),
        findings,
    })
}

fn validate_multilayer_power_mesh_options(
    options: &MultilayerPowerMeshOptions,
) -> Result<(), String> {
    if options.rows == 0 || options.columns == 0 {
        return Err("multilayer power mesh requires positive row and column counts".into());
    }
    if options.layers.is_empty() {
        return Err("multilayer power mesh requires at least one layer".into());
    }
    let mut layer_names = BTreeSet::new();
    for (index, layer) in options.layers.iter().enumerate() {
        if layer.name.trim().is_empty() {
            return Err(format!("power mesh layer {} name cannot be empty", index));
        }
        if !layer_names.insert(layer.name.as_str()) {
            return Err(format!("duplicate power mesh layer '{}'", layer.name));
        }
        for (name, value) in [
            (
                "VDD horizontal resistance",
                layer.vdd_horizontal_ohm_per_segment,
            ),
            (
                "VDD vertical resistance",
                layer.vdd_vertical_ohm_per_segment,
            ),
            (
                "GND horizontal resistance",
                layer.gnd_horizontal_ohm_per_segment,
            ),
            (
                "GND vertical resistance",
                layer.gnd_vertical_ohm_per_segment,
            ),
            (
                "horizontal cross-section",
                layer.horizontal_cross_section_um2,
            ),
            ("vertical cross-section", layer.vertical_cross_section_um2),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "power mesh layer '{}' has invalid {} {}",
                    layer.name, name, value
                ));
            }
        }
    }
    for (index, via) in options.vias.iter().enumerate() {
        if via.row >= options.rows
            || via.column >= options.columns
            || via.from_layer >= options.layers.len()
            || via.to_layer >= options.layers.len()
        {
            return Err(format!(
                "power mesh via {} location is outside the mesh",
                index
            ));
        }
        if via.from_layer == via.to_layer {
            return Err(format!(
                "power mesh via {} must connect distinct layers",
                index
            ));
        }
        for (name, value) in [
            ("VDD resistance", via.vdd_resistance_ohm),
            ("GND resistance", via.gnd_resistance_ohm),
            ("area", via.area_um2),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!(
                    "power mesh via {} has invalid {} {}",
                    index, name, value
                ));
            }
        }
    }
    for (index, layer) in options.cell_layers.iter().enumerate() {
        if *layer >= options.layers.len() {
            return Err(format!(
                "power mesh cell layer mapping {} references undefined layer {}",
                index, layer
            ));
        }
    }
    for (index, tap) in options.taps.iter().enumerate() {
        if tap.layer >= options.layers.len()
            || tap.row >= options.rows
            || tap.column >= options.columns
        {
            return Err(format!(
                "power mesh tap {} location is outside the mesh",
                index
            ));
        }
        if !tap.vdd && !tap.gnd {
            return Err(format!("power mesh tap {} must enable VDD or GND", index));
        }
    }
    let mut domain_names = BTreeSet::new();
    let mut has_domain_vdd = false;
    let mut has_domain_gnd = false;
    for (index, domain) in options.power_domains.iter().enumerate() {
        if domain.name.trim().is_empty() {
            return Err(format!("power mesh domain {} name cannot be empty", index));
        }
        if !domain_names.insert(domain.name.as_str()) {
            return Err(format!("duplicate power mesh domain '{}'", domain.name));
        }
        if !domain.voltage_v.is_finite() || domain.voltage_v <= 0.0 {
            return Err(format!(
                "power mesh domain '{}' has invalid voltage {}",
                domain.name, domain.voltage_v
            ));
        }
        if domain.taps.is_empty() {
            return Err(format!(
                "power mesh domain '{}' requires at least one tap",
                domain.name
            ));
        }
        if !domain.taps.iter().any(|tap| tap.vdd) {
            return Err(format!(
                "power mesh domain '{}' requires at least one VDD tap",
                domain.name
            ));
        }
        for (tap_index, tap) in domain.taps.iter().enumerate() {
            if tap.layer >= options.layers.len()
                || tap.row >= options.rows
                || tap.column >= options.columns
            {
                return Err(format!(
                    "power mesh domain '{}' tap {} location is outside the mesh",
                    domain.name, tap_index
                ));
            }
            if !tap.vdd && !tap.gnd {
                return Err(format!(
                    "power mesh domain '{}' tap {} must enable VDD or GND",
                    domain.name, tap_index
                ));
            }
            has_domain_vdd |= tap.vdd;
            has_domain_gnd |= tap.gnd;
        }
    }
    for (index, domain) in options.cell_power_domains.iter().enumerate() {
        if !domain_names.contains(domain.as_str()) {
            return Err(format!(
                "power mesh cell domain mapping {} references undefined domain '{}'",
                index, domain
            ));
        }
    }
    for (name, value) in [
        ("voltage", options.voltage_v),
        ("maximum drop", options.max_total_drop_v),
        ("maximum node current", options.max_node_current_a),
        ("maximum segment current", options.max_segment_current_a),
        ("tolerance", options.tolerance_v),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid multilayer power mesh {} {}", name, value));
        }
    }
    if options.max_iterations == 0 {
        return Err("multilayer power mesh requires a positive iteration limit".into());
    }
    validate_power_mesh_em_options(&options.em)?;
    if !options.use_boundary_taps {
        if !options.taps.iter().any(|tap| tap.vdd) && !has_domain_vdd {
            return Err("multilayer power mesh requires at least one VDD tap".into());
        }
        if !options.taps.iter().any(|tap| tap.gnd) && !has_domain_gnd {
            return Err("multilayer power mesh requires at least one GND tap".into());
        }
    }
    Ok(())
}

fn validate_power_mesh_em_options(options: &PowerMeshEmOptions) -> Result<(), String> {
    if !options.enabled {
        return Ok(());
    }
    for (name, value) in [
        ("temperature", options.temperature_k),
        ("reference temperature", options.reference_temperature_k),
        (
            "reference current density",
            options.reference_current_density_a_per_um2,
        ),
        ("reference lifetime", options.reference_lifetime_hours),
        ("activation energy", options.activation_energy_ev),
        ("minimum lifetime", options.min_lifetime_hours),
    ] {
        if !value.is_finite() || value <= 0.0 {
            return Err(format!("invalid power mesh EM {} {}", name, value));
        }
    }
    if !options.current_density_exponent.is_finite() || options.current_density_exponent <= 0.0 {
        return Err(format!(
            "invalid power mesh EM current density exponent {}",
            options.current_density_exponent
        ));
    }
    Ok(())
}

/// Estimate relative EM lifetime using Black's equation.
///
/// The result is normalized to a user-supplied lifetime at a reference current
/// density and temperature. It is suitable for relative what-if analysis, not
/// as a foundry-qualified absolute lifetime signoff value.
pub fn estimate_power_mesh_em_lifetime(
    current_density_a_per_um2: f64,
    options: &PowerMeshEmOptions,
) -> Result<f64, String> {
    validate_power_mesh_em_options(options)?;
    if !current_density_a_per_um2.is_finite() || current_density_a_per_um2 < 0.0 {
        return Err(format!(
            "invalid EM current density {} A/um^2",
            current_density_a_per_um2
        ));
    }
    if current_density_a_per_um2 == 0.0 {
        return Ok(f64::INFINITY);
    }
    const BOLTZMANN_EV_PER_K: f64 = 8.617_333_262_145e-5;
    let density_ratio = options.reference_current_density_a_per_um2 / current_density_a_per_um2;
    let temperature_ratio = ((options.activation_energy_ev / BOLTZMANN_EV_PER_K)
        * (1.0 / options.temperature_k - 1.0 / options.reference_temperature_k))
        .exp();
    Ok(options.reference_lifetime_hours
        * density_ratio.powf(options.current_density_exponent)
        * temperature_ratio)
}

fn power_mesh_segment_em_values(
    current_a: f64,
    area_um2: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<(f64, f64), String> {
    let current_density = current_a / area_um2;
    let lifetime = if options.em.enabled {
        estimate_power_mesh_em_lifetime(current_density, &options.em)?
    } else {
        f64::INFINITY
    };
    Ok((current_density, lifetime))
}

#[allow(clippy::needless_range_loop)]
fn multilayer_tap_mask(
    options: &MultilayerPowerMeshOptions,
    select_vdd: bool,
) -> Vec<Vec<Vec<bool>>> {
    let mut taps = vec![vec![vec![false; options.columns]; options.rows]; options.layers.len()];
    if options.use_boundary_taps {
        for layer in 0..options.layers.len() {
            for row in 0..options.rows {
                for column in 0..options.columns {
                    if row == 0
                        || row + 1 == options.rows
                        || column == 0
                        || column + 1 == options.columns
                    {
                        taps[layer][row][column] = true;
                    }
                }
            }
        }
    }
    for tap in &options.taps {
        if select_vdd {
            taps[tap.layer][tap.row][tap.column] |= tap.vdd;
        } else {
            taps[tap.layer][tap.row][tap.column] |= tap.gnd;
        }
    }
    taps
}

fn multilayer_fixed_voltages(
    options: &MultilayerPowerMeshOptions,
    network: PowerMeshNetwork,
) -> Result<Vec<Vec<Vec<Option<f64>>>>, String> {
    let taps = multilayer_tap_mask(options, network == PowerMeshNetwork::Vdd);
    let global_voltage = if network == PowerMeshNetwork::Vdd {
        options.voltage_v
    } else {
        0.0
    };
    let mut fixed =
        vec![vec![vec![None::<f64>; options.columns]; options.rows]; options.layers.len()];
    for layer in 0..options.layers.len() {
        for row in 0..options.rows {
            for column in 0..options.columns {
                if taps[layer][row][column] {
                    fixed[layer][row][column] = Some(global_voltage);
                }
            }
        }
    }
    for domain in &options.power_domains {
        for tap in &domain.taps {
            let enabled = if network == PowerMeshNetwork::Vdd {
                tap.vdd
            } else {
                tap.gnd
            };
            if !enabled {
                continue;
            }
            let voltage = if network == PowerMeshNetwork::Vdd {
                domain.voltage_v
            } else {
                0.0
            };
            let existing = &mut fixed[tap.layer][tap.row][tap.column];
            match *existing {
                Some(current) if (current - voltage).abs() > 1e-12 => {
                    return Err(format!(
                        "multilayer power mesh location ({},{},{}) has conflicting {} fixed voltages {} and {}",
                        tap.layer,
                        tap.row,
                        tap.column,
                        if network == PowerMeshNetwork::Vdd { "VDD" } else { "GND" },
                        current,
                        voltage
                    ));
                }
                _ => *existing = Some(voltage),
            }
        }
    }
    Ok(fixed)
}

type MultilayerVoltage = Vec<Vec<Vec<f64>>>;
type MultilayerMeshSolve = (MultilayerVoltage, usize, f64);

#[allow(clippy::too_many_arguments)]
#[allow(clippy::needless_range_loop)]
fn solve_multilayer_power_mesh_network(
    options: &MultilayerPowerMeshOptions,
    network: PowerMeshNetwork,
    injected_current: &[Vec<Vec<f64>>],
    initial_voltage_v: f64,
    current_sign: f64,
    fixed_voltage: &[Vec<Vec<Option<f64>>>],
) -> Result<MultilayerMeshSolve, String> {
    let mut voltage =
        vec![vec![vec![initial_voltage_v; options.columns]; options.rows]; options.layers.len()];
    let mut iterations = 0;
    let mut final_delta = f64::INFINITY;
    while iterations < options.max_iterations {
        let mut max_delta = 0.0_f64;
        for layer in 0..options.layers.len() {
            for row in 0..options.rows {
                for column in 0..options.columns {
                    if let Some(fixed) = fixed_voltage[layer][row][column] {
                        voltage[layer][row][column] = fixed;
                        continue;
                    }
                    let mut conductance_sum = 0.0_f64;
                    let mut neighbor_voltage_sum = 0.0_f64;
                    let layer_resistance = &options.layers[layer];
                    let (horizontal, vertical) = match network {
                        PowerMeshNetwork::Vdd => (
                            layer_resistance.vdd_horizontal_ohm_per_segment,
                            layer_resistance.vdd_vertical_ohm_per_segment,
                        ),
                        PowerMeshNetwork::Gnd => (
                            layer_resistance.gnd_horizontal_ohm_per_segment,
                            layer_resistance.gnd_vertical_ohm_per_segment,
                        ),
                    };
                    if row > 0 {
                        let conductance = 1.0 / vertical;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row - 1][column];
                    }
                    if row + 1 < options.rows {
                        let conductance = 1.0 / vertical;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row + 1][column];
                    }
                    if column > 0 {
                        let conductance = 1.0 / horizontal;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row][column - 1];
                    }
                    if column + 1 < options.columns {
                        let conductance = 1.0 / horizontal;
                        conductance_sum += conductance;
                        neighbor_voltage_sum += conductance * voltage[layer][row][column + 1];
                    }
                    for via in &options.vias {
                        let resistance = match network {
                            PowerMeshNetwork::Vdd => via.vdd_resistance_ohm,
                            PowerMeshNetwork::Gnd => via.gnd_resistance_ohm,
                        };
                        if via.row == row
                            && via.column == column
                            && (via.from_layer == layer || via.to_layer == layer)
                        {
                            let other_layer = if via.from_layer == layer {
                                via.to_layer
                            } else {
                                via.from_layer
                            };
                            let conductance = 1.0 / resistance;
                            conductance_sum += conductance;
                            neighbor_voltage_sum += conductance * voltage[other_layer][row][column];
                        }
                    }
                    if conductance_sum <= f64::EPSILON {
                        return Err(format!(
                            "multilayer power mesh node layer {} ({},{}) is disconnected",
                            layer, row, column
                        ));
                    }
                    let updated = (neighbor_voltage_sum
                        + current_sign * injected_current[layer][row][column])
                        / conductance_sum;
                    let delta = (updated - voltage[layer][row][column]).abs();
                    max_delta = max_delta.max(delta);
                    voltage[layer][row][column] = updated;
                }
            }
        }
        iterations += 1;
        final_delta = max_delta;
        if max_delta <= options.tolerance_v {
            return Ok((voltage, iterations, final_delta));
        }
    }
    Err(format!(
        "multilayer power mesh solver did not converge: final delta {:.6e} V after {} iterations",
        final_delta, iterations
    ))
}

/// Solve a layer/via-aware VDD/GND mesh from explicit cell currents.
#[allow(clippy::needless_range_loop)]
pub fn analyze_multilayer_mesh_currents(
    nl: &Netlist,
    placement: &Placement,
    cell_current_a: &[f64],
    total_power_uw: f64,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    if placement.cell_pos.len() != nl.cell_count() || placement.cell_width.len() != nl.cell_count()
    {
        return Err("multilayer power mesh placement/netlist cell count mismatch".into());
    }
    if cell_current_a.len() != nl.cell_count() {
        return Err("multilayer power mesh current/netlist cell count mismatch".into());
    }
    if !options.cell_layers.is_empty() && options.cell_layers.len() != nl.cell_count() {
        return Err("multilayer power mesh cell layer mapping count mismatch".into());
    }
    if !options.cell_power_domains.is_empty() && options.cell_power_domains.len() != nl.cell_count()
    {
        return Err("multilayer power mesh cell domain mapping count mismatch".into());
    }
    validate_multilayer_power_mesh_options(options)?;

    let domain_voltages = options
        .power_domains
        .iter()
        .map(|domain| (domain.name.as_str(), domain.voltage_v))
        .collect::<BTreeMap<_, _>>();
    let reference_vdd_v = options
        .power_domains
        .iter()
        .map(|domain| domain.voltage_v)
        .fold(options.voltage_v, f64::max);

    let mut injected_current =
        vec![vec![vec![0.0_f64; options.columns]; options.rows]; options.layers.len()];
    let mut cells = Vec::with_capacity(nl.cell_count());
    for cell_index in 0..nl.cell_count() {
        let position = &placement.cell_pos[cell_index];
        if position.row >= placement.rows {
            return Err(format!(
                "cell '{}' row {} is outside placement rows {}",
                nl.cell(cell_index).name,
                position.row,
                placement.rows
            ));
        }
        let physical_row = position.row as f64 + 0.5;
        let mesh_row =
            ((physical_row * options.rows as f64) / placement.rows.max(1) as f64).floor();
        let mesh_row = mesh_row.clamp(0.0, (options.rows - 1) as f64) as usize;
        let center_site =
            position.site as f64 + placement.cell_width[cell_index].max(1) as f64 / 2.0;
        let normalized_column = (center_site / SITES_PER_ROW as f64).clamp(0.0, 0.999999);
        let mesh_column = (normalized_column * options.columns as f64).floor();
        let mesh_column = mesh_column.clamp(0.0, (options.columns - 1) as f64) as usize;
        let layer = options.cell_layers.get(cell_index).copied().unwrap_or(0);
        let power_domain = options.cell_power_domains.get(cell_index).cloned();
        let supply_voltage_v = power_domain
            .as_deref()
            .and_then(|domain| domain_voltages.get(domain).copied());
        let current_a = cell_current_a[cell_index];
        if !current_a.is_finite() || current_a < 0.0 {
            return Err(format!(
                "cell '{}' has invalid multilayer power-mesh current {}",
                nl.cell(cell_index).name,
                current_a
            ));
        }
        injected_current[layer][mesh_row][mesh_column] += current_a;
        cells.push(MultilayerPowerMeshCell {
            cell: cell_index,
            name: nl.cell(cell_index).name.clone(),
            layer,
            mesh_row,
            mesh_column,
            current_a,
            local_total_drop_v: 0.0,
            power_domain,
            supply_voltage_v,
        });
    }

    let vdd_fixed = multilayer_fixed_voltages(options, PowerMeshNetwork::Vdd)?;
    let gnd_fixed = multilayer_fixed_voltages(options, PowerMeshNetwork::Gnd)?;
    let (vdd, vdd_iterations, vdd_delta) = solve_multilayer_power_mesh_network(
        options,
        PowerMeshNetwork::Vdd,
        &injected_current,
        reference_vdd_v,
        -1.0,
        &vdd_fixed,
    )?;
    let (gnd, gnd_iterations, gnd_delta) = solve_multilayer_power_mesh_network(
        options,
        PowerMeshNetwork::Gnd,
        &injected_current,
        0.0,
        1.0,
        &gnd_fixed,
    )?;

    let mut nodes = Vec::with_capacity(options.layers.len() * options.rows * options.columns);
    let mut worst_drop = 0.0_f64;
    let mut worst_node = None;
    for layer in 0..options.layers.len() {
        for row in 0..options.rows {
            for column in 0..options.columns {
                let vdd_drop = (reference_vdd_v - vdd[layer][row][column]).max(0.0);
                let gnd_drop = gnd[layer][row][column].max(0.0);
                let total_drop = vdd_drop + gnd_drop;
                if total_drop > worst_drop {
                    worst_drop = total_drop;
                    worst_node = Some((layer, row, column));
                }
                nodes.push(MultilayerPowerMeshNode {
                    layer,
                    layer_name: options.layers[layer].name.clone(),
                    row,
                    column,
                    vdd_v: vdd[layer][row][column],
                    gnd_v: gnd[layer][row][column],
                    vdd_drop_v: vdd_drop,
                    gnd_drop_v: gnd_drop,
                    total_drop_v: total_drop,
                    injected_current_a: injected_current[layer][row][column],
                });
            }
        }
    }
    let node_index = |layer: usize, row: usize, column: usize| {
        (layer * options.rows + row) * options.columns + column
    };
    for cell in &mut cells {
        let node = &nodes[node_index(cell.layer, cell.mesh_row, cell.mesh_column)];
        let reference = cell.supply_voltage_v.unwrap_or(options.voltage_v);
        let local_vdd_drop = (reference - node.vdd_v).max(0.0);
        cell.local_total_drop_v = local_vdd_drop + node.gnd_drop_v;
    }

    let mut segments = Vec::new();
    for (network, voltages) in [(PowerMeshNetwork::Vdd, &vdd), (PowerMeshNetwork::Gnd, &gnd)] {
        for (layer, layer_options) in options.layers.iter().enumerate() {
            let (horizontal, vertical) = match network {
                PowerMeshNetwork::Vdd => (
                    layer_options.vdd_horizontal_ohm_per_segment,
                    layer_options.vdd_vertical_ohm_per_segment,
                ),
                PowerMeshNetwork::Gnd => (
                    layer_options.gnd_horizontal_ohm_per_segment,
                    layer_options.gnd_vertical_ohm_per_segment,
                ),
            };
            for row in 0..options.rows {
                for column in 0..options.columns.saturating_sub(1) {
                    let voltage_drop =
                        voltages[layer][row][column] - voltages[layer][row][column + 1];
                    let current = (voltage_drop / horizontal).abs();
                    let (current_density, em_lifetime) = power_mesh_segment_em_values(
                        current,
                        layer_options.horizontal_cross_section_um2,
                        options,
                    )?;
                    segments.push(MultilayerPowerMeshSegment {
                        network,
                        orientation: PowerMeshSegmentOrientation::Horizontal,
                        from_layer: layer,
                        from_row: row,
                        from_column: column,
                        to_layer: layer,
                        to_row: row,
                        to_column: column + 1,
                        resistance_ohm: horizontal,
                        voltage_drop_v: voltage_drop,
                        current_a: current,
                        current_density_a_per_um2: current_density,
                        em_lifetime_hours: em_lifetime,
                    });
                }
            }
            for row in 0..options.rows.saturating_sub(1) {
                for column in 0..options.columns {
                    let voltage_drop =
                        voltages[layer][row][column] - voltages[layer][row + 1][column];
                    let current = (voltage_drop / vertical).abs();
                    let (current_density, em_lifetime) = power_mesh_segment_em_values(
                        current,
                        layer_options.vertical_cross_section_um2,
                        options,
                    )?;
                    segments.push(MultilayerPowerMeshSegment {
                        network,
                        orientation: PowerMeshSegmentOrientation::Vertical,
                        from_layer: layer,
                        from_row: row,
                        from_column: column,
                        to_layer: layer,
                        to_row: row + 1,
                        to_column: column,
                        resistance_ohm: vertical,
                        voltage_drop_v: voltage_drop,
                        current_a: current,
                        current_density_a_per_um2: current_density,
                        em_lifetime_hours: em_lifetime,
                    });
                }
            }
        }
        for via in &options.vias {
            let (from_layer, to_layer) = (via.from_layer, via.to_layer);
            let resistance = match network {
                PowerMeshNetwork::Vdd => via.vdd_resistance_ohm,
                PowerMeshNetwork::Gnd => via.gnd_resistance_ohm,
            };
            let voltage_drop =
                voltages[from_layer][via.row][via.column] - voltages[to_layer][via.row][via.column];
            let current = (voltage_drop / resistance).abs();
            let (current_density, em_lifetime) =
                power_mesh_segment_em_values(current, via.area_um2, options)?;
            segments.push(MultilayerPowerMeshSegment {
                network,
                orientation: PowerMeshSegmentOrientation::Via,
                from_layer,
                from_row: via.row,
                from_column: via.column,
                to_layer,
                to_row: via.row,
                to_column: via.column,
                resistance_ohm: resistance,
                voltage_drop_v: voltage_drop,
                current_a: current,
                current_density_a_per_um2: current_density,
                em_lifetime_hours: em_lifetime,
            });
        }
    }

    let total_current_a: f64 = cell_current_a.iter().sum();
    let worst_segment_current_a = segments
        .iter()
        .map(|segment| segment.current_a)
        .fold(0.0, f64::max);
    let worst_em_lifetime_hours = segments
        .iter()
        .map(|segment| segment.em_lifetime_hours)
        .reduce(f64::min)
        .unwrap_or(f64::INFINITY);
    let mut findings = Vec::new();
    if worst_drop > options.max_total_drop_v {
        findings.push(IrFinding {
            code: "MULTILAYER-MESH-IR-DROP".into(),
            severity: IrSeverity::Error,
            row: worst_node.map(|(_, row, _)| row),
            message: format!(
                "worst multilayer power-mesh drop {:.6} V at {:?} exceeds {:.6} V",
                worst_drop, worst_node, options.max_total_drop_v
            ),
        });
    }
    for node in &nodes {
        if node.injected_current_a > options.max_node_current_a {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-NODE-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(node.row),
                message: format!(
                    "layer {} mesh node ({},{}) current {:.6} A exceeds {:.6} A",
                    node.layer_name,
                    node.row,
                    node.column,
                    node.injected_current_a,
                    options.max_node_current_a
                ),
            });
        }
    }
    for segment in &segments {
        if segment.current_a > options.max_segment_current_a {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-SEGMENT-CURRENT".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} layer segment {:?} {} ({},{},{})-{} ({},{},{}) current {:.6} A exceeds {:.6} A",
                    segment.network,
                    segment.orientation,
                    options.layers[segment.from_layer].name,
                    segment.from_row,
                    segment.from_column,
                    segment.from_layer,
                    options.layers[segment.to_layer].name,
                    segment.to_row,
                    segment.to_column,
                    segment.to_layer,
                    segment.current_a,
                    options.max_segment_current_a
                ),
            });
        }
        if options.em.enabled
            && segment.em_lifetime_hours.is_finite()
            && segment.em_lifetime_hours < options.em.min_lifetime_hours
        {
            findings.push(IrFinding {
                code: "MULTILAYER-MESH-EM-LIFETIME".into(),
                severity: IrSeverity::Error,
                row: Some(segment.from_row),
                message: format!(
                    "{:?} {} segment {:?} ({},{},{})-{} ({},{},{}) current density {:.6} A/um^2 has EM lifetime {:.3e} h below {:.3e} h",
                    segment.network,
                    options.layers[segment.from_layer].name,
                    segment.orientation,
                    segment.from_row,
                    segment.from_column,
                    segment.from_layer,
                    options.layers[segment.to_layer].name,
                    segment.to_row,
                    segment.to_column,
                    segment.to_layer,
                    segment.current_density_a_per_um2,
                    segment.em_lifetime_hours,
                    options.em.min_lifetime_hours
                ),
            });
        }
    }

    let power_domain_results = options
        .power_domains
        .iter()
        .map(|supply| {
            let domain_cells = cells
                .iter()
                .filter(|cell| cell.power_domain.as_deref() == Some(supply.name.as_str()))
                .collect::<Vec<_>>();
            MultilayerPowerDomainResult {
                name: supply.name.clone(),
                voltage_v: supply.voltage_v,
                cells: domain_cells.len(),
                total_current_a: domain_cells.iter().map(|cell| cell.current_a).sum(),
                worst_local_drop_v: domain_cells
                    .iter()
                    .map(|cell| cell.local_total_drop_v)
                    .fold(0.0, f64::max),
            }
        })
        .collect::<Vec<_>>();

    Ok(MultilayerPowerMeshResult {
        voltage_v: options.voltage_v,
        total_power_uw,
        total_current_a,
        worst_drop_v: worst_drop,
        worst_node,
        worst_segment_current_a,
        worst_em_lifetime_hours,
        nodes,
        cells,
        segments,
        iterations: vdd_iterations.max(gnd_iterations),
        final_delta_v: vdd_delta.max(gnd_delta),
        findings,
        power_domains: power_domain_results,
    })
}

/// Analyze a multilayer mesh from a switching-power report.
pub fn analyze_multilayer_mesh(
    nl: &Netlist,
    placement: &Placement,
    power: &PowerReport,
    options: &MultilayerPowerMeshOptions,
) -> Result<MultilayerPowerMeshResult, String> {
    if power.cells.len() != nl.cell_count() {
        return Err("multilayer power mesh power/netlist cell count mismatch".into());
    }
    let total_sites: usize = placement.cell_width.iter().sum();
    let total_sites = total_sites.max(1);
    let mut cell_power = vec![0.0_f64; nl.cell_count()];
    for report_cell in &power.cells {
        if report_cell.cell >= nl.cell_count() {
            return Err(format!(
                "power report cell index {} is out of range",
                report_cell.cell
            ));
        }
        let leakage_share = power.static_power_uw * placement.cell_width[report_cell.cell] as f64
            / total_sites as f64;
        cell_power[report_cell.cell] = report_cell.dynamic_power_uw + leakage_share;
    }
    let cell_current = cell_power
        .iter()
        .map(|power_uw| power_uw / options.voltage_v / 1_000_000.0)
        .collect::<Vec<_>>();
    let total_power_uw: f64 = cell_power.iter().sum();
    analyze_multilayer_mesh_currents(nl, placement, &cell_current, total_power_uw, options)
}

/// Render a layer/via-aware power-mesh report.
pub fn multilayer_power_mesh_report(result: &MultilayerPowerMeshResult) -> String {
    let mut out = format!(
        "Multilayer power-mesh IR report: nodes={}, cells={}, segments={}, iterations={}, final_delta={:.6e} V\n",
        result.nodes.len(),
        result.cells.len(),
        result.segments.len(),
        result.iterations,
        result.final_delta_v
    );
    for node in result
        .nodes
        .iter()
        .filter(|node| node.injected_current_a > 0.0)
    {
        out.push_str(&format!(
            "  layer '{}' ({},{}): current {:.6} A, total drop {:.6} V\n",
            node.layer_name, node.row, node.column, node.injected_current_a, node.total_drop_v
        ));
    }
    for domain in &result.power_domains {
        out.push_str(&format!(
            "  domain '{}' V={:.6} cells={} I={:.9} A worst_local_drop={:.9} V\n",
            domain.name,
            domain.voltage_v,
            domain.cells,
            domain.total_current_a,
            domain.worst_local_drop_v
        ));
    }
    for segment in result
        .segments
        .iter()
        .filter(|segment| segment.orientation == PowerMeshSegmentOrientation::Via)
    {
        out.push_str(&format!(
            "  via {:?} ({},{}) {}->{}: current {:.6} A, J {:.6} A/um^2, EM {:.3e} h\n",
            segment.network,
            segment.from_row,
            segment.from_column,
            segment.from_layer,
            segment.to_layer,
            segment.current_a,
            segment.current_density_a_per_um2,
            segment.em_lifetime_hours
        ));
    }
    out.push_str(&format!(
        "  worst drop: {:.6} V at {:?}; worst segment current: {:.6} A; worst EM {:.3e} h; errors={}\n",
        result.worst_drop_v,
        result.worst_node,
        result.worst_segment_current_a,
        result.worst_em_lifetime_hours,
        result.has_errors()
    ));
    for finding in &result.findings {
        out.push_str(&format!("  {}: {}\n", finding.code, finding.message));
    }
    out
}
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

fn validate_domain_options(domain: &str, options: &DomainIrOptions) -> Result<(), String> {
    validate_f64(
        &format!("domain '{}' voltage", domain),
        options.voltage_v,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' VDD resistance", domain),
        options.vdd_ohm_per_site,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' GND resistance", domain),
        options.gnd_ohm_per_site,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' maximum drop", domain),
        options.max_total_drop_v,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' maximum rail current", domain),
        options.max_rail_current_a,
        true,
    )?;
    Ok(())
}

fn validate_f64(name: &str, value: f64, positive: bool) -> Result<(), String> {
    if !value.is_finite() || (positive && value <= 0.0) {
        return Err(format!("invalid {} {}", name, value));
    }
    Ok(())
}

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
    use eda_core::netlist::{Cell, CellKind};

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
            cell_pos: vec![eda_pnr::Pos { row: 1, site: 15 }],
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
            cell_pos: vec![eda_pnr::Pos { row: 1, site: 15 }],
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
