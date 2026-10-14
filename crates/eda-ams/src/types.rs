//! Shared SPICE circuit and result types.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

pub const GROUND_ALIASES: [&str; 3] = ["0", "gnd", "GND"];
pub const SPICE_NOMINAL_TEMPERATURE_K: f64 = 300.0;

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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiodeModel {
    pub saturation_current_a: f64,
    pub emission_coefficient: f64,
    pub series_resistance_ohm: f64,
    pub zero_bias_junction_cap_f: f64,
    pub junction_potential_v: f64,
    pub grading_coefficient: f64,
    pub forward_knee: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub energy_gap_ev: f64,
    pub saturation_current_temperature_exponent: f64,
    pub transit_time_s: f64,
}

impl DiodeModel {
    pub fn validate(&self) -> Result<(), String> {
        if !self.saturation_current_a.is_finite() || self.saturation_current_a <= 0.0 {
            return Err("diode IS must be finite and positive".into());
        }
        if !self.emission_coefficient.is_finite() || self.emission_coefficient <= 0.0 {
            return Err("diode N must be finite and positive".into());
        }
        if !self.series_resistance_ohm.is_finite() || self.series_resistance_ohm < 0.0 {
            return Err("diode RS must be finite and nonnegative".into());
        }
        ensure_finite_nonnegative(self.zero_bias_junction_cap_f, "diode CJO")?;
        ensure_finite_positive(self.junction_potential_v, "diode VJ")?;
        ensure_finite_nonnegative(self.grading_coefficient, "diode M")?;
        if self.grading_coefficient >= 1.0 {
            return Err("diode M must be below 1.0".into());
        }
        ensure_finite_nonnegative(self.forward_knee, "diode FC")?;
        if self.forward_knee >= 0.999 {
            return Err("diode FC must be below 0.999".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "diode KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "diode AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "diode EF")?;
        ensure_finite_positive(self.energy_gap_ev, "diode EG")?;
        ensure_finite_nonnegative(self.saturation_current_temperature_exponent, "diode XTI")?;
        ensure_finite_nonnegative(self.transit_time_s, "diode TT")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MosfetPolarity {
    Nmos,
    Pmos,
}

impl MosfetPolarity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "nmos" | "n" => Some(Self::Nmos),
            "pmos" | "p" => Some(Self::Pmos),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MosfetModelKind {
    Level1,
    Bsim3LongChannel,
}

impl MosfetModelKind {
    pub fn from_level(value: f64) -> Option<Self> {
        match value {
            1.0 => Some(Self::Level1),
            8.0 => Some(Self::Bsim3LongChannel),
            _ => None,
        }
    }

    pub fn as_level(self) -> f64 {
        match self {
            Self::Level1 => 1.0,
            Self::Bsim3LongChannel => 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MosfetModel {
    pub polarity: MosfetPolarity,
    pub kind: MosfetModelKind,
    pub transconductance_a_per_v2: f64,
    pub threshold_voltage_v: f64,
    pub channel_length_modulation_per_v: f64,
    pub body_effect_v_per_sqrt_v: f64,
    pub surface_potential_v: f64,
    pub mobility_degradation_inverse_v: f64,
    pub velocity_saturation_voltage_v: f64,
    pub gate_source_overlap_cap_per_m: f64,
    pub gate_drain_overlap_cap_per_m: f64,
    pub gate_bulk_cap_per_m2: f64,
    pub gate_oxide_cap_per_m2: f64,
    pub source_junction_sidewall_cap_per_m: f64,
    pub drain_junction_sidewall_cap_per_m: f64,
    pub junction_potential_v: f64,
    pub junction_grading_coefficient: f64,
    pub junction_forward_knee: f64,
    pub saturation_charge_partition: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub mobility_temperature_exponent: f64,
    pub bsim3_body_bias_threshold_coefficient: f64,
    pub bsim3_low_field_mobility_m2_per_v_s: f64,
    pub bsim3_mobility_degradation_first_order_m_per_v: f64,
    pub bsim3_mobility_degradation_second_order_m2_per_v2: f64,
    pub bsim3_mobility_body_bias_coefficient_m2_per_v2: f64,
    pub bsim3_oxide_thickness_m: f64,
    pub bsim3_saturation_velocity_m_per_s: f64,
    pub bsim3_short_channel_threshold_amplitude: f64,
    pub bsim3_short_channel_length_scale_m: f64,
    pub bsim3_short_channel_drain_bias_coefficient_per_v: f64,
    pub bsim3_narrow_width_threshold_amplitude_v: f64,
    pub bsim3_narrow_width_exponent: f64,
    pub bsim3_reference_width_m: f64,
    pub bsim3_threshold_temperature_coefficient_v: f64,
    pub bsim3_threshold_body_temperature_coefficient_v_per_v: f64,
    pub bsim3_threshold_length_temperature_coefficient_v_m: f64,
    pub geometry_bin: Option<MosfetGeometryBin>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MosfetGeometryBin {
    pub min_width_m: f64,
    pub max_width_m: f64,
    pub min_length_m: f64,
    pub max_length_m: f64,
}

impl MosfetGeometryBin {
    fn validate(&self) -> Result<(), String> {
        for (value, name) in [
            (self.min_width_m, "MOS WMIN"),
            (self.max_width_m, "MOS WMAX"),
            (self.min_length_m, "MOS LMIN"),
            (self.max_length_m, "MOS LMAX"),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{name} must be finite and positive"));
            }
        }
        if self.min_width_m > self.max_width_m {
            return Err("MOS WMIN cannot exceed WMAX".into());
        }
        if self.min_length_m > self.max_length_m {
            return Err("MOS LMIN cannot exceed LMAX".into());
        }
        Ok(())
    }

    fn contains(&self, width_m: f64, length_m: f64) -> bool {
        (self.min_width_m..=self.max_width_m).contains(&width_m)
            && (self.min_length_m..=self.max_length_m).contains(&length_m)
    }
}

impl Default for MosfetModel {
    fn default() -> Self {
        Self {
            polarity: MosfetPolarity::Nmos,
            kind: MosfetModelKind::Level1,
            transconductance_a_per_v2: 1e-4,
            threshold_voltage_v: 0.45,
            channel_length_modulation_per_v: 0.0,
            body_effect_v_per_sqrt_v: 0.0,
            surface_potential_v: 0.7,
            mobility_degradation_inverse_v: 0.0,
            velocity_saturation_voltage_v: 0.0,
            gate_source_overlap_cap_per_m: 0.0,
            gate_drain_overlap_cap_per_m: 0.0,
            gate_bulk_cap_per_m2: 0.0,
            gate_oxide_cap_per_m2: 0.0,
            source_junction_sidewall_cap_per_m: 0.0,
            drain_junction_sidewall_cap_per_m: 0.0,
            junction_potential_v: 0.8,
            junction_grading_coefficient: 0.5,
            junction_forward_knee: 0.5,
            saturation_charge_partition: 0.0,
            flicker_coefficient_a2: 0.0,
            flicker_current_exponent: 1.0,
            flicker_frequency_exponent: 1.0,
            mobility_temperature_exponent: 1.5,
            bsim3_body_bias_threshold_coefficient: 0.0,
            bsim3_low_field_mobility_m2_per_v_s: 0.045,
            bsim3_mobility_degradation_first_order_m_per_v: 1.0e-15,
            bsim3_mobility_degradation_second_order_m2_per_v2: 1.0e-24,
            bsim3_mobility_body_bias_coefficient_m2_per_v2: 0.0,
            bsim3_oxide_thickness_m: 2.0e-9,
            bsim3_saturation_velocity_m_per_s: 80_000.0,
            bsim3_short_channel_threshold_amplitude: 0.0,
            bsim3_short_channel_length_scale_m: 30.0e-9,
            bsim3_short_channel_drain_bias_coefficient_per_v: 0.0,
            bsim3_narrow_width_threshold_amplitude_v: 0.0,
            bsim3_narrow_width_exponent: 0.3,
            bsim3_reference_width_m: 250.0e-9,
            bsim3_threshold_temperature_coefficient_v: -0.16,
            bsim3_threshold_body_temperature_coefficient_v_per_v: -0.03,
            bsim3_threshold_length_temperature_coefficient_v_m: 0.0,
            geometry_bin: None,
        }
    }
}

impl MosfetModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.transconductance_a_per_v2, "MOS KP")?;
        ensure_finite(self.threshold_voltage_v, "MOS VTO")?;
        ensure_finite_nonnegative(self.channel_length_modulation_per_v, "MOS LAMBDA")?;
        ensure_finite_nonnegative(self.body_effect_v_per_sqrt_v, "MOS GAMMA")?;
        ensure_finite_positive(self.surface_potential_v, "MOS PHI")?;
        ensure_finite_nonnegative(self.mobility_degradation_inverse_v, "MOS THETA")?;
        ensure_finite_nonnegative(self.velocity_saturation_voltage_v, "MOS VSAT")?;
        ensure_finite_nonnegative(self.gate_source_overlap_cap_per_m, "MOS CGSO")?;
        ensure_finite_nonnegative(self.gate_drain_overlap_cap_per_m, "MOS CGDO")?;
        ensure_finite_nonnegative(self.gate_bulk_cap_per_m2, "MOS CGB")?;
        ensure_finite_nonnegative(self.gate_oxide_cap_per_m2, "MOS COX")?;
        ensure_finite_nonnegative(self.source_junction_sidewall_cap_per_m, "MOS CJSW/CJSWS")?;
        ensure_finite_nonnegative(self.drain_junction_sidewall_cap_per_m, "MOS CJSW/CJSWD")?;
        ensure_finite_positive(self.junction_potential_v, "MOS PB")?;
        if !self.junction_grading_coefficient.is_finite()
            || !(0.0..=1.0).contains(&self.junction_grading_coefficient)
        {
            return Err("MOS MJ must be finite and between zero and one".into());
        }
        if !self.junction_forward_knee.is_finite()
            || self.junction_forward_knee < 0.0
            || self.junction_forward_knee >= 1.0
        {
            return Err("MOS FC must be finite and between zero and one".into());
        }
        ensure_finite_nonnegative(
            self.saturation_charge_partition,
            "MOS saturation charge partition",
        )?;
        if self.saturation_charge_partition > 1.0 {
            return Err("MOS saturation charge partition cannot exceed 1.0".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "MOS KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "MOS AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "MOS EF")?;
        ensure_finite_nonnegative(self.mobility_temperature_exponent, "MOS MEXP")?;
        ensure_finite_nonnegative(self.bsim3_body_bias_threshold_coefficient, "BSIM3 K2")?;
        ensure_finite_positive(self.bsim3_low_field_mobility_m2_per_v_s, "BSIM3 U0")?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_degradation_first_order_m_per_v,
            "BSIM3 UA",
        )?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_degradation_second_order_m2_per_v2,
            "BSIM3 UB",
        )?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_body_bias_coefficient_m2_per_v2,
            "BSIM3 UC",
        )?;
        ensure_finite_positive(self.bsim3_oxide_thickness_m, "BSIM3 TOX")?;
        if self.kind == MosfetModelKind::Bsim3LongChannel {
            ensure_finite_positive(self.bsim3_saturation_velocity_m_per_s, "BSIM3 VSAT")?;
        } else if !self.bsim3_saturation_velocity_m_per_s.is_finite()
            || self.bsim3_saturation_velocity_m_per_s < 0.0
        {
            return Err("BSIM3 VSAT must be finite and non-negative".into());
        }
        ensure_finite_nonnegative(self.bsim3_short_channel_threshold_amplitude, "BSIM3 DVT0")?;
        ensure_finite_positive(self.bsim3_short_channel_length_scale_m, "BSIM3 DVT1")?;
        ensure_finite_nonnegative(
            self.bsim3_short_channel_drain_bias_coefficient_per_v,
            "BSIM3 DVT2",
        )?;
        ensure_finite_nonnegative(self.bsim3_narrow_width_threshold_amplitude_v, "BSIM3 DVT0W")?;
        ensure_finite_nonnegative(self.bsim3_narrow_width_exponent, "BSIM3 DVT1W")?;
        ensure_finite_positive(self.bsim3_reference_width_m, "BSIM3 W0")?;
        ensure_finite(self.bsim3_threshold_temperature_coefficient_v, "BSIM3 KT1")?;
        ensure_finite(
            self.bsim3_threshold_body_temperature_coefficient_v_per_v,
            "BSIM3 KT2",
        )?;
        ensure_finite(
            self.bsim3_threshold_length_temperature_coefficient_v_m,
            "BSIM3 KT1L",
        )?;
        if let Some(bin) = self.geometry_bin {
            bin.validate()?;
        }
        if self.kind == MosfetModelKind::Bsim3LongChannel
            && self.velocity_saturation_voltage_v != 0.0
        {
            return Err(
                "BSIM3 uses physical VSAT in m/s; do not set legacy voltage-domain VSAT".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BjtPolarity {
    Npn,
    Pnp,
}

impl BjtPolarity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "npn" | "n" => Some(Self::Npn),
            "pnp" | "p" => Some(Self::Pnp),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BjtModel {
    pub polarity: BjtPolarity,
    pub saturation_current_a: f64,
    pub forward_beta: f64,
    pub reverse_beta: f64,
    pub forward_early_voltage_v: f64,
    pub reverse_early_voltage_v: f64,
    pub base_emitter_zero_bias_cap_f: f64,
    pub base_collector_zero_bias_cap_f: f64,
    pub base_emitter_junction_potential_v: f64,
    pub base_collector_junction_potential_v: f64,
    pub base_emitter_grading_coefficient: f64,
    pub base_collector_grading_coefficient: f64,
    pub junction_forward_knee: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub energy_gap_ev: f64,
    pub saturation_current_temperature_exponent: f64,
    pub forward_transit_time_s: f64,
    pub reverse_transit_time_s: f64,
}

impl Default for BjtModel {
    fn default() -> Self {
        Self {
            polarity: BjtPolarity::Npn,
            saturation_current_a: 1e-14,
            forward_beta: 100.0,
            reverse_beta: 1.0,
            forward_early_voltage_v: 0.0,
            reverse_early_voltage_v: 0.0,
            base_emitter_zero_bias_cap_f: 0.0,
            base_collector_zero_bias_cap_f: 0.0,
            base_emitter_junction_potential_v: 0.7,
            base_collector_junction_potential_v: 0.7,
            base_emitter_grading_coefficient: 0.33,
            base_collector_grading_coefficient: 0.33,
            junction_forward_knee: 0.5,
            flicker_coefficient_a2: 0.0,
            flicker_current_exponent: 1.0,
            flicker_frequency_exponent: 1.0,
            energy_gap_ev: 1.12,
            saturation_current_temperature_exponent: 3.0,
            forward_transit_time_s: 0.0,
            reverse_transit_time_s: 0.0,
        }
    }
}

impl BjtModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.saturation_current_a, "BJT IS")?;
        ensure_finite_positive(self.forward_beta, "BJT BF")?;
        ensure_finite_positive(self.reverse_beta, "BJT BR")?;
        ensure_finite_nonnegative(self.forward_early_voltage_v, "BJT VAF")?;
        ensure_finite_nonnegative(self.reverse_early_voltage_v, "BJT VAR")?;
        ensure_finite_nonnegative(self.base_emitter_zero_bias_cap_f, "BJT CJE")?;
        ensure_finite_nonnegative(self.base_collector_zero_bias_cap_f, "BJT CJC")?;
        ensure_finite_positive(self.base_emitter_junction_potential_v, "BJT VJE")?;
        ensure_finite_positive(self.base_collector_junction_potential_v, "BJT VJC")?;
        ensure_finite_nonnegative(self.base_emitter_grading_coefficient, "BJT MJE")?;
        ensure_finite_nonnegative(self.base_collector_grading_coefficient, "BJT MJC")?;
        if self.base_emitter_grading_coefficient >= 1.0
            || self.base_collector_grading_coefficient >= 1.0
        {
            return Err("BJT grading coefficients must be below 1.0".into());
        }
        ensure_finite_nonnegative(self.junction_forward_knee, "BJT FC")?;
        if self.junction_forward_knee >= 0.999 {
            return Err("BJT FC must be below 0.999".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "BJT KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "BJT AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "BJT EF")?;
        ensure_finite_positive(self.energy_gap_ev, "BJT EG")?;
        ensure_finite_nonnegative(self.saturation_current_temperature_exponent, "BJT XTI")?;
        ensure_finite_nonnegative(self.forward_transit_time_s, "BJT TF")?;
        ensure_finite_nonnegative(self.reverse_transit_time_s, "BJT TR")?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpiceCircuit {
    pub title: String,
    pub temperature_k: f64,
    pub elements: Vec<SpiceElement>,
    pub analyses: Vec<Analysis>,
    pub diode_models: BTreeMap<String, DiodeModel>,
    pub mosfet_models: BTreeMap<String, MosfetModel>,
    pub bjt_models: BTreeMap<String, BjtModel>,
    pub locations: BTreeMap<String, ElementLocation>,
    pub statistical_models: BTreeMap<String, StatisticalModel>,
    pub resistor_layer_models: BTreeMap<String, ResistorLayerModel>,
}

impl Default for SpiceCircuit {
    fn default() -> Self {
        Self {
            title: String::new(),
            temperature_k: 300.0,
            elements: Vec::new(),
            analyses: Vec::new(),
            diode_models: BTreeMap::new(),
            mosfet_models: BTreeMap::new(),
            bjt_models: BTreeMap::new(),
            locations: BTreeMap::new(),
            statistical_models: BTreeMap::new(),
            resistor_layer_models: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementLocation {
    pub x_m: f64,
    pub y_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResistorLayerModel {
    pub sheet_resistance_ohm_per_square: f64,
    pub first_temperature_coefficient_per_k: f64,
    pub second_temperature_coefficient_per_k2: f64,
    pub width_offset_m: f64,
    pub minimum_width_m: f64,
    pub maximum_width_m: f64,
    pub width_bins: Vec<ResistorWidthBin>,
}

impl ResistorLayerModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.sheet_resistance_ohm_per_square, "resistor layer SHEET")?;
        ensure_finite(
            self.first_temperature_coefficient_per_k,
            "resistor layer TC1",
        )?;
        ensure_finite(
            self.second_temperature_coefficient_per_k2,
            "resistor layer TC2",
        )?;
        ensure_finite(self.width_offset_m, "resistor layer WOFF")?;
        ensure_finite_nonnegative(self.minimum_width_m, "resistor layer MINW")?;
        ensure_finite_nonnegative(self.maximum_width_m, "resistor layer MAXW")?;
        if self.maximum_width_m > 0.0 && self.maximum_width_m < self.minimum_width_m {
            return Err("resistor layer MAXW cannot be below MINW".into());
        }
        for (index, bin) in self.width_bins.iter().enumerate() {
            ensure_finite_positive(bin.width_m, "resistor layer-bin width")?;
            ensure_finite_positive(
                bin.sheet_resistance_ohm_per_square,
                "resistor layer-bin SHEET",
            )?;
            ensure_finite(bin.width_offset_m, "resistor layer-bin WOFF")?;
            if index > 0 && bin.width_m <= self.width_bins[index - 1].width_m {
                return Err("resistor layer-bin widths must be strictly increasing".into());
            }
        }
        Ok(())
    }

    pub fn effective_sheet(&self, width_m: f64) -> Result<(f64, f64), String> {
        ensure_finite_positive(width_m, "resistor width")?;
        let Some(first) = self.width_bins.first() else {
            return Ok((self.sheet_resistance_ohm_per_square, self.width_offset_m));
        };
        let Some(last) = self.width_bins.last() else {
            return Ok((self.sheet_resistance_ohm_per_square, self.width_offset_m));
        };
        if width_m <= first.width_m {
            return Ok((first.sheet_resistance_ohm_per_square, first.width_offset_m));
        }
        if width_m >= last.width_m {
            return Ok((last.sheet_resistance_ohm_per_square, last.width_offset_m));
        }
        let upper = self.width_bins.partition_point(|bin| bin.width_m < width_m);
        let left = &self.width_bins[upper - 1];
        let right = &self.width_bins[upper];
        let span = right.width_m - left.width_m;
        if span <= 0.0 {
            return Err("resistor layer-bin widths must be strictly increasing".into());
        }
        let alpha = (width_m - left.width_m) / span;
        Ok((
            left.sheet_resistance_ohm_per_square
                + alpha
                    * (right.sheet_resistance_ohm_per_square
                        - left.sheet_resistance_ohm_per_square),
            left.width_offset_m + alpha * (right.width_offset_m - left.width_offset_m),
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResistorWidthBin {
    pub width_m: f64,
    pub sheet_resistance_ohm_per_square: f64,
    pub width_offset_m: f64,
}

impl SpiceCircuit {
    pub fn set_temperature_celsius(&mut self, celsius: f64) -> Result<(), String> {
        let kelvin = celsius + 273.15;
        if !kelvin.is_finite() || !(1.0..=1_000.0).contains(&kelvin) {
            return Err(format!("invalid circuit temperature {} C", celsius));
        }
        self.temperature_k = kelvin;
        Ok(())
    }
}

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

impl SpiceCircuit {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn add_element(&mut self, element: SpiceElement) -> Result<(), String> {
        if element.name.is_empty() {
            return Err("SPICE element name cannot be empty".into());
        }
        if self
            .elements
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&element.name))
        {
            return Err(format!("duplicate SPICE element '{}'", element.name));
        }
        self.elements.push(element);
        Ok(())
    }

    pub fn node_names(&self) -> Vec<String> {
        let mut nodes = std::collections::BTreeSet::new();
        for element in &self.elements {
            for node in &element.nodes {
                if !GROUND_ALIASES.contains(&node.as_str()) {
                    nodes.insert(node.clone());
                }
            }
        }
        nodes.into_iter().collect()
    }

    pub fn element(&self, name: &str) -> Option<&SpiceElement> {
        self.elements
            .iter()
            .find(|element| element.name.eq_ignore_ascii_case(name))
    }

    pub fn mosfet_model_for(
        &self,
        name: &str,
        width_m: f64,
        length_m: f64,
    ) -> Result<&MosfetModel, String> {
        if let Some(model) = self.mosfet_models.get(name) {
            if model.geometry_bin.is_none() {
                return Ok(model);
            }
        }
        let prefix = format!("{name}.");
        let mut matches = Vec::new();
        for (candidate_name, model) in &self.mosfet_models {
            let Some(candidate) = model.geometry_bin else {
                continue;
            };
            if candidate_name.starts_with(&prefix) && candidate.contains(width_m, length_m) {
                matches.push(candidate_name.clone());
            }
        }
        match matches.as_slice() {
            [selected] => Ok(&self.mosfet_models[selected]),
            [] => Err(format!(
                "no MOSFET model bin '{name}' covers W={width_m} L={length_m}"
            )),
            many => Err(format!(
                "MOSFET model '{name}' has {} overlapping bins: {}",
                many.len(),
                many.join(", ")
            )),
        }
    }

    fn has_sensitivity_parameter(&self, name: &str) -> bool {
        let Some((object, field)) = name.rsplit_once('.') else {
            return false;
        };
        let field = field.to_ascii_lowercase();
        if let Some(element) = self.element(object) {
            return match (&element.kind, field.as_str()) {
                (SpiceElementKind::Resistor { .. }, "r")
                | (SpiceElementKind::Capacitor { .. }, "c")
                | (SpiceElementKind::Inductor { .. }, "l") => true,
                (
                    SpiceElementKind::VoltageSource { waveform, .. }
                    | SpiceElementKind::CurrentSource { waveform, .. },
                    "dc",
                ) => matches!(waveform, SourceWaveform::Dc(_)),
                (SpiceElementKind::BipolarJunctionTransistor { .. }, "area") => true,
                (
                    SpiceElementKind::VoltageSource {
                        ac_magnitude: Some(_),
                        ..
                    }
                    | SpiceElementKind::CurrentSource {
                        ac_magnitude: Some(_),
                        ..
                    },
                    "ac",
                ) => true,
                _ => false,
            };
        }
        if field == "is"
            && self
                .diode_models
                .keys()
                .any(|model| model.eq_ignore_ascii_case(object))
        {
            return true;
        }
        let Some(model) = object.strip_prefix("model:") else {
            return false;
        };
        match field.as_str() {
            "is" => self.diode_models.contains_key(model) || self.bjt_models.contains_key(model),
            "vto" | "kp" => self.mosfet_models.contains_key(model),
            "vth0" | "k1" | "u0" | "tox" | "vsat" | "dvt0" | "dvt1" | "dvt2" | "dvt0w"
            | "dvt1w" | "w0" | "kt1" | "kt2" | "kt1l" => self
                .mosfet_models
                .get(model)
                .is_some_and(|model| model.kind == MosfetModelKind::Bsim3LongChannel),
            "bf" => self.bjt_models.contains_key(model),
            _ => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.temperature_k, "circuit temperature")?;
        if self.temperature_k > 1_000.0 {
            return Err("circuit temperature cannot exceed 1000 K".into());
        }
        if self.elements.is_empty() {
            return Err("SPICE circuit has no elements".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for element in &self.elements {
            if element.name.is_empty() || !names.insert(element.name.to_ascii_lowercase()) {
                return Err(format!("invalid or duplicate element '{}'", element.name));
            }
            let expected = match element.kind {
                SpiceElementKind::Resistor { .. }
                | SpiceElementKind::Capacitor { .. }
                | SpiceElementKind::Inductor { .. }
                | SpiceElementKind::VoltageSource { .. }
                | SpiceElementKind::CurrentSource { .. }
                | SpiceElementKind::Diode { .. } => 2,
                SpiceElementKind::Mosfet { .. } => 4,
                SpiceElementKind::BipolarJunctionTransistor { .. } => 3,
                SpiceElementKind::VoltageControlledVoltageSource { .. }
                | SpiceElementKind::VoltageControlledCurrentSource { .. } => 4,
                SpiceElementKind::CurrentControlledVoltageSource { .. }
                | SpiceElementKind::CurrentControlledCurrentSource { .. } => 2,
            };
            if element.nodes.len() != expected {
                if matches!(
                    element.kind,
                    SpiceElementKind::BipolarJunctionTransistor { .. }
                ) && element.nodes.len() == 4
                {
                    // Four-terminal SPICE BJT includes the substrate node.
                } else {
                    return Err(format!(
                        "'{}' expects {} nodes, found {}",
                        element.name,
                        expected,
                        element.nodes.len()
                    ));
                }
            }
            match element.kind {
                SpiceElementKind::Resistor {
                    resistance,
                    first_temperature_coefficient_per_k,
                    second_temperature_coefficient_per_k2,
                    sheet_resistance_ohm_per_square,
                    length_m,
                    width_m,
                    multiplier,
                    ref layer,
                } => {
                    ensure_finite_positive(resistance, "resistance")?;
                    ensure_finite_positive(multiplier, "physical resistor multiplier")?;
                    ensure_finite(first_temperature_coefficient_per_k, "resistor TC1")?;
                    ensure_finite(second_temperature_coefficient_per_k2, "resistor TC2")?;
                    ensure_finite_nonnegative(sheet_resistance_ohm_per_square, "resistor SHEET")?;
                    ensure_finite_nonnegative(length_m, "physical resistor L")?;
                    ensure_finite_nonnegative(width_m, "physical resistor W")?;
                    if sheet_resistance_ohm_per_square > 0.0 && (length_m <= 0.0 || width_m <= 0.0)
                    {
                        return Err(format!(
                            "physical resistor '{}' needs positive L and W",
                            element.name
                        ));
                    }
                    if let Some(layer_name) = layer {
                        let Some(model) = self
                            .resistor_layer_models
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
                            .map(|(_, model)| model)
                        else {
                            return Err(format!(
                                "resistor '{}' uses unknown layer model '{}'",
                                element.name, layer_name
                            ));
                        };
                        if length_m <= 0.0 || width_m <= 0.0 {
                            return Err(format!(
                                "layer-model resistor '{}' needs positive L and W",
                                element.name
                            ));
                        }
                        if width_m < model.minimum_width_m
                            || (model.maximum_width_m > 0.0 && width_m > model.maximum_width_m)
                        {
                            return Err(format!(
                                "resistor '{}' width is outside layer '{}' limits",
                                element.name, layer_name
                            ));
                        }
                        let (_, bin_width_offset) = model
                            .effective_sheet(width_m)
                            .map_err(|e| format!("resistor '{}': {}", element.name, e))?;
                        if width_m + bin_width_offset <= 0.0 {
                            return Err(format!(
                                "resistor '{}' has non-positive effective width",
                                element.name
                            ));
                        }
                    }
                    let delta = self.temperature_k - SPICE_NOMINAL_TEMPERATURE_K;
                    if let Some(layer_name) = layer {
                        let model = self
                            .resistor_layer_models
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
                            .map(|(_, model)| model)
                            .ok_or_else(|| {
                                format!("unknown resistor layer model '{layer_name}'")
                            })?;
                        let sheet_factor = 1.0
                            + model.first_temperature_coefficient_per_k * delta
                            + model.second_temperature_coefficient_per_k2 * delta * delta;
                        if !sheet_factor.is_finite() || sheet_factor <= 0.0 {
                            return Err(format!(
                                "layer model '{}' has non-positive sheet resistance at {} K",
                                layer_name, self.temperature_k
                            ));
                        }
                    }
                    let factor = 1.0
                        + first_temperature_coefficient_per_k * delta
                        + second_temperature_coefficient_per_k2 * delta * delta;
                    if !factor.is_finite() || factor <= 0.0 {
                        return Err(format!(
                            "resistor '{}' has non-positive resistance at {} K",
                            element.name, self.temperature_k
                        ));
                    }
                }
                SpiceElementKind::Capacitor {
                    capacitance,
                    initial_voltage,
                } => {
                    ensure_finite_positive(capacitance, "capacitance")?;
                    ensure_finite(initial_voltage, "capacitor initial voltage")?;
                }
                SpiceElementKind::Inductor {
                    inductance,
                    initial_current,
                } => {
                    ensure_finite_positive(inductance, "inductance")?;
                    ensure_finite(initial_current, "inductor initial current")?;
                }
                SpiceElementKind::VoltageSource {
                    ac_magnitude,
                    ac_phase_deg,
                    ..
                } => {
                    if let Some(magnitude) = ac_magnitude {
                        ensure_finite_nonnegative(magnitude, "AC magnitude")?;
                    }
                    ensure_finite(ac_phase_deg, "AC phase")?;
                }
                SpiceElementKind::CurrentSource {
                    ac_magnitude,
                    ac_phase_deg,
                    ..
                } => {
                    if let Some(magnitude) = ac_magnitude {
                        ensure_finite_nonnegative(magnitude, "AC magnitude")?;
                    }
                    ensure_finite(ac_phase_deg, "AC phase")?;
                }
                SpiceElementKind::VoltageControlledVoltageSource { gain }
                | SpiceElementKind::VoltageControlledCurrentSource {
                    transconductance: gain,
                } => {
                    ensure_finite(gain, "controlled-source gain")?;
                }
                SpiceElementKind::CurrentControlledVoltageSource {
                    ref controlling_source,
                    transresistance,
                } => {
                    ensure_finite(transresistance, "CCVS transresistance")?;
                    ensure_control_source(self, controlling_source.as_str(), &element.name)?;
                }
                SpiceElementKind::CurrentControlledCurrentSource {
                    ref controlling_source,
                    gain,
                } => {
                    ensure_finite(gain, "CCCS gain")?;
                    ensure_control_source(self, controlling_source.as_str(), &element.name)?;
                }
                SpiceElementKind::Diode { ref model } => {
                    if !self.diode_models.contains_key(model.as_str()) {
                        return Err(format!("unknown diode model '{}'", model));
                    }
                }
                SpiceElementKind::Mosfet {
                    ref model,
                    width_m,
                    length_m,
                    multiplier,
                } => {
                    self.mosfet_model_for(model, width_m, length_m)?;
                    ensure_finite_positive(width_m, "MOS W")?;
                    ensure_finite_positive(length_m, "MOS L")?;
                    ensure_finite_positive(multiplier, "MOS M")?;
                }
                SpiceElementKind::BipolarJunctionTransistor { ref model, area } => {
                    if !self.bjt_models.contains_key(model.as_str()) {
                        return Err(format!("unknown BJT model '{}'", model));
                    }
                    ensure_finite_positive(area, "BJT AREA")?;
                }
            }
        }
        for (name, model) in &self.diode_models {
            if name.trim().is_empty() {
                return Err("diode model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.mosfet_models {
            if name.trim().is_empty() {
                return Err("MOSFET model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.bjt_models {
            if name.trim().is_empty() {
                return Err("BJT model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.resistor_layer_models {
            if name.trim().is_empty() {
                return Err("resistor layer model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, location) in &self.locations {
            if !self
                .elements
                .iter()
                .any(|element| element.name.eq_ignore_ascii_case(name))
            {
                return Err(format!("location target '{name}' does not exist"));
            }
            ensure_finite(location.x_m, "element location X")?;
            ensure_finite(location.y_m, "element location Y")?;
        }
        for (name, model) in &self.statistical_models {
            if name.trim().is_empty() {
                return Err("statistical model name cannot be empty".into());
            }
            model.validate()?;
            for parameter in &model.parameters {
                if !self.has_sensitivity_parameter(&parameter.parameter) {
                    return Err(format!(
                        "statistical model parameter '{}' does not exist",
                        parameter.parameter
                    ));
                }
            }
            for correlation in &model.correlations {
                if !self.has_sensitivity_parameter(&correlation.left)
                    || !self.has_sensitivity_parameter(&correlation.right)
                {
                    return Err(format!(
                        "statistical model correlation '{}' does not exist",
                        correlation.left
                    ));
                }
            }
        }
        for analysis in &self.analyses {
            if let Analysis::NoiseSweep {
                output_node,
                input_source,
                ..
            } = analysis
            {
                if !self.node_names().iter().any(|node| node == output_node) {
                    return Err(format!(
                        "noise output node '{}' does not exist",
                        output_node
                    ));
                }
                match self.element(input_source).map(|element| &element.kind) {
                    Some(
                        SpiceElementKind::VoltageSource { .. }
                        | SpiceElementKind::CurrentSource { .. },
                    ) => {}
                    _ => {
                        return Err(format!(
                            "noise input source '{}' does not exist",
                            input_source
                        ))
                    }
                }
            }
            if let Analysis::MonteCarlo(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "Monte Carlo output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "Monte Carlo target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for correlation in &options.correlations {
                    if !self.has_sensitivity_parameter(&correlation.left) {
                        return Err(format!(
                            "unknown Monte Carlo correlation parameter '{}'",
                            correlation.left
                        ));
                    }
                    if !self.has_sensitivity_parameter(&correlation.right) {
                        return Err(format!(
                            "unknown Monte Carlo correlation parameter '{}'",
                            correlation.right
                        ));
                    }
                }
                if let Some(model_name) = &options.statistical_model {
                    if !self.statistical_models.contains_key(model_name) {
                        return Err(format!(
                            "Monte Carlo statistical model '{model_name}' does not exist"
                        ));
                    }
                }
            }
            if let Analysis::Sensitivity(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "sensitivity output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "sensitivity target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
            if let Analysis::GlobalSensitivity(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "global sensitivity output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "global sensitivity target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
            let has_diode = self
                .elements
                .iter()
                .any(|element| matches!(element.kind, SpiceElementKind::Diode { .. }));
            if let Analysis::Derivative(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "derivative output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if options.target == DerivativeTarget::AcMagnitude {
                    let linear_ac = !has_diode
                        && self.elements.iter().all(|element| {
                            matches!(
                                element.kind,
                                SpiceElementKind::Resistor { .. }
                                    | SpiceElementKind::Capacitor { .. }
                                    | SpiceElementKind::Inductor { .. }
                                    | SpiceElementKind::VoltageSource { .. }
                                    | SpiceElementKind::CurrentSource { .. }
                            )
                        });
                    if has_diode {
                        let resistor_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(element.kind, SpiceElementKind::Resistor { .. })
                            })
                            .count();
                        let diode_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(element.kind, SpiceElementKind::Diode { .. })
                            })
                            .count();
                        let source_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(
                                    &element.kind,
                                    SpiceElementKind::VoltageSource {
                                        waveform: SourceWaveform::Dc(_),
                                        ac_magnitude: Some(_),
                                        ..
                                    }
                                )
                            })
                            .count();
                        if self.elements.len() != 3
                            || resistor_count != 1
                            || diode_count != 1
                            || source_count != 1
                            || !self
                                .analyses
                                .iter()
                                .any(|candidate| matches!(candidate, Analysis::AcSweep { .. }))
                        {
                            return Err(
                                "nonlinear AC derivatives support one series R, one diode, and one DC/AC V source"
                                    .into(),
                            );
                        }
                    } else if !linear_ac {
                        return Err(
                            "AC analytic derivatives support linear RCL and source circuits only"
                                .into(),
                        );
                    }
                } else if options.target == DerivativeTarget::TransientFinal {
                    if !self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                    {
                        return Err(
                            "transient derivative target lacks its base .tran analysis".into()
                        );
                    }
                    let linear = self.elements.iter().all(|element| {
                        matches!(
                            element.kind,
                            SpiceElementKind::Resistor { .. }
                                | SpiceElementKind::Capacitor { .. }
                                | SpiceElementKind::Inductor { .. }
                                | SpiceElementKind::VoltageSource { .. }
                                | SpiceElementKind::CurrentSource { .. }
                        )
                    });
                    if !linear {
                        return Err(
                            "transient analytic derivatives support linear RCL and source circuits only"
                                .into(),
                        );
                    }
                }
                for parameter in &options.parameters {
                    let supported_dc = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            field.eq_ignore_ascii_case("IS")
                                && self
                                    .diode_models
                                    .keys()
                                    .any(|model| model.eq_ignore_ascii_case(object))
                                || (field.eq_ignore_ascii_case("VTO")
                                    || field.eq_ignore_ascii_case("KP"))
                                    && self
                                        .mosfet_models
                                        .keys()
                                        .any(|model| model.eq_ignore_ascii_case(object))
                                || (field.eq_ignore_ascii_case("IS")
                                    || field.eq_ignore_ascii_case("BF"))
                                    && self
                                        .bjt_models
                                        .keys()
                                        .any(|model| model.eq_ignore_ascii_case(object))
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("R")
                                        && matches!(element.kind, SpiceElementKind::Resistor { .. })
                                })
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("AREA")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::BipolarJunctionTransistor { .. }
                                        )
                                })
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("DC")
                                        && match &element.kind {
                                            SpiceElementKind::VoltageSource {
                                                waveform, ..
                                            }
                                            | SpiceElementKind::CurrentSource {
                                                waveform, ..
                                            } => {
                                                matches!(waveform, SourceWaveform::Dc(_))
                                            }
                                            _ => false,
                                        }
                                })
                        })
                        .unwrap_or(false);
                    let supported_ac = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("C")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Capacitor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("L")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Inductor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("AC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            } | SpiceElementKind::CurrentSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            }
                                        ))
                            })
                        })
                        .unwrap_or(false);
                    let supported_nonlinear_ac =
                        parameter.rsplit_once('.').is_some_and(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("DC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            }
                                        ))
                                    || (field.eq_ignore_ascii_case("AC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            }
                                        ))
                            }) || self
                                .diode_models
                                .keys()
                                .any(|model| model.eq_ignore_ascii_case(object))
                                && (field.eq_ignore_ascii_case("IS")
                                    || field.eq_ignore_ascii_case("TT")
                                    || field.eq_ignore_ascii_case("CJO")
                                    || field.eq_ignore_ascii_case("VJ")
                                    || field.eq_ignore_ascii_case("M")
                                    || field.eq_ignore_ascii_case("FC"))
                        });
                    let supported_transient = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("C")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Capacitor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("L")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Inductor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("DC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            } | SpiceElementKind::CurrentSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            }
                                        ))
                            })
                        })
                        .unwrap_or(false);
                    let supported = match options.target {
                        DerivativeTarget::Dc => supported_dc,
                        DerivativeTarget::AcMagnitude => {
                            if has_diode {
                                supported_nonlinear_ac
                            } else {
                                supported_ac
                            }
                        }
                        DerivativeTarget::TransientFinal => supported_transient,
                    };
                    if !supported {
                        return Err(match options.target {
                            DerivativeTarget::Dc => format!(
                                "derivative parameter '{parameter}' must be a resistor R, DC source value, diode model IS, MOS model VTO/KP, BJT model IS/BF, or BJT instance AREA"
                            ),
                            DerivativeTarget::AcMagnitude if has_diode => format!(
                                "nonlinear AC derivative parameter '{parameter}' must be R, diode IS/TT/CJO/VJ/M/FC, DC source, or AC source magnitude"
                            ),
                            DerivativeTarget::AcMagnitude => format!(
                                "AC derivative parameter '{parameter}' must be R, C, L, or an AC source magnitude"
                            ),
                            DerivativeTarget::TransientFinal => format!(
                                "transient derivative parameter '{parameter}' must be the series R, C, or source DC value"
                            ),
                        });
                    }
                }
            }
            if let Analysis::Optimization(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "optimization output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "optimization target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for parameter in &options.parameters {
                    if !self.has_sensitivity_parameter(&parameter.parameter) {
                        return Err(format!(
                            "unknown optimization parameter '{}'",
                            parameter.parameter
                        ));
                    }
                }
            }
            if let Analysis::Pareto(options) = analysis {
                for output_node in &options.output_nodes {
                    if !self.node_names().iter().any(|node| node == output_node) {
                        return Err(format!(
                            "Pareto optimization output node '{}' does not exist",
                            output_node
                        ));
                    }
                }
                for constraint in &options.constraints {
                    for variable in constraint.variable_names() {
                        if !self.node_names().iter().any(|node| node == &variable) {
                            return Err(format!(
                                "Pareto optimization constraint node '{}' does not exist",
                                variable
                            ));
                        }
                    }
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "Pareto optimization target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for parameter in &options.parameters {
                    if !self.has_sensitivity_parameter(&parameter.parameter) {
                        return Err(format!(
                            "unknown Pareto optimization parameter '{}'",
                            parameter.parameter
                        ));
                    }
                }
            }
            if let Analysis::PeriodicSteadyState(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "PSS output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if !self
                    .analyses
                    .iter()
                    .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                {
                    return Err("PSS requires a .tran analysis".into());
                }
            }
            if let Analysis::EnvelopeFollow(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "envelope output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if !self
                    .analyses
                    .iter()
                    .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                {
                    return Err("envelope following requires a .tran analysis".into());
                }
            }
            if let Analysis::Aging(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "aging output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "aging target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
        }
        for analysis in &self.analyses {
            validate_analysis(analysis)?;
        }
        Ok(())
    }
}

fn ensure_finite(value: f64, what: &str) -> Result<(), String> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(format!("{} must be finite", what))
    }
}

fn ensure_finite_nonnegative(value: f64, what: &str) -> Result<(), String> {
    ensure_finite(value, what)?;
    if value < 0.0 {
        Err(format!("{} must be nonnegative", what))
    } else {
        Ok(())
    }
}

fn ensure_finite_positive(value: f64, what: &str) -> Result<(), String> {
    ensure_finite(value, what)?;
    if value <= 0.0 {
        Err(format!("{} must be positive", what))
    } else {
        Ok(())
    }
}

fn ensure_control_source(circuit: &SpiceCircuit, name: &str, user: &str) -> Result<(), String> {
    match circuit.element(name) {
        Some(element) if matches!(element.kind, SpiceElementKind::VoltageSource { .. }) => Ok(()),
        _ => Err(format!(
            "'{}' must control from a voltage source (found no '{}')",
            user, name
        )),
    }
}

fn validate_analysis(analysis: &Analysis) -> Result<(), String> {
    match analysis {
        Analysis::OperatingPoint => Ok(()),
        Analysis::DcSweep {
            start, stop, step, ..
        } => {
            ensure_finite(*start, "DC sweep start")?;
            ensure_finite(*stop, "DC sweep stop")?;
            ensure_finite(*step, "DC sweep step")?;
            if *step == 0.0 || (*stop < *start && *step > 0.0) || (*stop > *start && *step < 0.0) {
                return Err("DC sweep direction and step must agree".into());
            }
            Ok(())
        }
        Analysis::Transient {
            step_s,
            stop_s,
            method,
        } => {
            ensure_finite_positive(*step_s, "transient step")?;
            ensure_finite_positive(*stop_s, "transient stop time")?;
            if step_s > stop_s {
                return Err("transient step cannot exceed stop time".into());
            }
            if IntegrationMethod::parse(method.as_str()).is_none() {
                return Err("unknown transient integration method".into());
            }
            Ok(())
        }
        Analysis::AcSweep {
            kind,
            points,
            start_hz,
            stop_hz,
        } => {
            if *points == 0 {
                return Err("AC sweep needs at least one point".into());
            }
            ensure_finite_positive(*start_hz, "AC start frequency")?;
            ensure_finite_positive(*stop_hz, "AC stop frequency")?;
            if start_hz > stop_hz {
                return Err("AC start frequency cannot exceed stop frequency".into());
            }
            let _ = kind;
            Ok(())
        }
        Analysis::NoiseSweep {
            output_node,
            input_source,
            kind,
            points,
            start_hz,
            stop_hz,
        } => {
            if output_node.trim().is_empty() {
                return Err("noise output node cannot be empty".into());
            }
            if input_source.trim().is_empty() {
                return Err("noise input source cannot be empty".into());
            }
            if *points == 0 {
                return Err("noise sweep needs at least one point".into());
            }
            ensure_finite_positive(*start_hz, "noise start frequency")?;
            ensure_finite_positive(*stop_hz, "noise stop frequency")?;
            if start_hz > stop_hz {
                return Err("noise start frequency cannot exceed stop frequency".into());
            }
            let _ = kind;
            Ok(())
        }
        Analysis::MonteCarlo(options) => options.validate(),
        Analysis::Sensitivity(options) => options.validate(),
        Analysis::GlobalSensitivity(options) => options.validate(),
        Analysis::Optimization(options) => options.validate(),
        Analysis::Derivative(options) => options.validate(),
        Analysis::Pareto(options) => options.validate(),
        Analysis::PeriodicSteadyState(options) => options.validate(),
        Analysis::EnvelopeFollow(options) => options.validate(),
        Analysis::Aging(options) => options.validate(),
    }
}
