//! DC, transient, AC, statistical, optimization, and Pareto results.

use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct DcOperatingPoint {
    pub node_voltages: BTreeMap<String, f64>,
    pub branch_currents: BTreeMap<String, f64>,
    pub iterations: usize,
    pub converged: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DcSweepPoint {
    pub source_value: f64,
    pub operating_point: DcOperatingPoint,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DcSweepResult {
    pub source: String,
    pub points: Vec<DcSweepPoint>,
}

impl DcSweepResult {
    pub fn node_values(&self, node: &str) -> Result<Vec<f64>, String> {
        self.points
            .iter()
            .map(|point| {
                point
                    .operating_point
                    .node_voltages
                    .get(node)
                    .copied()
                    .ok_or_else(|| format!("unknown node '{}'", node))
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransientSample {
    pub time_s: f64,
    pub node_voltages: BTreeMap<String, f64>,
    pub branch_currents: BTreeMap<String, f64>,
    pub capacitor_currents: BTreeMap<String, f64>,
    /// Local lumped electrothermal state per MOS/BJT element name.
    pub device_temperatures: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransientResult {
    pub method: IntegrationMethod,
    pub step_s: f64,
    pub stop_s: f64,
    pub samples: Vec<TransientSample>,
    pub convergence_failures: usize,
}

impl TransientResult {
    pub fn time(&self) -> Vec<f64> {
        self.samples.iter().map(|sample| sample.time_s).collect()
    }

    pub fn node_values(&self, node: &str) -> Result<Vec<f64>, String> {
        self.samples
            .iter()
            .map(|sample| {
                sample
                    .node_voltages
                    .get(node)
                    .copied()
                    .ok_or_else(|| format!("unknown node '{}'", node))
            })
            .collect()
    }

    pub fn device_names(&self) -> Vec<String> {
        self.samples
            .iter()
            .flat_map(|sample| sample.device_temperatures.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn device_temperature_values(&self, element: &str) -> Result<Vec<f64>, String> {
        self.samples
            .iter()
            .map(|sample| {
                sample
                    .device_temperatures
                    .get(element)
                    .copied()
                    .ok_or_else(|| format!("unknown thermal device '{}'", element))
            })
            .collect()
    }

    pub fn sample_at(&self, time_s: f64) -> Option<&TransientSample> {
        self.samples
            .iter()
            .find(|sample| (sample.time_s - time_s).abs() <= self.step_s * 1e-9)
    }

    pub fn final_sample(&self) -> Option<&TransientSample> {
        self.samples.last()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PssSample {
    pub phase: f64,
    pub time_s: f64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PssHarmonic {
    pub index: usize,
    pub frequency_hz: f64,
    pub phasor: Phasor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PssResult {
    pub output_node: String,
    pub fundamental_hz: f64,
    pub automatic_fundamental: bool,
    pub cycles: usize,
    pub converged: bool,
    pub residual: f64,
    pub samples: Vec<PssSample>,
    pub harmonics: Vec<PssHarmonic>,
}

impl PssResult {
    pub fn harmonic(&self, index: usize) -> Option<&PssHarmonic> {
        self.harmonics
            .iter()
            .find(|harmonic| harmonic.index == index)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeSample {
    pub time_s: f64,
    pub envelope: f64,
    pub phase_rad: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeResult {
    pub output_node: String,
    pub carrier_hz: f64,
    pub window_s: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub average: f64,
    pub samples: Vec<EnvelopeSample>,
}

impl EnvelopeResult {
    pub fn values(&self) -> Vec<f64> {
        self.samples.iter().map(|sample| sample.envelope).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgingResult {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub age_years: f64,
    pub time_exponent: f64,
    pub stress: f64,
    pub nominal_output: f64,
    pub aged_output: f64,
    pub circuit: SpiceCircuit,
}

impl AgingResult {
    pub fn output_delta(&self) -> f64 {
        self.aged_output - self.nominal_output
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phasor {
    pub real: f64,
    pub imaginary: f64,
}

impl Phasor {
    pub fn magnitude(&self) -> f64 {
        self.real.hypot(self.imaginary)
    }

    pub fn magnitude_db(&self) -> f64 {
        20.0 * self.magnitude().log10()
    }

    pub fn phase_deg(&self) -> f64 {
        self.imaginary.atan2(self.real).to_degrees()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcPoint {
    pub frequency_hz: f64,
    pub node_phasors: BTreeMap<String, Phasor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcResult {
    pub kind: AcSweepKind,
    pub points: Vec<AcPoint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoisePoint {
    pub frequency_hz: f64,
    pub output_v_per_sqrt_hz: f64,
    pub input_referred_per_sqrt_hz: f64,
    pub contributions: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NoiseResult {
    pub output_node: String,
    pub input_source: String,
    pub points: Vec<NoisePoint>,
}

impl NoiseResult {
    pub fn frequencies(&self) -> Vec<f64> {
        self.points.iter().map(|point| point.frequency_hz).collect()
    }

    pub fn output_noise(&self) -> Vec<f64> {
        self.points
            .iter()
            .map(|point| point.output_v_per_sqrt_hz)
            .collect()
    }

    pub fn input_referred_noise(&self) -> Vec<f64> {
        self.points
            .iter()
            .map(|point| point.input_referred_per_sqrt_hz)
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonteCarloSample {
    pub index: usize,
    pub value: f64,
    pub randomized_objects: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MonteCarloStatistics {
    pub minimum: f64,
    pub maximum: f64,
    pub mean: f64,
    pub standard_deviation: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonteCarloResult {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub samples: Vec<MonteCarloSample>,
    pub statistics: MonteCarloStatistics,
}

impl MonteCarloResult {
    pub fn values(&self) -> Vec<f64> {
        self.samples.iter().map(|sample| sample.value).collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensitivityEntry {
    pub parameter: String,
    pub object: String,
    pub nominal_parameter: f64,
    pub perturbation: f64,
    pub low_output: f64,
    pub high_output: f64,
    pub absolute_sensitivity: f64,
    pub normalized_sensitivity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensitivityResult {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub nominal_output: f64,
    pub entries: Vec<SensitivityEntry>,
}

impl SensitivityResult {
    pub fn find(&self, parameter: &str) -> Option<&SensitivityEntry> {
        self.entries
            .iter()
            .find(|entry| entry.parameter.eq_ignore_ascii_case(parameter))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalSensitivityEntry {
    pub parameter: String,
    pub object: String,
    pub nominal_parameter: f64,
    pub first_order_index: f64,
    pub total_order_index: f64,
    pub linear_variance_contribution: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalSensitivityResult {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub nominal_output: f64,
    pub output_variance: f64,
    pub linearized_variance: f64,
    pub entries: Vec<GlobalSensitivityEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeEntry {
    pub parameter: String,
    pub nominal_parameter: f64,
    pub gradient: f64,
    pub hessian_row: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeResult {
    pub target: DerivativeTarget,
    pub frequency_hz: f64,
    pub output_node: String,
    pub nominal_output: f64,
    pub entries: Vec<DerivativeEntry>,
}

impl DerivativeResult {
    pub fn gradient(&self) -> Vec<f64> {
        self.entries.iter().map(|entry| entry.gradient).collect()
    }

    pub fn hessian(&self) -> Vec<Vec<f64>> {
        self.entries
            .iter()
            .map(|entry| entry.hessian_row.clone())
            .collect()
    }

    pub fn find(&self, parameter: &str) -> Option<&DerivativeEntry> {
        self.entries
            .iter()
            .find(|entry| entry.parameter.eq_ignore_ascii_case(parameter))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizationParameterResult {
    pub parameter: String,
    pub initial: f64,
    pub final_value: f64,
    pub lower: f64,
    pub upper: f64,
    pub quantum: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizationResult {
    pub objective: OptimizationObjective,
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub goal_value: f64,
    pub initial_output: f64,
    pub initial_score: f64,
    pub final_output: f64,
    pub final_score: f64,
    pub converged: bool,
    pub generations: usize,
    pub evaluations: usize,
    pub parameters: Vec<OptimizationParameterResult>,
    pub circuit: SpiceCircuit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParetoPoint {
    pub outputs: Vec<f64>,
    pub crowding_distance: f64,
    pub reference_direction: Option<Vec<f64>>,
    pub constraint_values: Vec<f64>,
    pub constraint_violation: f64,
    pub constraints_satisfied: bool,
    pub parameters: Vec<OptimizationParameterResult>,
    pub circuit: SpiceCircuit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParetoResult {
    pub target: MonteCarloTarget,
    pub output_nodes: Vec<String>,
    pub senses: Vec<ParetoSense>,
    pub front_method: ParetoFrontMethod,
    pub adaptive_references: bool,
    pub constraints: Vec<ParetoConstraint>,
    pub points: Vec<ParetoPoint>,
    pub generations: usize,
    pub evaluations: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParetoFrontMethod {
    CrowdingDistance,
    ReferenceDirections,
}

impl ParetoFrontMethod {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "crowding" | "crowding-distance" | "nsga2" => Some(Self::CrowdingDistance),
            "reference" | "reference-directions" | "nsga3" => Some(Self::ReferenceDirections),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrowdingDistance => "crowding",
            Self::ReferenceDirections => "reference",
        }
    }
}

pub fn reference_direction_count(dimensions: usize, divisions: usize) -> Option<usize> {
    if dimensions == 0 || dimensions > 8 || divisions == 0 {
        return None;
    }
    let numerator = ((divisions + 1)..(divisions + dimensions))
        .try_fold(1usize, |acc, value| acc.checked_mul(value))?;
    let denominator = (1..dimensions).try_fold(1usize, |acc, value| acc.checked_mul(value))?;
    numerator.checked_div(denominator)
}

pub fn constraint_violation(value: f64, sense: ConstraintSense, bound: f64) -> f64 {
    match sense {
        ConstraintSense::LessEqual => (value - bound).max(0.0),
        ConstraintSense::GreaterEqual => (bound - value).max(0.0),
        ConstraintSense::Equal => (value - bound).abs(),
    }
}

pub fn pareto_dominates(left: &[f64], right: &[f64], senses: &[ParetoSense]) -> bool {
    if left.len() != right.len() || left.len() != senses.len() {
        return false;
    }
    let no_worse = left
        .iter()
        .zip(right)
        .zip(senses)
        .all(|((a, b), sense)| match sense {
            ParetoSense::Minimize => a <= b,
            ParetoSense::Maximize => a >= b,
        });
    let strictly_better = left
        .iter()
        .zip(right)
        .zip(senses)
        .any(|((a, b), sense)| match sense {
            ParetoSense::Minimize => a < b,
            ParetoSense::Maximize => a > b,
        });
    no_worse && strictly_better
}

pub fn constrained_pareto_dominates(
    left_outputs: &[f64],
    left_violation: f64,
    right_outputs: &[f64],
    right_violation: f64,
    senses: &[ParetoSense],
) -> bool {
    if left_violation > 1e-10 && right_violation > 1e-10 {
        return left_violation < right_violation;
    }
    if left_violation > 1e-10 || right_violation > 1e-10 {
        return right_violation > 1e-10;
    }
    pareto_dominates(left_outputs, right_outputs, senses)
}

impl ParetoResult {
    pub fn is_nondominated(&self) -> bool {
        self.points.iter().enumerate().all(|(left, left_point)| {
            self.points
                .iter()
                .enumerate()
                .filter(|(right, _)| *right != left)
                .all(|(_, right_point)| {
                    !constrained_pareto_dominates(
                        &right_point.outputs,
                        right_point.constraint_violation,
                        &left_point.outputs,
                        left_point.constraint_violation,
                        &self.senses,
                    )
                })
        })
    }
}

impl GlobalSensitivityResult {
    pub fn find(&self, parameter: &str) -> Option<&GlobalSensitivityEntry> {
        self.entries
            .iter()
            .find(|entry| entry.parameter.eq_ignore_ascii_case(parameter))
    }
}

impl AcResult {
    pub fn frequencies(&self) -> Vec<f64> {
        self.points.iter().map(|point| point.frequency_hz).collect()
    }

    pub fn node_magnitude(&self, node: &str) -> Result<Vec<f64>, String> {
        self.points
            .iter()
            .map(|point| {
                point
                    .node_phasors
                    .get(node)
                    .map(Phasor::magnitude)
                    .ok_or_else(|| format!("unknown node '{}'", node))
            })
            .collect()
    }

    pub fn node_magnitude_db(&self, node: &str) -> Result<Vec<f64>, String> {
        self.points
            .iter()
            .map(|point| {
                point
                    .node_phasors
                    .get(node)
                    .map(Phasor::magnitude_db)
                    .ok_or_else(|| format!("unknown node '{}'", node))
            })
            .collect()
    }
}
