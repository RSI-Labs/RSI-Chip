//! Typed options, mesh/domain models, and IR/UPF result objects.

use std::collections::BTreeMap;

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
