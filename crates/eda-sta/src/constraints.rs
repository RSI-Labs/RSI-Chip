//! Minimal SDC timing constraints and constrained slack evaluation.
//!
//! Supported commands:
//! - `set_units`
//!   - time/capacitance scaling into canonical ns/pF
//!   - resistance/voltage/current metadata
//! - `create_clock`
//! - `create_generated_clock`
//! - `set_clock_uncertainty`
//! - `set_clock_latency`
//! - `set_clock_transition`
//! - `set_clock_groups`
//! - `set_input_delay`
//! - `set_output_delay`
//! - `set_false_path`
//! - `set_multicycle_path`
//!   - ordered typed `-through` object lists
//!   - optional hierarchy-scoped `-instance` pattern
//! - `set_disable_timing`
//!   - optional hierarchy-scoped `-instance` pattern
//! - `set_case_analysis`
//!   - optional hierarchy-scoped `-instance` pattern
//! - `set_max_delay`
//! - `set_min_delay`
//!   - ordered typed `-through` object lists
//!   - optional hierarchy-scoped `-instance` pattern
//! - `set_timing_derate`
//!   - optional hierarchy-scoped `-instance` pattern for cell and pin rules
//! - `set_max_transition`
//! - `set_max_capacitance`
//! - `set_max_fanout`
//!   - design-wide signal-quality limits consumed by quality analysis
//!
//! The parser intentionally accepts the common Synopsys option/port-list
//! ordering rather than requiring arguments to appear in one fixed order.

use crate::quality::ScopedQualityLimit;
use crate::{HoldTimingPath, StaResult, TimingPath};
use eda_core::netlist::{NetId, Netlist};
use std::collections::{BTreeMap, BTreeSet};

mod analysis;
mod parser;
mod paths;

pub use analysis::{analyze_constrained, analyze_with_sdc};
pub use parser::parse_sdc;
pub(crate) use parser::{pin_check_derate, scoped_cell_timing_derate, scoped_pin_timing_derate};
use parser::{upsert_scoped_value, validate_derate};
pub use paths::*;
pub(crate) use paths::{timing_arc_disabled, timing_check_disabled};

#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub name: String,
    pub period: f64,
    pub rise: f64,
    pub fall: f64,
    /// Ideal/source latency common to launch and capture references.
    pub source_latency_ns: f64,
    /// Insertion/network latency from the clock reference to local sinks.
    pub network_latency_ns: f64,
    /// SDC transition on the rising clock edge.
    pub rise_transition_ns: f64,
    /// SDC transition on the falling clock edge.
    pub fall_transition_ns: f64,
    /// Minimum SDC transition on the rising clock edge.
    pub min_rise_transition_ns: f64,
    /// Minimum SDC transition on the falling clock edge.
    pub min_fall_transition_ns: f64,
    /// Parent clock for `create_generated_clock`; a root clock has no parent.
    pub source_clock: Option<String>,
    /// Frequency division factor; period is parent period times this factor.
    pub divide_by: Option<f64>,
    /// Frequency multiplication factor; period is parent period divided by
    /// this factor.
    pub multiply_by: Option<f64>,
    pub ports: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortDelay {
    pub port: String,
    pub clock: String,
    pub delay: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockGroupKind {
    Asynchronous,
    LogicallyExclusive,
    PhysicallyExclusive,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockGroup {
    pub name: String,
    pub kind: ClockGroupKind,
    pub groups: Vec<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathException {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Display names for each ordered `-through` group.
    pub through: Vec<String>,
    pub from_object: Option<TimingObjectRef>,
    pub to_object: Option<TimingObjectRef>,
    pub through_groups: Vec<Vec<TimingObjectRef>>,
    /// Optional hierarchy-instance selector.
    ///
    /// The exception only applies when the concrete path passes through an
    /// instance matching this pattern.
    pub instance_pattern: Option<String>,
    pub multicycle: Option<f64>,
    /// Timing-check class selected by `-setup` or `-hold`.
    pub check: PathExceptionCheck,
}

/// The setup/hold class to which an exception applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathExceptionCheck {
    Both,
    Setup,
    Hold,
}

impl PathExceptionCheck {
    pub fn applies_setup(self) -> bool {
        matches!(self, Self::Both | Self::Setup)
    }

    pub fn applies_hold(self) -> bool {
        matches!(self, Self::Both | Self::Hold)
    }

    pub fn applies_early(self, early: bool) -> bool {
        if early {
            self.applies_hold()
        } else {
            self.applies_setup()
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Both => "setup+hold",
            Self::Setup => "setup",
            Self::Hold => "hold",
        }
    }
}

/// A `set_disable_timing` object or arc selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisabledTimingArc {
    /// Trailing command object, typically a cell or pin access list entry.
    pub object: Option<TimingObjectRef>,
    pub from: Option<TimingObjectRef>,
    pub to: Option<TimingObjectRef>,
    /// Optional hierarchy-instance selector.
    pub instance_pattern: Option<String>,
}

/// A `set_case_analysis` constant applied to a port, pin, or net object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaseAnalysis {
    pub object: TimingObjectRef,
    pub value: bool,
    /// Optional hierarchy-instance selector.
    pub instance_pattern: Option<String>,
}

/// Timing check omitted by a false-path exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcludedTimingCheck {
    Setup,
    Hold,
    Recovery,
    Removal,
}

/// A concrete timing path removed by a false-path exception.
#[derive(Clone, Debug)]
pub struct ExcludedTimingPath {
    pub check: ExcludedTimingCheck,
    pub path: TimingPath,
    pub exception: PathException,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathDelayLimit {
    pub from: Option<String>,
    pub to: Option<String>,
    /// Display names for each ordered `-through` group.
    pub through: Vec<String>,
    pub from_object: Option<TimingObjectRef>,
    pub to_object: Option<TimingObjectRef>,
    pub through_groups: Vec<Vec<TimingObjectRef>>,
    /// Optional hierarchy-instance selector.
    pub instance_pattern: Option<String>,
    pub limit: f64,
}

/// One matched `set_max_delay` or `set_min_delay` constraint.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedTimingDelayLimit {
    pub maximum: bool,
    pub limit: PathDelayLimit,
    pub slack_ns: f64,
}

/// SDC object reference used by timing exceptions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingObjectKind {
    Port,
    Pin,
    Net,
    Cell,
    Clock,
}

/// One token in an SDC object-name wildcard pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingObjectPatternToken {
    Literal(char),
    /// `?`: exactly one character within one hierarchy level.
    AnyOneCharacter,
    /// `*`: zero or more characters within one hierarchy level.
    AnyWithinLevel,
    /// `**`: zero or more characters, including hierarchy separators.
    AnyHierarchy,
}

/// Dependency-free wildcard matcher for flat and hierarchical SDC objects.
///
/// Hierarchy separators are `/` and `.`. A single `*` or `?` does not cross a
/// separator; `**` does. A backslash escapes the following wildcard character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingObjectPattern {
    tokens: Vec<TimingObjectPatternToken>,
    pattern: String,
}

impl TimingObjectPattern {
    pub fn new(pattern: impl Into<String>) -> Self {
        let pattern = pattern.into();
        let mut chars = pattern.chars().peekable();
        let mut tokens = Vec::new();
        while let Some(current) = chars.next() {
            let token = if current == '\\' {
                if let Some(&next) = chars.peek() {
                    chars.next();
                    TimingObjectPatternToken::Literal(next)
                } else {
                    TimingObjectPatternToken::Literal(current)
                }
            } else if current == '*' {
                if chars.peek().is_some_and(|next| *next == '*') {
                    chars.next();
                    // A `**/` prefix also matches the current level, so
                    // `**/u*` matches both `u0` and `block/u0`.
                    if chars
                        .peek()
                        .is_some_and(|next| *next == '/' || *next == '.')
                    {
                        chars.next();
                    }
                    TimingObjectPatternToken::AnyHierarchy
                } else {
                    TimingObjectPatternToken::AnyWithinLevel
                }
            } else if current == '?' {
                TimingObjectPatternToken::AnyOneCharacter
            } else {
                TimingObjectPatternToken::Literal(current)
            };
            tokens.push(token);
        }
        Self { tokens, pattern }
    }

    pub fn is_pattern(&self) -> bool {
        self.tokens
            .iter()
            .any(|token| !matches!(token, TimingObjectPatternToken::Literal(_)))
    }

    pub fn as_str(&self) -> &str {
        &self.pattern
    }

    pub fn matches(&self, input: &str) -> bool {
        let input = input.chars().collect::<Vec<_>>();
        Self::match_tokens(&self.tokens, &input)
    }

    fn match_tokens(tokens: &[TimingObjectPatternToken], input: &[char]) -> bool {
        let Some((token, rest_tokens)) = tokens.split_first() else {
            return input.is_empty();
        };
        match token {
            TimingObjectPatternToken::Literal(expected) => {
                input.first().is_some_and(|actual| actual == expected)
                    && Self::match_tokens(rest_tokens, &input[1..])
            }
            TimingObjectPatternToken::AnyOneCharacter => input.first().is_some_and(|actual| {
                *actual != '/' && *actual != '.' && Self::match_tokens(rest_tokens, &input[1..])
            }),
            TimingObjectPatternToken::AnyWithinLevel => {
                if Self::match_tokens(rest_tokens, input) {
                    return true;
                }
                input
                    .first()
                    .is_some_and(|actual| *actual != '/' && *actual != '.')
                    && Self::match_tokens(tokens, &input[1..])
            }
            TimingObjectPatternToken::AnyHierarchy => {
                (0..=input.len()).any(|skip| Self::match_tokens(rest_tokens, &input[skip..]))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimingObjectRef {
    pub kind: TimingObjectKind,
    pub name: String,
}

impl TimingObjectRef {
    pub fn new(kind: TimingObjectKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }

    pub fn label(&self) -> String {
        match self.kind {
            TimingObjectKind::Port | TimingObjectKind::Net => self.name.clone(),
            TimingObjectKind::Pin => format!("pin {}", self.name),
            TimingObjectKind::Cell => format!("cell {}", self.name),
            TimingObjectKind::Clock => format!("clock {}", self.name),
        }
    }

    pub fn pattern(&self) -> TimingObjectPattern {
        TimingObjectPattern::new(&self.name)
    }

    pub fn is_pattern(&self) -> bool {
        self.pattern().is_pattern()
    }

    pub fn matches_name(&self, name: &str) -> bool {
        self.pattern().matches(name)
    }

    pub fn matches_object(&self, object: &TimingObjectRef) -> bool {
        self.kind == object.kind && self.matches_name(&object.name)
    }
}

/// Clock edge used by a constrained timing check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingClockEdgeKind {
    Rise,
    Fall,
}

/// One launch or capture clock reference.
///
/// `virtual_reference` marks an external primary-input/output reference; such
/// an edge has source latency but no clock-network insertion latency.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingClockEdge {
    pub clock: String,
    pub edge: TimingClockEdgeKind,
    pub edge_time_ns: f64,
    pub source_latency_ns: f64,
    pub network_latency_ns: f64,
    pub absolute_time_ns: f64,
    pub virtual_reference: bool,
}

/// Launch/capture edge and insertion-delay detail for one constrained path.
#[derive(Clone, Debug, PartialEq)]
pub struct TimingPathClocks {
    pub launch: TimingClockEdge,
    pub capture: TimingClockEdge,
    pub edge_separation_ns: f64,
    pub insertion_skew_ns: f64,
}

/// Global early/late timing-derate factors. A factor of 1.0 is neutral.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimingDerate {
    pub early: f64,
    pub late: f64,
}

/// A hierarchy-scoped cell timing-derate rule.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedCellTimingDerate {
    pub cell_pattern: String,
    pub instance_pattern: Option<String>,
    pub check: bool,
    pub derate: TimingDerate,
}

/// A hierarchy-scoped pin timing-derate rule.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedPinTimingDerate {
    pub cell_pattern: String,
    pub pin_pattern: String,
    pub instance_pattern: Option<String>,
    pub check: bool,
    pub derate: TimingDerate,
}

impl Default for TimingDerate {
    fn default() -> Self {
        Self {
            early: 1.0,
            late: 1.0,
        }
    }
}

/// A flattened cell timing pin, such as combinational input `u1/a`,
/// combinational output `u1/y`, or sequential output `ff/q`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimingPin {
    pub cell: String,
    pub pin: String,
}

impl TimingPin {
    pub fn new(cell: impl Into<String>, pin: impl Into<String>) -> Self {
        Self {
            cell: cell.into(),
            pin: pin.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimingConstraints {
    /// Source-unit metadata; all numeric fields remain canonical ns/pF.
    pub units: SdcUnits,
    pub clocks: BTreeMap<String, Clock>,
    pub clock_groups: Vec<ClockGroup>,
    pub input_delays: BTreeMap<String, PortDelay>,
    pub output_delays: BTreeMap<String, PortDelay>,
    pub false_paths: Vec<PathException>,
    pub multicycle_paths: Vec<PathException>,
    pub disabled_timing_arcs: Vec<DisabledTimingArc>,
    pub case_analyses: Vec<CaseAnalysis>,
    pub max_delay_paths: Vec<PathDelayLimit>,
    pub min_delay_paths: Vec<PathDelayLimit>,
    pub clock_uncertainty_ns: BTreeMap<String, f64>,
    pub hold_clock_uncertainty_ns: BTreeMap<String, f64>,
    pub timing_derate: TimingDerate,
    pub cell_timing_derates: BTreeMap<String, TimingDerate>,
    pub scoped_cell_timing_derates: Vec<ScopedCellTimingDerate>,
    pub port_timing_derates: BTreeMap<String, TimingDerate>,
    pub pin_timing_derates: BTreeMap<TimingPin, TimingDerate>,
    pub scoped_pin_timing_derates: Vec<ScopedPinTimingDerate>,
    pub check_timing_derate: TimingDerate,
    pub cell_check_derates: BTreeMap<String, TimingDerate>,
    pub pin_check_derates: BTreeMap<TimingPin, TimingDerate>,
    /// Design-wide SDC transition limit in nanoseconds.
    pub max_transition_ns: Option<f64>,
    /// Design-wide SDC load limit in picofarads.
    pub max_capacitance_pf: Option<f64>,
    /// Design-wide SDC fanout limit.
    pub max_fanout: Option<usize>,
    /// Object-scoped transition limits; the last matching rule wins.
    pub scoped_max_transition_ns: Vec<ScopedQualityLimit>,
    /// Object-scoped capacitance limits; the last matching rule wins.
    pub scoped_max_capacitance_pf: Vec<ScopedQualityLimit>,
    /// Object-scoped fanout limits; the last matching rule wins.
    pub scoped_max_fanout: Vec<ScopedQualityLimit>,
    /// Object-scoped input transitions in nanoseconds.
    pub input_transitions: Vec<ScopedQualityLimit>,
    /// Object-scoped port/pin loads in picofarads.
    pub load_capacitances: Vec<ScopedQualityLimit>,
}

/// Canonical SDC source-unit metadata.
///
/// Parsers normalize values into nanoseconds/picofarads before timing or
/// quality analysis. These fields preserve the last source representation for
/// tool reporting and scenario serialization.
#[derive(Clone, Debug, PartialEq)]
pub struct SdcUnits {
    pub time_unit: String,
    pub time_to_ns_factor: f64,
    pub capacitance_unit: String,
    pub capacitance_to_pf_factor: f64,
    pub resistance_unit: String,
    pub resistance_to_ohm_factor: f64,
    pub voltage_unit: String,
    pub voltage_to_v_factor: f64,
    pub current_unit: String,
    pub current_to_a_factor: f64,
}

impl Default for SdcUnits {
    fn default() -> Self {
        Self {
            time_unit: "ns".into(),
            time_to_ns_factor: 1.0,
            capacitance_unit: "pf".into(),
            capacitance_to_pf_factor: 1.0,
            resistance_unit: "ohm".into(),
            resistance_to_ohm_factor: 1.0,
            voltage_unit: "v".into(),
            voltage_to_v_factor: 1.0,
            current_unit: "a".into(),
            current_to_a_factor: 1.0,
        }
    }
}

/// Semantic difference between two SDC constraint sets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TimingConstraintDiff {
    pub changed_clocks: BTreeSet<String>,
    pub changed_input_ports: BTreeSet<String>,
    pub changed_output_ports: BTreeSet<String>,
    pub changed_cell_derates: BTreeSet<String>,
    pub changed_pin_derates: BTreeSet<TimingPin>,
    pub scoped_timing_derates_changed: bool,
    pub changed_port_derates: BTreeSet<String>,
    pub changed_cell_check_derates: BTreeSet<String>,
    pub changed_pin_check_derates: BTreeSet<TimingPin>,
    pub changed_clock_uncertainty: BTreeSet<String>,
    pub changed_hold_uncertainty: BTreeSet<String>,
    pub timing_exceptions_changed: bool,
    pub clock_groups_changed: bool,
    pub delay_limits_changed: bool,
    pub disabled_timing_changed: bool,
    pub case_analysis_changed: bool,
    pub global_timing_derate_changed: bool,
    pub global_check_derate_changed: bool,
    pub design_rules_changed: bool,
    pub input_transitions_changed: bool,
    pub loads_changed: bool,
    /// Case analysis, disable timing, or a global delay derate changed; the
    /// base arrival graph must be rebuilt.
    pub full_base_recompute_required: bool,
    /// At least one base arrival, arc, or endpoint input changed.
    pub base_recompute_required: bool,
    /// At least one constraint affecting constrained slack summaries changed.
    pub constrained_recompute_required: bool,
    /// Signal-quality limits changed and quality analysis should rerun.
    pub quality_recompute_required: bool,
}

impl TimingConstraints {
    pub fn add_clock(&mut self, clock: Clock) -> Result<(), String> {
        if clock.name.is_empty() {
            return Err("clock name cannot be empty".into());
        }
        if clock.period <= 0.0 || !clock.period.is_finite() {
            return Err(format!("clock '{}' period must be positive", clock.name));
        }
        if clock.fall <= clock.rise {
            return Err(format!(
                "clock '{}' fall time must be later than rise time",
                clock.name
            ));
        }
        if clock.fall - clock.rise >= clock.period {
            return Err(format!(
                "clock '{}' high pulse must be shorter than its period",
                clock.name
            ));
        }
        let clock_name = clock.name.clone();
        if self.clocks.insert(clock_name.clone(), clock).is_some() {
            return Err(format!("duplicate clock '{}'", clock_name));
        }
        Ok(())
    }

    pub fn add_generated_clock(
        &mut self,
        name: impl Into<String>,
        source: &str,
        divide_by: Option<f64>,
        multiply_by: Option<f64>,
        ports: Vec<String>,
    ) -> Result<(), String> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("generated clock name cannot be empty".into());
        }
        if divide_by.is_some() == multiply_by.is_some() {
            return Err(format!(
                "generated clock '{}' requires exactly one of -divide_by or -multiply_by",
                name
            ));
        }
        for (option, value) in [("-divide_by", divide_by), ("-multiply_by", multiply_by)] {
            if let Some(value) = value {
                if !value.is_finite() || value <= 0.0 {
                    return Err(format!(
                        "generated clock '{}' {} must be positive and finite",
                        name, option
                    ));
                }
            }
        }
        let source_name = self
            .clocks
            .keys()
            .find(|clock| clock.as_str() == source)
            .cloned()
            .or_else(|| {
                self.clocks
                    .iter()
                    .find(|(_, definition)| definition.ports.iter().any(|port| port == source))
                    .map(|(name, _)| name.clone())
            })
            .ok_or_else(|| format!("generated clock '{}' has unknown source '{}'", name, source))?;
        let source = self.clocks.get(&source_name).ok_or_else(|| {
            format!(
                "generated clock '{}' parent '{}' disappeared",
                name, source_name
            )
        })?;
        let period = if let Some(divide) = divide_by {
            source.period * divide
        } else {
            let multiply = multiply_by.expect("checked multiplication factor");
            source.period / multiply
        };
        let clock = Clock {
            name,
            period,
            rise: 0.0,
            fall: period / 2.0,
            source_latency_ns: source.source_latency_ns + source.network_latency_ns,
            network_latency_ns: 0.0,
            rise_transition_ns: source.rise_transition_ns,
            fall_transition_ns: source.fall_transition_ns,
            min_rise_transition_ns: source.min_rise_transition_ns,
            min_fall_transition_ns: source.min_fall_transition_ns,
            source_clock: Some(source_name),
            divide_by,
            multiply_by,
            ports,
        };
        self.add_clock(clock)
    }

    pub fn add_clock_group(&mut self, group: ClockGroup) -> Result<(), String> {
        if group.name.trim().is_empty() {
            return Err("clock group name cannot be empty".into());
        }
        if group.groups.len() < 2 {
            return Err(format!(
                "clock group '{}' requires at least two groups",
                group.name
            ));
        }
        let mut seen = BTreeSet::new();
        for clocks in &group.groups {
            if clocks.is_empty() {
                return Err(format!(
                    "clock group '{}' contains an empty clock set",
                    group.name
                ));
            }
            for clock in clocks {
                self.check_clock(clock)?;
                if !seen.insert(clock.clone()) {
                    return Err(format!(
                        "clock '{}' appears more than once in group '{}'",
                        clock, group.name
                    ));
                }
            }
        }
        self.clock_groups.push(group);
        Ok(())
    }

    pub fn clock_group_kind(&self, left: &str, right: &str) -> Option<ClockGroupKind> {
        self.clock_groups
            .iter()
            .filter(|group| {
                group
                    .groups
                    .iter()
                    .any(|clocks| clocks.iter().any(|clock| clock == left))
                    && group
                        .groups
                        .iter()
                        .any(|clocks| clocks.iter().any(|clock| clock == right))
            })
            .find(|group| {
                let left_group = group
                    .groups
                    .iter()
                    .position(|clocks| clocks.iter().any(|clock| clock == left));
                let right_group = group
                    .groups
                    .iter()
                    .position(|clocks| clocks.iter().any(|clock| clock == right));
                left_group.is_some_and(|left| right_group.is_some_and(|right| left != right))
            })
            .map(|group| group.kind)
    }

    pub fn set_input_delay(&mut self, d: PortDelay) -> Result<(), String> {
        self.check_clock(&d.clock)?;
        if !d.delay.is_finite() {
            return Err(format!("input delay for '{}' is not finite", d.port));
        }
        self.input_delays.insert(d.port.clone(), d);
        Ok(())
    }

    pub fn set_output_delay(&mut self, d: PortDelay) -> Result<(), String> {
        self.check_clock(&d.clock)?;
        if !d.delay.is_finite() {
            return Err(format!("output delay for '{}' is not finite", d.port));
        }
        self.output_delays.insert(d.port.clone(), d);
        Ok(())
    }

    pub fn set_clock_uncertainty(&mut self, clock: String, uncertainty: f64) -> Result<(), String> {
        self.check_clock(&clock)?;
        if uncertainty < 0.0 || !uncertainty.is_finite() {
            return Err(format!("invalid uncertainty for clock '{}'", clock));
        }
        self.clock_uncertainty_ns.insert(clock, uncertainty);
        Ok(())
    }

    pub fn set_clock_latency(
        &mut self,
        clock: String,
        latency: f64,
        source: bool,
    ) -> Result<(), String> {
        self.check_clock(&clock)?;
        if latency < 0.0 || !latency.is_finite() {
            return Err(format!("invalid latency for clock '{}'", clock));
        }
        let definition = self
            .clocks
            .get_mut(&clock)
            .expect("clock existence checked above");
        if source {
            definition.source_latency_ns = latency;
        } else {
            definition.network_latency_ns = latency;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_clock_transition(
        &mut self,
        clock: String,
        min_rise: Option<f64>,
        min_fall: Option<f64>,
        max_rise: Option<f64>,
        max_fall: Option<f64>,
    ) -> Result<(), String> {
        self.check_clock(&clock)?;
        for (edge, value) in [
            ("minimum rise", min_rise),
            ("minimum fall", min_fall),
            ("maximum rise", max_rise),
            ("maximum fall", max_fall),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || value < 0.0 {
                    return Err(format!("invalid {edge} transition for clock '{clock}'"));
                }
            }
        }
        let definition = self
            .clocks
            .get_mut(&clock)
            .expect("clock existence checked above");
        if let Some(value) = min_rise {
            definition.min_rise_transition_ns = value;
        }
        if let Some(value) = min_fall {
            definition.min_fall_transition_ns = value;
        }
        if let Some(value) = max_rise {
            definition.rise_transition_ns = value;
        }
        if let Some(value) = max_fall {
            definition.fall_transition_ns = value;
        }
        Ok(())
    }

    pub fn add_max_delay(&mut self, limit: PathDelayLimit) -> Result<(), String> {
        self.validate_delay_limit(&limit, "set_max_delay")?;
        self.max_delay_paths.push(limit);
        Ok(())
    }

    pub fn add_min_delay(&mut self, limit: PathDelayLimit) -> Result<(), String> {
        self.validate_delay_limit(&limit, "set_min_delay")?;
        self.min_delay_paths.push(limit);
        Ok(())
    }

    fn validate_delay_limit(&self, limit: &PathDelayLimit, command: &str) -> Result<(), String> {
        if limit.from.is_none() && limit.to.is_none() && limit.through_groups.is_empty() {
            return Err(format!(
                "{} requires -from, -to, -through, or a combination",
                command
            ));
        }
        if limit.limit <= 0.0 || !limit.limit.is_finite() {
            return Err(format!("{} limit must be positive and finite", command));
        }
        Ok(())
    }

    pub fn set_hold_clock_uncertainty(
        &mut self,
        clock: String,
        uncertainty: f64,
    ) -> Result<(), String> {
        self.check_clock(&clock)?;
        if uncertainty < 0.0 || !uncertainty.is_finite() {
            return Err(format!("invalid hold uncertainty for clock '{}'", clock));
        }
        self.hold_clock_uncertainty_ns.insert(clock, uncertainty);
        Ok(())
    }

    pub fn add_disabled_timing_arc(&mut self, arc: DisabledTimingArc) -> Result<(), String> {
        if arc.object.is_none() && arc.from.is_none() && arc.to.is_none() {
            return Err("set_disable_timing requires a cell/pin object, -from, or -to".into());
        }
        self.disabled_timing_arcs.push(arc);
        Ok(())
    }

    pub fn add_case_analysis(&mut self, analysis: CaseAnalysis) -> Result<(), String> {
        self.case_analyses.push(analysis);
        Ok(())
    }

    pub fn set_timing_derate(
        &mut self,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        if early.is_none() && late.is_none() {
            return Err("set_timing_derate requires -early, -late, or both".into());
        }
        for (kind, factor) in [("early", early), ("late", late)] {
            if let Some(factor) = factor {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(format!(
                        "{} timing derate must be positive and finite",
                        kind
                    ));
                }
            }
        }
        if let Some(early) = early {
            self.timing_derate.early = early;
        }
        if let Some(late) = late {
            self.timing_derate.late = late;
        }
        Ok(())
    }

    pub fn set_max_transition_ns(&mut self, value: f64) -> Result<(), String> {
        if !value.is_finite() || value <= 0.0 {
            return Err("set_max_transition limit must be positive and finite".into());
        }
        self.max_transition_ns = Some(value);
        Ok(())
    }

    pub fn set_max_capacitance_pf(&mut self, value: f64) -> Result<(), String> {
        if !value.is_finite() || value <= 0.0 {
            return Err("set_max_capacitance limit must be positive and finite".into());
        }
        self.max_capacitance_pf = Some(value);
        Ok(())
    }

    pub fn set_max_fanout(&mut self, value: usize) -> Result<(), String> {
        if value == 0 {
            return Err("set_max_fanout limit must be positive".into());
        }
        self.max_fanout = Some(value);
        Ok(())
    }

    pub fn add_scoped_max_transition_ns(
        &mut self,
        object: TimingObjectRef,
        value: f64,
    ) -> Result<(), String> {
        Self::validate_scoped_quality_object(&object, "set_max_transition")?;
        if !value.is_finite() || value <= 0.0 {
            return Err("set_max_transition limit must be positive and finite".into());
        }
        self.scoped_max_transition_ns.push(ScopedQualityLimit {
            object,
            limit: value,
        });
        Ok(())
    }

    pub fn add_scoped_max_capacitance_pf(
        &mut self,
        object: TimingObjectRef,
        value: f64,
    ) -> Result<(), String> {
        Self::validate_scoped_quality_object(&object, "set_max_capacitance")?;
        if !value.is_finite() || value <= 0.0 {
            return Err("set_max_capacitance limit must be positive and finite".into());
        }
        self.scoped_max_capacitance_pf.push(ScopedQualityLimit {
            object,
            limit: value,
        });
        Ok(())
    }

    pub fn add_scoped_max_fanout(
        &mut self,
        object: TimingObjectRef,
        value: usize,
    ) -> Result<(), String> {
        Self::validate_scoped_quality_object(&object, "set_max_fanout")?;
        if value == 0 {
            return Err("set_max_fanout limit must be positive".into());
        }
        self.scoped_max_fanout.push(ScopedQualityLimit {
            object,
            limit: value as f64,
        });
        Ok(())
    }

    pub fn add_input_transition(
        &mut self,
        object: TimingObjectRef,
        value: f64,
    ) -> Result<(), String> {
        Self::validate_transition_load_object(&object, "set_input_transition")?;
        if !value.is_finite() || value < 0.0 {
            return Err("set_input_transition value must be finite and nonnegative".into());
        }
        upsert_scoped_value(&mut self.input_transitions, object, value);
        Ok(())
    }

    pub fn add_load_capacitance(
        &mut self,
        object: TimingObjectRef,
        value: f64,
    ) -> Result<(), String> {
        Self::validate_transition_load_object(&object, "set_load")?;
        if !value.is_finite() || value < 0.0 {
            return Err("set_load value must be finite and nonnegative".into());
        }
        upsert_scoped_value(&mut self.load_capacitances, object, value);
        Ok(())
    }

    fn validate_scoped_quality_object(
        object: &TimingObjectRef,
        command: &str,
    ) -> Result<(), String> {
        if object.name.trim().is_empty() {
            return Err(format!("{command} object name cannot be empty"));
        }
        if object.kind == TimingObjectKind::Clock {
            return Err(format!("{command} does not support clock objects"));
        }
        Ok(())
    }

    fn validate_transition_load_object(
        object: &TimingObjectRef,
        command: &str,
    ) -> Result<(), String> {
        Self::validate_scoped_quality_object(object, command)?;
        if !matches!(
            object.kind,
            TimingObjectKind::Port | TimingObjectKind::Pin | TimingObjectKind::Net
        ) {
            return Err(format!("{command} supports ports, pins, and nets only"));
        }
        Ok(())
    }

    pub fn quality_options(&self) -> crate::QualityOptions {
        crate::QualityOptions {
            max_transition_ns: self.max_transition_ns,
            max_capacitance_pf: self.max_capacitance_pf,
            max_fanout: self.max_fanout,
            scoped_max_transition_ns: self.scoped_max_transition_ns.clone(),
            scoped_max_capacitance_pf: self.scoped_max_capacitance_pf.clone(),
            scoped_max_fanout: self.scoped_max_fanout.clone(),
            input_transitions: self.input_transitions.clone(),
            load_capacitances: self.load_capacitances.clone(),
            ..Default::default()
        }
    }

    /// Set a cell-specific delay derate. Omitted components inherit the
    /// current global factor rather than silently resetting to 1.0.
    pub fn set_cell_timing_derate(
        &mut self,
        cell: impl AsRef<str>,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        let cell = cell.as_ref();
        if cell.trim().is_empty() {
            return Err("cell timing derate object cannot be empty".into());
        }
        for (kind, factor) in [("early", early), ("late", late)] {
            if let Some(factor) = factor {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(format!(
                        "{} cell timing derate must be positive and finite",
                        kind
                    ));
                }
            }
        }
        let mut derate = self
            .cell_timing_derates
            .get(cell)
            .copied()
            .unwrap_or(self.timing_derate);
        if let Some(early) = early {
            derate.early = early;
        }
        if let Some(late) = late {
            derate.late = late;
        }
        self.cell_timing_derates.insert(cell.to_string(), derate);
        Ok(())
    }

    /// Add a cell timing-derate rule selected by cell and hierarchy patterns.
    pub fn add_scoped_cell_timing_derate(
        &mut self,
        cell_pattern: impl Into<String>,
        instance_pattern: Option<String>,
        check: bool,
        derate: TimingDerate,
    ) -> Result<(), String> {
        let cell_pattern = cell_pattern.into();
        if cell_pattern.trim().is_empty() {
            return Err("scoped cell timing derate pattern cannot be empty".into());
        }
        if instance_pattern
            .as_ref()
            .is_some_and(|instance| instance.trim().is_empty())
        {
            return Err("scoped cell timing derate instance pattern cannot be empty".into());
        }
        let rule = ScopedCellTimingDerate {
            cell_pattern,
            instance_pattern,
            check,
            derate,
        };
        self.scoped_cell_timing_derates.push(rule);
        Ok(())
    }

    /// Set a port-specific external-delay derate. Input launch delays and
    /// output external delays use the corresponding early/late factor.
    pub fn set_port_timing_derate(
        &mut self,
        port: impl AsRef<str>,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        let port = port.as_ref();
        if port.trim().is_empty() {
            return Err("port timing derate object cannot be empty".into());
        }
        for (kind, factor) in [("early", early), ("late", late)] {
            if let Some(factor) = factor {
                if !factor.is_finite() || factor <= 0.0 {
                    return Err(format!(
                        "{} port timing derate must be positive and finite",
                        kind
                    ));
                }
            }
        }
        let mut derate = self
            .port_timing_derates
            .get(port)
            .copied()
            .unwrap_or(self.timing_derate);
        if let Some(early) = early {
            derate.early = early;
        }
        if let Some(late) = late {
            derate.late = late;
        }
        self.port_timing_derates.insert(port.to_string(), derate);
        Ok(())
    }

    /// Set a pin-specific delay derate. An input-pin rule overrides the
    /// enclosing cell for that one input-to-output arc; an output-pin rule
    /// applies to all arcs reaching that output and to sequential clk-to-q.
    pub fn set_pin_timing_derate(
        &mut self,
        cell: impl AsRef<str>,
        pin: impl AsRef<str>,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        let cell = cell.as_ref().trim();
        let pin = pin.as_ref().trim();
        if cell.is_empty() || pin.is_empty() {
            return Err("pin timing derate requires <cell>/<pin>".into());
        }
        validate_derate(early, late, "pin timing derate")?;
        let key = TimingPin::new(cell, pin);
        let mut derate = self
            .pin_timing_derates
            .get(&key)
            .copied()
            .unwrap_or_else(|| {
                self.cell_timing_derates
                    .get(cell)
                    .copied()
                    .unwrap_or(self.timing_derate)
            });
        if let Some(early) = early {
            derate.early = early;
        }
        if let Some(late) = late {
            derate.late = late;
        }
        self.pin_timing_derates.insert(key, derate);
        Ok(())
    }

    /// Add a pin timing-derate rule selected by cell, pin, and hierarchy.
    pub fn add_scoped_pin_timing_derate(
        &mut self,
        cell_pattern: impl Into<String>,
        pin_pattern: impl Into<String>,
        instance_pattern: Option<String>,
        check: bool,
        derate: TimingDerate,
    ) -> Result<(), String> {
        let cell_pattern = cell_pattern.into();
        let pin_pattern = pin_pattern.into();
        if cell_pattern.trim().is_empty() || pin_pattern.trim().is_empty() {
            return Err("scoped pin timing derate patterns cannot be empty".into());
        }
        if instance_pattern
            .as_ref()
            .is_some_and(|instance| instance.trim().is_empty())
        {
            return Err("scoped pin timing derate instance pattern cannot be empty".into());
        }
        self.scoped_pin_timing_derates.push(ScopedPinTimingDerate {
            cell_pattern,
            pin_pattern,
            instance_pattern,
            check,
            derate,
        });
        Ok(())
    }

    /// Set global setup/hold check derates. Setup uses the late factor and
    /// hold uses the early factor.
    pub fn set_check_timing_derate(
        &mut self,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        validate_derate(early, late, "check timing derate")?;
        if let Some(early) = early {
            self.check_timing_derate.early = early;
        }
        if let Some(late) = late {
            self.check_timing_derate.late = late;
        }
        Ok(())
    }

    /// Set a cell-specific setup/hold check derate.
    pub fn set_cell_check_derate(
        &mut self,
        cell: impl AsRef<str>,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        let cell = cell.as_ref();
        if cell.trim().is_empty() {
            return Err("cell check derate object cannot be empty".into());
        }
        validate_derate(early, late, "cell check derate")?;
        let mut derate = self
            .cell_check_derates
            .get(cell)
            .copied()
            .unwrap_or(self.check_timing_derate);
        if let Some(early) = early {
            derate.early = early;
        }
        if let Some(late) = late {
            derate.late = late;
        }
        self.cell_check_derates.insert(cell.to_string(), derate);
        Ok(())
    }

    /// Set a pin-specific setup/hold/recovery/removal/min-pulse check derate.
    /// The pin must be the sequential data, reset, or clock input associated
    /// with the check.
    pub fn set_pin_check_derate(
        &mut self,
        cell: impl AsRef<str>,
        pin: impl AsRef<str>,
        early: Option<f64>,
        late: Option<f64>,
    ) -> Result<(), String> {
        let cell = cell.as_ref().trim();
        let pin = pin.as_ref().trim();
        if cell.is_empty() || pin.is_empty() {
            return Err("pin check derate requires <cell>/<pin>".into());
        }
        validate_derate(early, late, "pin check derate")?;
        let key = TimingPin::new(cell, pin);
        let mut derate = self
            .pin_check_derates
            .get(&key)
            .copied()
            .unwrap_or_else(|| {
                self.cell_check_derates
                    .get(cell)
                    .copied()
                    .unwrap_or(self.check_timing_derate)
            });
        if let Some(early) = early {
            derate.early = early;
        }
        if let Some(late) = late {
            derate.late = late;
        }
        self.pin_check_derates.insert(key, derate);
        Ok(())
    }

    fn check_clock(&self, name: &str) -> Result<(), String> {
        if self.clocks.contains_key(name) {
            Ok(())
        } else {
            Err(format!("unknown clock '{}'", name))
        }
    }

    pub fn default_clock(&self) -> Result<&Clock, String> {
        self.clocks
            .values()
            .next()
            .ok_or_else(|| "SDC has no clock definition".into())
    }
}

fn changed_map_names<K, V>(old: &BTreeMap<K, V>, new: &BTreeMap<K, V>) -> BTreeSet<K>
where
    K: Clone + Ord,
    V: PartialEq,
{
    old.keys()
        .chain(new.keys())
        .filter(|key| old.get(*key) != new.get(*key))
        .cloned()
        .collect()
}

/// Compare two SDC constraint sets and classify incremental STA work.
pub fn diff_timing_constraints(
    old: &TimingConstraints,
    new: &TimingConstraints,
) -> TimingConstraintDiff {
    let mut diff = TimingConstraintDiff {
        changed_clocks: changed_map_names(&old.clocks, &new.clocks),
        changed_input_ports: changed_map_names(&old.input_delays, &new.input_delays),
        changed_output_ports: changed_map_names(&old.output_delays, &new.output_delays),
        changed_cell_derates: changed_map_names(&old.cell_timing_derates, &new.cell_timing_derates),
        changed_pin_derates: changed_map_names(&old.pin_timing_derates, &new.pin_timing_derates),
        changed_port_derates: changed_map_names(&old.port_timing_derates, &new.port_timing_derates),
        changed_cell_check_derates: changed_map_names(
            &old.cell_check_derates,
            &new.cell_check_derates,
        ),
        changed_pin_check_derates: changed_map_names(
            &old.pin_check_derates,
            &new.pin_check_derates,
        ),
        changed_clock_uncertainty: changed_map_names(
            &old.clock_uncertainty_ns,
            &new.clock_uncertainty_ns,
        ),
        changed_hold_uncertainty: changed_map_names(
            &old.hold_clock_uncertainty_ns,
            &new.hold_clock_uncertainty_ns,
        ),
        timing_exceptions_changed: old.false_paths != new.false_paths
            || old.multicycle_paths != new.multicycle_paths,
        clock_groups_changed: old.clock_groups != new.clock_groups,
        delay_limits_changed: old.max_delay_paths != new.max_delay_paths
            || old.min_delay_paths != new.min_delay_paths,
        disabled_timing_changed: old.disabled_timing_arcs != new.disabled_timing_arcs,
        case_analysis_changed: old.case_analyses != new.case_analyses,
        global_timing_derate_changed: old.timing_derate != new.timing_derate,
        global_check_derate_changed: old.check_timing_derate != new.check_timing_derate,
        design_rules_changed: old.max_transition_ns != new.max_transition_ns
            || old.max_capacitance_pf != new.max_capacitance_pf
            || old.max_fanout != new.max_fanout
            || old.scoped_max_transition_ns != new.scoped_max_transition_ns
            || old.scoped_max_capacitance_pf != new.scoped_max_capacitance_pf
            || old.scoped_max_fanout != new.scoped_max_fanout
            || old.input_transitions != new.input_transitions
            || old.load_capacitances != new.load_capacitances,
        input_transitions_changed: old.input_transitions != new.input_transitions,
        loads_changed: old.load_capacitances != new.load_capacitances,
        scoped_timing_derates_changed: old.scoped_cell_timing_derates
            != new.scoped_cell_timing_derates
            || old.scoped_pin_timing_derates != new.scoped_pin_timing_derates,
        ..TimingConstraintDiff::default()
    };

    diff.full_base_recompute_required = diff.case_analysis_changed
        || diff.disabled_timing_changed
        || diff.global_timing_derate_changed
        || diff.scoped_timing_derates_changed;
    diff.base_recompute_required = diff.full_base_recompute_required
        || !diff.changed_input_ports.is_empty()
        || !diff.changed_cell_derates.is_empty()
        || !diff.changed_pin_derates.is_empty()
        || !diff.changed_port_derates.is_empty();
    diff.constrained_recompute_required = diff.base_recompute_required
        || diff.global_check_derate_changed
        || diff.timing_exceptions_changed
        || diff.clock_groups_changed
        || diff.delay_limits_changed
        || !diff.changed_clocks.is_empty()
        || !diff.changed_output_ports.is_empty()
        || !diff.changed_cell_check_derates.is_empty()
        || !diff.changed_pin_check_derates.is_empty()
        || !diff.changed_clock_uncertainty.is_empty()
        || !diff.changed_hold_uncertainty.is_empty();
    diff.quality_recompute_required = diff.design_rules_changed;
    diff
}

#[derive(Clone, Debug)]
pub struct ConstrainedTiming {
    pub base: StaResult,
    pub clock: String,
    pub period: f64,
    pub clock_rise: f64,
    pub clock_fall: f64,
    pub uncertainty: f64,
    pub hold_uncertainty: f64,
    pub source_latency: f64,
    pub network_latency: f64,
    pub early_derate: f64,
    pub late_derate: f64,
    pub cell_derates: BTreeMap<String, TimingDerate>,
    pub port_derates: BTreeMap<String, TimingDerate>,
    pub pin_derates: BTreeMap<TimingPin, TimingDerate>,
    pub early_check_derate: f64,
    pub late_check_derate: f64,
    pub cell_check_derates: BTreeMap<String, TimingDerate>,
    pub pin_check_derates: BTreeMap<TimingPin, TimingDerate>,
    pub reg2reg_slack: f64,
    pub in2reg_slack: f64,
    pub reg2reg_hold_slack: f64,
    pub in2reg_hold_slack: f64,
    pub hold_slack: f64,
    pub output_slack: f64,
    pub max_delay_slack: f64,
    pub min_delay_slack: f64,
    pub min_pulse_high_slack: f64,
    pub min_pulse_low_slack: f64,
    pub min_pulse_width_slack: f64,
    /// Every concrete sequential min-pulse check, including positive-slack
    /// checks. Checks are retained by stable cell/clock names for violation
    /// tracking and waiver audit.
    pub min_pulse_width_checks: Vec<MinPulseWidthCheck>,
    pub recovery_slack: f64,
    pub removal_slack: f64,
    pub async_reset_slack: f64,
    pub worst_slack: f64,
    pub exceptions_applied: usize,
    /// Endpoint-name to multicycle edge multiplier used for clock metadata.
    pub path_cycle_multipliers: BTreeMap<String, f64>,
    /// Endpoint-object-keyed multicycle multipliers, disambiguating pins and
    /// ports that reference the same net.
    pub path_cycle_multipliers_by_endpoint: BTreeMap<String, f64>,
    /// Exception objects matching each endpoint, including false paths.
    pub path_exceptions: BTreeMap<String, Vec<PathException>>,
    /// Matched max-delay constraints by endpoint-object key.
    pub path_max_delay_limits: BTreeMap<String, Vec<AppliedTimingDelayLimit>>,
    /// Matched min-delay constraints by endpoint-object key.
    pub path_min_delay_limits: BTreeMap<String, Vec<AppliedTimingDelayLimit>>,
    /// Worst representative slack per net. Endpoint nets retain setup/hold
    /// slacks; external outputs retain output/max-delay slacks.
    pub net_slacks: BTreeMap<String, NetSlack>,
}

/// Polarity of a sequential clock min-pulse-width check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MinPulsePolarity {
    High,
    Low,
}

impl MinPulsePolarity {
    pub fn label(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

/// One register clock-pin min-pulse check.
#[derive(Clone, Debug)]
pub struct MinPulseWidthCheck {
    pub cell: String,
    pub clock: String,
    pub polarity: MinPulsePolarity,
    /// Actual high or low clock width in ns.
    pub actual_ns: f64,
    /// Library requirement before check derate, in ns.
    pub required_ns: f64,
    /// Library requirement after check derate, in ns.
    pub effective_required_ns: f64,
    pub slack_ns: f64,
}

#[derive(Clone, Debug)]
pub struct ConstrainedTimingPath {
    pub path: TimingPath,
    pub slack_ns: f64,
    pub required_ns: f64,
    pub clocks: TimingPathClocks,
    pub exceptions: Vec<PathException>,
    pub delay_limits: Vec<AppliedTimingDelayLimit>,
}

#[derive(Clone, Debug)]
pub struct ConstrainedHoldTimingPath {
    pub path: HoldTimingPath,
    pub slack_ns: f64,
    pub required_ns: f64,
    pub clocks: TimingPathClocks,
    pub exceptions: Vec<PathException>,
    pub delay_limits: Vec<AppliedTimingDelayLimit>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetSlack {
    pub setup_ns: Option<f64>,
    pub hold_ns: Option<f64>,
    pub output_ns: Option<f64>,
    pub max_delay_ns: Option<f64>,
    pub min_delay_ns: Option<f64>,
    pub recovery_ns: Option<f64>,
    pub removal_ns: Option<f64>,
}

impl NetSlack {
    /// Minimum setup-like slack. This is the value used by late-path
    /// physical optimization; hold is intentionally kept separate.
    pub fn late_slack_ns(&self) -> Option<f64> {
        [
            self.setup_ns,
            self.output_ns,
            self.max_delay_ns,
            self.min_delay_ns,
            self.recovery_ns,
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }

    pub fn worst_slack_ns(&self) -> Option<f64> {
        [
            self.setup_ns,
            self.hold_ns,
            self.output_ns,
            self.max_delay_ns,
            self.recovery_ns,
            self.removal_ns,
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetCheckKind {
    Recovery,
    Removal,
}

#[derive(Clone, Debug)]
pub struct ConstrainedResetTimingPath {
    pub path: TimingPath,
    pub kind: ResetCheckKind,
    pub slack_ns: f64,
    pub required_ns: f64,
    pub clocks: TimingPathClocks,
    pub exceptions: Vec<PathException>,
}

fn record_net_slack(
    out: &mut BTreeMap<String, NetSlack>,
    net: &str,
    setup: Option<f64>,
    hold: Option<f64>,
    output: Option<f64>,
    max_delay: Option<f64>,
    min_delay: Option<f64>,
) {
    let entry = out.entry(net.to_string()).or_default();
    if let Some(value) = setup {
        entry.setup_ns = Some(entry.setup_ns.map_or(value, |old| old.min(value)));
    }
    if let Some(value) = hold {
        entry.hold_ns = Some(entry.hold_ns.map_or(value, |old| old.min(value)));
    }
    if let Some(value) = output {
        entry.output_ns = Some(entry.output_ns.map_or(value, |old| old.min(value)));
    }
    if let Some(value) = max_delay {
        entry.max_delay_ns = Some(entry.max_delay_ns.map_or(value, |old| old.min(value)));
    }
    if let Some(value) = min_delay {
        entry.min_delay_ns = Some(entry.min_delay_ns.map_or(value, |old| old.min(value)));
    }
}

/// Human-readable constrained timing report.
pub fn constrained_report(r: &ConstrainedTiming) -> String {
    let mut out = format!(
        "Constrained STA report for clock '{}': period {:.3} ns, setup uncertainty {:.3} ns, hold uncertainty {:.3} ns, source latency {:.3} ns, network latency {:.3} ns, early derate {:.3}, late derate {:.3}, check early {:.3}, check late {:.3}\n  worst slack : {:.3} ns\n  setup reg-to-reg : {:.3} ns\n  setup in-to-reg  : {:.3} ns\n  hold reg-to-reg  : {:.3} ns\n  hold in-to-reg   : {:.3} ns\n  max delay     : {:.3} ns\n  min delay     : {:.3} ns\n  min pulse high : {:.3} ns\n  min pulse low  : {:.3} ns\n  recovery      : {:.3} ns\n  removal       : {:.3} ns\n  output        : {:.3} ns\n  exceptions    : {}\n  disabled arcs : {}\n  case constants: {}\n  cell derates   : {}\n  port derates   : {}\n  pin derates   : {}\n  check derates  : {}\n  pin check derates : {}\n",
        r.clock,
        r.period,
        r.uncertainty,
        r.hold_uncertainty,
        r.source_latency,
        r.network_latency,
        r.early_derate,
        r.late_derate,
        r.early_check_derate,
        r.late_check_derate,
        r.worst_slack,
        r.reg2reg_slack,
        r.in2reg_slack,
        r.reg2reg_hold_slack,
        r.in2reg_hold_slack,
        r.max_delay_slack,
        r.min_delay_slack,
        r.min_pulse_high_slack,
        r.min_pulse_low_slack,
        r.recovery_slack,
        r.removal_slack,
        r.output_slack,
        r.exceptions_applied,
        r.base.disabled_timing_arcs,
        r.base.case_analysis_nets.len(),
        r.cell_derates.len(),
        r.port_derates.len(),
        r.pin_derates.len(),
        r.cell_check_derates.len(),
        r.pin_check_derates.len()
    );
    for check in &r.min_pulse_width_checks {
        if check.slack_ns < 0.0 && check.slack_ns.is_finite() {
            out.push_str(&format!(
                "  min pulse {} violation at '{}' clock '{}': actual {:.6} ns, required {:.6} ns, slack {:.6} ns\n",
                check.polarity.label(),
                check.cell,
                check.clock,
                check.actual_ns,
                check.effective_required_ns,
                check.slack_ns
            ));
        }
    }
    out
}

#[cfg(test)]
#[path = "constraints/tests.rs"]
mod tests;
