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

/// Parse an SDC subset. Each command must be on its own line.
pub fn parse_sdc(source: &str) -> Result<TimingConstraints, String> {
    let mut out = TimingConstraints::default();
    let mut units = SdcUnits::default();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let tokens = tokenize(line).map_err(|e| format!("line {}: {}", line_number, e))?;
        if tokens.first().map(String::as_str) == Some("set_units") {
            parse_set_units(&mut units, &tokens)
                .map_err(|e| format!("line {}: {}", line_number, e))?;
            out.units = units.clone();
            continue;
        }
        let normalized = normalize_unit_values(&tokens, &units)
            .map_err(|e| format!("line {}: {}", line_number, e))?;
        parse_command(&mut out, &normalized).map_err(|e| format!("line {}: {}", line_number, e))?;
    }
    Ok(out)
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(pos) => &line[..pos],
        None => line,
    }
}

fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '[' || c == ']' {
            chars.next();
            out.push(c.to_string());
        } else if c == '"' {
            chars.next();
            let mut word = String::new();
            let mut closed = false;
            for ch in chars.by_ref() {
                if ch == '"' {
                    closed = true;
                    break;
                }
                word.push(ch);
            }
            if !closed {
                return Err("unterminated quoted string".into());
            }
            out.push(word);
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == '[' || c == ']' || c == '"' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            if word.is_empty() {
                return Err("empty SDC token".into());
            }
            out.push(word);
        }
    }
    Ok(out)
}

fn parse_command(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    match tokens.first().map(String::as_str) {
        Some("create_clock") => parse_create_clock(out, tokens),
        Some("create_generated_clock") => parse_generated_clock(out, tokens),
        Some("set_clock_groups") => parse_clock_group(out, tokens),
        Some("set_input_delay") => parse_port_delay(out, tokens, false),
        Some("set_output_delay") => parse_port_delay(out, tokens, true),
        Some("set_clock_uncertainty") => parse_uncertainty(out, tokens),
        Some("set_clock_latency") => parse_clock_latency(out, tokens),
        Some("set_clock_transition") => parse_clock_transition(out, tokens),
        Some("set_false_path") => parse_exception(out, tokens, false),
        Some("set_multicycle_path") => parse_exception(out, tokens, true),
        Some("set_disable_timing") => parse_disable_timing(out, tokens),
        Some("set_case_analysis") => parse_case_analysis(out, tokens),
        Some("set_max_delay") => parse_delay_limit(out, tokens, true),
        Some("set_min_delay") => parse_delay_limit(out, tokens, false),
        Some("set_timing_derate") => parse_timing_derate(out, tokens),
        Some("set_max_transition") => parse_quality_limit(out, tokens, false),
        Some("set_max_capacitance") => parse_quality_limit(out, tokens, true),
        Some("set_max_fanout") => parse_max_fanout(out, tokens),
        Some("set_input_transition") => parse_input_transition(out, tokens),
        Some("set_load") => parse_load(out, tokens),
        Some(other) => Err(format!("unsupported SDC command '{}'", other)),
        None => Err("empty SDC command".into()),
    }
}

fn unit_factor(value: &str, dimension: &str) -> Result<(String, f64), String> {
    let normalized = value.trim().trim_start_matches('1').to_ascii_lowercase();
    let normalized = normalized.replace('_', "");
    if normalized.is_empty() || !normalized.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(format!("invalid {dimension} unit '{value}'"));
    }
    let factor = match dimension {
        "time" => match normalized.as_str() {
            "fs" => 1.0e-6,
            "ps" => 1.0e-3,
            "ns" => 1.0,
            "us" => 1.0e3,
            "ms" => 1.0e6,
            "s" => 1.0e9,
            _ => return Err(format!("unsupported time unit '{value}'")),
        },
        "capacitance" => match normalized.as_str() {
            "ff" => 1.0e-3,
            "pf" => 1.0,
            "nf" => 1.0e3,
            "uf" => 1.0e6,
            "mf" => 1.0e9,
            "f" => 1.0e-3,
            _ => return Err(format!("unsupported capacitance unit '{value}'")),
        },
        "resistance" => match normalized.as_str() {
            "mohm" => 1.0e-3,
            "ohm" => 1.0,
            "kohm" => 1.0e3,
            "megohm" => 1.0e6,
            "gohm" => 1.0e9,
            _ => return Err(format!("unsupported resistance unit '{value}'")),
        },
        "voltage" => match normalized.as_str() {
            "uv" => 1.0e-6,
            "mv" => 1.0e-3,
            "v" => 1.0,
            "kv" => 1.0e3,
            _ => return Err(format!("unsupported voltage unit '{value}'")),
        },
        "current" => match normalized.as_str() {
            "ua" => 1.0e-6,
            "ma" => 1.0e-3,
            "a" => 1.0,
            "ka" => 1.0e3,
            _ => return Err(format!("unsupported current unit '{value}'")),
        },
        _ => return Err(format!("unknown unit dimension '{dimension}'")),
    };
    Ok((normalized, factor))
}

fn parse_set_units(units: &mut SdcUnits, tokens: &[String]) -> Result<(), String> {
    const OPTIONS: [(&str, &str); 5] = [
        ("-time", "time"),
        ("-capacitance", "capacitance"),
        ("-resistance", "resistance"),
        ("-voltage", "voltage"),
        ("-current", "current"),
    ];
    let mut seen = BTreeSet::new();
    for flag in tokens[1..].iter().filter(|token| token.starts_with('-')) {
        if tokens[1..]
            .iter()
            .filter(|other| other.as_str() == flag.as_str())
            .count()
            > 1
        {
            return Err(format!("duplicate set_units option '{flag}'"));
        }
    }
    for (flag, dimension) in OPTIONS {
        let Some(value) = value_after(tokens, flag)? else {
            continue;
        };
        if !seen.insert(flag.to_string()) {
            return Err(format!("duplicate set_units option '{flag}'"));
        }
        let (unit, factor) = unit_factor(&value, dimension)?;
        match dimension {
            "time" => {
                units.time_unit = unit;
                units.time_to_ns_factor = factor;
            }
            "capacitance" => {
                units.capacitance_unit = unit;
                units.capacitance_to_pf_factor = factor;
            }
            "resistance" => {
                units.resistance_unit = unit;
                units.resistance_to_ohm_factor = factor;
            }
            "voltage" => {
                units.voltage_unit = unit;
                units.voltage_to_v_factor = factor;
            }
            "current" => {
                units.current_unit = unit;
                units.current_to_a_factor = factor;
            }
            _ => unreachable!(),
        }
    }
    if seen.is_empty() {
        return Err("set_units requires at least one unit option".into());
    }
    for token in &tokens[1..] {
        if token.starts_with('-') && !seen.contains(token) {
            return Err(format!("unsupported set_units option '{token}'"));
        }
    }
    Ok(())
}

fn scale_token_at(tokens: &mut [String], index: usize, factor: f64) {
    if let Some(token) = tokens.get(index) {
        if let Ok(value) = token.parse::<f64>() {
            tokens[index] = format!("{}", value * factor);
        }
    }
}

fn first_number_position(tokens: &[String]) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, token)| token.parse::<f64>().is_ok())
        .map(|(index, _)| index)
}

fn normalize_unit_values(tokens: &[String], units: &SdcUnits) -> Result<Vec<String>, String> {
    let mut out = tokens.to_vec();
    let command = tokens.first().map(String::as_str).unwrap_or_default();
    match command {
        "create_clock" => {
            if let Some(index) = tokens.iter().position(|token| token == "-period") {
                scale_token_at(&mut out, index + 1, units.time_to_ns_factor);
            }
            for flag in ["-edge_rise", "-rise", "-edge_fall", "-fall"] {
                if let Some(index) = tokens.iter().position(|token| token == flag) {
                    scale_token_at(&mut out, index + 1, units.time_to_ns_factor);
                }
            }
        }
        "set_input_delay"
        | "set_output_delay"
        | "set_clock_uncertainty"
        | "set_max_delay"
        | "set_min_delay"
        | "set_input_transition"
        | "set_max_transition" => scale_token_at(&mut out, 1, units.time_to_ns_factor),
        "set_clock_latency" | "set_clock_transition" => {
            if let Some(index) = first_number_position(tokens) {
                scale_token_at(&mut out, index, units.time_to_ns_factor);
            }
        }
        "set_max_capacitance" => scale_token_at(&mut out, 1, units.capacitance_to_pf_factor),
        "set_load" => {
            let index = if tokens.get(1).map(String::as_str) == Some("-pin_load") {
                2
            } else {
                1
            };
            scale_token_at(&mut out, index, units.capacitance_to_pf_factor);
        }
        _ => {}
    }
    Ok(out)
}

fn value_after(tokens: &[String], flag: &str) -> Result<Option<String>, String> {
    match tokens.iter().position(|t| t == flag) {
        Some(pos) => tokens
            .get(pos + 1)
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("missing value after {}", flag)),
        None => Ok(None),
    }
}

fn access_list(tokens: &[String], flag: &str) -> Result<Vec<String>, String> {
    let pos = tokens
        .iter()
        .position(|t| t == flag)
        .ok_or_else(|| format!("missing {}", flag))?;
    let mut pos = pos + 1;
    if tokens.get(pos).map(String::as_str) != Some("[") {
        return Err(format!("{} must be followed by an access list", flag));
    }
    pos += 1;
    let mut names = Vec::new();
    while pos < tokens.len() && tokens[pos] != "]" {
        // Skip the accessor itself (get_ports/get_pins/get_clocks/etc.).
        if pos == pos.saturating_sub(1) || !names.is_empty() || !tokens[pos].starts_with("get_") {
            names.push(tokens[pos].clone());
        }
        pos += 1;
    }
    if pos >= tokens.len() {
        return Err(format!("unterminated access list after {}", flag));
    }
    if names.is_empty() {
        return Err(format!("empty access list after {}", flag));
    }
    Ok(names)
}

fn access_groups(tokens: &[String], flag: &str) -> Result<Vec<Vec<String>>, String> {
    let mut groups = Vec::new();
    let mut start = 0;
    while let Some(flag_pos) = tokens[start..].iter().position(|token| token == flag) {
        let flag_pos = flag_pos + start;
        let mut pos = flag_pos + 1;
        if tokens.get(pos).map(String::as_str) != Some("[") {
            return Err(format!("{flag} must be followed by an access list"));
        }
        pos += 1;
        let mut names = Vec::new();
        while pos < tokens.len() && tokens[pos] != "]" {
            if !names.is_empty() || !tokens[pos].starts_with("get_") {
                names.push(tokens[pos].clone());
            }
            pos += 1;
        }
        if pos >= tokens.len() {
            return Err(format!("unterminated access list after {flag}"));
        }
        if names.is_empty() {
            return Err(format!("empty access list after {flag}"));
        }
        groups.push(names);
        start = pos + 1;
    }
    if groups.is_empty() {
        return Err(format!("missing {flag}"));
    }
    Ok(groups)
}

fn typed_access_list(tokens: &[String], flag: &str) -> Result<Vec<TimingObjectRef>, String> {
    Ok(typed_access_groups(tokens, flag)?
        .into_iter()
        .next()
        .unwrap_or_default())
}

fn typed_access_groups(tokens: &[String], flag: &str) -> Result<Vec<Vec<TimingObjectRef>>, String> {
    let mut groups = Vec::new();
    let mut start = 0;
    while let Some(flag_pos) = tokens[start..].iter().position(|t| t == flag) {
        let flag_pos = flag_pos + start;
        let mut pos = flag_pos + 1;
        if tokens.get(pos).map(String::as_str) != Some("[") {
            return Err(format!("{} must be followed by an access list", flag));
        }
        pos += 1;

        let mut objects = Vec::new();
        let mut kind: Option<TimingObjectKind> = None;
        while pos < tokens.len() && tokens[pos] != "]" {
            let token = &tokens[pos];
            if token.starts_with("get_") {
                kind = Some(match token.as_str() {
                    "get_ports" => TimingObjectKind::Port,
                    "get_pins" => TimingObjectKind::Pin,
                    "get_nets" => TimingObjectKind::Net,
                    "get_cells" => TimingObjectKind::Cell,
                    "get_clocks" => TimingObjectKind::Clock,
                    other => return Err(format!("unsupported SDC accessor '{}'", other)),
                });
            } else {
                let kind = kind.unwrap_or(TimingObjectKind::Net);
                objects.push(TimingObjectRef::new(kind, token.clone()));
            }
            pos += 1;
        }
        if pos >= tokens.len() {
            return Err(format!("unterminated access list after {}", flag));
        }
        if objects.is_empty() {
            return Err(format!("empty access list after {}", flag));
        }
        groups.push(objects);
        start = pos + 1;
    }
    if groups.is_empty() {
        return Err(format!("missing {}", flag));
    }
    Ok(groups)
}

fn typed_trailing_access_list(tokens: &[String]) -> Result<Vec<TimingObjectRef>, String> {
    let mut objects = Vec::new();
    for (open, token) in tokens.iter().enumerate() {
        if token != "[" {
            continue;
        }
        let attached_to_flag = open > 0
            && tokens
                .get(open - 1)
                .is_some_and(|previous| previous.starts_with('-'));
        if attached_to_flag {
            continue;
        }

        let mut pos = open + 1;
        let mut kind: Option<TimingObjectKind> = None;
        let mut current = Vec::new();
        while pos < tokens.len() && tokens[pos] != "]" {
            let token = &tokens[pos];
            if token.starts_with("get_") {
                kind = Some(match token.as_str() {
                    "get_ports" => TimingObjectKind::Port,
                    "get_pins" => TimingObjectKind::Pin,
                    "get_nets" => TimingObjectKind::Net,
                    "get_cells" => TimingObjectKind::Cell,
                    "get_clocks" => TimingObjectKind::Clock,
                    other => return Err(format!("unsupported SDC accessor '{}'", other)),
                });
            } else {
                let kind = kind.ok_or_else(|| {
                    "set_disable_timing object list requires get_cells or get_pins".to_string()
                })?;
                current.push(TimingObjectRef::new(kind, token.clone()));
            }
            pos += 1;
        }
        if pos >= tokens.len() {
            return Err("unterminated set_disable_timing object list".into());
        }
        objects.extend(current);
    }
    Ok(objects)
}

fn upsert_scoped_value(values: &mut Vec<ScopedQualityLimit>, object: TimingObjectRef, value: f64) {
    values.retain(|rule| rule.object != object);
    values.push(ScopedQualityLimit {
        object,
        limit: value,
    });
}

fn trailing_access_list(tokens: &[String]) -> Result<Vec<String>, String> {
    let Some(open) = tokens.iter().position(|t| t == "[") else {
        return Err("missing SDC access list".into());
    };
    let mut pos = open + 1;
    let mut names = Vec::new();
    while pos < tokens.len() && tokens[pos] != "]" {
        if !names.is_empty() || !tokens[pos].starts_with("get_") {
            names.push(tokens[pos].clone());
        }
        pos += 1;
    }
    if pos >= tokens.len() {
        return Err("unterminated SDC access list".into());
    }
    if names.is_empty() {
        return Err("empty SDC access list".into());
    }
    Ok(names)
}

fn parse_create_clock(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let name = match value_after(tokens, "-name")? {
        Some(name) => name,
        None => "clk".to_string(),
    };
    let period = value_after(tokens, "-period")?
        .ok_or_else(|| "create_clock requires -period".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid create_clock period".to_string())?;
    let rise = match value_after(tokens, "-edge_rise")?.or(value_after(tokens, "-rise")?) {
        Some(v) => v
            .parse::<f64>()
            .map_err(|_| "invalid rise time".to_string())?,
        None => 0.0,
    };
    let fall = match value_after(tokens, "-edge_fall")?.or(value_after(tokens, "-fall")?) {
        Some(v) => v
            .parse::<f64>()
            .map_err(|_| "invalid fall time".to_string())?,
        None => period / 2.0,
    };
    let ports = if tokens.iter().any(|t| t == "-ports") {
        access_list(tokens, "-ports")?
    } else {
        trailing_access_list(tokens).unwrap_or_default()
    };
    out.add_clock(Clock {
        name,
        period,
        rise,
        fall,
        source_latency_ns: 0.0,
        network_latency_ns: 0.0,
        rise_transition_ns: 0.0,
        fall_transition_ns: 0.0,
        min_rise_transition_ns: 0.0,
        min_fall_transition_ns: 0.0,
        source_clock: None,
        divide_by: None,
        multiply_by: None,
        ports,
    })
}

fn parse_generated_clock(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let name = value_after(tokens, "-name")?
        .ok_or_else(|| "create_generated_clock requires -name".to_string())?;
    let source = if tokens.iter().any(|token| token == "-source") {
        access_list(tokens, "-source")?
            .first()
            .cloned()
            .ok_or_else(|| "create_generated_clock source list cannot be empty".to_string())?
    } else {
        return Err("create_generated_clock requires -source".into());
    };
    let divide_by = match value_after(tokens, "-divide_by")? {
        Some(value) => Some(
            value
                .parse::<f64>()
                .map_err(|_| "invalid -divide_by value".to_string())?,
        ),
        None => None,
    };
    let multiply_by = match value_after(tokens, "-multiply_by")? {
        Some(value) => Some(
            value
                .parse::<f64>()
                .map_err(|_| "invalid -multiply_by value".to_string())?,
        ),
        None => None,
    };
    let ports = if tokens.iter().any(|token| token == "-ports") {
        access_list(tokens, "-ports")?
    } else {
        trailing_access_list(tokens).unwrap_or_default()
    };
    out.add_generated_clock(name, &source, divide_by, multiply_by, ports)
}

fn parse_clock_group(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let asynchronous = tokens.iter().any(|token| token == "-asynchronous");
    let logical = tokens.iter().any(|token| token == "-logically_exclusive");
    let physical = tokens.iter().any(|token| token == "-physically_exclusive");
    if usize::from(asynchronous) + usize::from(logical) + usize::from(physical) != 1 {
        return Err("set_clock_groups requires exactly one relationship kind".into());
    }
    let kind = if asynchronous {
        ClockGroupKind::Asynchronous
    } else if logical {
        ClockGroupKind::LogicallyExclusive
    } else {
        ClockGroupKind::PhysicallyExclusive
    };
    let name = value_after(tokens, "-name")?
        .unwrap_or_else(|| format!("clock_group_{}", out.clock_groups.len() + 1));
    out.add_clock_group(ClockGroup {
        name,
        kind,
        groups: access_groups(tokens, "-group")?,
    })
}

fn parse_clock_latency(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let source = tokens.iter().any(|token| token == "-source");
    let value_pos = tokens
        .iter()
        .position(|token| token.parse::<f64>().is_ok())
        .ok_or_else(|| "set_clock_latency requires a numeric latency".to_string())?;
    let latency = tokens[value_pos]
        .parse::<f64>()
        .map_err(|_| "invalid clock latency".to_string())?;
    let clocks = if tokens.iter().any(|token| token == "-clocks") {
        access_list(tokens, "-clocks")?
    } else {
        trailing_access_list(tokens)?
    };
    if clocks.is_empty() {
        return Err("set_clock_latency requires a clock access list".into());
    }
    for clock in clocks {
        out.set_clock_latency(clock, latency, source)?;
    }
    Ok(())
}

fn parse_clock_transition(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let rise = tokens.iter().any(|token| token == "-rise");
    let fall = tokens.iter().any(|token| token == "-fall");
    let minimum = tokens.iter().any(|token| token == "-min");
    let maximum = tokens.iter().any(|token| token == "-max");
    let value_position = tokens
        .iter()
        .enumerate()
        .skip(1)
        .find(|(position, token)| {
            *position > 0
                && !token.starts_with('-')
                && tokens
                    .get(*position - 1)
                    .is_some_and(|previous| previous.starts_with('-'))
        })
        .map(|(position, _)| position)
        .or_else(|| {
            tokens
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, token)| !token.starts_with('-') && token.as_str() != "[")
                .map(|(position, _)| position)
        })
        .ok_or_else(|| "set_clock_transition requires a numeric value".to_string())?;
    let value = tokens[value_position]
        .parse::<f64>()
        .map_err(|_| "invalid set_clock_transition value".to_string())?;
    if tokens.iter().any(|token| {
        token.starts_with('-') && !matches!(token.as_str(), "-rise" | "-fall" | "-min" | "-max")
    }) {
        return Err("unsupported set_clock_transition option".into());
    }
    let objects = typed_trailing_access_list(&tokens[value_position + 1..])?;
    if objects.is_empty()
        || objects
            .iter()
            .any(|object| object.kind != TimingObjectKind::Clock)
    {
        return Err("set_clock_transition requires [get_clocks ...]".into());
    }
    let set_minimum = minimum || !maximum;
    let set_maximum = maximum || !minimum;
    let set_rise = rise || !fall;
    let set_fall = fall || !rise;
    let min_rise = (set_minimum && set_rise).then_some(value);
    let min_fall = (set_minimum && set_fall).then_some(value);
    let max_rise = (set_maximum && set_rise).then_some(value);
    let max_fall = (set_maximum && set_fall).then_some(value);
    for object in objects {
        out.set_clock_transition(object.name, min_rise, min_fall, max_rise, max_fall)?;
    }
    Ok(())
}

fn parse_port_delay(
    out: &mut TimingConstraints,
    tokens: &[String],
    output: bool,
) -> Result<(), String> {
    let delay = tokens
        .get(1)
        .ok_or_else(|| "port delay command requires a numeric value".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid port delay value".to_string())?;
    let clock = value_after(tokens, "-clock")?.unwrap_or_else(|| "clk".to_string());
    let ports = if tokens.iter().any(|t| t == "-ports") {
        access_list(tokens, "-ports")?
    } else {
        trailing_access_list(tokens)?
    };
    for port in ports {
        let d = PortDelay {
            port,
            clock: clock.clone(),
            delay,
        };
        if output {
            out.set_output_delay(d)?;
        } else {
            out.set_input_delay(d)?;
        }
    }
    Ok(())
}

fn parse_uncertainty(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let value = tokens
        .get(1)
        .ok_or_else(|| "set_clock_uncertainty requires a value".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid clock uncertainty".to_string())?;
    let setup = tokens.iter().any(|t| t == "-setup");
    let hold = tokens.iter().any(|t| t == "-hold");
    if setup == hold {
        return Err("set_clock_uncertainty requires exactly one of -setup or -hold".into());
    }
    let clocks = if tokens.iter().any(|t| t == "-clocks") {
        access_list(tokens, "-clocks")?
    } else if setup {
        access_list(tokens, "-setup")?
    } else {
        access_list(tokens, "-hold")?
    };
    for clock in clocks {
        if setup {
            out.set_clock_uncertainty(clock, value)?;
        } else {
            out.set_hold_clock_uncertainty(clock, value)?;
        }
    }
    Ok(())
}

fn parse_exception(
    out: &mut TimingConstraints,
    tokens: &[String],
    multicycle: bool,
) -> Result<(), String> {
    let from_object = if tokens.iter().any(|t| t == "-from") {
        typed_access_list(tokens, "-from")?.into_iter().next()
    } else {
        None
    };
    let to_object = if tokens.iter().any(|t| t == "-to") {
        typed_access_list(tokens, "-to")?.into_iter().next()
    } else {
        None
    };
    let through_groups = if tokens.iter().any(|t| t == "-through") {
        typed_access_groups(tokens, "-through")?
    } else {
        Vec::new()
    };
    let instance_pattern = parse_instance_pattern(tokens, "path exception")?;
    if from_object.is_none() && to_object.is_none() && through_groups.is_empty() {
        return Err("path exception requires -from, -to, or -through".into());
    }
    let from = from_object.as_ref().map(|object| object.name.clone());
    let to = to_object.as_ref().map(|object| object.name.clone());
    let through = through_groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|object| object.name.clone())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect::<Vec<_>>();
    let exception = if multicycle {
        let multiplier = tokens
            .get(1)
            .ok_or_else(|| "set_multicycle_path requires a multiplier".to_string())?
            .parse::<f64>()
            .map_err(|_| "invalid multicycle multiplier".to_string())?;
        if multiplier <= 0.0 {
            return Err("multicycle multiplier must be positive".into());
        }
        PathException {
            from,
            to,
            through,
            from_object,
            to_object,
            through_groups,
            instance_pattern: instance_pattern.clone(),
            multicycle: Some(multiplier),
        }
    } else {
        PathException {
            from,
            to,
            through,
            from_object,
            to_object,
            through_groups,
            instance_pattern,
            multicycle: None,
        }
    };
    if multicycle {
        out.multicycle_paths.push(exception);
    } else {
        out.false_paths.push(exception);
    }
    Ok(())
}

fn parse_instance_pattern(tokens: &[String], context: &str) -> Result<Option<String>, String> {
    let Some(position) = tokens.iter().position(|token| token == "-instance") else {
        return Ok(None);
    };
    let pattern = tokens
        .get(position + 1)
        .ok_or_else(|| format!("{} -instance requires a pattern", context))?;
    if pattern.trim().is_empty() {
        return Err(format!("{} instance pattern cannot be empty", context));
    }
    if tokens
        .iter()
        .skip(position + 2)
        .any(|token| token == "-instance")
    {
        return Err(format!("{} has duplicate -instance", context));
    }
    Ok(Some(pattern.clone()))
}

fn parse_disable_timing(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let from = if tokens.iter().any(|token| token == "-from") {
        typed_access_list(tokens, "-from")?.into_iter().next()
    } else {
        None
    };
    let to = if tokens.iter().any(|token| token == "-to") {
        typed_access_list(tokens, "-to")?.into_iter().next()
    } else {
        None
    };
    let instance_pattern = parse_instance_pattern(tokens, "disabled timing arc")?;
    let objects = typed_trailing_access_list(tokens)?;
    if objects.is_empty() {
        return out.add_disabled_timing_arc(DisabledTimingArc {
            object: None,
            from,
            to,
            instance_pattern,
        });
    }

    for object in objects {
        out.add_disabled_timing_arc(DisabledTimingArc {
            object: Some(object),
            from: from.clone(),
            to: to.clone(),
            instance_pattern: instance_pattern.clone(),
        })?;
    }
    Ok(())
}

fn parse_case_analysis(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let value = tokens
        .get(1)
        .ok_or_else(|| "set_case_analysis requires 0 or 1".to_string())?;
    let value = match value.as_str() {
        "0" => false,
        "1" => true,
        other => return Err(format!("invalid set_case_analysis value '{}'", other)),
    };
    let objects = typed_trailing_access_list(tokens)?;
    if objects.is_empty() {
        return Err("set_case_analysis requires an object access list".into());
    }
    let instance_pattern = parse_instance_pattern(tokens, "case analysis")?;
    for object in objects {
        out.add_case_analysis(CaseAnalysis {
            object,
            value,
            instance_pattern: instance_pattern.clone(),
        })?;
    }
    Ok(())
}

fn parse_delay_limit(
    out: &mut TimingConstraints,
    tokens: &[String],
    maximum: bool,
) -> Result<(), String> {
    let command = if maximum {
        "set_max_delay"
    } else {
        "set_min_delay"
    };
    let limit = tokens
        .get(1)
        .ok_or_else(|| format!("{} requires a delay value", command))?
        .parse::<f64>()
        .map_err(|_| format!("invalid {} value", command))?;
    let from_object = if tokens.iter().any(|t| t == "-from") {
        typed_access_list(tokens, "-from")?.into_iter().next()
    } else {
        None
    };
    let to_object = if tokens.iter().any(|t| t == "-to") {
        typed_access_list(tokens, "-to")?.into_iter().next()
    } else {
        None
    };
    let through_groups = if tokens.iter().any(|t| t == "-through") {
        typed_access_groups(tokens, "-through")?
    } else {
        Vec::new()
    };
    let instance_pattern = parse_instance_pattern(tokens, "delay limit")?;
    let through = through_groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|object| object.name.clone())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect::<Vec<_>>();
    let constraint = PathDelayLimit {
        from: from_object.as_ref().map(|object| object.name.clone()),
        to: to_object.as_ref().map(|object| object.name.clone()),
        through,
        from_object,
        to_object,
        through_groups,
        instance_pattern,
        limit,
    };
    if maximum {
        out.add_max_delay(constraint)
    } else {
        out.add_min_delay(constraint)
    }
}

fn parse_quality_limit(
    out: &mut TimingConstraints,
    tokens: &[String],
    capacitance: bool,
) -> Result<(), String> {
    let command = if capacitance {
        "set_max_capacitance"
    } else {
        "set_max_transition"
    };
    let value = tokens[1]
        .parse::<f64>()
        .map_err(|_| format!("invalid {command} limit"))?;
    if tokens.len() == 2 {
        return if capacitance {
            out.set_max_capacitance_pf(value)
        } else {
            out.set_max_transition_ns(value)
        };
    }
    let objects = quality_objects(&tokens[2..], command)?;
    for object in objects {
        if capacitance {
            out.add_scoped_max_capacitance_pf(object, value)?;
        } else {
            out.add_scoped_max_transition_ns(object, value)?;
        }
    }
    Ok(())
}

fn parse_max_fanout(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let value = tokens[1]
        .parse::<usize>()
        .map_err(|_| "invalid set_max_fanout limit".to_string())?;
    if tokens.len() == 2 {
        return out.set_max_fanout(value);
    }
    let objects = quality_objects(&tokens[2..], "set_max_fanout")?;
    for object in objects {
        out.add_scoped_max_fanout(object, value)?;
    }
    Ok(())
}

fn parse_input_transition(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let value = tokens
        .get(1)
        .ok_or_else(|| "set_input_transition requires a numeric value".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid set_input_transition value".to_string())?;
    let objects = quality_objects(&tokens[2..], "set_input_transition")?;
    for object in objects {
        out.add_input_transition(object, value)?;
    }
    Ok(())
}

fn parse_load(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    if tokens.iter().any(|token| token == "-wire_load") {
        return Err("wire-load models are not supported by set_load".into());
    }
    let value_position = if tokens.get(1).map(String::as_str) == Some("-pin_load") {
        2
    } else {
        1
    };
    let value = tokens
        .get(value_position)
        .ok_or_else(|| "set_load requires a numeric value".to_string())?
        .parse::<f64>()
        .map_err(|_| "invalid set_load value".to_string())?;
    let objects = quality_objects(&tokens[value_position + 1..], "set_load")?;
    for object in objects {
        out.add_load_capacitance(object, value)?;
    }
    Ok(())
}

fn quality_objects(tokens: &[String], command: &str) -> Result<Vec<TimingObjectRef>, String> {
    if tokens.is_empty() || tokens[0] != "[" {
        return Err(format!(
            "{command} accepts one design-wide value or a trailing object access list"
        ));
    }
    let objects = typed_trailing_access_list(tokens)?;
    if objects.is_empty() {
        return Err(format!("{command} object access list cannot be empty"));
    }
    if objects
        .iter()
        .any(|object| object.kind == TimingObjectKind::Clock)
    {
        return Err(format!("{command} does not support clock objects"));
    }
    Ok(objects)
}

fn parse_timing_derate(out: &mut TimingConstraints, tokens: &[String]) -> Result<(), String> {
    let early = match value_after(tokens, "-early")? {
        Some(value) => Some(
            value
                .parse::<f64>()
                .map_err(|_| "invalid early timing derate".to_string())?,
        ),
        None => None,
    };
    let late = match value_after(tokens, "-late")? {
        Some(value) => Some(
            value
                .parse::<f64>()
                .map_err(|_| "invalid late timing derate".to_string())?,
        ),
        None => None,
    };
    validate_derate(early, late, "set_timing_derate")?;
    let instance_pattern = parse_instance_pattern(tokens, "timing derate")?;
    let cell_delay = tokens.iter().any(|t| t == "-cell_delay");
    let cell_check = tokens.iter().any(|t| t == "-cell_check");
    if cell_delay && cell_check {
        return Err("choose either -cell_delay or -cell_check".into());
    }
    if tokens.iter().any(|t| t == "-net_delay") {
        return Err("net-delay timing derates are not supported".into());
    }
    if tokens.iter().any(|t| t == "get_cells") {
        let cells = trailing_access_list(tokens)?;
        for cell in cells {
            if instance_pattern.is_some() {
                let mut derate = if cell_check {
                    out.check_timing_derate
                } else {
                    out.timing_derate
                };
                if let Some(early) = early {
                    derate.early = early;
                }
                if let Some(late) = late {
                    derate.late = late;
                }
                out.add_scoped_cell_timing_derate(
                    cell,
                    instance_pattern.clone(),
                    cell_check,
                    derate,
                )?;
            } else if cell_check {
                out.set_cell_check_derate(cell, early, late)?;
            } else {
                out.set_cell_timing_derate(cell, early, late)?;
            }
        }
        return Ok(());
    }
    if tokens.iter().any(|t| t == "get_pins") {
        let pins = trailing_access_list(tokens)?;
        for pin in pins {
            let (cell, pin) = pin
                .split_once('/')
                .ok_or_else(|| "pin timing derate requires <cell>/<pin>".to_string())?;
            if instance_pattern.is_some() {
                let mut derate = if cell_check {
                    out.check_timing_derate
                } else {
                    out.timing_derate
                };
                if let Some(early) = early {
                    derate.early = early;
                }
                if let Some(late) = late {
                    derate.late = late;
                }
                out.add_scoped_pin_timing_derate(
                    cell,
                    pin,
                    instance_pattern.clone(),
                    cell_check,
                    derate,
                )?;
            } else if cell_check {
                out.set_pin_check_derate(cell, pin, early, late)?;
            } else {
                out.set_pin_timing_derate(cell, pin, early, late)?;
            }
        }
        return Ok(());
    }
    if tokens.iter().any(|t| t == "get_ports") {
        if cell_delay || cell_check {
            return Err("cell derate scopes cannot be combined with port objects".into());
        }
        if instance_pattern.is_some() {
            return Err("port timing derates do not support -instance".into());
        }
        let ports = trailing_access_list(tokens)?;
        for port in ports {
            out.set_port_timing_derate(port, early, late)?;
        }
        return Ok(());
    }
    if tokens.iter().any(|t| t.starts_with("get_")) || tokens.iter().any(|t| t == "[") {
        return Err("set_timing_derate objects must use [get_cells ...] or [get_ports ...]".into());
    }
    if instance_pattern.is_some() {
        return Err("global timing derates do not support -instance".into());
    }
    if cell_check {
        return out.set_check_timing_derate(early, late);
    }
    out.set_timing_derate(early, late)
}

fn validate_derate(early: Option<f64>, late: Option<f64>, what: &str) -> Result<(), String> {
    if early.is_none() && late.is_none() {
        return Err(format!("{} requires -early, -late, or both", what));
    }
    for (kind, factor) in [("early", early), ("late", late)] {
        if let Some(factor) = factor {
            if !factor.is_finite() || factor <= 0.0 {
                return Err(format!(
                    "{} {} factor must be positive and finite",
                    what, kind
                ));
            }
        }
    }
    Ok(())
}

fn cell_check_derate(c: &TimingConstraints, cell: &str) -> TimingDerate {
    c.cell_check_derates
        .get(cell)
        .copied()
        .unwrap_or(c.check_timing_derate)
}

pub(crate) fn scoped_cell_timing_derate(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    check: bool,
) -> Option<TimingDerate> {
    let cell_name = &nl.cell(cell_id).name;
    let mut result = None;
    for rule in &c.scoped_cell_timing_derates {
        if rule.check != check
            || !TimingObjectPattern::new(&rule.cell_pattern).matches(cell_name)
            || !rule
                .instance_pattern
                .as_ref()
                .is_none_or(|pattern| cell_hierarchy_matches(nl, cell_id, pattern))
        {
            continue;
        }
        result = Some(rule.derate);
    }
    result
}

pub(crate) fn scoped_pin_timing_derate(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    pin_name: &str,
    check: bool,
) -> Option<TimingDerate> {
    let cell_name = &nl.cell(cell_id).name;
    let mut result = None;
    for rule in &c.scoped_pin_timing_derates {
        if rule.check != check
            || !TimingObjectPattern::new(&rule.cell_pattern).matches(cell_name)
            || !TimingObjectPattern::new(&rule.pin_pattern).matches(pin_name)
            || !rule
                .instance_pattern
                .as_ref()
                .is_some_and(|pattern| cell_hierarchy_matches(nl, cell_id, pattern))
        {
            continue;
        }
        result = Some(rule.derate);
    }
    result
}

pub(crate) fn pin_check_derate(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    pin_index: usize,
) -> TimingDerate {
    let cell = nl.cell(cell_id);
    let Some(pin_name) = (match &cell.kind {
        eda_core::netlist::CellKind::Lib(id) => nl
            .library()
            .and_then(|library| library.get(*id).inputs.get(pin_index).cloned()),
        _ => Some(pin_index.to_string()),
    }) else {
        return cell_check_derate(c, &cell.name);
    };
    if let Some(derate) = scoped_pin_timing_derate(nl, c, cell_id, &pin_name, true) {
        return derate;
    }
    if let Some(derate) = c
        .pin_check_derates
        .get(&TimingPin::new(&cell.name, pin_name))
        .copied()
    {
        return derate;
    }
    if let Some(derate) = scoped_cell_timing_derate(nl, c, cell_id, true) {
        return derate;
    }
    cell_check_derate(c, &cell.name)
}

/// Evaluate constrained setup and hold slacks. Input delays are already
/// reflected in `base` when produced by `analyze_with_constraints`; output
/// delays reduce the external required time. Setup uncertainty reduces the
/// effective period, while hold uncertainty tightens the minimum required
/// data-stable time. Endpoint false paths apply to both checks.
pub fn analyze_constrained(
    nl: &Netlist,
    base: StaResult,
    c: &TimingConstraints,
) -> Result<ConstrainedTiming, String> {
    let clock = c.default_clock()?;
    let uncertainty = c
        .clock_uncertainty_ns
        .get(&clock.name)
        .copied()
        .unwrap_or(0.0);
    let hold_uncertainty = c
        .hold_clock_uncertainty_ns
        .get(&clock.name)
        .copied()
        .unwrap_or(0.0);
    let source_latency = clock.source_latency_ns;
    let network_latency = clock.network_latency_ns;
    let effective_period = clock.period - uncertainty;
    if effective_period <= 0.0 {
        return Err("effective clock period is non-positive".into());
    }

    let mut exceptions_applied = 0usize;
    let mut reg2reg_slack = f64::INFINITY;
    let mut in2reg_slack = f64::INFINITY;
    let mut reg2reg_hold_slack = f64::INFINITY;
    let mut in2reg_hold_slack = f64::INFINITY;
    let mut max_delay_slack = f64::INFINITY;
    let mut min_delay_slack = f64::INFINITY;
    let mut net_slacks = BTreeMap::new();
    let mut path_cycle_multipliers = BTreeMap::new();
    let mut path_cycle_multipliers_by_endpoint = BTreeMap::new();
    let mut path_exceptions = BTreeMap::new();
    let mut path_max_delay_limits = BTreeMap::new();
    let mut path_min_delay_limits = BTreeMap::new();

    for &(cell_id, data_arrival) in base.setup_endpoint_list() {
        let cell = nl.cell(cell_id);
        let Some(data_net) = cell.inputs.get(1).copied() else {
            continue;
        };
        let endpoint = &nl.net(data_net).name;
        let endpoint_objects = endpoint_timing_objects(nl, data_net, endpoint, Some((cell_id, 1)));
        let endpoint_key = timing_pin_ref(nl, cell_id, 1)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                data_net,
                endpoint,
                Some(cell_id),
                false,
                &endpoint_objects,
                c,
            ),
        );
        let multiplier = match path_multiplier(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                continue;
            }
            Some(m) => m,
            None => 1.0,
        };
        let check_derate = pin_check_derate(nl, c, cell_id, 1);
        let clock_net = cell.inputs.first().copied();
        let setup = crate::seq_setup_at(
            nl,
            cell_id,
            base.net_transition(data_net),
            clock_net.and_then(|net| base.net_transition(net)),
        )? * check_derate.late;
        let slack = effective_period * multiplier - data_arrival - setup;
        path_cycle_multipliers.insert(endpoint.to_string(), multiplier);
        path_cycle_multipliers_by_endpoint.insert(timing_object_key(&endpoint_key), multiplier);
        let endpoint_max_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            false,
            c,
            true,
            data_arrival,
            &endpoint_objects,
        );
        let endpoint_min_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint,
            Some(cell_id),
            true,
            c,
            false,
            data_arrival,
            &endpoint_objects,
        );
        path_max_delay_limits.insert(
            timing_object_key(&endpoint_key),
            endpoint_max_delay_limits.clone(),
        );
        path_min_delay_limits.insert(timing_object_key(&endpoint_key), endpoint_min_delay_limits);
        let net_max_delay = endpoint_max_delay_limits
            .iter()
            .map(|limit| limit.slack_ns)
            .fold(f64::INFINITY, f64::min);
        max_delay_slack = max_delay_slack.min(net_max_delay);
        if crate::driven_from_sequential(nl, data_net) {
            reg2reg_slack = reg2reg_slack.min(slack);
            record_net_slack(
                &mut net_slacks,
                endpoint,
                Some(slack),
                None,
                None,
                Some(net_max_delay),
                None,
            );
        } else {
            let endpoint_slack = slack + network_latency;
            in2reg_slack = in2reg_slack.min(endpoint_slack);
            record_net_slack(
                &mut net_slacks,
                endpoint,
                Some(endpoint_slack),
                None,
                None,
                Some(net_max_delay),
                None,
            );
        }
    }

    for endpoint in base.hold_endpoint_list() {
        let cell = nl.cell(endpoint.cell);
        let Some(data_net) = cell.inputs.get(1).copied() else {
            continue;
        };
        let endpoint_name = &nl.net(data_net).name;
        let endpoint_objects =
            endpoint_timing_objects(nl, data_net, endpoint_name, Some((endpoint.cell, 1)));
        let endpoint_key = timing_pin_ref(nl, endpoint.cell, 1)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint_name));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                data_net,
                endpoint_name,
                Some(endpoint.cell),
                true,
                &endpoint_objects,
                c,
            ),
        );
        if matches!(
            path_multiplier(
                &base,
                nl,
                data_net,
                endpoint_name,
                Some(endpoint.cell),
                true,
                &endpoint_objects,
                c,
            ),
            Some(f64::INFINITY)
        ) {
            exceptions_applied += 1;
            continue;
        }
        let check_derate = pin_check_derate(nl, c, endpoint.cell, 1);
        let clock_net = cell.inputs.first().copied();
        let hold = crate::seq_hold_at(
            nl,
            endpoint.cell,
            base.net_min_transition(data_net),
            clock_net.and_then(|net| base.net_min_transition(net)),
        )? * check_derate.early;
        let slack = endpoint.data_arrival - hold - hold_uncertainty;
        let endpoint_min_delay_limits = matching_path_delay_limits(
            &base,
            nl,
            data_net,
            endpoint_name,
            Some(endpoint.cell),
            true,
            c,
            false,
            endpoint.data_arrival,
            &endpoint_objects,
        );
        path_min_delay_limits.insert(
            timing_object_key(&endpoint_key),
            endpoint_min_delay_limits.clone(),
        );
        let net_min_delay = endpoint_min_delay_limits
            .iter()
            .map(|limit| limit.slack_ns)
            .fold(f64::INFINITY, f64::min);
        min_delay_slack = min_delay_slack.min(net_min_delay);
        let endpoint_slack = if endpoint.launched_from_sequential {
            slack
        } else {
            slack + network_latency
        };
        record_net_slack(
            &mut net_slacks,
            endpoint_name,
            None,
            Some(endpoint_slack),
            None,
            None,
            Some(net_min_delay),
        );
        if endpoint.launched_from_sequential {
            reg2reg_hold_slack = reg2reg_hold_slack.min(slack);
        } else {
            in2reg_hold_slack = in2reg_hold_slack.min(endpoint_slack);
        }
    }

    let high_width = clock.fall - clock.rise;
    let low_width = clock.period - high_width;
    let mut min_pulse_width_checks = Vec::new();
    for (cell_id, cell) in nl.cells().iter().enumerate() {
        let Some((required_high, required_low)) = crate::seq_min_pulse(nl, &cell.kind) else {
            continue;
        };
        let Some(&clock_net) = cell.inputs.first() else {
            continue;
        };
        let clock_net_name = &nl.net(clock_net).name;
        if base.case_analysis_nets.contains_key(clock_net_name) {
            continue;
        }
        let clock_matches =
            clock.ports.is_empty() || clock.ports.iter().any(|port| port == clock_net_name);
        if clock_matches {
            let check_derate = pin_check_derate(nl, c, cell_id, 0);
            for (polarity, actual, required) in [
                (MinPulsePolarity::High, high_width, required_high),
                (MinPulsePolarity::Low, low_width, required_low),
            ] {
                let effective_required = required * check_derate.late;
                min_pulse_width_checks.push(MinPulseWidthCheck {
                    cell: cell.name.clone(),
                    clock: clock_net_name.clone(),
                    polarity,
                    actual_ns: actual,
                    required_ns: required,
                    effective_required_ns: effective_required,
                    slack_ns: actual - effective_required,
                });
            }
        }
    }
    let min_pulse_high_slack = min_pulse_width_checks
        .iter()
        .filter(|check| check.polarity == MinPulsePolarity::High)
        .map(|check| check.slack_ns)
        .fold(f64::INFINITY, f64::min);
    let min_pulse_low_slack = min_pulse_width_checks
        .iter()
        .filter(|check| check.polarity == MinPulsePolarity::Low)
        .map(|check| check.slack_ns)
        .fold(f64::INFINITY, f64::min);
    let min_pulse_width_slack = min_pulse_high_slack.min(min_pulse_low_slack);

    let mut recovery_slack = f64::INFINITY;
    let mut removal_slack = f64::INFINITY;
    for endpoint in base.reset_endpoint_list() {
        let cell = nl.cell(endpoint.cell);
        let Some((reset_pin, _, _)) = crate::seq_reset(nl, &cell.kind) else {
            continue;
        };
        let Some(&reset_net) = cell.inputs.get(reset_pin) else {
            continue;
        };
        let endpoint_name = &nl.net(reset_net).name;
        let endpoint_objects = endpoint_timing_objects(
            nl,
            reset_net,
            endpoint_name,
            Some((endpoint.cell, reset_pin)),
        );
        let endpoint_key = timing_pin_ref(nl, endpoint.cell, reset_pin)
            .unwrap_or_else(|| TimingObjectRef::new(TimingObjectKind::Net, endpoint_name));
        record_path_exceptions(
            &mut path_exceptions,
            &endpoint_key,
            matching_path_exceptions(
                &base,
                nl,
                reset_net,
                endpoint_name,
                Some(endpoint.cell),
                false,
                &endpoint_objects,
                c,
            ),
        );
        let multiplier = match path_multiplier(
            &base,
            nl,
            reset_net,
            endpoint_name,
            Some(endpoint.cell),
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                continue;
            }
            Some(multiplier) => multiplier,
            None => 1.0,
        };
        path_cycle_multipliers.insert(endpoint_name.to_string(), multiplier);
        path_cycle_multipliers_by_endpoint.insert(timing_object_key(&endpoint_key), multiplier);
        let clock_net = cell.inputs.first().copied();
        let check_derate = pin_check_derate(nl, c, endpoint.cell, reset_pin);
        let recovery = crate::seq_recovery_at(
            nl,
            endpoint.cell,
            base.net_transition(reset_net),
            clock_net.and_then(|net| base.net_transition(net)),
        )? * check_derate.late;
        let removal = crate::seq_removal_at(
            nl,
            endpoint.cell,
            base.net_min_transition(reset_net),
            clock_net.and_then(|net| base.net_min_transition(net)),
        )? * check_derate.early;
        let endpoint_recovery_slack =
            effective_period * multiplier + network_latency - endpoint.reset_arrival - recovery;
        let endpoint_removal_slack =
            endpoint.reset_min_arrival + network_latency - removal - hold_uncertainty;
        recovery_slack = recovery_slack.min(endpoint_recovery_slack);
        removal_slack = removal_slack.min(endpoint_removal_slack);
        let reset_slack = net_slacks.entry(endpoint_name.clone()).or_default();
        reset_slack.recovery_ns = Some(
            reset_slack
                .recovery_ns
                .map_or(endpoint_recovery_slack, |old| {
                    old.min(endpoint_recovery_slack)
                }),
        );
        reset_slack.removal_ns = Some(
            reset_slack
                .removal_ns
                .map_or(endpoint_removal_slack, |old| {
                    old.min(endpoint_removal_slack)
                }),
        );
    }
    let async_reset_slack = recovery_slack.min(removal_slack);

    let mut output_arrival = f64::NEG_INFINITY;
    let mut output_net = usize::MAX;
    let mut output_name = String::new();
    for port in &nl.ports {
        if port.dir != eda_core::netlist::Dir::Out {
            continue;
        }
        for &net in &port.bits {
            if let Some(arrival) = base.net_arrival(net) {
                let delay = c
                    .output_delays
                    .get(&port.name)
                    .map(|d| d.delay)
                    .unwrap_or(0.0);
                let derate = c
                    .port_timing_derates
                    .get(&port.name)
                    .copied()
                    .unwrap_or_default();
                let external_delay = delay * derate.late;
                if arrival + external_delay > output_arrival {
                    output_arrival = arrival + external_delay;
                    output_net = net;
                    output_name = port.name.clone();
                }
            }
        }
    }

    let output_slack = if output_arrival.is_finite() {
        let launch_latency = if crate::driven_from_sequential(nl, output_net) {
            source_latency + network_latency
        } else {
            0.0
        };
        let endpoint_objects = endpoint_timing_objects(nl, output_net, &output_name, None);
        match path_multiplier(
            &base,
            nl,
            output_net,
            &output_name,
            None,
            false,
            &endpoint_objects,
            c,
        ) {
            Some(f64::INFINITY) => {
                exceptions_applied += 1;
                f64::INFINITY
            }
            Some(m) => effective_period * m - output_arrival - launch_latency,
            None => effective_period - output_arrival - launch_latency,
        }
    } else {
        f64::INFINITY
    };

    for port in &nl.ports {
        if port.dir != eda_core::netlist::Dir::Out {
            continue;
        }
        for &net in &port.bits {
            if let Some(arrival) = base.net_arrival(net) {
                let endpoint_objects = endpoint_timing_objects(nl, net, &port.name, None);
                let output_key = TimingObjectRef::new(TimingObjectKind::Port, &port.name);
                let endpoint_max_delay_limits = matching_path_delay_limits(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    false,
                    c,
                    true,
                    arrival,
                    &endpoint_objects,
                );
                path_max_delay_limits.insert(
                    timing_object_key(&output_key),
                    endpoint_max_delay_limits.clone(),
                );
                let net_max_delay = endpoint_max_delay_limits
                    .iter()
                    .map(|limit| limit.slack_ns)
                    .fold(f64::INFINITY, f64::min);
                max_delay_slack = max_delay_slack.min(net_max_delay);
                let delay = c
                    .output_delays
                    .get(&port.name)
                    .map(|d| d.delay)
                    .unwrap_or(0.0);
                let derate = c
                    .port_timing_derates
                    .get(&port.name)
                    .copied()
                    .unwrap_or_default();
                let output_arrival = arrival + delay * derate.late;
                let launch_latency = if crate::driven_from_sequential(nl, net) {
                    source_latency + network_latency
                } else {
                    0.0
                };
                let multiplier = match path_multiplier(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    false,
                    &endpoint_objects,
                    c,
                ) {
                    Some(f64::INFINITY) => f64::INFINITY,
                    Some(multiplier) => multiplier,
                    None => 1.0,
                };
                path_cycle_multipliers.insert(nl.net(net).name.clone(), multiplier);
                path_cycle_multipliers_by_endpoint
                    .insert(timing_object_key(&output_key), multiplier);
                record_path_exceptions(
                    &mut path_exceptions,
                    &output_key,
                    matching_path_exceptions(
                        &base,
                        nl,
                        net,
                        &port.name,
                        None,
                        false,
                        &endpoint_objects,
                        c,
                    ),
                );
                let output_slack = effective_period * multiplier - output_arrival - launch_latency;
                record_net_slack(
                    &mut net_slacks,
                    &nl.net(net).name,
                    None,
                    None,
                    Some(output_slack),
                    Some(net_max_delay),
                    None,
                );
            }
            if let Some(arrival) = base.net_min_arrival(net) {
                let endpoint_objects = endpoint_timing_objects(nl, net, &port.name, None);
                let output_key = TimingObjectRef::new(TimingObjectKind::Port, &port.name);
                let endpoint_min_delay_limits = matching_path_delay_limits(
                    &base,
                    nl,
                    net,
                    &port.name,
                    None,
                    true,
                    c,
                    false,
                    arrival,
                    &endpoint_objects,
                );
                path_min_delay_limits.insert(
                    timing_object_key(&output_key),
                    endpoint_min_delay_limits.clone(),
                );
                let net_min_delay = endpoint_min_delay_limits
                    .iter()
                    .map(|limit| limit.slack_ns)
                    .fold(f64::INFINITY, f64::min);
                min_delay_slack = min_delay_slack.min(net_min_delay);
                record_net_slack(
                    &mut net_slacks,
                    &nl.net(net).name,
                    None,
                    None,
                    None,
                    None,
                    Some(net_min_delay),
                );
            }
        }
    }

    let hold_slack = reg2reg_hold_slack.min(in2reg_hold_slack);
    let worst_slack = reg2reg_slack
        .min(in2reg_slack)
        .min(output_slack)
        .min(hold_slack)
        .min(max_delay_slack)
        .min(min_delay_slack)
        .min(min_pulse_width_slack)
        .min(async_reset_slack);
    Ok(ConstrainedTiming {
        base,
        clock: clock.name.clone(),
        period: clock.period,
        clock_rise: clock.rise,
        clock_fall: clock.fall,
        uncertainty,
        hold_uncertainty,
        source_latency,
        network_latency,
        early_derate: c.timing_derate.early,
        late_derate: c.timing_derate.late,
        cell_derates: c.cell_timing_derates.clone(),
        port_derates: c.port_timing_derates.clone(),
        pin_derates: c.pin_timing_derates.clone(),
        early_check_derate: c.check_timing_derate.early,
        late_check_derate: c.check_timing_derate.late,
        cell_check_derates: c.cell_check_derates.clone(),
        pin_check_derates: c.pin_check_derates.clone(),
        reg2reg_slack,
        in2reg_slack,
        reg2reg_hold_slack,
        in2reg_hold_slack,
        hold_slack,
        output_slack,
        max_delay_slack,
        min_delay_slack,
        min_pulse_high_slack,
        min_pulse_low_slack,
        min_pulse_width_slack,
        min_pulse_width_checks,
        recovery_slack,
        removal_slack,
        async_reset_slack,
        worst_slack,
        exceptions_applied,
        path_cycle_multipliers,
        path_cycle_multipliers_by_endpoint,
        path_exceptions,
        path_max_delay_limits,
        path_min_delay_limits,
        net_slacks,
    })
}

/// Parse SDC, analyze input-delay-adjusted arrivals, and report setup slacks.
pub fn analyze_with_sdc(
    nl: &Netlist,
    constraints: &TimingConstraints,
) -> Result<ConstrainedTiming, String> {
    let base = crate::analyze_with_constraints(nl, constraints)?;
    analyze_constrained(nl, base, constraints)
}

fn constrained_clock_edge(
    timing: &ConstrainedTiming,
    local_reference: bool,
    cycle_multiplier: f64,
) -> TimingClockEdge {
    let network_latency = if local_reference {
        timing.network_latency
    } else {
        0.0
    };
    TimingClockEdge {
        clock: timing.clock.clone(),
        edge: TimingClockEdgeKind::Rise,
        edge_time_ns: timing.clock_rise,
        source_latency_ns: timing.source_latency,
        network_latency_ns: network_latency,
        absolute_time_ns: timing.clock_rise
            + timing.source_latency
            + network_latency
            + timing.period * cycle_multiplier,
        virtual_reference: !local_reference,
    }
}

fn constrained_path_clocks(
    nl: &Netlist,
    path: &TimingPath,
    timing: &ConstrainedTiming,
    capture_cycles: f64,
) -> TimingPathClocks {
    let launch_local = matches!(path.launch, crate::TimingLaunch::SequentialCell(_));
    let capture_local = matches!(path.endpoint, crate::TimingPathEndpoint::SequentialCell(_));
    let launch = constrained_clock_edge(timing, launch_local, 0.0);
    let capture = constrained_clock_edge(timing, capture_local, capture_cycles);
    let edge_separation_ns = capture.absolute_time_ns - launch.absolute_time_ns;
    let insertion_skew_ns = capture.network_latency_ns - launch.network_latency_ns;
    let _ = nl;
    TimingPathClocks {
        launch,
        capture,
        edge_separation_ns,
        insertion_skew_ns,
    }
}

fn timing_object_key(object: &TimingObjectRef) -> String {
    format!("{:?}:{}", object.kind, object.name)
}

fn data_path_endpoint_key(nl: &Netlist, path: &TimingPath) -> String {
    match &path.endpoint {
        crate::TimingPathEndpoint::SequentialCell(_) => path
            .cells
            .last()
            .and_then(|&cell| timing_pin_ref(nl, cell, 1))
            .map(|pin| timing_object_key(&pin))
            .unwrap_or_else(|| {
                timing_object_key(&TimingObjectRef::new(
                    TimingObjectKind::Net,
                    path.nets
                        .last()
                        .map(|&net| nl.net(net).name.clone())
                        .unwrap_or_default(),
                ))
            }),
        crate::TimingPathEndpoint::OutputPort(name) => {
            timing_object_key(&TimingObjectRef::new(TimingObjectKind::Port, name))
        }
    }
}

fn reset_path_endpoint_key(nl: &Netlist, path: &TimingPath) -> String {
    path.cells
        .last()
        .and_then(|&cell| {
            crate::seq_reset(nl, &nl.cell(cell).kind)
                .and_then(|(pin, _, _)| timing_pin_ref(nl, cell, pin))
        })
        .map(|pin| timing_object_key(&pin))
        .unwrap_or_else(|| {
            timing_object_key(&TimingObjectRef::new(
                TimingObjectKind::Net,
                path.nets
                    .last()
                    .map(|&net| nl.net(net).name.clone())
                    .unwrap_or_default(),
            ))
        })
}

fn endpoint_cycle_multiplier(nl: &Netlist, path: &TimingPath, timing: &ConstrainedTiming) -> f64 {
    let endpoint_key = data_path_endpoint_key(nl, path);
    if let Some(multiplier) = timing.path_cycle_multipliers_by_endpoint.get(&endpoint_key) {
        return *multiplier;
    }
    path.nets
        .last()
        .map(|&net| {
            timing
                .path_cycle_multipliers
                .get(&nl.net(net).name)
                .copied()
                .unwrap_or(1.0)
        })
        .unwrap_or(1.0)
}

fn reset_endpoint_cycle_multiplier(
    nl: &Netlist,
    path: &TimingPath,
    timing: &ConstrainedTiming,
) -> f64 {
    let endpoint_key = reset_path_endpoint_key(nl, path);
    if let Some(multiplier) = timing.path_cycle_multipliers_by_endpoint.get(&endpoint_key) {
        return *multiplier;
    }
    path.nets
        .last()
        .map(|&net| {
            timing
                .path_cycle_multipliers
                .get(&nl.net(net).name)
                .copied()
                .unwrap_or(1.0)
        })
        .unwrap_or(1.0)
}

/// Late setup/output paths annotated with constrained slack.
///
/// Paths whose exceptions make slack infinite are omitted. Results are sorted
/// from worst slack to best slack. `max_paths == 0` returns all finite paths.
pub fn constrained_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedTimingPath> {
    let mut paths = timing
        .base
        .timing_paths(nl, 0)
        .into_iter()
        .filter_map(|path| {
            let endpoint_net = *path.nets.last()?;
            let name = nl.net(endpoint_net).name.clone();
            let exception_key = data_path_endpoint_key(nl, &path);
            let exceptions = timing
                .path_exceptions
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                return None;
            }
            let delay_limits = timing
                .path_max_delay_limits
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            let base_slack = match &path.endpoint {
                crate::TimingPathEndpoint::SequentialCell(_) => {
                    timing.net_slacks.get(&name)?.setup_ns
                }
                crate::TimingPathEndpoint::OutputPort(_) => timing.net_slacks.get(&name)?.output_ns,
            }?;
            let slack = delay_limits
                .iter()
                .map(|limit| limit.slack_ns)
                .fold(base_slack, f64::min);
            if !slack.is_finite() {
                return None;
            }
            let required_ns = path.arrival_ns + slack;
            let clocks = constrained_path_clocks(
                nl,
                &path,
                timing,
                endpoint_cycle_multiplier(nl, &path, timing),
            );
            Some(ConstrainedTimingPath {
                path,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
                delay_limits,
            })
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.slack_ns
            .total_cmp(&right.slack_ns)
            .then_with(|| right.path.arrival_ns.total_cmp(&left.path.arrival_ns))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Early hold paths annotated with constrained hold slack.
///
/// Paths are sorted from worst to best hold slack. `max_paths == 0` returns
/// every finite hold endpoint.
pub fn constrained_hold_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedHoldTimingPath> {
    let mut paths = timing
        .base
        .hold_timing_paths(nl, timing.hold_uncertainty, 0)
        .into_iter()
        .filter_map(|path| {
            let endpoint_net = *path.path.nets.last()?;
            let name = nl.net(endpoint_net).name.clone();
            let exception_key = data_path_endpoint_key(nl, &path.path);
            let exceptions = timing
                .path_exceptions
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                return None;
            }
            let delay_limits = timing
                .path_min_delay_limits
                .get(&exception_key)
                .cloned()
                .unwrap_or_default();
            let slack = timing.net_slacks.get(&name)?.hold_ns?;
            if !slack.is_finite() {
                return None;
            }
            let required_ns = path.path.arrival_ns - slack;
            let clocks = constrained_path_clocks(nl, &path.path, timing, 0.0);
            Some(ConstrainedHoldTimingPath {
                path,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
                delay_limits,
            })
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        left.slack_ns.total_cmp(&right.slack_ns).then_with(|| {
            right
                .path
                .path
                .arrival_ns
                .total_cmp(&left.path.path.arrival_ns)
        })
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Recovery/removal paths annotated with constrained asynchronous-reset slack.
pub fn constrained_reset_timing_paths(
    nl: &Netlist,
    timing: &ConstrainedTiming,
    max_paths: usize,
) -> Vec<ConstrainedResetTimingPath> {
    let mut paths = Vec::new();
    for reset in timing.base.reset_timing_paths(nl, 0) {
        let recovery_name = &nl.net(reset.recovery.nets.last().copied().unwrap()).name;
        let removal_name = &nl.net(reset.removal.nets.last().copied().unwrap()).name;
        let recovery_clocks = constrained_path_clocks(
            nl,
            &reset.recovery,
            timing,
            reset_endpoint_cycle_multiplier(nl, &reset.recovery, timing),
        );
        let removal_clocks = constrained_path_clocks(nl, &reset.removal, timing, 0.0);
        let recovery_exception_key = reset_path_endpoint_key(nl, &reset.recovery);
        let recovery_exceptions = timing
            .path_exceptions
            .get(&recovery_exception_key)
            .cloned()
            .unwrap_or_default();
        let removal_exception_key = reset_path_endpoint_key(nl, &reset.removal);
        let removal_exceptions = timing
            .path_exceptions
            .get(&removal_exception_key)
            .cloned()
            .unwrap_or_default();
        let candidates = [
            (
                reset.recovery,
                ResetCheckKind::Recovery,
                timing
                    .net_slacks
                    .get(recovery_name)
                    .and_then(|slack| slack.recovery_ns),
                recovery_clocks,
                recovery_exceptions,
            ),
            (
                reset.removal,
                ResetCheckKind::Removal,
                timing
                    .net_slacks
                    .get(removal_name)
                    .and_then(|slack| slack.removal_ns),
                removal_clocks,
                removal_exceptions,
            ),
        ];
        for (path, kind, slack, clocks, exceptions) in candidates {
            if exceptions
                .iter()
                .any(|exception| exception.multicycle.is_none())
            {
                continue;
            }
            let Some(slack) = slack.filter(|slack| slack.is_finite()) else {
                continue;
            };
            let required_ns = match kind {
                ResetCheckKind::Recovery => path.arrival_ns + slack,
                ResetCheckKind::Removal => path.arrival_ns - slack,
            };
            paths.push(ConstrainedResetTimingPath {
                path,
                kind,
                slack_ns: slack,
                required_ns,
                clocks,
                exceptions,
            });
        }
    }
    paths.sort_by(|left, right| {
        left.slack_ns
            .total_cmp(&right.slack_ns)
            .then_with(|| right.path.arrival_ns.total_cmp(&left.path.arrival_ns))
    });
    if max_paths > 0 {
        paths.truncate(max_paths);
    }
    paths
}

/// Concrete setup, hold, recovery, and removal paths omitted by false-path
/// exceptions.
pub fn excluded_timing_paths(nl: &Netlist, timing: &ConstrainedTiming) -> Vec<ExcludedTimingPath> {
    let mut out = Vec::new();
    for path in timing.base.timing_paths(nl, 0) {
        let key = data_path_endpoint_key(nl, &path);
        for exception in timing
            .path_exceptions
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|exception| exception.multicycle.is_none())
        {
            out.push(ExcludedTimingPath {
                check: ExcludedTimingCheck::Setup,
                path: path.clone(),
                exception: exception.clone(),
            });
        }
    }

    for hold in timing
        .base
        .hold_timing_paths(nl, timing.hold_uncertainty, 0)
    {
        let key = data_path_endpoint_key(nl, &hold.path);
        for exception in timing
            .path_exceptions
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|exception| exception.multicycle.is_none())
        {
            out.push(ExcludedTimingPath {
                check: ExcludedTimingCheck::Hold,
                path: hold.path.clone(),
                exception: exception.clone(),
            });
        }
    }

    for reset in timing.base.reset_timing_paths(nl, 0) {
        for (path, check) in [
            (&reset.recovery, ExcludedTimingCheck::Recovery),
            (&reset.removal, ExcludedTimingCheck::Removal),
        ] {
            let key = reset_path_endpoint_key(nl, path);
            for exception in timing
                .path_exceptions
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|exception| exception.multicycle.is_none())
            {
                out.push(ExcludedTimingPath {
                    check,
                    path: path.clone(),
                    exception: exception.clone(),
                });
            }
        }
    }

    out.sort_by(|left, right| {
        endpoint_debug(&left.path.endpoint)
            .cmp(&endpoint_debug(&right.path.endpoint))
            .then(format!("{:?}", left.check).cmp(&format!("{:?}", right.check)))
    });
    out
}

pub fn excluded_timing_path_report(nl: &Netlist, paths: &[ExcludedTimingPath]) -> String {
    let mut out = format!("Excluded timing path report: {} path(s)\n", paths.len());
    for excluded in paths {
        let check = match excluded.check {
            ExcludedTimingCheck::Setup => "setup",
            ExcludedTimingCheck::Hold => "hold",
            ExcludedTimingCheck::Recovery => "recovery",
            ExcludedTimingCheck::Removal => "removal",
        };
        let from = excluded.exception.from.as_deref().unwrap_or("<any>");
        let to = excluded.exception.to.as_deref().unwrap_or("<any>");
        let through = if excluded.exception.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", excluded.exception.through.join(" -> "))
        };
        out.push_str(&format!(
            "  {} {} excluded by false path from {} to {}{}\n",
            check,
            endpoint_debug(&excluded.path.endpoint),
            from,
            to,
            through
        ));
        let cells = excluded
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n", cells));
    }
    out
}

fn endpoint_debug(endpoint: &crate::TimingPathEndpoint) -> String {
    match endpoint {
        crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {name}"),
        crate::TimingPathEndpoint::OutputPort(name) => format!("output {name}"),
    }
}

pub fn constrained_timing_path_report(nl: &Netlist, paths: &[ConstrainedTimingPath]) -> String {
    let mut out = format!("Constrained timing path report: {} path(s)\n", paths.len());
    for constrained in paths {
        let path = &constrained.path;
        out.push_str(&format!(
            "  slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            constrained.slack_ns, path.arrival_ns, constrained.required_ns
        ));
        let endpoint = match &path.endpoint {
            crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            crate::TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        let cells = path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!(
            "    endpoint: {}\n    cells: {}\n",
            endpoint, cells
        ));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        push_delay_limits_report(&constrained.delay_limits, &mut out);
        crate::push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

fn push_clock_edge_report(label: &str, edge: &TimingClockEdge, out: &mut String) {
    let edge_kind = match edge.edge {
        TimingClockEdgeKind::Rise => "rise",
        TimingClockEdgeKind::Fall => "fall",
    };
    let reference = if edge.virtual_reference {
        "virtual"
    } else {
        "local"
    };
    out.push_str(&format!(
        "    {}: {} {} at {:.6} ns, source {:.6} ns, network {:.6} ns, absolute {:.6} ns ({})\n",
        label,
        edge.clock,
        edge_kind,
        edge.edge_time_ns,
        edge.source_latency_ns,
        edge.network_latency_ns,
        edge.absolute_time_ns,
        reference
    ));
}

fn push_path_clocks_report(clocks: &TimingPathClocks, out: &mut String) {
    out.push_str("    clocks:\n");
    push_clock_edge_report("launch", &clocks.launch, out);
    push_clock_edge_report("capture", &clocks.capture, out);
    out.push_str(&format!(
        "    edge separation {:.6} ns, insertion skew {:.6} ns\n",
        clocks.edge_separation_ns, clocks.insertion_skew_ns
    ));
}

fn push_path_exceptions_report(exceptions: &[PathException], out: &mut String) {
    if exceptions.is_empty() {
        out.push_str("    exceptions: <none>\n");
        return;
    }
    out.push_str("    exceptions:\n");
    for exception in exceptions {
        let from = exception.from.as_deref().unwrap_or("<any>");
        let to = exception.to.as_deref().unwrap_or("<any>");
        let through = if exception.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", exception.through.join(" -> "))
        };
        let instance = exception
            .instance_pattern
            .as_deref()
            .map(|pattern| format!(" instance {}", pattern))
            .unwrap_or_default();
        if let Some(multicycle) = exception.multicycle {
            out.push_str(&format!(
                "      multicycle {:.6}, from {} to {}{}{}\n",
                multicycle, from, to, through, instance
            ));
        } else {
            out.push_str(&format!(
                "      false path from {} to {}{}{}\n",
                from, to, through, instance
            ));
        }
    }
}

fn push_delay_limits_report(limits: &[AppliedTimingDelayLimit], out: &mut String) {
    if limits.is_empty() {
        out.push_str("    delay limits: <none>\n");
        return;
    }
    out.push_str("    delay limits:\n");
    for applied in limits {
        let kind = if applied.maximum { "max" } else { "min" };
        let from = applied.limit.from.as_deref().unwrap_or("<any>");
        let to = applied.limit.to.as_deref().unwrap_or("<any>");
        let through = if applied.limit.through.is_empty() {
            String::new()
        } else {
            format!(" through {}", applied.limit.through.join(" -> "))
        };
        let instance = applied
            .limit
            .instance_pattern
            .as_deref()
            .map(|pattern| format!(" instance {}", pattern))
            .unwrap_or_default();
        out.push_str(&format!(
            "      {} {:.6} ns, slack {:.6} ns, from {} to {}{}{}\n",
            kind, applied.limit.limit, applied.slack_ns, from, to, through, instance
        ));
    }
}

pub fn constrained_hold_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedHoldTimingPath],
) -> String {
    let mut out = format!(
        "Constrained hold timing path report: {} path(s)\n",
        paths.len()
    );
    for constrained in paths {
        out.push_str(&format!(
            "  hold slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            constrained.slack_ns, constrained.path.path.arrival_ns, constrained.required_ns
        ));
        let endpoint = match &constrained.path.path.endpoint {
            crate::TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            crate::TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        let cells = constrained
            .path
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!(
            "    endpoint: {}\n    cells: {}\n",
            endpoint, cells
        ));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        push_delay_limits_report(&constrained.delay_limits, &mut out);
        crate::push_timing_arcs_report(nl, &constrained.path.path.arcs, &mut out);
    }
    out
}

pub fn constrained_reset_timing_path_report(
    nl: &Netlist,
    paths: &[ConstrainedResetTimingPath],
) -> String {
    let mut out = format!(
        "Constrained reset timing path report: {} path(s)\n",
        paths.len()
    );
    for constrained in paths {
        let kind = match constrained.kind {
            ResetCheckKind::Recovery => "recovery",
            ResetCheckKind::Removal => "removal",
        };
        out.push_str(&format!(
            "  {} slack {:.6} ns, arrival {:.6} ns, required {:.6} ns\n",
            kind, constrained.slack_ns, constrained.path.arrival_ns, constrained.required_ns
        ));
        let cells = constrained
            .path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n", cells));
        push_path_clocks_report(&constrained.clocks, &mut out);
        push_path_exceptions_report(&constrained.exceptions, &mut out);
        crate::push_timing_arcs_report(nl, &constrained.path.arcs, &mut out);
    }
    out
}

fn path_start_names(nl: &Netlist, start: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![start];
    let mut seen = std::collections::HashSet::new();
    while let Some(net) = stack.pop() {
        if !seen.insert(net) {
            continue;
        }
        out.push(nl.net(net).name.clone());
        for &ci in &nl.net(net).drivers {
            let cell = nl.cell(ci);
            if nl.cell_is_sequential(&cell.kind) {
                continue;
            }
            for &input in &cell.inputs {
                stack.push(input);
            }
        }
    }
    out
}

fn timing_pin_ref(nl: &Netlist, cell_id: usize, pin: usize) -> Option<TimingObjectRef> {
    let pin = crate::input_pin_name(nl, cell_id, pin)?;
    Some(TimingObjectRef::new(
        TimingObjectKind::Pin,
        format!("{}/{}", nl.cell(cell_id).name, pin),
    ))
}

fn timing_output_pin_ref(nl: &Netlist, cell_id: usize) -> Option<TimingObjectRef> {
    let pin = crate::output_pin_name(nl, cell_id)?;
    Some(TimingObjectRef::new(
        TimingObjectKind::Pin,
        format!("{}/{}", nl.cell(cell_id).name, pin),
    ))
}

fn disabled_selector_matches_pin(
    selector: &TimingObjectRef,
    nl: &Netlist,
    cell_id: usize,
    pin_ref: Option<&TimingObjectRef>,
) -> bool {
    match selector.kind {
        TimingObjectKind::Cell => selector.matches_name(&nl.cell(cell_id).name),
        TimingObjectKind::Pin => pin_ref.is_some_and(|pin| selector.matches_object(pin)),
        _ => false,
    }
}

fn cell_hierarchy_matches(nl: &Netlist, cell_id: usize, pattern: &str) -> bool {
    nl.cell_hierarchy_instance(cell_id)
        .is_some_and(|instance| TimingObjectPattern::new(pattern).matches(&instance.path))
}

/// True when a combinational or sequential cell arc is disabled by SDC.
///
/// `from_pin` and `to_pin` are cell input-pin indexes. A `None` `to_pin`
/// denotes the cell output. Check-only disables use the same selector logic
/// with the checked input pin as the endpoint.
pub(crate) fn timing_arc_disabled(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    from_pin: Option<usize>,
    to_pin: Option<usize>,
) -> bool {
    let from_ref = from_pin.and_then(|pin| timing_pin_ref(nl, cell_id, pin));
    let to_ref = match to_pin {
        Some(pin) => timing_pin_ref(nl, cell_id, pin),
        None => timing_output_pin_ref(nl, cell_id),
    };
    c.disabled_timing_arcs.iter().any(|disabled| {
        let object_matches = disabled.object.as_ref().is_some_and(|object| {
            object.kind == TimingObjectKind::Cell && object.matches_name(&nl.cell(cell_id).name)
                || (object.kind == TimingObjectKind::Pin
                    && (from_ref
                        .as_ref()
                        .is_some_and(|pin| object.matches_object(pin))
                        || to_ref
                            .as_ref()
                            .is_some_and(|pin| object.matches_object(pin))))
        });
        let from_matches = disabled.from.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, from_ref.as_ref())
        });
        let to_matches = disabled.to.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, to_ref.as_ref())
        });
        let qualifiers_match = from_matches && to_matches;
        let instance_matches = disabled
            .instance_pattern
            .as_ref()
            .is_none_or(|pattern| cell_hierarchy_matches(nl, cell_id, pattern));
        (object_matches || disabled.object.is_none()) && qualifiers_match && instance_matches
    })
}

pub(crate) fn timing_check_disabled(
    nl: &Netlist,
    c: &TimingConstraints,
    cell_id: usize,
    check_pin: usize,
) -> bool {
    c.disabled_timing_arcs.iter().any(|disabled| {
        let Some(pin_ref) = timing_pin_ref(nl, cell_id, check_pin) else {
            return false;
        };
        let object_matches = disabled.object.as_ref().is_some_and(|object| {
            object.kind == TimingObjectKind::Cell && object.matches_name(&nl.cell(cell_id).name)
                || (object.kind == TimingObjectKind::Pin && object.matches_object(&pin_ref))
        });
        let from_matches = disabled.from.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, Some(&pin_ref))
        });
        let to_matches = disabled.to.as_ref().is_none_or(|selector| {
            disabled_selector_matches_pin(selector, nl, cell_id, Some(&pin_ref))
        });
        let qualifiers_match = from_matches && to_matches;
        let instance_matches = disabled
            .instance_pattern
            .as_ref()
            .is_none_or(|pattern| cell_hierarchy_matches(nl, cell_id, pattern));
        (object_matches || disabled.object.is_none()) && qualifiers_match && instance_matches
    })
}

fn push_unique_object(objects: &mut Vec<TimingObjectRef>, object: TimingObjectRef) {
    if !objects.contains(&object) {
        objects.push(object);
    }
}

fn path_start_objects(nl: &Netlist, endpoint_net: usize) -> Vec<TimingObjectRef> {
    let mut objects = Vec::new();
    for net in path_start_names(nl, endpoint_net) {
        let Some(net_id) = nl.net_by_name(&net) else {
            continue;
        };
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Net, net.clone()),
        );
        if let Some(port) = nl
            .ports
            .iter()
            .find(|port| port.dir == eda_core::netlist::Dir::In && port.bits.contains(&net_id))
        {
            push_unique_object(
                &mut objects,
                TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
            );
        }
        for &cell_id in &nl.net(net_id).drivers {
            let cell = nl.cell(cell_id);
            push_unique_object(
                &mut objects,
                TimingObjectRef::new(TimingObjectKind::Cell, cell.name.clone()),
            );
            if nl.cell_is_sequential(&cell.kind) {
                if let Some(pin) = timing_pin_ref(nl, cell_id, 0) {
                    push_unique_object(&mut objects, pin);
                }
                if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
                    push_unique_object(&mut objects, pin);
                }
            } else {
                for pin in 0..cell.inputs.len() {
                    if let Some(pin_ref) = timing_pin_ref(nl, cell_id, pin) {
                        push_unique_object(&mut objects, pin_ref);
                    }
                }
                if let Some(pin_ref) = timing_output_pin_ref(nl, cell_id) {
                    push_unique_object(&mut objects, pin_ref);
                }
            }
        }
    }
    objects
}

fn endpoint_timing_objects(
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<(usize, usize)>,
) -> Vec<TimingObjectRef> {
    let mut objects = vec![TimingObjectRef::new(TimingObjectKind::Net, endpoint_name)];
    if let Some(port) = nl
        .ports
        .iter()
        .find(|port| port.dir == eda_core::netlist::Dir::Out && port.bits.contains(&endpoint_net))
    {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
        );
    }
    if let Some((cell_id, input_pin)) = endpoint_cell {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Cell, nl.cell(cell_id).name.clone()),
        );
        if let Some(pin) = timing_pin_ref(nl, cell_id, input_pin) {
            push_unique_object(&mut objects, pin);
        }
    }
    for &cell_id in &nl.net(endpoint_net).drivers {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Cell, nl.cell(cell_id).name.clone()),
        );
        if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
            push_unique_object(&mut objects, pin);
        }
    }
    objects
}

fn timing_object_matches(
    object: &TimingObjectRef,
    candidates: &[TimingObjectRef],
    fallback_name: &str,
) -> bool {
    if candidates
        .iter()
        .any(|candidate| object.matches_object(candidate))
    {
        return true;
    }

    // A cell exception also matches an explicitly referenced pin of that cell.
    if object.kind == TimingObjectKind::Cell {
        return candidates.iter().any(|candidate| {
            if candidate.kind != TimingObjectKind::Pin {
                return false;
            }
            candidate
                .name
                .rsplit_once('/')
                .is_some_and(|(cell_name, _pin)| object.matches_name(cell_name))
        });
    }
    object.matches_name(fallback_name)
}

fn timing_object_name_matches(
    pattern: &str,
    candidates: &[TimingObjectRef],
    fallback_name: &str,
) -> bool {
    let pattern = TimingObjectPattern::new(pattern);
    pattern.matches(fallback_name)
        || candidates
            .iter()
            .any(|candidate| pattern.matches(&candidate.name))
}

fn net_objects(nl: &Netlist, net: NetId) -> Vec<TimingObjectRef> {
    let mut objects = vec![TimingObjectRef::new(
        TimingObjectKind::Net,
        nl.net(net).name.clone(),
    )];
    if let Some(port) = nl.ports.iter().find(|port| port.bits.contains(&net)) {
        push_unique_object(
            &mut objects,
            TimingObjectRef::new(TimingObjectKind::Port, port.name.clone()),
        );
    }
    objects
}

fn path_object_stages(nl: &Netlist, cells: &[usize], nets: &[NetId]) -> Vec<Vec<TimingObjectRef>> {
    let mut stages = Vec::new();
    if let Some(&first_net) = nets.first() {
        stages.push(net_objects(nl, first_net));
    }

    for (cell_index, &cell_id) in cells.iter().enumerate() {
        let cell = nl.cell(cell_id);
        let first = cell_index == 0;
        let last = cell_index + 1 == cells.len();
        let sequential = nl.cell_is_sequential(&cell.kind);
        let mut input_stage = vec![TimingObjectRef::new(
            TimingObjectKind::Cell,
            cell.name.clone(),
        )];
        for (pin, &input) in cell.inputs.iter().enumerate() {
            let on_path = nets.contains(&input);
            let clock_pin = sequential && pin == 0 && (first || last);
            if on_path || clock_pin {
                if let Some(pin_ref) = timing_pin_ref(nl, cell_id, pin) {
                    push_unique_object(&mut input_stage, pin_ref);
                }
            }
        }
        stages.push(input_stage);

        let mut output_stage = Vec::new();
        if let Some(pin) = timing_output_pin_ref(nl, cell_id) {
            output_stage.push(pin);
        }
        if let Some(output_net) = nets.iter().find(|&&net| net == cell.output) {
            for object in net_objects(nl, *output_net) {
                push_unique_object(&mut output_stage, object);
            }
        }
        stages.push(output_stage);
    }
    stages
}

fn stage_contains_object(stage: &[TimingObjectRef], object: &TimingObjectRef) -> bool {
    timing_object_matches(object, stage, "")
}

fn through_groups_match(groups: &[Vec<TimingObjectRef>], stages: &[Vec<TimingObjectRef>]) -> bool {
    let mut stage_index = 0;
    for group in groups {
        let Some(match_index) = stages[stage_index..].iter().position(|stage| {
            group
                .iter()
                .any(|object| stage_contains_object(stage, object))
        }) else {
            return false;
        };
        stage_index += match_index + 1;
    }
    true
}

fn path_hierarchy_instances(nl: &Netlist, cells: &[usize], endpoint_net: NetId) -> Vec<String> {
    cells
        .iter()
        .filter_map(|&cell| nl.cell_hierarchy_instance(cell))
        .map(|instance| instance.path.clone())
        .chain(
            nl.owning_hierarchy_instance(endpoint_net)
                .map(|instance| instance.path.clone()),
        )
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn matching_path_exceptions(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    endpoint_objects: &[TimingObjectRef],
    c: &TimingConstraints,
) -> Vec<PathException> {
    let starts = path_start_objects(nl, endpoint_net);
    let (cells, nets) = if early {
        base.early_path_topology(nl, endpoint_net, endpoint_cell)
    } else {
        base.late_path_topology(nl, endpoint_net, endpoint_cell)
    };
    let stages = path_object_stages(nl, &cells, &nets);
    let path_instances = path_hierarchy_instances(nl, &cells, endpoint_net);
    let matches = |ex: &PathException| {
        let to_matches = match &ex.to_object {
            Some(object) => timing_object_matches(object, endpoint_objects, endpoint_name),
            None => ex
                .to
                .as_deref()
                .map(|to| timing_object_name_matches(to, endpoint_objects, endpoint_name))
                .unwrap_or(true),
        };
        let from_matches = match &ex.from_object {
            Some(object) => timing_object_matches(object, &starts, endpoint_name),
            None => ex
                .from
                .as_deref()
                .map(|from| timing_object_name_matches(from, &starts, endpoint_name))
                .unwrap_or(true),
        };
        let through_matches =
            ex.through_groups.is_empty() || through_groups_match(&ex.through_groups, &stages);
        let instance_matches = ex.instance_pattern.as_ref().is_none_or(|pattern| {
            path_instances
                .iter()
                .any(|instance| TimingObjectPattern::new(pattern).matches(instance))
        });
        to_matches && from_matches && through_matches && instance_matches
    };
    c.false_paths
        .iter()
        .chain(c.multicycle_paths.iter())
        .filter(|ex| matches(ex))
        .cloned()
        .collect()
}

fn record_path_exceptions(
    out: &mut BTreeMap<String, Vec<PathException>>,
    endpoint: &TimingObjectRef,
    exceptions: Vec<PathException>,
) {
    if exceptions.is_empty() {
        return;
    }
    let entry = out
        .entry(format!("{:?}:{}", endpoint.kind, endpoint.name))
        .or_default();
    if entry.is_empty() {
        *entry = exceptions;
    }
}

#[allow(clippy::too_many_arguments)]
fn path_multiplier(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    endpoint_objects: &[TimingObjectRef],
    c: &TimingConstraints,
) -> Option<f64> {
    let exceptions = matching_path_exceptions(
        base,
        nl,
        endpoint_net,
        endpoint_name,
        endpoint_cell,
        early,
        endpoint_objects,
        c,
    );
    if exceptions
        .iter()
        .any(|exception| exception.multicycle.is_none())
    {
        return Some(f64::INFINITY);
    }
    let mut multiplier = 1.0;
    let mut found = false;
    for ex in exceptions {
        multiplier *= ex.multicycle.unwrap_or(1.0);
        found = true;
    }
    if found {
        Some(multiplier)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn matching_path_delay_limits(
    base: &StaResult,
    nl: &Netlist,
    endpoint_net: usize,
    endpoint_name: &str,
    endpoint_cell: Option<usize>,
    early: bool,
    c: &TimingConstraints,
    maximum: bool,
    arrival: f64,
    endpoint_objects: &[TimingObjectRef],
) -> Vec<AppliedTimingDelayLimit> {
    if matches!(
        path_multiplier(
            base,
            nl,
            endpoint_net,
            endpoint_name,
            endpoint_cell,
            early,
            endpoint_objects,
            c
        ),
        Some(f64::INFINITY)
    ) {
        return Vec::new();
    }
    let starts = path_start_objects(nl, endpoint_net);
    let (cells, nets) = if early {
        base.early_path_topology(nl, endpoint_net, endpoint_cell)
    } else {
        base.late_path_topology(nl, endpoint_net, endpoint_cell)
    };
    let stages = path_object_stages(nl, &cells, &nets);
    let path_instances = path_hierarchy_instances(nl, &cells, endpoint_net);
    let matches = |limit: &PathDelayLimit| {
        let to_matches = match &limit.to_object {
            Some(object) => timing_object_matches(object, endpoint_objects, endpoint_name),
            None => limit
                .to
                .as_deref()
                .map(|to| timing_object_name_matches(to, endpoint_objects, endpoint_name))
                .unwrap_or(true),
        };
        let from_matches = match &limit.from_object {
            Some(object) => timing_object_matches(object, &starts, endpoint_name),
            None => limit
                .from
                .as_deref()
                .map(|from| timing_object_name_matches(from, &starts, endpoint_name))
                .unwrap_or(true),
        };
        let through_matches =
            limit.through_groups.is_empty() || through_groups_match(&limit.through_groups, &stages);
        let instance_matches = limit.instance_pattern.as_ref().is_none_or(|pattern| {
            path_instances
                .iter()
                .any(|instance| TimingObjectPattern::new(pattern).matches(instance))
        });
        to_matches && from_matches && through_matches && instance_matches
    };
    let limits = if maximum {
        &c.max_delay_paths
    } else {
        &c.min_delay_paths
    };
    limits
        .iter()
        .filter(|limit| matches(limit))
        .map(|limit| {
            let slack_ns = if maximum {
                limit.limit - arrival
            } else {
                arrival - limit.limit
            };
            AppliedTimingDelayLimit {
                maximum,
                limit: limit.clone(),
                slack_ns,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, CellKind, Dir, NetlistHierarchyInstance, NetlistHierarchyPort};
    use std::sync::Arc;

    #[test]
    fn timing_object_patterns_match_flat_and_hierarchical_names() {
        let local = TimingObjectPattern::new("u*");
        assert!(local.is_pattern());
        assert!(local.matches("u0"));
        assert!(local.matches("u123"));
        assert!(!local.matches("top/u0"));

        let single_character = TimingObjectPattern::new("u?");
        assert!(single_character.matches("u0"));
        assert!(!single_character.matches("u01"));
        assert!(!single_character.matches("top/u0"));

        let recursive = TimingObjectPattern::new("**/u*");
        assert!(recursive.matches("u0"));
        assert!(recursive.matches("top/u0"));
        assert!(recursive.matches("a/b/u0"));

        let dot_hierarchy = TimingObjectPattern::new("top.*");
        assert!(dot_hierarchy.matches("top.sub"));
        assert!(!dot_hierarchy.matches("top/sub"));

        let escaped = TimingObjectPattern::new(r"u\*");
        assert!(escaped.matches("u*"));
        assert!(!escaped.matches("u0"));
        assert!(!escaped.is_pattern());
    }

    #[test]
    fn parses_common_sdc_subset() {
        let s = parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports clk]
            set_clock_uncertainty 0.10 -setup [get_clocks core]
            set_input_delay 0.30 -clock core [get_ports a]
            set_output_delay 0.40 -clock core [get_ports y]
            set_false_path -from [get_ports b] -to [get_ports y]
            set_multicycle_path 2 -from [get_ports a] -to [get_ports y]
            set_disable_timing -from [get_pins u1/a] -to [get_pins u1/y]
            set_case_analysis 0 [get_ports b]
            set_max_delay 1.5 -from [get_ports a] -to [get_ports y]
            set_min_delay 0.5 -from [get_ports a] -to [get_ports y]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
        "#,
        )
        .unwrap();
        assert_eq!(s.clocks["core"].period, 4.0);
        assert_eq!(s.input_delays["a"].delay, 0.30);
        assert_eq!(s.output_delays["y"].delay, 0.40);
        assert_eq!(s.false_paths.len(), 1);
        assert_eq!(s.multicycle_paths.len(), 1);
        assert_eq!(s.disabled_timing_arcs.len(), 1);
        assert_eq!(s.case_analyses.len(), 1);
        assert_eq!(s.max_delay_paths.len(), 1);
        assert_eq!(s.min_delay_paths.len(), 1);
        assert_eq!(s.timing_derate.early, 0.9);
        assert_eq!(s.timing_derate.late, 1.1);
        assert_eq!(s.cell_timing_derates["u1"].early, 0.7);
        assert_eq!(s.cell_timing_derates["u1"].late, 1.3);
    }

    #[test]
    fn set_units_normalizes_subsequent_time_and_capacitance_values() {
        let source = r#"
            set_units -time 1ps -capacitance 1ff -resistance 1kohm -voltage 1mv -current 1ma
            create_clock -name clk -period 100 [get_ports clk]
            set_clock_uncertainty 10 -setup [get_clocks clk]
            set_clock_latency 20 [get_clocks clk]
            set_clock_transition 3 [get_clocks clk]
            set_input_delay 40 -clock clk [get_ports a]
            set_max_delay 300 -from [get_ports a] -to [get_ports y]
            set_max_transition 6
            set_max_capacitance 200
            set_load 70 [get_ports y]
        "#;
        let constraints = parse_sdc(source).unwrap();
        assert_eq!(constraints.units.time_unit, "ps");
        assert_eq!(constraints.units.capacitance_unit, "ff");
        assert_eq!(constraints.units.resistance_unit, "kohm");
        assert_eq!(constraints.units.voltage_unit, "mv");
        assert_eq!(constraints.units.current_unit, "ma");
        assert_eq!(constraints.clocks["clk"].period, 0.1);
        assert_eq!(constraints.clocks["clk"].fall, 0.05);
        assert_eq!(constraints.clock_uncertainty_ns["clk"], 0.01);
        assert_eq!(constraints.clocks["clk"].network_latency_ns, 0.02);
        assert_eq!(constraints.clocks["clk"].rise_transition_ns, 0.003);
        assert_eq!(constraints.input_delays["a"].delay, 0.04);
        assert_eq!(constraints.max_delay_paths[0].limit, 0.3);
        assert_eq!(constraints.max_transition_ns, Some(0.006));
        assert_eq!(constraints.max_capacitance_pf, Some(0.2));
        assert_eq!(constraints.load_capacitances[0].limit, 0.07);

        let defaults = parse_sdc("set_units -time ns -capacitance pf\n").unwrap();
        assert_eq!(defaults.units, SdcUnits::default());
        assert!(parse_sdc("set_units -time parsecs").is_err());
        assert!(parse_sdc("set_units -time 1ps -time 1ns").is_err());
        assert!(parse_sdc("set_units -unknown 1").is_err());
    }

    #[test]
    fn rejects_unknown_clock_and_bad_period() {
        assert!(parse_sdc("set_input_delay 1 -clock missing [get_ports a]").is_err());
        assert!(parse_sdc("create_clock -name c -period 0 [get_ports clk]").is_err());
        assert!(parse_sdc("set_max_delay 0 -to [get_ports y]").is_err());
        assert!(parse_sdc("set_min_delay -1 -to [get_ports y]").is_err());
        assert!(parse_sdc("set_clock_latency -1 [get_clocks c]").is_err());
        assert!(parse_sdc("set_timing_derate -early 0").is_err());
        assert!(parse_sdc("set_timing_derate").is_err());
        assert!(parse_sdc("set_timing_derate -early 1 [get_nets a]").is_err());
        assert!(
            parse_sdc("set_timing_derate -cell_delay -cell_check -early 1 [get_cells u1]").is_err()
        );
        assert!(parse_sdc("set_timing_derate -early 1 [get_pins u1]").is_err());
        assert!(parse_sdc("set_timing_derate -cell_check -early 1 [get_pins u1/a]").is_ok());
    }

    #[test]
    fn parses_and_derives_generated_clocks() {
        let s = parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports clk]
            create_generated_clock -name div2 -source [get_clocks core] -divide_by 2 -ports [get_ports div2]
            create_generated_clock -name fast -source [get_ports clk] -multiply_by 2 -ports [get_ports fast]
        "#,
        )
        .unwrap();
        assert_eq!(s.clocks["div2"].source_clock.as_deref(), Some("core"));
        assert_eq!(s.clocks["div2"].divide_by, Some(2.0));
        assert_eq!(s.clocks["div2"].period, 8.0);
        assert_eq!(s.clocks["div2"].ports, vec!["div2".to_string()]);
        assert_eq!(s.clocks["fast"].source_clock.as_deref(), Some("core"));
        assert_eq!(s.clocks["fast"].multiply_by, Some(2.0));
        assert_eq!(s.clocks["fast"].period, 2.0);
        assert_eq!(s.clocks["fast"].source_latency_ns, 0.0);

        let inherited = parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports clk]
            set_clock_latency -source 0.3 [get_clocks core]
            set_clock_latency 0.2 [get_clocks core]
            create_generated_clock -name div4 -source [get_clocks core] -divide_by 4 -ports [get_ports div4]
        "#,
        )
        .unwrap();
        assert_eq!(inherited.clocks["div4"].source_latency_ns, 0.5);
        assert_eq!(inherited.clocks["div4"].period, 16.0);

        assert!(parse_sdc(
            "create_generated_clock -name bad -source [get_clocks missing] -divide_by 2 -ports [get_ports bad]"
        )
        .is_err());
        assert!(parse_sdc(
            "create_clock -name core -period 4 [get_ports clk]\ncreate_generated_clock -name bad -source [get_clocks core] -divide_by 0 -ports [get_ports bad]"
        )
        .is_err());
    }

    #[test]
    fn parses_and_queries_clock_groups() {
        let s = parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports core_clk]
            create_clock -name io -period 6.0 [get_ports io_clk]
            create_clock -name scan -period 8.0 [get_ports scan_clk]
            set_clock_groups -name domains -asynchronous -group [get_clocks core] -group [get_clocks io scan]
        "#,
        )
        .unwrap();
        assert_eq!(s.clock_groups.len(), 1);
        assert_eq!(s.clock_groups[0].kind, ClockGroupKind::Asynchronous);
        assert_eq!(
            s.clock_group_kind("core", "io"),
            Some(ClockGroupKind::Asynchronous)
        );
        assert_eq!(
            s.clock_group_kind("core", "scan"),
            Some(ClockGroupKind::Asynchronous)
        );
        assert_eq!(s.clock_group_kind("io", "scan"), None);
        assert_eq!(s.clock_group_kind("core", "missing"), None);

        let changed = parse_sdc(
            r#"
            create_clock -name core -period 4.0 [get_ports core_clk]
            create_clock -name io -period 6.0 [get_ports io_clk]
            set_clock_groups -name exclusive -logically_exclusive -group [get_clocks core] -group [get_clocks io]
        "#,
        )
        .unwrap();
        let diff = diff_timing_constraints(&s, &changed);
        assert!(diff.clock_groups_changed);
        assert!(diff.constrained_recompute_required);

        assert!(parse_sdc(
            "create_clock -name core -period 4 [get_ports core]\nset_clock_groups -asynchronous -group [get_clocks core]"
        )
        .is_err());
        assert!(parse_sdc(
            "create_clock -name core -period 4 [get_ports core]\nset_clock_groups -asynchronous -group [get_clocks core] -group [get_clocks core]"
        )
        .is_err());
    }

    #[test]
    fn timing_derate_scales_late_and_early_arrivals() {
        let neutral = parse_sdc("create_clock -name clk -period 3 [get_ports clk]").unwrap();
        let derated = parse_sdc(
            r#"
            create_clock -name clk -period 3 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_timing_derate -early 0.8 -late 1.2
        "#,
        )
        .unwrap();
        let nl = and_netlist();
        let neutral_base = crate::analyze_with_constraints(&nl, &neutral).unwrap();
        let derated_base = crate::analyze_with_constraints(&nl, &derated).unwrap();
        let y = nl
            .ports
            .iter()
            .find(|port| port.name == "y")
            .and_then(|port| port.bits.first().copied())
            .unwrap();
        assert_eq!(neutral_base.net_arrival(y), Some(1.0));
        assert_eq!(neutral_base.net_min_arrival(y), Some(1.0));
        assert_eq!(derated_base.net_arrival(y), Some(1.44));
        assert_eq!(derated_base.net_min_arrival(y), Some(0.8));

        let constrained = analyze_with_sdc(&nl, &derated).unwrap();
        assert_eq!(constrained.early_derate, 0.8);
        assert_eq!(constrained.late_derate, 1.2);
        assert!(constrained_report(&constrained).contains("early derate 0.800"));
    }

    #[test]
    fn cell_timing_derate_overrides_global_for_cell_arcs() {
        let constrained = parse_sdc(
            r#"
            create_clock -name clk -period 4 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_cells u1]
        "#,
        )
        .unwrap();
        let nl = and_netlist();
        let base = crate::analyze_with_constraints(&nl, &constrained).unwrap();
        let y = nl
            .ports
            .iter()
            .find(|port| port.name == "y")
            .and_then(|port| port.bits.first().copied())
            .unwrap();
        // Input a late path: 0.2 * global 1.1 + AND arc 1.0 * cell 2.0.
        assert_eq!(base.net_arrival(y), Some(2.22));
        // Input b has no external delay; early path is AND arc 1.0 * 0.5.
        assert_eq!(base.net_min_arrival(y), Some(0.5));
        let result = analyze_with_sdc(&nl, &constrained).unwrap();
        assert_eq!(result.cell_derates["u1"].late, 2.0);
        assert!(constrained_report(&result).contains("cell derates   : 1"));

        let mut missing = constrained.clone();
        missing
            .set_cell_timing_derate("missing", Some(1.0), Some(1.0))
            .unwrap();
        assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
    }

    #[test]
    fn port_timing_derate_scales_external_delays() {
        let constraints = parse_sdc(
            r#"
            create_clock -name clk -period 4 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports b]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -early 0.5 -late 2.0 [get_ports a]
            set_timing_derate -late 2.0 [get_ports y]
        "#,
        )
        .unwrap();
        let nl = and_netlist();
        let base = crate::analyze_with_constraints(&nl, &constraints).unwrap();
        let y = nl
            .ports
            .iter()
            .find(|port| port.name == "y")
            .and_then(|port| port.bits.first().copied())
            .unwrap();
        // Late via a: 0.2 * 2.0 + AND arc 1.0 * global 1.1.
        assert_eq!(base.net_arrival(y), Some(1.5));
        // Early via a: 0.2 * 0.5 + AND arc 1.0 * global 0.9.
        assert_eq!(base.net_min_arrival(y), Some(1.0));

        let result = analyze_with_sdc(&nl, &constraints).unwrap();
        assert_eq!(result.port_derates.len(), 2);
        assert_eq!(result.port_derates["a"].early, 0.5);
        assert_eq!(result.port_derates["y"].late, 2.0);
        // Output arrival includes output delay 0.4 * y-late 2.0.
        assert!((result.output_slack - 1.7).abs() < 1e-12);
        assert!(constrained_report(&result).contains("port derates   : 2"));

        let mut missing = constraints.clone();
        missing
            .set_port_timing_derate("missing", Some(1.0), Some(1.0))
            .unwrap();
        assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
    }

    #[test]
    fn pin_timing_derate_overrides_cell_delay_derate() {
        let constraints = parse_sdc(
            r#"
            create_clock -name clk -period 4 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports b]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins u1/a]
        "#,
        )
        .unwrap();
        let nl = and_netlist();
        let base = crate::analyze_with_constraints(&nl, &constraints).unwrap();
        let y = nl
            .ports
            .iter()
            .find(|port| port.name == "y")
            .and_then(|port| port.bits.first().copied())
            .unwrap();
        // Late via pin a: 0.2 * global 1.1 + arc 1.0 * pin 2.0.
        assert_eq!(base.net_arrival(y), Some(2.22));
        // Early via pin a: 0.2 * global 0.9 + arc 1.0 * pin 0.5.
        assert_eq!(base.net_min_arrival(y), Some(0.68));

        let result = analyze_with_sdc(&nl, &constraints).unwrap();
        assert_eq!(result.pin_derates.len(), 1);
        assert_eq!(result.pin_derates[&TimingPin::new("u1", "a")].late, 2.0);
        assert!(constrained_report(&result).contains("pin derates   : 1"));

        let mut missing = constraints.clone();
        missing
            .set_pin_timing_derate("u1", "missing", Some(1.0), Some(1.0))
            .unwrap();
        assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
    }

    #[test]
    fn output_pin_timing_derate_scales_cell_and_clk_to_q() {
        let combinational = parse_sdc(
            r#"
            create_clock -name clk -period 4 [get_ports clk]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_timing_derate -cell_delay -early 0.7 -late 1.3 [get_cells u1]
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins u1/y]
        "#,
        )
        .unwrap();
        let and = and_netlist();
        let and_result = crate::analyze_with_constraints(&and, &combinational).unwrap();
        let y = and
            .ports
            .iter()
            .find(|port| port.name == "y")
            .and_then(|port| port.bits.first().copied())
            .unwrap();
        assert_eq!(and_result.net_arrival(y), Some(2.2));
        // The b input has no external delay; the output-pin early derate
        // applies to its arc as well.
        assert_eq!(and_result.net_min_arrival(y), Some(0.5));

        let sequential = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_pins launch/q]
        "#,
        )
        .unwrap();
        let flops = two_flop_netlist();
        let flop_result = analyze_with_sdc(&flops, &sequential).unwrap();
        assert!((flop_result.reg2reg_slack - 1.0).abs() < 1e-12);
        assert!((flop_result.reg2reg_hold_slack - 0.4).abs() < 1e-12);
        assert_eq!(flop_result.pin_derates.len(), 1);
    }

    #[test]
    fn cell_check_derate_scales_setup_and_hold_requirements() {
        let nl = two_flop_netlist();
        let global = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0
        "#,
        )
        .unwrap();
        let global_result = analyze_with_sdc(&nl, &global).unwrap();
        // Capture endpoint: period 4 - clk-to-q 1.2 - setup 0.6 * late 2.
        assert!((global_result.reg2reg_slack - 1.6).abs() < 1e-12);
        // Input launch endpoint: 0 arrival - hold 0.2 * early 0.5.
        assert!((global_result.in2reg_hold_slack + 0.1).abs() < 1e-12);
        assert_eq!(global_result.early_check_derate, 0.5);
        assert_eq!(global_result.late_check_derate, 2.0);
        // Default 4 ns clock has 2 ns high and low widths; DFF requirement
        // is 0.25 ns * late 2.0.
        assert!((global_result.min_pulse_width_slack - 1.5).abs() < 1e-12);
        assert!(constrained_report(&global_result).contains("check derates  : 0"));

        let capture = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_cells capture]
        "#,
        )
        .unwrap();
        let capture_result = analyze_with_sdc(&nl, &capture).unwrap();
        assert!((capture_result.reg2reg_slack - 1.6).abs() < 1e-12);
        assert_eq!(capture_result.cell_check_derates.len(), 1);
        assert!(constrained_report(&capture_result).contains("check derates  : 1"));

        let mut missing = capture.clone();
        missing
            .set_cell_check_derate("missing", Some(1.0), Some(1.0))
            .unwrap();
        assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
    }

    #[test]
    fn pin_check_derate_overrides_cell_check_rules() {
        let nl = two_flop_netlist();
        let constraints = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_timing_derate -cell_check -early 0.8 -late 1.2
            set_timing_derate -cell_check -early 0.7 -late 1.3 [get_cells capture]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_pins capture/d]
        "#,
        )
        .unwrap();
        let result = analyze_with_sdc(&nl, &constraints).unwrap();
        // Capture data-pin setup: 4 - 1.2 clk-to-q - 0.6 setup * pin late 2.
        assert!((result.reg2reg_slack - 1.6).abs() < 1e-12);
        // Capture data-pin hold: 1.2 clk-to-q - 0.2 hold * pin early 0.5.
        assert!((result.reg2reg_hold_slack - 1.1).abs() < 1e-12);
        assert_eq!(result.pin_check_derates.len(), 1);
        assert!(constrained_report(&result).contains("pin check derates : 1"));

        let mut missing = constraints.clone();
        missing
            .set_pin_check_derate("capture", "missing", Some(1.0), Some(1.0))
            .unwrap();
        assert!(crate::analyze_with_constraints(&nl, &missing).is_err());
    }

    fn and_netlist() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("sdc");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl
    }

    fn two_flop_netlist() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("hold");
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

    #[test]
    fn constrained_paths_expose_launch_capture_clock_metadata() {
        let nl = two_flop_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.1 [get_clocks clk]
            set_clock_latency 0.2 [get_clocks clk]
        "#,
        )
        .unwrap();
        let timing = analyze_with_sdc(&nl, &c).unwrap();
        let paths = constrained_timing_paths(&nl, &timing, 0);
        let sequential = paths
            .iter()
            .find(|path| {
                matches!(path.path.launch, crate::TimingLaunch::SequentialCell(_))
                    && matches!(
                        path.path.endpoint,
                        crate::TimingPathEndpoint::SequentialCell(_)
                    )
            })
            .unwrap();
        assert_eq!(sequential.clocks.launch.edge, TimingClockEdgeKind::Rise);
        assert_eq!(sequential.clocks.launch.network_latency_ns, 0.2);
        assert_eq!(sequential.clocks.capture.network_latency_ns, 0.2);
        assert!((sequential.clocks.edge_separation_ns - 4.0).abs() < 1e-12);
        assert!((sequential.clocks.insertion_skew_ns - 0.0).abs() < 1e-12);

        let input = paths
            .iter()
            .find(|path| {
                matches!(path.path.launch, crate::TimingLaunch::PrimaryInput(_))
                    && matches!(
                        path.path.endpoint,
                        crate::TimingPathEndpoint::SequentialCell(_)
                    )
            })
            .unwrap();
        assert!(input.clocks.launch.virtual_reference);
        assert!(!input.clocks.capture.virtual_reference);
        assert!((input.clocks.edge_separation_ns - 4.2).abs() < 1e-12);
        assert!((input.clocks.insertion_skew_ns - 0.2).abs() < 1e-12);

        let report = constrained_timing_path_report(&nl, &paths);
        assert!(
            report.contains("launch: clk rise at 0.000000 ns")
                && report.contains("capture: clk rise at 0.000000 ns")
                && report.contains("edge separation 4.200000 ns, insertion skew 0.200000 ns"),
            "{report}"
        );
    }

    #[test]
    fn input_output_delay_and_uncertainty_affect_slack() {
        let nl = and_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_clock_uncertainty 0.1 -setup [get_clocks clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        // 2.0ns period - 0.1 uncertainty - 0.3 input delay
        // - 1.0ns AND2 - 0.4 output delay = 0.2ns.
        assert!((r.output_slack - 0.2).abs() < 1e-9);
        assert!((r.worst_slack - 0.2).abs() < 1e-9);
        let y = r.net_slacks.get("y").unwrap();
        assert!((y.output_ns.unwrap() - 0.2).abs() < 1e-9);
        assert!((y.late_slack_ns().unwrap() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn hold_uncertainty_and_min_arrivals_affect_hold_slack() {
        let nl = two_flop_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        // Launch flop: 1.2ns clk->q; capture hold is 0.2ns.
        assert!((r.reg2reg_hold_slack - 0.95).abs() < 1e-9);
        // Primary input launch: 1.0ns input delay - 0.2ns hold - 0.05ns uncertainty.
        assert!((r.in2reg_hold_slack - 0.75).abs() < 1e-9);
        assert!((r.hold_slack - 0.75).abs() < 1e-9);
        assert!((r.worst_slack - 0.75).abs() < 1e-9);
        assert!((r.hold_uncertainty - 0.05).abs() < 1e-9);
        let d = r.net_slacks.get("d").unwrap();
        let q1 = r.net_slacks.get("q1").unwrap();
        assert!((d.setup_ns.unwrap() - 2.3).abs() < 1e-9);
        assert!((d.hold_ns.unwrap() - 0.75).abs() < 1e-9);
        assert!((q1.setup_ns.unwrap() - 2.1).abs() < 1e-9);
        assert!((q1.hold_ns.unwrap() - 0.95).abs() < 1e-9);

        let paths = constrained_timing_paths(&nl, &r, 0);
        assert_eq!(paths.len(), 3);
        assert!((paths[0].slack_ns - 2.1).abs() < 1e-9);
        assert!((paths[0].path.arrival_ns - 1.2).abs() < 1e-9);
        assert!((paths[0].required_ns - 3.3).abs() < 1e-9);
        assert!(matches!(
            paths[0].path.endpoint,
            crate::TimingPathEndpoint::SequentialCell(_)
        ));
        assert!(constrained_timing_path_report(&nl, &paths).contains("slack 2.100"));

        let hold_paths = constrained_hold_timing_paths(&nl, &r, 0);
        assert_eq!(hold_paths.len(), 2);
        assert!((hold_paths[0].slack_ns - 0.75).abs() < 1e-9);
        assert!((hold_paths[0].path.path.arrival_ns - 1.0).abs() < 1e-9);
        assert!((hold_paths[0].required_ns - 0.25).abs() < 1e-9);
        assert!(matches!(
            hold_paths[0].path.path.launch,
            crate::TimingLaunch::PrimaryInput(_)
        ));
        assert!(matches!(
            hold_paths[1].path.path.launch,
            crate::TimingLaunch::SequentialCell(_)
        ));
        assert!(constrained_hold_timing_path_report(&nl, &hold_paths).contains("hold slack 0.750"));
    }

    #[test]
    fn max_and_min_delay_limits_apply_to_paths() {
        let nl = and_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_max_delay 0.8 -from [get_ports a] -to [get_ports y]
            set_min_delay 0.5 -from [get_ports a] -to [get_ports y]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        // AND2 max and min input-to-output delay are both 1.0 ns.
        assert!((r.max_delay_slack + 0.2).abs() < 1e-9);
        assert!((r.min_delay_slack - 0.5).abs() < 1e-9);
        assert!((r.worst_slack + 0.2).abs() < 1e-9);
    }

    #[test]
    fn parses_design_wide_quality_limits() {
        let sdc = parse_sdc(
            "set_max_transition 0.25\n\
             set_max_capacitance 0.04\n\
             set_max_fanout 8",
        )
        .unwrap();
        assert_eq!(sdc.max_transition_ns, Some(0.25));
        assert_eq!(sdc.max_capacitance_pf, Some(0.04));
        assert_eq!(sdc.max_fanout, Some(8));
        let options = sdc.quality_options();
        assert_eq!(options.max_transition_ns, Some(0.25));
        assert_eq!(options.max_capacitance_pf, Some(0.04));
        assert_eq!(options.max_fanout, Some(8));

        assert!(parse_sdc("set_max_transition 0").is_err());
        assert!(parse_sdc("set_max_capacitance -1").is_err());
        assert!(parse_sdc("set_max_fanout 0").is_err());
        assert!(parse_sdc("set_max_fanout x").is_err());
        assert!(parse_sdc("set_max_transition 0.1 [get_clocks clk]").is_err());
        assert!(parse_sdc("set_max_transition 0.1 -from [get_ports a]").is_err());
    }

    #[test]
    fn parses_object_scoped_quality_limits() {
        let sdc = parse_sdc(
            "set_max_transition 10\n\
             set_max_transition 0.11 [get_ports y]\n\
             set_max_transition 0.12 [get_pins u1/Y]\n\
             set_max_capacitance 10\n\
             set_max_capacitance 0.021 [get_nets n*]\n\
             set_max_fanout 16\n\
             set_max_fanout 2 [get_cells u*]",
        )
        .unwrap();
        assert_eq!(sdc.max_transition_ns, Some(10.0));
        assert_eq!(sdc.scoped_max_transition_ns.len(), 2);
        assert_eq!(
            sdc.scoped_max_transition_ns[0].object,
            TimingObjectRef::new(TimingObjectKind::Port, "y")
        );
        assert_eq!(sdc.scoped_max_transition_ns[0].limit, 0.11);
        assert_eq!(
            sdc.scoped_max_transition_ns[1].object,
            TimingObjectRef::new(TimingObjectKind::Pin, "u1/Y")
        );
        assert_eq!(sdc.scoped_max_capacitance_pf.len(), 1);
        assert_eq!(
            sdc.scoped_max_capacitance_pf[0].object.kind,
            TimingObjectKind::Net
        );
        assert_eq!(sdc.scoped_max_fanout[0].limit, 2.0);
        let options = sdc.quality_options();
        assert_eq!(
            options.scoped_max_transition_ns,
            sdc.scoped_max_transition_ns
        );
        assert_eq!(
            options.scoped_max_capacitance_pf,
            sdc.scoped_max_capacitance_pf
        );
        assert_eq!(options.scoped_max_fanout, sdc.scoped_max_fanout);
    }

    #[test]
    fn parses_input_transition_and_load_rules() {
        let sdc = parse_sdc(
            "set_input_transition 0.12 [get_ports a]\n\
             set_input_transition 0.20 [get_pins u1/a]\n\
             set_input_transition 0.30 [get_pins u1/a]\n\
             set_load 0.021 [get_ports y]\n\
             set_load -pin_load 0.032 [get_pins u1/a]",
        )
        .unwrap();
        assert_eq!(sdc.input_transitions.len(), 2);
        assert_eq!(sdc.input_transitions[0].limit, 0.12);
        assert_eq!(sdc.input_transitions[1].object.name, "u1/a");
        assert_eq!(sdc.input_transitions[1].limit, 0.30);
        assert_eq!(sdc.load_capacitances.len(), 2);
        assert_eq!(sdc.load_capacitances[0].limit, 0.021);
        assert_eq!(sdc.load_capacitances[1].limit, 0.032);
        let options = sdc.quality_options();
        assert_eq!(options.input_transitions, sdc.input_transitions);
        assert_eq!(options.load_capacitances, sdc.load_capacitances);

        assert!(parse_sdc("set_input_transition -1 [get_ports a]").is_err());
        assert!(parse_sdc("set_load -1 [get_ports y]").is_err());
        assert!(parse_sdc("set_input_transition 0.1 [get_cells u1]").is_err());
        assert!(parse_sdc("set_load 0.1 [get_clocks clk]").is_err());
        assert!(parse_sdc("set_load -wire_load 0.1 [get_ports y]").is_err());
    }

    #[test]
    fn parses_clock_transition_rules() {
        let sdc = parse_sdc(
            "create_clock -name clk -period 4 [get_ports clk]\n\
             set_clock_transition -rise 0.12 [get_clocks clk]\n\
             set_clock_transition -fall 0.24 [get_clocks clk]",
        )
        .unwrap();
        assert_eq!(sdc.clocks["clk"].rise_transition_ns, 0.12);
        assert_eq!(sdc.clocks["clk"].fall_transition_ns, 0.24);

        let both = parse_sdc(
            "create_clock -name c -period 2 [get_ports c]\n\
             set_clock_transition -min -max 0.31 [get_clocks c]",
        )
        .unwrap();
        assert_eq!(both.clocks["c"].rise_transition_ns, 0.31);
        assert_eq!(both.clocks["c"].fall_transition_ns, 0.31);

        let split = parse_sdc(
            "create_clock -name c -period 2 [get_ports c]\n\
             set_clock_transition -min 0.21 [get_clocks c]\n\
             set_clock_transition -max 0.33 [get_clocks c]",
        )
        .unwrap();
        assert_eq!(split.clocks["c"].min_rise_transition_ns, 0.21);
        assert_eq!(split.clocks["c"].min_fall_transition_ns, 0.21);
        assert_eq!(split.clocks["c"].rise_transition_ns, 0.33);
        assert_eq!(split.clocks["c"].fall_transition_ns, 0.33);

        assert!(parse_sdc(
            "create_clock -name c -period 2 [get_ports c]\n\
             set_clock_transition -rise -1 [get_clocks c]"
        )
        .is_err());
        assert!(parse_sdc(
            "create_clock -name c -period 2 [get_ports c]\n\
             set_clock_transition 0.1 [get_ports c]"
        )
        .is_err());
    }

    #[test]
    fn min_pulse_width_checks_clock_waveform() {
        let nl = two_flop_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 -fall 0.3 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        // Standard DFFs require 0.25 ns high and low pulses.
        assert!((r.min_pulse_high_slack - 0.05).abs() < 1e-9);
        assert!((r.min_pulse_low_slack - 3.45).abs() < 1e-9);
        assert!((r.min_pulse_width_slack - 0.05).abs() < 1e-9);
        assert!((r.worst_slack - 0.05).abs() < 1e-9);
    }

    #[test]
    fn recovery_and_removal_checks_async_reset() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("async");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let rst = nl.add_net("rst");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Lib(lib.find("DFFR").unwrap()),
            inputs: vec![clk, d, rst],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("rst", Dir::In, vec![rst]);
        nl.add_port("q", Dir::Out, vec![q]);
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
        "#,
        )
        .unwrap();
        let base = crate::analyze_with_constraints(&nl, &c).unwrap();
        assert_eq!(base.reset_endpoint_list().len(), 1);
        let unconstrained_reset = base.reset_timing_paths(&nl, 0);
        assert_eq!(unconstrained_reset.len(), 1);
        assert_eq!(unconstrained_reset[0].recovery.cells.len(), 1);
        assert_eq!(unconstrained_reset[0].removal.nets, vec![rst]);
        assert!(
            crate::reset_timing_path_report(&nl, &unconstrained_reset).contains("recovery arrival")
        );
        let r = analyze_constrained(&nl, base, &c).unwrap();
        // Effective period 3.9 - reset arrival 1.2 - recovery 0.3.
        assert!((r.recovery_slack - 2.4).abs() < 1e-9);
        // Reset arrival 1.2 - removal 0.15 - hold uncertainty 0.05.
        assert!((r.removal_slack - 1.0).abs() < 1e-9);
        assert!((r.async_reset_slack - 1.0).abs() < 1e-9);

        let reset_slack = r.net_slacks.get("rst").unwrap();
        assert!((reset_slack.recovery_ns.unwrap() - 2.4).abs() < 1e-9);
        assert!((reset_slack.removal_ns.unwrap() - 1.0).abs() < 1e-9);
        let reset_paths = constrained_reset_timing_paths(&nl, &r, 0);
        assert_eq!(reset_paths.len(), 2);
        assert_eq!(reset_paths[1].kind, ResetCheckKind::Recovery);
        assert!((reset_paths[1].slack_ns - 2.4).abs() < 1e-9);
        assert!((reset_paths[1].required_ns - 3.6).abs() < 1e-9);
        assert_eq!(reset_paths[0].kind, ResetCheckKind::Removal);
        assert!((reset_paths[0].slack_ns - 1.0).abs() < 1e-9);
        assert!((reset_paths[0].required_ns - 0.2).abs() < 1e-9);
        assert!(constrained_reset_timing_path_report(&nl, &reset_paths)
            .contains("Constrained reset timing path report"));

        let derated = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.2 -clock clk [get_ports rst]
            set_clock_uncertainty 0.10 -setup [get_clocks clk]
            set_clock_uncertainty 0.05 -hold [get_clocks clk]
            set_timing_derate -cell_check -early 0.5 -late 2.0 [get_cells ff]
        "#,
        )
        .unwrap();
        let derated_result = analyze_with_sdc(&nl, &derated).unwrap();
        // Recovery is a max check: 3.9 - 1.2 - 0.3 * late 2.0.
        assert!((derated_result.recovery_slack - 2.1).abs() < 1e-12);
        // Removal is a min check: 1.2 - 0.15 * early 0.5 - 0.05.
        assert!((derated_result.removal_slack - 1.075).abs() < 1e-12);
        assert_eq!(derated_result.cell_check_derates.len(), 1);
    }

    #[test]
    fn clock_latency_affects_io_but_not_reg2reg() {
        let nl = two_flop_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_clock_latency -source 0.7 [get_clocks clk]
            set_clock_latency 0.3 [get_clocks clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        // Common source/network latency cancels for same-clock reg-to-reg.
        assert!((r.reg2reg_slack - 2.2).abs() < 1e-9);
        // External input launch does not include local clock network latency.
        assert!((r.in2reg_slack - 2.7).abs() < 1e-9);
        // Internal flop launch includes source plus local network latency.
        assert!((r.output_slack - 1.8).abs() < 1e-9);
        assert!((r.source_latency - 0.7).abs() < 1e-9);
        assert!((r.network_latency - 0.3).abs() < 1e-9);
    }

    #[test]
    fn false_path_relaxes_external_endpoint() {
        let nl = and_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_false_path -from [get_ports a] -to [get_ports y]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        assert!(r.output_slack.is_infinite());
        assert_eq!(r.exceptions_applied, 1);
    }

    #[test]
    fn instance_scoped_false_path_applies_only_to_matching_hierarchy() {
        let mut nl = and_netlist();
        let y = nl.net_by_name("y").unwrap();
        nl.hierarchy = vec![NetlistHierarchyInstance {
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
        }];
        let source = r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_false_path -from [get_ports a] -to [get_ports y] -instance top.u1.u_leaf
        "#;
        let matching = parse_sdc(source).unwrap();
        assert_eq!(
            matching.false_paths[0].instance_pattern.as_deref(),
            Some("top.u1.u_leaf")
        );
        let result = analyze_with_sdc(&nl, &matching).unwrap();
        assert!(result.output_slack.is_infinite());
        assert_eq!(result.exceptions_applied, 1);

        let nonmatching = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_false_path -from [get_ports a] -to [get_ports y] -instance top.other.*
        "#,
        )
        .unwrap();
        let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
        assert!(result.output_slack.is_finite());
        assert_eq!(result.exceptions_applied, 0);
        assert!(parse_sdc("set_false_path -from a -instance").is_err());
    }

    #[test]
    fn instance_scoped_delay_limit_applies_only_to_matching_hierarchy() {
        let mut nl = and_netlist();
        let y = nl.net_by_name("y").unwrap();
        nl.hierarchy = vec![NetlistHierarchyInstance {
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
        }];
        let source = r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_max_delay 0.2 -from [get_ports a] -to [get_ports y] -instance top.u1.u_leaf
        "#;
        let matching = parse_sdc(source).unwrap();
        assert_eq!(
            matching.max_delay_paths[0].instance_pattern.as_deref(),
            Some("top.u1.u_leaf")
        );
        let result = analyze_with_sdc(&nl, &matching).unwrap();
        assert_eq!(result.path_max_delay_limits["Port:y"].len(), 1);
        assert!(result.max_delay_slack.is_finite());
        assert!((result.max_delay_slack + 1.1).abs() < 1e-12);

        let nonmatching = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_max_delay 0.2 -from [get_ports a] -to [get_ports y] -instance top.other.*
        "#,
        )
        .unwrap();
        let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
        assert_eq!(result.path_max_delay_limits["Port:y"].len(), 0);
        assert!(result.max_delay_slack.is_infinite());
        assert!(parse_sdc("set_max_delay 1 -from a -instance").is_err());
    }

    #[test]
    fn instance_scoped_disabled_timing_arc_matches_only_hierarchy_owner() {
        let mut nl = and_netlist();
        let y = nl.net_by_name("y").unwrap();
        nl.hierarchy = vec![NetlistHierarchyInstance {
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
        }];
        let source = "set_disable_timing [get_cells u1] -instance top.u1.u_leaf";
        let matching = parse_sdc(source).unwrap();
        assert_eq!(
            matching.disabled_timing_arcs[0].instance_pattern.as_deref(),
            Some("top.u1.u_leaf")
        );
        assert!(timing_arc_disabled(&nl, &matching, 0, Some(0), None));

        let nonmatching =
            parse_sdc("set_disable_timing [get_cells u1] -instance top.other.*").unwrap();
        assert!(!timing_arc_disabled(&nl, &nonmatching, 0, Some(0), None));
        assert!(parse_sdc("set_disable_timing -instance top.u1").is_err());
        assert!(parse_sdc("set_disable_timing -instance").is_err());
    }

    #[test]
    fn instance_scoped_timing_derate_applies_only_to_hierarchy_owner() {
        let mut nl = and_netlist();
        let y = nl.net_by_name("y").unwrap();
        nl.hierarchy = vec![NetlistHierarchyInstance {
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
        }];
        let matching = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_timing_derate -cell_delay -late 2.0 [get_cells *] -instance top.u1.u_leaf
        "#,
        )
        .unwrap();
        assert_eq!(matching.scoped_cell_timing_derates.len(), 1);
        assert_eq!(
            matching.scoped_cell_timing_derates[0]
                .instance_pattern
                .as_deref(),
            Some("top.u1.u_leaf")
        );
        let result = analyze_with_sdc(&nl, &matching).unwrap();
        assert!((result.base.net_arrival(y).unwrap() - 2.0).abs() < 1e-12);

        let nonmatching = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports a]
            set_timing_derate -cell_delay -late 2.0 [get_cells *] -instance top.other.*
        "#,
        )
        .unwrap();
        let result = analyze_with_sdc(&nl, &nonmatching).unwrap();
        assert!((result.base.net_arrival(y).unwrap() - 1.0).abs() < 1e-12);
        assert!(parse_sdc("set_timing_derate -late 2 -instance top.u1").is_err());
        assert!(parse_sdc("set_timing_derate -late 2 [get_ports a] -instance top.u1").is_err());
    }

    #[test]
    fn multicycle_path_doubles_output_requirement() {
        let nl = and_netlist();
        let c = parse_sdc(
            r#"
            create_clock -name clk -period 2.0 [get_ports clk]
            set_input_delay 0.3 -clock clk [get_ports a]
            set_output_delay 0.4 -clock clk [get_ports y]
            set_multicycle_path 2 -from [get_ports a] -to [get_ports y]
        "#,
        )
        .unwrap();
        let r = analyze_with_sdc(&nl, &c).unwrap();
        assert!((r.output_slack - 2.3).abs() < 1e-9);
    }
}
