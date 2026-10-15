//! Analysis directives, optimization controls, and statistical options.

use super::circuit::{ensure_finite, ensure_finite_nonnegative, ensure_finite_positive};
use super::*;
use std::collections::BTreeMap;
use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IntegrationMethod {
    BackwardEuler,
    Trapezoidal,
}

impl IntegrationMethod {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "be" | "backward_euler" | "backward-euler" => Some(Self::BackwardEuler),
            "trap" | "trapezoidal" | "trapezoid" => Some(Self::Trapezoidal),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackwardEuler => "backward_euler",
            Self::Trapezoidal => "trapezoidal",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SourceWaveform {
    Dc(f64),
    Sin {
        offset: f64,
        amplitude: f64,
        frequency_hz: f64,
        delay_s: f64,
        damping_per_s: f64,
    },
    Am {
        offset: f64,
        amplitude: f64,
        carrier_hz: f64,
        modulation_hz: f64,
        depth: f64,
    },
    Pulse {
        initial: f64,
        pulsed: f64,
        delay_s: f64,
        rise_s: f64,
        fall_s: f64,
        width_s: f64,
        period_s: f64,
    },
    Pwl(Vec<(f64, f64)>),
}

impl SourceWaveform {
    pub fn value_at(&self, time_s: f64) -> f64 {
        match self {
            Self::Dc(value) => *value,
            Self::Sin {
                offset,
                amplitude,
                frequency_hz,
                delay_s,
                damping_per_s,
            } => {
                if time_s <= *delay_s {
                    *offset
                } else {
                    let t = time_s - delay_s;
                    offset + amplitude * (-damping_per_s * t).exp() * (TAU * frequency_hz * t).sin()
                }
            }
            Self::Am {
                offset,
                amplitude,
                carrier_hz,
                modulation_hz,
                depth,
            } => {
                let modulation = 1.0 + depth * (TAU * modulation_hz * time_s).sin();
                offset + amplitude * modulation * (TAU * carrier_hz * time_s).sin()
            }
            Self::Pulse {
                initial,
                pulsed,
                delay_s,
                rise_s,
                fall_s,
                width_s,
                period_s,
            } => {
                let active_width = rise_s + width_s + fall_s;
                let period = if *period_s > 0.0 {
                    period_s.max(active_width)
                } else {
                    f64::INFINITY
                };
                let local = if period.is_finite() && time_s >= *delay_s {
                    (time_s - delay_s) % period
                } else {
                    time_s - delay_s
                };
                if local < 0.0 {
                    *initial
                } else if local < *rise_s {
                    let alpha = if *rise_s > 0.0 { local / *rise_s } else { 1.0 };
                    initial + (pulsed - initial) * alpha
                } else if local < *rise_s + width_s {
                    *pulsed
                } else if local < *rise_s + width_s + fall_s {
                    let after = local - rise_s - width_s;
                    let alpha = if *fall_s > 0.0 { after / *fall_s } else { 1.0 };
                    pulsed + (initial - pulsed) * alpha
                } else {
                    *initial
                }
            }
            Self::Pwl(points) => {
                if points.is_empty() {
                    0.0
                } else if time_s <= points[0].0 {
                    points[0].1
                } else if time_s >= points[points.len() - 1].0 {
                    points[points.len() - 1].1
                } else {
                    let idx = points.partition_point(|(t, _)| *t <= time_s);
                    let (t0, v0) = points[idx - 1];
                    let (t1, v1) = points[idx];
                    if t1 == t0 {
                        v1
                    } else {
                        v0 + (v1 - v0) * (time_s - t0) / (t1 - t0)
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpiceElementKind {
    Resistor {
        resistance: f64,
        first_temperature_coefficient_per_k: f64,
        second_temperature_coefficient_per_k2: f64,
        sheet_resistance_ohm_per_square: f64,
        length_m: f64,
        width_m: f64,
        multiplier: f64,
        layer: Option<String>,
    },
    Capacitor {
        capacitance: f64,
        initial_voltage: f64,
    },
    Inductor {
        inductance: f64,
        initial_current: f64,
    },
    VoltageSource {
        waveform: SourceWaveform,
        ac_magnitude: Option<f64>,
        ac_phase_deg: f64,
    },
    CurrentSource {
        waveform: SourceWaveform,
        ac_magnitude: Option<f64>,
        ac_phase_deg: f64,
    },
    VoltageControlledVoltageSource {
        gain: f64,
    },
    CurrentControlledVoltageSource {
        controlling_source: String,
        transresistance: f64,
    },
    VoltageControlledCurrentSource {
        transconductance: f64,
    },
    CurrentControlledCurrentSource {
        controlling_source: String,
        gain: f64,
    },
    Diode {
        model: String,
    },
    Mosfet {
        model: String,
        width_m: f64,
        length_m: f64,
        multiplier: f64,
    },
    BipolarJunctionTransistor {
        model: String,
        area: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpiceElement {
    pub name: String,
    pub nodes: Vec<String>,
    pub kind: SpiceElementKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcSweepKind {
    Linear,
    Decade,
    Octave,
}

impl AcSweepKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "lin" | "linear" => Some(Self::Linear),
            "dec" | "decade" => Some(Self::Decade),
            "oct" | "octave" => Some(Self::Octave),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Decade => "decade",
            Self::Octave => "octave",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Analysis {
    OperatingPoint,
    DcSweep {
        source: String,
        start: f64,
        stop: f64,
        step: f64,
    },
    Transient {
        step_s: f64,
        stop_s: f64,
        method: IntegrationMethod,
    },
    AcSweep {
        kind: AcSweepKind,
        points: usize,
        start_hz: f64,
        stop_hz: f64,
    },
    NoiseSweep {
        output_node: String,
        input_source: String,
        kind: AcSweepKind,
        points: usize,
        start_hz: f64,
        stop_hz: f64,
    },
    MonteCarlo(MonteCarloOptions),
    Sensitivity(SensitivityOptions),
    GlobalSensitivity(GlobalSensitivityOptions),
    Optimization(OptimizationOptions),
    Derivative(DerivativeOptions),
    Pareto(ParetoOptions),
    PeriodicSteadyState(PssOptions),
    EnvelopeFollow(EnvelopeOptions),
    Aging(AgingOptions),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvelopeOptions {
    pub output_node: String,
    pub carrier_hz: f64,
    pub carrier_cycles: f64,
    pub points: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgingOptions {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub frequency_hz: f64,
    pub age_years: f64,
    pub time_exponent: f64,
    pub resistance_increase_per_year: f64,
    pub mos_threshold_shift_v_per_year: f64,
    pub mos_kp_degradation_per_year: f64,
}

impl Default for AgingOptions {
    fn default() -> Self {
        Self {
            target: MonteCarloTarget::OperatingPoint,
            output_node: "out".into(),
            frequency_hz: 1.0,
            age_years: 1.0,
            time_exponent: 0.5,
            resistance_increase_per_year: 0.0,
            mos_threshold_shift_v_per_year: 0.0,
            mos_kp_degradation_per_year: 0.0,
        }
    }
}

impl AgingOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("aging output node cannot be empty".into());
        }
        ensure_finite_positive(self.frequency_hz, "aging frequency")?;
        if !self.age_years.is_finite() || self.age_years < 0.0 || self.age_years > 1_000_000.0 {
            return Err("aging duration must be between 0 and 1000000 years".into());
        }
        if !self.time_exponent.is_finite() || self.time_exponent <= 0.0 || self.time_exponent > 2.0
        {
            return Err("aging time exponent must be between 0 and 2".into());
        }
        ensure_finite_nonnegative(
            self.resistance_increase_per_year,
            "aging resistance increase",
        )?;
        ensure_finite_nonnegative(
            self.mos_threshold_shift_v_per_year,
            "aging MOS threshold shift",
        )?;
        ensure_finite_nonnegative(self.mos_kp_degradation_per_year, "aging MOS KP degradation")?;
        if self.age_years > 0.0 {
            let stress = self.age_years.powf(self.time_exponent);
            if self.mos_kp_degradation_per_year * stress >= 1.0 {
                return Err("aging would remove all MOS transconductance".into());
            }
        }
        Ok(())
    }
}

impl Default for EnvelopeOptions {
    fn default() -> Self {
        Self {
            output_node: "out".into(),
            carrier_hz: 1.0,
            carrier_cycles: 4.0,
            points: 256,
        }
    }
}

impl EnvelopeOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("envelope output node cannot be empty".into());
        }
        ensure_finite_positive(self.carrier_hz, "envelope carrier frequency")?;
        ensure_finite_positive(self.carrier_cycles, "envelope carrier cycles")?;
        if self.carrier_cycles < 1.0 || self.carrier_cycles > 1_000.0 {
            return Err("envelope carrier cycles must be between 1 and 1000".into());
        }
        if self.points < 4 || self.points > 2_048 {
            return Err("envelope points must be between 4 and 2048".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PssOptions {
    pub output_node: String,
    pub fundamental_hz: f64,
    pub automatic_fundamental: bool,
    pub threshold: Option<f64>,
    pub minimum_cycles: usize,
    pub maximum_cycles: usize,
    pub samples_per_cycle: usize,
    pub harmonics: usize,
    pub tolerance: f64,
}

impl Default for PssOptions {
    fn default() -> Self {
        Self {
            output_node: "out".into(),
            fundamental_hz: 1.0,
            automatic_fundamental: false,
            threshold: None,
            minimum_cycles: 4,
            maximum_cycles: 64,
            samples_per_cycle: 64,
            harmonics: 8,
            tolerance: 1e-5,
        }
    }
}

impl PssOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("PSS output node cannot be empty".into());
        }
        ensure_finite_positive(self.fundamental_hz, "PSS fundamental frequency")?;
        if let Some(threshold) = self.threshold {
            ensure_finite(threshold, "PSS threshold")?;
        }
        if self.minimum_cycles < 2 || self.maximum_cycles > 10_000 {
            return Err("PSS cycles must be between 2 and 10000".into());
        }
        if self.minimum_cycles > self.maximum_cycles {
            return Err("PSS minimum cycles cannot exceed maximum cycles".into());
        }
        if self.samples_per_cycle < 16 || self.samples_per_cycle > 2_048 {
            return Err("PSS samples per cycle must be between 16 and 2048".into());
        }
        if self.harmonics == 0 || self.harmonics > 64 {
            return Err("PSS harmonic count must be between 1 and 64".into());
        }
        if self.harmonics >= self.samples_per_cycle / 2 {
            return Err("PSS harmonic count must be below the Nyquist bin".into());
        }
        if self.maximum_cycles.saturating_mul(self.samples_per_cycle) > 1_000_000 {
            return Err("PSS would require more than 1000000 transient samples".into());
        }
        ensure_finite_positive(self.tolerance, "PSS tolerance")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptimizationObjective {
    Match,
    Minimize,
    Maximize,
}

impl OptimizationObjective {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "match" | "target" => Some(Self::Match),
            "min" | "minimize" => Some(Self::Minimize),
            "max" | "maximize" => Some(Self::Maximize),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::Minimize => "min",
            Self::Maximize => "max",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizationParameter {
    pub parameter: String,
    pub lower: f64,
    pub upper: f64,
    pub initial: Option<f64>,
    pub quantum: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptimizationOptions {
    pub objective: OptimizationObjective,
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub goal_value: f64,
    pub frequency_hz: f64,
    pub parameters: Vec<OptimizationParameter>,
    pub iterations: usize,
    pub population: usize,
    pub seed: u64,
    pub tolerance: f64,
    pub differential_weight: f64,
    pub crossover_probability: f64,
}

impl Default for OptimizationOptions {
    fn default() -> Self {
        Self {
            objective: OptimizationObjective::Match,
            target: MonteCarloTarget::OperatingPoint,
            output_node: "out".into(),
            goal_value: 0.0,
            frequency_hz: 1.0,
            parameters: Vec::new(),
            iterations: 64,
            population: 20,
            seed: 1,
            tolerance: 1e-7,
            differential_weight: 0.7,
            crossover_probability: 0.85,
        }
    }
}

impl OptimizationOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.parameters.is_empty() {
            return Err("optimization needs at least one parameter".into());
        }
        if self.parameters.len() > 16 {
            return Err("optimization supports at most 16 parameters".into());
        }
        ensure_finite(self.goal_value, "optimization goal")?;
        ensure_finite_positive(self.frequency_hz, "optimization frequency")?;
        if self.output_node.trim().is_empty() {
            return Err("optimization output node cannot be empty".into());
        }
        if self.iterations == 0 || self.iterations > 10_000 {
            return Err("optimization iterations must be between 1 and 10000".into());
        }
        if self.population < 5 || self.population > 128 {
            return Err("optimization population must be between 5 and 128".into());
        }
        ensure_finite_positive(self.tolerance, "optimization tolerance")?;
        ensure_finite_positive(self.differential_weight, "optimization differential weight")?;
        if self.differential_weight > 2.0 {
            return Err("optimization differential weight cannot exceed 2.0".into());
        }
        ensure_finite_nonnegative(
            self.crossover_probability,
            "optimization crossover probability",
        )?;
        if self.crossover_probability > 1.0 {
            return Err("optimization crossover probability cannot exceed 1.0".into());
        }
        for parameter in &self.parameters {
            if parameter.parameter.trim().is_empty() {
                return Err("optimization parameter name cannot be empty".into());
            }
            ensure_finite(parameter.lower, "optimization lower bound")?;
            ensure_finite(parameter.upper, "optimization upper bound")?;
            if parameter.lower >= parameter.upper {
                return Err(format!(
                    "optimization parameter '{}' must have lower < upper",
                    parameter.parameter
                ));
            }
            if let Some(initial) = parameter.initial {
                ensure_finite(initial, "optimization initial value")?;
                if !(parameter.lower..=parameter.upper).contains(&initial) {
                    return Err(format!(
                        "optimization initial value for '{}' is outside its bounds",
                        parameter.parameter
                    ));
                }
            }
            if let Some(quantum) = parameter.quantum {
                ensure_finite_positive(quantum, "optimization quantum")?;
                let span = parameter.upper - parameter.lower;
                if quantum > span + span.abs() * 1e-12 {
                    return Err(format!(
                        "optimization quantum for '{}' cannot exceed its bounds",
                        parameter.parameter
                    ));
                }
                if let Some(initial) = parameter.initial {
                    let steps = ((initial - parameter.lower) / quantum).round();
                    let snapped = parameter.lower + steps * quantum;
                    let tolerance = quantum * 1e-9;
                    if (initial - snapped).abs() > tolerance
                        || snapped < parameter.lower - tolerance
                        || snapped > parameter.upper + tolerance
                    {
                        return Err(format!(
                            "optimization initial value for '{}' is outside its quantum grid",
                            parameter.parameter
                        ));
                    }
                }
            }
        }
        for (index, parameter) in self.parameters.iter().enumerate() {
            if self
                .parameters
                .iter()
                .skip(index + 1)
                .any(|other| other.parameter.eq_ignore_ascii_case(&parameter.parameter))
            {
                return Err(format!(
                    "duplicate optimization parameter '{}'",
                    parameter.parameter
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParetoSense {
    Minimize,
    Maximize,
}

impl ParetoSense {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "min" | "minimize" => Some(Self::Minimize),
            "max" | "maximize" => Some(Self::Maximize),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimize => "min",
            Self::Maximize => "max",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintSense {
    LessEqual,
    GreaterEqual,
    Equal,
}

impl ConstraintSense {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LessEqual => "<=",
            Self::GreaterEqual => ">=",
            Self::Equal => "==",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstraintFunction {
    Abs,
    Sqrt,
    Exp,
    Ln,
    Log10,
    Min,
    Max,
}

impl ConstraintFunction {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "abs" => Some(Self::Abs),
            "sqrt" => Some(Self::Sqrt),
            "exp" => Some(Self::Exp),
            "ln" | "log" => Some(Self::Ln),
            "log10" => Some(Self::Log10),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Abs => "abs",
            Self::Sqrt => "sqrt",
            Self::Exp => "exp",
            Self::Ln => "ln",
            Self::Log10 => "log10",
            Self::Min => "min",
            Self::Max => "max",
        }
    }

    pub fn arity(self) -> usize {
        match self {
            Self::Min | Self::Max => 2,
            Self::Abs | Self::Sqrt | Self::Exp | Self::Ln | Self::Log10 => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstraintExpression {
    Constant(f64),
    Node(String),
    Add(Box<ConstraintExpression>, Box<ConstraintExpression>),
    Subtract(Box<ConstraintExpression>, Box<ConstraintExpression>),
    Multiply(Box<ConstraintExpression>, Box<ConstraintExpression>),
    Divide(Box<ConstraintExpression>, Box<ConstraintExpression>),
    Power(Box<ConstraintExpression>, Box<ConstraintExpression>),
    Function(ConstraintFunction, Vec<ConstraintExpression>),
}

impl ConstraintExpression {
    pub fn evaluate(&self, values: &BTreeMap<String, f64>) -> Result<f64, String> {
        let value = match self {
            Self::Constant(value) => *value,
            Self::Node(node) => *values
                .get(node)
                .ok_or_else(|| format!("constraint expression has unknown node '{node}'"))?,
            Self::Add(left, right) => left.evaluate(values)? + right.evaluate(values)?,
            Self::Subtract(left, right) => left.evaluate(values)? - right.evaluate(values)?,
            Self::Multiply(left, right) => left.evaluate(values)? * right.evaluate(values)?,
            Self::Divide(left, right) => {
                let denominator = right.evaluate(values)?;
                if denominator.abs() <= f64::EPSILON {
                    return Err("constraint expression division by zero".into());
                }
                left.evaluate(values)? / denominator
            }
            Self::Power(left, right) => left.evaluate(values)?.powf(right.evaluate(values)?),
            Self::Function(function, arguments) => {
                let arguments = arguments
                    .iter()
                    .map(|argument| argument.evaluate(values))
                    .collect::<Result<Vec<_>, _>>()?;
                match function {
                    ConstraintFunction::Abs => arguments[0].abs(),
                    ConstraintFunction::Sqrt => {
                        if arguments[0] < 0.0 {
                            return Err("constraint sqrt domain is negative".into());
                        }
                        arguments[0].sqrt()
                    }
                    ConstraintFunction::Exp => arguments[0].exp(),
                    ConstraintFunction::Ln => {
                        if arguments[0] <= 0.0 {
                            return Err("constraint ln domain is non-positive".into());
                        }
                        arguments[0].ln()
                    }
                    ConstraintFunction::Log10 => {
                        if arguments[0] <= 0.0 {
                            return Err("constraint log10 domain is non-positive".into());
                        }
                        arguments[0].log10()
                    }
                    ConstraintFunction::Min => arguments.into_iter().fold(f64::INFINITY, f64::min),
                    ConstraintFunction::Max => {
                        arguments.into_iter().fold(f64::NEG_INFINITY, f64::max)
                    }
                }
            }
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err("constraint expression evaluated to a non-finite value".into())
        }
    }

    pub fn nodes(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_nodes(&mut out);
        out
    }

    fn collect_nodes(&self, out: &mut Vec<String>) {
        match self {
            Self::Constant(_) => {}
            Self::Node(node) => {
                if !out.iter().any(|other| other.eq_ignore_ascii_case(node)) {
                    out.push(node.clone());
                }
            }
            Self::Add(left, right)
            | Self::Subtract(left, right)
            | Self::Multiply(left, right)
            | Self::Divide(left, right)
            | Self::Power(left, right) => {
                left.collect_nodes(out);
                right.collect_nodes(out);
            }
            Self::Function(_, arguments) => {
                for argument in arguments {
                    argument.collect_nodes(out);
                }
            }
        }
    }

    pub fn format(&self) -> String {
        match self {
            Self::Constant(value) => crate::parser::format_number(*value),
            Self::Node(node) => format!("V({node})"),
            Self::Add(left, right) => {
                format!("({}+{})", left.format(), right.format())
            }
            Self::Subtract(left, right) => {
                format!("({}-{})", left.format(), right.format())
            }
            Self::Multiply(left, right) => {
                format!("({}*{})", left.format(), right.format())
            }
            Self::Divide(left, right) => {
                format!("({}/{})", left.format(), right.format())
            }
            Self::Power(left, right) => {
                format!("({}^{})", left.format(), right.format())
            }
            Self::Function(function, arguments) => {
                let arguments = arguments
                    .iter()
                    .map(|argument| argument.format())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{}({})", function.name(), arguments)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParetoConstraint {
    pub node: String,
    pub sense: ConstraintSense,
    pub bound: f64,
    pub expression: Option<ConstraintExpression>,
}

impl ParetoConstraint {
    pub fn variable_names(&self) -> Vec<String> {
        match &self.expression {
            Some(expression) => expression.nodes(),
            None => vec![self.node.clone()],
        }
    }

    pub fn evaluate(&self, values: &BTreeMap<String, f64>) -> Result<f64, String> {
        match &self.expression {
            Some(expression) => expression.evaluate(values),
            None => values
                .get(&self.node)
                .copied()
                .ok_or_else(|| format!("constraint has unknown node '{}'", self.node)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParetoOptions {
    pub target: MonteCarloTarget,
    pub output_nodes: Vec<String>,
    pub senses: Vec<ParetoSense>,
    pub frequency_hz: f64,
    pub parameters: Vec<OptimizationParameter>,
    pub population: usize,
    pub iterations: usize,
    pub seed: u64,
    pub differential_weight: f64,
    pub crossover_probability: f64,
    pub maximum_points: usize,
    pub front_method: ParetoFrontMethod,
    pub reference_divisions: usize,
    pub adaptive_references: bool,
    pub constraints: Vec<ParetoConstraint>,
}

impl Default for ParetoOptions {
    fn default() -> Self {
        Self {
            target: MonteCarloTarget::OperatingPoint,
            output_nodes: vec!["out".into(), "power".into()],
            senses: vec![ParetoSense::Minimize, ParetoSense::Minimize],
            frequency_hz: 1.0,
            parameters: Vec::new(),
            population: 32,
            iterations: 64,
            seed: 1,
            differential_weight: 0.7,
            crossover_probability: 0.85,
            maximum_points: 128,
            front_method: ParetoFrontMethod::CrowdingDistance,
            reference_divisions: 4,
            adaptive_references: false,
            constraints: Vec::new(),
        }
    }
}

impl ParetoOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_nodes.len() < 2 || self.output_nodes.len() > 8 {
            return Err("Pareto optimization needs between 2 and 8 output nodes".into());
        }
        if self.senses.len() != self.output_nodes.len() {
            return Err("Pareto optimization needs one sense per output".into());
        }
        if self.output_nodes.iter().any(|node| node.trim().is_empty()) {
            return Err("Pareto optimization output node cannot be empty".into());
        }
        for (index, node) in self.output_nodes.iter().enumerate() {
            if self
                .output_nodes
                .iter()
                .skip(index + 1)
                .any(|other| other.eq_ignore_ascii_case(node))
            {
                return Err("Pareto optimization output nodes must be distinct".into());
            }
        }
        ensure_finite_positive(self.frequency_hz, "Pareto optimization frequency")?;
        if self.parameters.is_empty() {
            return Err("Pareto optimization needs at least one parameter".into());
        }
        if self.parameters.len() > 16 {
            return Err("Pareto optimization supports at most 16 parameters".into());
        }
        for parameter in &self.parameters {
            if parameter.parameter.trim().is_empty() {
                return Err("Pareto optimization parameter name cannot be empty".into());
            }
            ensure_finite(parameter.lower, "Pareto optimization lower bound")?;
            ensure_finite(parameter.upper, "Pareto optimization upper bound")?;
            if parameter.lower >= parameter.upper {
                return Err(format!(
                    "Pareto optimization parameter '{}' must have lower < upper",
                    parameter.parameter
                ));
            }
            if let Some(initial) = parameter.initial {
                ensure_finite(initial, "Pareto optimization initial value")?;
                if !(parameter.lower..=parameter.upper).contains(&initial) {
                    return Err(format!(
                        "Pareto optimization initial value for '{}' is outside its bounds",
                        parameter.parameter
                    ));
                }
            }
            if let Some(quantum) = parameter.quantum {
                ensure_finite_positive(quantum, "Pareto optimization quantum")?;
                let span = parameter.upper - parameter.lower;
                if quantum > span + span.abs() * 1e-12 {
                    return Err(format!(
                        "Pareto optimization quantum for '{}' cannot exceed its bounds",
                        parameter.parameter
                    ));
                }
                if let Some(initial) = parameter.initial {
                    let steps = ((initial - parameter.lower) / quantum).round();
                    let snapped = parameter.lower + steps * quantum;
                    let tolerance = quantum * 1e-9;
                    if (initial - snapped).abs() > tolerance
                        || snapped < parameter.lower - tolerance
                        || snapped > parameter.upper + tolerance
                    {
                        return Err(format!(
                            "Pareto optimization initial value for '{}' is outside its quantum grid",
                            parameter.parameter
                        ));
                    }
                }
            }
        }
        for (index, parameter) in self.parameters.iter().enumerate() {
            if self
                .parameters
                .iter()
                .skip(index + 1)
                .any(|other| other.parameter.eq_ignore_ascii_case(&parameter.parameter))
            {
                return Err(format!(
                    "duplicate Pareto optimization parameter '{}'",
                    parameter.parameter
                ));
            }
        }
        if self.population < 10 || self.population > 512 {
            return Err("Pareto optimization population must be between 10 and 512".into());
        }
        if self.iterations == 0 || self.iterations > 10_000 {
            return Err("Pareto optimization iterations must be between 1 and 10000".into());
        }
        ensure_finite_positive(
            self.differential_weight,
            "Pareto optimization differential weight",
        )?;
        if self.differential_weight > 2.0 {
            return Err("Pareto optimization differential weight cannot exceed 2.0".into());
        }
        ensure_finite_nonnegative(
            self.crossover_probability,
            "Pareto optimization crossover probability",
        )?;
        if self.crossover_probability > 1.0 {
            return Err("Pareto optimization crossover probability cannot exceed 1.0".into());
        }
        if self.maximum_points < 2 || self.maximum_points > 1024 {
            return Err("Pareto optimization point limit must be between 2 and 1024".into());
        }
        if self.reference_divisions == 0 || self.reference_divisions > 20 {
            return Err("Pareto reference divisions must be between 1 and 20".into());
        }
        let combinations =
            reference_direction_count(self.output_nodes.len(), self.reference_divisions)
                .ok_or_else(|| "Pareto reference direction count overflowed".to_string())?;
        if combinations > 4096 {
            return Err("Pareto reference direction set would exceed 4096 directions".into());
        }
        for constraint in &self.constraints {
            if constraint.expression.is_none() && constraint.node.trim().is_empty() {
                return Err("Pareto optimization constraint node cannot be empty".into());
            }
            ensure_finite(constraint.bound, "Pareto optimization constraint bound")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonteCarloTarget {
    OperatingPoint,
    TransientFinal,
    AcMagnitude,
}

impl MonteCarloTarget {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "op" | "dc" => Some(Self::OperatingPoint),
            "tran" | "transient" | "final" => Some(Self::TransientFinal),
            "ac" | "gain" => Some(Self::AcMagnitude),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::OperatingPoint => "op",
            Self::TransientFinal => "tran",
            Self::AcMagnitude => "ac",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DerivativeOptions {
    pub target: DerivativeTarget,
    pub output_node: String,
    pub frequency_hz: f64,
    pub parameters: Vec<String>,
}

impl Default for DerivativeOptions {
    fn default() -> Self {
        Self {
            target: DerivativeTarget::Dc,
            output_node: "out".into(),
            frequency_hz: 1_000.0,
            parameters: Vec::new(),
        }
    }
}

impl DerivativeOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("derivative output node cannot be empty".into());
        }
        if self.target == DerivativeTarget::AcMagnitude
            && (!self.frequency_hz.is_finite() || self.frequency_hz <= 0.0)
        {
            return Err("AC derivative frequency must be finite and positive".into());
        }
        if self.parameters.is_empty() {
            return Err("derivative analysis needs at least one parameter".into());
        }
        if self.parameters.len() > 16 {
            return Err("derivative analysis supports at most 16 parameters".into());
        }
        for parameter in &self.parameters {
            if parameter.trim().is_empty() {
                return Err("derivative parameter cannot be empty".into());
            }
        }
        for (index, parameter) in self.parameters.iter().enumerate() {
            if self
                .parameters
                .iter()
                .skip(index + 1)
                .any(|other| other.eq_ignore_ascii_case(parameter))
            {
                return Err(format!("duplicate derivative parameter '{parameter}'"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivativeTarget {
    Dc,
    AcMagnitude,
    TransientFinal,
}

impl DerivativeTarget {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "dc" | "op" => Some(Self::Dc),
            "ac" | "acmag" | "magnitude" => Some(Self::AcMagnitude),
            "tran" | "transient" | "transient-final" => Some(Self::TransientFinal),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dc => "dc",
            Self::AcMagnitude => "ac",
            Self::TransientFinal => "tran",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonteCarloDistribution {
    Uniform,
    Normal,
    Lognormal,
}

impl MonteCarloDistribution {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "uniform" | "unif" => Some(Self::Uniform),
            "normal" | "gaussian" | "gauss" => Some(Self::Normal),
            "lognormal" | "log-normal" | "lnormal" => Some(Self::Lognormal),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uniform => "uniform",
            Self::Normal => "normal",
            Self::Lognormal => "lognormal",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatisticalParameterModel {
    pub parameter: String,
    pub distribution: MonteCarloDistribution,
    pub relative_sigma: bool,
    pub sigma: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatisticalModel {
    pub name: String,
    pub parameters: Vec<StatisticalParameterModel>,
    pub correlations: Vec<ParameterCorrelation>,
}

impl StatisticalModel {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("statistical model name cannot be empty".into());
        }
        if self.parameters.is_empty() {
            return Err(format!(
                "statistical model '{}' needs at least one parameter",
                self.name
            ));
        }
        for parameter in &self.parameters {
            if parameter.parameter.trim().is_empty() {
                return Err(format!(
                    "statistical model '{}' has an empty parameter name",
                    self.name
                ));
            }
            ensure_finite_positive(parameter.sigma, "statistical model parameter sigma")?;
            if matches!(parameter.distribution, MonteCarloDistribution::Lognormal)
                && !parameter.relative_sigma
            {
                return Err(format!(
                    "statistical model lognormal parameter '{}' requires a relative sigma",
                    parameter.parameter
                ));
            }
        }
        for (index, parameter) in self.parameters.iter().enumerate() {
            if self
                .parameters
                .iter()
                .skip(index + 1)
                .any(|other| other.parameter.eq_ignore_ascii_case(&parameter.parameter))
            {
                return Err(format!(
                    "duplicate statistical model parameter '{}'",
                    parameter.parameter
                ));
            }
        }
        for correlation in &self.correlations {
            if correlation.left.trim().is_empty() || correlation.right.trim().is_empty() {
                return Err("statistical model correlation parameter cannot be empty".into());
            }
            if !correlation.coefficient.is_finite()
                || correlation.coefficient < -1.0
                || correlation.coefficient > 1.0
            {
                return Err(format!(
                    "statistical model correlation between '{}' and '{}' must be between -1 and 1",
                    correlation.left, correlation.right
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonteCarloOptions {
    pub runs: usize,
    pub seed: u64,
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub frequency_hz: f64,
    pub resistor_tolerance: f64,
    pub capacitor_tolerance: f64,
    pub inductor_tolerance: f64,
    pub diode_is_decades: f64,
    pub mos_vto_tolerance: f64,
    pub mos_kp_tolerance: f64,
    pub bjt_beta_tolerance: f64,
    pub bjt_is_decades: f64,
    pub resistor_correlation: f64,
    pub capacitor_correlation: f64,
    pub inductor_correlation: f64,
    pub diode_is_correlation: f64,
    pub mos_vto_correlation: f64,
    pub mos_kp_correlation: f64,
    pub bjt_beta_correlation: f64,
    pub bjt_is_correlation: f64,
    pub correlations: Vec<ParameterCorrelation>,
    pub covariances: Vec<ParameterCovariance>,
    pub spatial_correlation: f64,
    pub spatial_correlation_length_m: f64,
    pub distribution: MonteCarloDistribution,
    pub statistical_model: Option<String>,
}

impl Default for MonteCarloOptions {
    fn default() -> Self {
        Self {
            runs: 100,
            seed: 1,
            target: MonteCarloTarget::OperatingPoint,
            output_node: "out".into(),
            frequency_hz: 1.0,
            resistor_tolerance: 0.10,
            capacitor_tolerance: 0.10,
            inductor_tolerance: 0.10,
            diode_is_decades: 0.0,
            mos_vto_tolerance: 0.02,
            mos_kp_tolerance: 0.05,
            bjt_beta_tolerance: 0.05,
            bjt_is_decades: 0.0,
            resistor_correlation: 0.0,
            capacitor_correlation: 0.0,
            inductor_correlation: 0.0,
            diode_is_correlation: 0.0,
            mos_vto_correlation: 0.0,
            mos_kp_correlation: 0.0,
            bjt_beta_correlation: 0.0,
            bjt_is_correlation: 0.0,
            correlations: Vec::new(),
            covariances: Vec::new(),
            spatial_correlation: 0.0,
            spatial_correlation_length_m: 1.0,
            distribution: MonteCarloDistribution::Uniform,
            statistical_model: None,
        }
    }
}

impl MonteCarloOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.runs == 0 || self.runs > 1_000_000 {
            return Err("Monte Carlo runs must be between 1 and 1000000".into());
        }
        if self.output_node.trim().is_empty() {
            return Err("Monte Carlo output node cannot be empty".into());
        }
        ensure_finite_positive(self.frequency_hz, "Monte Carlo frequency")?;
        for (value, name) in [
            (self.resistor_tolerance, "Monte Carlo resistor tolerance"),
            (self.capacitor_tolerance, "Monte Carlo capacitor tolerance"),
            (self.inductor_tolerance, "Monte Carlo inductor tolerance"),
            (self.mos_vto_tolerance, "Monte Carlo MOS VTO tolerance"),
            (self.mos_kp_tolerance, "Monte Carlo MOS KP tolerance"),
            (self.bjt_beta_tolerance, "Monte Carlo BJT BF tolerance"),
            (
                self.resistor_correlation,
                "Monte Carlo resistor correlation",
            ),
            (
                self.capacitor_correlation,
                "Monte Carlo capacitor correlation",
            ),
            (
                self.inductor_correlation,
                "Monte Carlo inductor correlation",
            ),
            (
                self.diode_is_correlation,
                "Monte Carlo diode IS correlation",
            ),
            (self.mos_vto_correlation, "Monte Carlo MOS VTO correlation"),
            (self.mos_kp_correlation, "Monte Carlo MOS KP correlation"),
            (self.bjt_beta_correlation, "Monte Carlo BJT BF correlation"),
            (self.bjt_is_correlation, "Monte Carlo BJT IS correlation"),
        ] {
            ensure_finite_nonnegative(value, name)?;
            if value > 1.0 {
                return Err(format!("{} cannot exceed 1.0", name));
            }
        }
        for (value, name) in [
            (self.diode_is_decades, "Monte Carlo diode IS decades"),
            (self.bjt_is_decades, "Monte Carlo BJT IS decades"),
        ] {
            ensure_finite_nonnegative(value, name)?;
            if value > 10.0 {
                return Err(format!("{} cannot exceed 10", name));
            }
        }
        for correlation in &self.correlations {
            if correlation.left.trim().is_empty() || correlation.right.trim().is_empty() {
                return Err("Monte Carlo correlation parameter cannot be empty".into());
            }
            if !correlation.coefficient.is_finite()
                || correlation.coefficient < -1.0
                || correlation.coefficient > 1.0
            {
                return Err(format!(
                    "Monte Carlo correlation between '{}' and '{}' must be between -1 and 1",
                    correlation.left, correlation.right
                ));
            }
        }
        if !self.covariances.is_empty()
            && matches!(self.distribution, MonteCarloDistribution::Uniform)
        {
            return Err(
                "Monte Carlo covariance input requires dist=normal or dist=lognormal".into(),
            );
        }
        for covariance in &self.covariances {
            if covariance.left.trim().is_empty() || covariance.right.trim().is_empty() {
                return Err("Monte Carlo covariance parameter cannot be empty".into());
            }
            ensure_finite(covariance.covariance, "Monte Carlo covariance")?;
            if covariance.left.eq_ignore_ascii_case(&covariance.right)
                && covariance.covariance < 0.0
            {
                return Err("Monte Carlo diagonal covariance cannot be negative".into());
            }
        }
        ensure_finite_nonnegative(self.spatial_correlation, "Monte Carlo spatial correlation")?;
        if self.spatial_correlation >= 1.0 {
            return Err("Monte Carlo spatial correlation cannot reach 1.0".into());
        }
        ensure_finite_positive(
            self.spatial_correlation_length_m,
            "Monte Carlo spatial correlation length",
        )?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensitivityOptions {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub frequency_hz: f64,
    pub relative_step: f64,
}

impl Default for SensitivityOptions {
    fn default() -> Self {
        Self {
            target: MonteCarloTarget::OperatingPoint,
            output_node: "out".into(),
            frequency_hz: 1.0,
            relative_step: 0.001,
        }
    }
}

impl SensitivityOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("sensitivity output node cannot be empty".into());
        }
        ensure_finite_positive(self.frequency_hz, "sensitivity frequency")?;
        ensure_finite_positive(self.relative_step, "sensitivity relative step")?;
        if self.relative_step > 0.1 {
            return Err("sensitivity relative step cannot exceed 0.1".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlobalSensitivityOptions {
    pub target: MonteCarloTarget,
    pub output_node: String,
    pub frequency_hz: f64,
    pub samples: usize,
    pub seed: u64,
    pub tolerance: f64,
    pub correlations: Vec<ParameterCorrelation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterCorrelation {
    pub left: String,
    pub right: String,
    pub coefficient: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterCovariance {
    pub left: String,
    pub right: String,
    pub covariance: f64,
}

impl Default for GlobalSensitivityOptions {
    fn default() -> Self {
        Self {
            target: MonteCarloTarget::OperatingPoint,
            output_node: "out".into(),
            frequency_hz: 1.0,
            samples: 256,
            seed: 1,
            tolerance: 0.20,
            correlations: Vec::new(),
        }
    }
}

impl GlobalSensitivityOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.output_node.trim().is_empty() {
            return Err("global sensitivity output node cannot be empty".into());
        }
        ensure_finite_positive(self.frequency_hz, "global sensitivity frequency")?;
        if self.samples < 16 || self.samples > 100_000 {
            return Err("global sensitivity samples must be between 16 and 100000".into());
        }
        ensure_finite_positive(self.tolerance, "global sensitivity tolerance")?;
        if self.tolerance > 1.0 {
            return Err("global sensitivity tolerance cannot exceed 1.0".into());
        }
        for correlation in &self.correlations {
            if correlation.left.trim().is_empty() || correlation.right.trim().is_empty() {
                return Err("global sensitivity correlation parameter cannot be empty".into());
            }
            if !correlation.coefficient.is_finite()
                || correlation.coefficient < -1.0
                || correlation.coefficient > 1.0
            {
                return Err(format!(
                    "global sensitivity correlation between '{}' and '{}' must be between -1 and 1",
                    correlation.left, correlation.right
                ));
            }
        }
        Ok(())
    }
}
