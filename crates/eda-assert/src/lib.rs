//! # eda-assert — SVA-like assertion checking over sampled VCD traces.
//!
//! The subset is deliberately small but useful for agent-driven regression:
//! Boolean expressions, `always`, `never`, and overlapped temporal implication
//! with an explicit bounded delay (`##[min:max]`).
//! `disable iff (expr)` aborts an obligation while the expression is true.
//! `assert property`, `assume property`, and `cover property` directives are
//! supported.

use eda_core::logic::LogicValue;
use eda_waveform::Vcd;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tri {
    False,
    True,
    Unknown,
}

impl Tri {
    fn not(self) -> Self {
        match self {
            Tri::False => Tri::True,
            Tri::True => Tri::False,
            Tri::Unknown => Tri::Unknown,
        }
    }

    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Tri::False, _) | (_, Tri::False) => Tri::False,
            (Tri::True, Tri::True) => Tri::True,
            _ => Tri::Unknown,
        }
    }

    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Tri::True, _) | (_, Tri::True) => Tri::True,
            (Tri::False, Tri::False) => Tri::False,
            _ => Tri::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoolExpr {
    True,
    False,
    Signal(String),
    Not(Box<BoolExpr>),
    And(Box<BoolExpr>, Box<BoolExpr>),
    Or(Box<BoolExpr>, Box<BoolExpr>),
    Implication(Box<BoolExpr>, Box<BoolExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Property {
    Immediate(BoolExpr),
    Always(BoolExpr),
    Implication {
        antecedent: BoolExpr,
        consequence: BoolExpr,
        min_cycle: usize,
        max_cycle: usize,
    },
    /// `antecedent[*min_repeat:max_repeat] |-> consequence`.
    ///
    /// The antecedent must hold for the selected number of consecutive clock
    /// samples. `antecedent[*]` and `antecedent[+]` set `unbounded`; dynamic
    /// checking and BMC interpret them through the available observation or
    /// bound. `overlapped` distinguishes `|->` at the final antecedent sample
    /// from `|=>` at the following sample.
    RepeatedImplication {
        antecedent: BoolExpr,
        min_repeat: usize,
        max_repeat: usize,
        /// When true, `max_repeat` is a placeholder and the bounded checker
        /// extends repetition through its available horizon. `[*]` starts at
        /// zero repetitions; `[+]` starts at one.
        unbounded: bool,
        overlapped: bool,
        consequence: BoolExpr,
    },
    /// `antecedent[=min:max] |-> consequence`.
    ///
    /// The antecedent expression must be true exactly the selected number of
    /// times between the sequence start and its endpoint; intermediate cycles
    /// may be false.
    NonconsecutiveImplication {
        antecedent: BoolExpr,
        min_count: usize,
        max_count: usize,
        overlapped: bool,
        consequence: BoolExpr,
    },
    /// `first ##[min_delay:max_delay] second |-> consequence`.
    ///
    /// The antecedent sequence starts when `first` holds and matches when
    /// `second` holds after the selected delay. Intermediate samples are not
    /// constrained.
    SequenceImplication {
        first: BoolExpr,
        second: BoolExpr,
        min_delay: usize,
        max_delay: usize,
        overlapped: bool,
        consequence: BoolExpr,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assertion {
    pub name: String,
    pub kind: AssertionKind,
    pub clock: Option<String>,
    pub edge: Option<Edge>,
    /// When this expression is true, evaluation of the current obligation is
    /// disabled. It corresponds to SVA `disable iff`.
    pub disable: Option<BoolExpr>,
    pub property: Property,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssertionKind {
    Assert,
    Assume,
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Pos,
    Neg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssertionStatus {
    Pass,
    Fail { sample: usize },
    CoverHit { sample: usize },
    CoverMiss,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertionResult {
    pub name: String,
    pub kind: AssertionKind,
    pub status: AssertionStatus,
    pub samples: usize,
    pub clock: Option<String>,
}

impl AssertionResult {
    pub fn passed(&self) -> bool {
        matches!(
            self.status,
            AssertionStatus::Pass | AssertionStatus::CoverHit { .. }
        )
    }
}

/// Parse a compact SVA-style assertion subset.
///
/// Supported forms:
/// - `assert property (@(posedge clk) expr);`
/// - `assume property (@(posedge clk) expr);`
/// - `assert property (@(posedge clk) always expr);`
/// - `assert property (@(posedge clk) never expr);`
/// - `assert property (@(posedge clk) ant |-> cons);`
/// - `assert property (@(posedge clk) ant |-> ##[min:max] cons);`
/// - `assert property (@(posedge clk) ant |=> cons);`
/// - `assert property (@(posedge clk) ant[*min:max] |-> cons);`
/// - `assert property (@(posedge clk) ant[*] |-> cons);`
/// - `assert property (@(posedge clk) ant[+] |=> cons);`
/// - `assert property (@(posedge clk) ant[=min:max] |-> cons);`
/// - `assert property (@(posedge clk) first ##[min:max] second |-> cons);`
/// - `cover property (@(posedge clk) ant |-> ##[min:max] cons);`
pub fn parse_assertions(source: &str) -> Result<Vec<Assertion>, String> {
    let tokens = lex(source)?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut pending_name: Option<String> = None;

    while pos < tokens.len() {
        match tokens[pos].clone() {
            Token::Word(word, line) if word == "property" => {
                let name = match tokens.get(pos + 1) {
                    Some(Token::Word(word, _)) => word.clone(),
                    _ => return Err(format!("line {}: property requires a name", line)),
                };
                expect_punct(&tokens, pos + 2, ";", line)?;
                pending_name = Some(name);
                pos += 3;
            }
            Token::Word(word, _line) if word == "endproperty" => {
                pending_name = None;
                pos += if matches!(tokens.get(pos + 1), Some(Token::Punct(";", _))) {
                    2
                } else {
                    1
                };
            }
            Token::Word(word, _line) if word == "assert" || word == "assume" || word == "cover" => {
                let kind = if word == "assert" {
                    AssertionKind::Assert
                } else if word == "assume" {
                    AssertionKind::Assume
                } else {
                    AssertionKind::Cover
                };
                let (assertion, next) = parse_assertion(&tokens, pos, kind, pending_name.clone())?;
                out.push(assertion);
                pos = next;
            }
            Token::Punct(";", _) => pos += 1,
            token => {
                let line = token.line();
                return Err(format!(
                    "line {}: expected property declaration or assertion, found {}",
                    line,
                    token.display()
                ));
            }
        }
    }
    if out.is_empty() {
        return Err("no assertions found".into());
    }
    Ok(out)
}

pub fn check_vcd(vcd: &Vcd, assertions: &[Assertion]) -> Result<Vec<AssertionResult>, String> {
    assertions
        .iter()
        .map(|assertion| check_one(vcd, assertion))
        .collect()
}

fn check_one(vcd: &Vcd, assertion: &Assertion) -> Result<AssertionResult, String> {
    let sample_times = sample_times(vcd, assertion)?;
    let mut samples = Vec::with_capacity(sample_times.len());
    let mut disabled = Vec::with_capacity(sample_times.len());
    for time in sample_times {
        let mut values = BTreeMap::new();
        for signal in signals(&assertion.property, assertion.disable.as_ref()) {
            values.insert(signal.clone(), signal_value(vcd, &signal, time)?);
        }
        let disabled_value = match assertion.disable.as_ref() {
            Some(expr) => eval(expr, &values),
            None => Tri::False,
        };
        disabled.push(disabled_value);
        samples.push(values);
    }

    let status = match &assertion.property {
        Property::Immediate(expr) | Property::Always(expr) => match assertion.kind {
            AssertionKind::Assert | AssertionKind::Assume => {
                immediate_status(&samples, &disabled, expr)
            }
            AssertionKind::Cover => immediate_cover_status(&samples, &disabled, expr),
        },
        Property::Implication {
            antecedent,
            consequence,
            min_cycle,
            max_cycle,
        } => match assertion.kind {
            AssertionKind::Assert | AssertionKind::Assume => implication_status(
                &samples,
                &disabled,
                antecedent,
                consequence,
                *min_cycle,
                *max_cycle,
            ),
            AssertionKind::Cover => implication_cover_status(
                &samples,
                &disabled,
                antecedent,
                consequence,
                *min_cycle,
                *max_cycle,
            ),
        },
        Property::RepeatedImplication {
            antecedent,
            min_repeat,
            max_repeat,
            unbounded,
            overlapped,
            consequence,
        } => repeated_implication_status(
            &samples,
            &disabled,
            antecedent,
            *min_repeat,
            *max_repeat,
            *unbounded,
            *overlapped,
            consequence,
            assertion.kind,
        ),
        Property::NonconsecutiveImplication {
            antecedent,
            min_count,
            max_count,
            overlapped,
            consequence,
        } => nonconsecutive_implication_status(
            &samples,
            &disabled,
            antecedent,
            *min_count,
            *max_count,
            *overlapped,
            consequence,
            assertion.kind,
        ),
        Property::SequenceImplication {
            first,
            second,
            min_delay,
            max_delay,
            overlapped,
            consequence,
        } => sequence_implication_status(
            &samples,
            &disabled,
            first,
            second,
            *min_delay,
            *max_delay,
            *overlapped,
            consequence,
            assertion.kind,
        ),
    };
    Ok(AssertionResult {
        name: assertion.name.clone(),
        kind: assertion.kind,
        status,
        samples: samples.len(),
        clock: assertion.clock.clone(),
    })
}

fn immediate_cover_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    expr: &BoolExpr,
) -> AssertionStatus {
    let mut saw_unknown = samples.is_empty();
    for (index, sample) in samples.iter().enumerate() {
        match disabled[index] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        match eval(expr, sample) {
            Tri::True => return AssertionStatus::CoverHit { sample: index },
            Tri::False => {}
            Tri::Unknown => saw_unknown = true,
        }
    }
    if saw_unknown {
        AssertionStatus::Unknown
    } else {
        AssertionStatus::CoverMiss
    }
}

fn immediate_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    expr: &BoolExpr,
) -> AssertionStatus {
    let mut saw_unknown = false;
    for (index, sample) in samples.iter().enumerate() {
        match disabled[index] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        match eval(expr, sample) {
            Tri::False => return AssertionStatus::Fail { sample: index },
            Tri::True => {}
            Tri::Unknown => saw_unknown = true,
        }
    }
    if saw_unknown || samples.is_empty() {
        AssertionStatus::Unknown
    } else {
        AssertionStatus::Pass
    }
}

fn implication_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    antecedent: &BoolExpr,
    consequence: &BoolExpr,
    min_cycle: usize,
    max_cycle: usize,
) -> AssertionStatus {
    if min_cycle > max_cycle {
        return AssertionStatus::Unknown;
    }
    let mut incomplete = false;
    for (index, sample) in samples.iter().enumerate() {
        if disabled[index] == Tri::True {
            continue;
        }
        if eval(antecedent, sample) != Tri::True {
            continue;
        }
        let mut matched = false;
        let mut unknown = false;
        let mut aborted = false;
        for offset in min_cycle..=max_cycle {
            let Some(target) = samples.get(index + offset) else {
                incomplete = true;
                continue;
            };
            match disabled[index + offset] {
                Tri::True => {
                    aborted = true;
                    break;
                }
                Tri::Unknown => unknown = true,
                Tri::False => {}
            }
            match eval(consequence, target) {
                Tri::True => {
                    matched = true;
                    break;
                }
                Tri::False => {}
                Tri::Unknown => unknown = true,
            }
        }
        if aborted {
            continue;
        }
        if !matched {
            if unknown || index + max_cycle >= samples.len() {
                incomplete = true;
            } else {
                return AssertionStatus::Fail { sample: index };
            }
        }
    }
    if incomplete {
        AssertionStatus::Unknown
    } else {
        AssertionStatus::Pass
    }
}

fn implication_cover_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    antecedent: &BoolExpr,
    consequence: &BoolExpr,
    min_cycle: usize,
    max_cycle: usize,
) -> AssertionStatus {
    if min_cycle > max_cycle {
        return AssertionStatus::Unknown;
    }
    let mut saw_unknown = samples.is_empty();
    let mut incomplete = false;
    for (index, sample) in samples.iter().enumerate() {
        match disabled[index] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        match eval(antecedent, sample) {
            Tri::True => {}
            Tri::False => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
        }
        let mut window_unknown = false;
        for offset in min_cycle..=max_cycle {
            let Some(target) = samples.get(index + offset) else {
                incomplete = true;
                continue;
            };
            if disabled[index + offset] != Tri::False {
                if disabled[index + offset] == Tri::Unknown {
                    window_unknown = true;
                }
                continue;
            }
            if eval(consequence, target) == Tri::True {
                return AssertionStatus::CoverHit { sample: index };
            }
            if eval(consequence, target) == Tri::Unknown {
                window_unknown = true;
            }
        }
        if window_unknown {
            saw_unknown = true;
        }
        // A fully observed miss does not override a later incomplete window:
        // that later obligation might still produce the cover hit.
    }
    if saw_unknown || incomplete {
        AssertionStatus::Unknown
    } else {
        AssertionStatus::CoverMiss
    }
}

#[allow(clippy::too_many_arguments)]
fn repeated_implication_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    antecedent: &BoolExpr,
    min_repeat: usize,
    max_repeat: usize,
    unbounded: bool,
    overlapped: bool,
    consequence: &BoolExpr,
    kind: AssertionKind,
) -> AssertionStatus {
    if !unbounded && (min_repeat == 0 || min_repeat > max_repeat) {
        return AssertionStatus::Unknown;
    }
    let mut saw_unknown = samples.is_empty();
    let mut incomplete = false;
    for (start, _sample) in samples.iter().enumerate() {
        match disabled[start] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        let repeat_max = if unbounded {
            samples.len() - start
        } else {
            max_repeat
        };
        for repeat in min_repeat..=repeat_max {
            let mut prefix_matched = true;
            let mut prefix_unknown = false;
            let mut prefix_disabled = false;
            for offset in 0..repeat {
                let Some(target) = samples.get(start + offset) else {
                    if prefix_matched {
                        incomplete = true;
                    }
                    prefix_matched = false;
                    break;
                };
                match disabled[start + offset] {
                    Tri::True => {
                        prefix_disabled = true;
                        break;
                    }
                    Tri::Unknown => {
                        prefix_unknown = true;
                        break;
                    }
                    Tri::False => {}
                }
                match eval(antecedent, target) {
                    Tri::True => {}
                    Tri::False => {
                        prefix_matched = false;
                        break;
                    }
                    Tri::Unknown => {
                        prefix_unknown = true;
                        break;
                    }
                }
            }
            if prefix_disabled {
                continue;
            }
            if prefix_unknown {
                saw_unknown = true;
                continue;
            }
            if !prefix_matched {
                continue;
            }

            let end = start + usize::from(!overlapped) + repeat.saturating_sub(1);
            let Some(target) = samples.get(end) else {
                incomplete = true;
                continue;
            };
            match disabled[end] {
                Tri::True => continue,
                Tri::Unknown => saw_unknown = true,
                Tri::False => {}
            }
            match (kind, eval(consequence, target)) {
                (_, Tri::Unknown) => saw_unknown = true,
                (AssertionKind::Assert | AssertionKind::Assume, Tri::False) => {
                    return AssertionStatus::Fail { sample: start };
                }
                (AssertionKind::Cover, Tri::True) => {
                    return AssertionStatus::CoverHit { sample: start };
                }
                _ => {}
            }
        }
    }
    if unbounded || saw_unknown || incomplete {
        AssertionStatus::Unknown
    } else if kind == AssertionKind::Cover {
        AssertionStatus::CoverMiss
    } else {
        AssertionStatus::Pass
    }
}

#[allow(clippy::too_many_arguments)]
fn nonconsecutive_implication_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    antecedent: &BoolExpr,
    min_count: usize,
    max_count: usize,
    overlapped: bool,
    consequence: &BoolExpr,
    kind: AssertionKind,
) -> AssertionStatus {
    if min_count == 0 || min_count > max_count {
        return AssertionStatus::Unknown;
    }
    let mut saw_unknown = samples.is_empty();
    let mut incomplete = false;
    for (start, sample) in samples.iter().enumerate() {
        match disabled[start] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        // A nonconsecutive repetition attempt starts at an antecedent hit.
        match eval(antecedent, sample) {
            Tri::True => {}
            Tri::False => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
        }

        let mut count = 1usize;
        if min_count == 1 {
            let end = start + usize::from(!overlapped);
            if !check_nonconsecutive_endpoint(
                samples,
                disabled,
                end,
                consequence,
                kind,
                &mut saw_unknown,
                &mut incomplete,
            ) {
                return match kind {
                    AssertionKind::Cover => AssertionStatus::CoverHit { sample: start },
                    AssertionKind::Assert | AssertionKind::Assume => {
                        AssertionStatus::Fail { sample: start }
                    }
                };
            }
        }
        for frame in start + 1..samples.len() {
            match disabled[frame] {
                Tri::True => break,
                Tri::Unknown => {
                    saw_unknown = true;
                    break;
                }
                Tri::False => {}
            }
            match eval(antecedent, &samples[frame]) {
                Tri::True => {
                    count += 1;
                    if count > max_count {
                        break;
                    }
                    if count >= min_count {
                        let end = frame + usize::from(!overlapped);
                        if !check_nonconsecutive_endpoint(
                            samples,
                            disabled,
                            end,
                            consequence,
                            kind,
                            &mut saw_unknown,
                            &mut incomplete,
                        ) {
                            return match kind {
                                AssertionKind::Cover => AssertionStatus::CoverHit { sample: start },
                                AssertionKind::Assert | AssertionKind::Assume => {
                                    AssertionStatus::Fail { sample: start }
                                }
                            };
                        }
                    }
                }
                Tri::False => {}
                Tri::Unknown => {
                    saw_unknown = true;
                    break;
                }
            }
        }
        if count < min_count {
            incomplete = true;
        }
    }
    if saw_unknown || incomplete {
        AssertionStatus::Unknown
    } else if kind == AssertionKind::Cover {
        AssertionStatus::CoverMiss
    } else {
        AssertionStatus::Pass
    }
}

#[allow(clippy::too_many_arguments)]
fn check_nonconsecutive_endpoint(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    end: usize,
    consequence: &BoolExpr,
    kind: AssertionKind,
    saw_unknown: &mut bool,
    incomplete: &mut bool,
) -> bool {
    let Some(target) = samples.get(end) else {
        *incomplete = true;
        return true;
    };
    match disabled[end] {
        Tri::True => return true,
        Tri::Unknown => *saw_unknown = true,
        Tri::False => {}
    }
    match (kind, eval(consequence, target)) {
        (_, Tri::Unknown) => *saw_unknown = true,
        (AssertionKind::Assert | AssertionKind::Assume, Tri::False) => return false,
        (AssertionKind::Cover, Tri::True) => return false,
        _ => {}
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn sequence_implication_status(
    samples: &[BTreeMap<String, Tri>],
    disabled: &[Tri],
    first: &BoolExpr,
    second: &BoolExpr,
    min_delay: usize,
    max_delay: usize,
    overlapped: bool,
    consequence: &BoolExpr,
    kind: AssertionKind,
) -> AssertionStatus {
    if min_delay > max_delay {
        return AssertionStatus::Unknown;
    }
    let mut saw_unknown = samples.is_empty();
    let mut incomplete = false;
    for (start, sample) in samples.iter().enumerate() {
        match disabled[start] {
            Tri::True => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
            Tri::False => {}
        }
        match eval(first, sample) {
            Tri::True => {}
            Tri::False => continue,
            Tri::Unknown => {
                saw_unknown = true;
                continue;
            }
        }

        for delay in min_delay..=max_delay {
            let Some(second_sample) = samples.get(start + delay) else {
                incomplete = true;
                continue;
            };
            match disabled[start + delay] {
                Tri::True => continue,
                Tri::Unknown => saw_unknown = true,
                Tri::False => {}
            }
            match eval(second, second_sample) {
                Tri::True => {}
                Tri::False => continue,
                Tri::Unknown => {
                    saw_unknown = true;
                    continue;
                }
            }

            let end = start + delay + usize::from(!overlapped);
            let Some(target) = samples.get(end) else {
                incomplete = true;
                continue;
            };
            match disabled[end] {
                Tri::True => continue,
                Tri::Unknown => saw_unknown = true,
                Tri::False => {}
            }
            match (kind, eval(consequence, target)) {
                (_, Tri::Unknown) => saw_unknown = true,
                (AssertionKind::Assert | AssertionKind::Assume, Tri::False) => {
                    return AssertionStatus::Fail { sample: start };
                }
                (AssertionKind::Cover, Tri::True) => {
                    return AssertionStatus::CoverHit { sample: start };
                }
                _ => {}
            }
        }
    }
    if saw_unknown || incomplete {
        AssertionStatus::Unknown
    } else if kind == AssertionKind::Cover {
        AssertionStatus::CoverMiss
    } else {
        AssertionStatus::Pass
    }
}

fn eval(expr: &BoolExpr, sample: &BTreeMap<String, Tri>) -> Tri {
    match expr {
        BoolExpr::True => Tri::True,
        BoolExpr::False => Tri::False,
        BoolExpr::Signal(name) => sample.get(name).copied().unwrap_or(Tri::Unknown),
        BoolExpr::Not(inner) => eval(inner, sample).not(),
        BoolExpr::And(a, b) => eval(a, sample).and(eval(b, sample)),
        BoolExpr::Or(a, b) => eval(a, sample).or(eval(b, sample)),
        BoolExpr::Implication(a, b) => eval(a, sample).not().or(eval(b, sample)),
    }
}

fn sample_times(vcd: &Vcd, assertion: &Assertion) -> Result<Vec<u64>, String> {
    let times = vcd.time_points();
    let Some(clock_name) = assertion.clock.as_deref() else {
        return Ok(times);
    };
    let clock = vcd
        .signal(clock_name)
        .ok_or_else(|| format!("assertion clock '{}' not found", clock_name))?;
    if clock.width != 1 {
        return Err(format!("assertion clock '{}' must be scalar", clock_name));
    }
    let mut out = Vec::new();
    for window in times.windows(2) {
        let (previous_time, current_time) = (window[0], window[1]);
        let previous = clock
            .value_at(previous_time)
            .and_then(|word| word.to_u64())
            .map(|value| value != 0);
        let current = clock
            .value_at(current_time)
            .and_then(|word| word.to_u64())
            .map(|value| value != 0);
        if let (Some(previous), Some(current)) = (previous, current) {
            let edge_matches = match assertion.edge {
                Some(Edge::Pos) => !previous && current,
                Some(Edge::Neg) => previous && !current,
                None => previous != current,
            };
            if edge_matches {
                out.push(current_time);
            }
        }
    }
    Ok(out)
}

fn signal_value(vcd: &Vcd, name: &str, time: u64) -> Result<Tri, String> {
    let value = if let Some(signal) = vcd.signal(name) {
        signal.value_at(time)
    } else if let Some(index) = name.rfind('[') {
        let bit = name[index + 1..]
            .trim_end_matches(']')
            .parse::<usize>()
            .map_err(|_| format!("invalid indexed signal '{}'", name))?;
        vcd.signal(&name[..index])
            .and_then(|signal| signal.value_at(time))
            .map(|word| eda_core::logic::LogicWord {
                bits: vec![word.bit(bit)],
            })
    } else {
        None
    };
    let Some(word) = value else {
        return Err(format!("assertion signal '{}' not found", name));
    };
    if word.width() == 1 {
        Ok(match word.bit(0) {
            LogicValue::Zero => Tri::False,
            LogicValue::One => Tri::True,
            _ => Tri::Unknown,
        })
    } else if !word.is_fully_known() {
        Ok(Tri::Unknown)
    } else {
        Ok(Tri::from(word.to_u64().unwrap_or(0) != 0))
    }
}

impl From<bool> for Tri {
    fn from(value: bool) -> Self {
        if value {
            Tri::True
        } else {
            Tri::False
        }
    }
}

fn signals(property: &Property, disable: Option<&BoolExpr>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(expr) = disable {
        walk_signals(expr, &mut out);
    }
    match property {
        Property::Immediate(expr) | Property::Always(expr) => walk_signals(expr, &mut out),
        Property::Implication {
            antecedent,
            consequence,
            ..
        } => {
            walk_signals(antecedent, &mut out);
            walk_signals(consequence, &mut out);
        }
        Property::RepeatedImplication {
            antecedent,
            consequence,
            ..
        } => {
            walk_signals(antecedent, &mut out);
            walk_signals(consequence, &mut out);
        }
        Property::NonconsecutiveImplication {
            antecedent,
            consequence,
            ..
        } => {
            walk_signals(antecedent, &mut out);
            walk_signals(consequence, &mut out);
        }
        Property::SequenceImplication {
            first,
            second,
            consequence,
            ..
        } => {
            walk_signals(first, &mut out);
            walk_signals(second, &mut out);
            walk_signals(consequence, &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn walk_signals(expr: &BoolExpr, out: &mut Vec<String>) {
    match expr {
        BoolExpr::Signal(signal) => {
            if !out.contains(signal) {
                out.push(signal.clone());
            }
        }
        BoolExpr::True | BoolExpr::False => {}
        BoolExpr::Not(inner) => walk_signals(inner, out),
        BoolExpr::And(a, b) | BoolExpr::Or(a, b) | BoolExpr::Implication(a, b) => {
            walk_signals(a, out);
            walk_signals(b, out);
        }
    }
}

#[derive(Clone, Debug)]
enum Token {
    Word(String, usize),
    Number(usize, usize),
    Punct(&'static str, usize),
}

impl Token {
    fn line(&self) -> usize {
        match self {
            Token::Word(_, line) | Token::Number(_, line) | Token::Punct(_, line) => *line,
        }
    }

    fn display(&self) -> String {
        match self {
            Token::Word(word, _) => word.clone(),
            Token::Number(value, _) => value.to_string(),
            Token::Punct(value, _) => (*value).to_string(),
        }
    }
}

fn lex(source: &str) -> Result<Vec<Token>, String> {
    let mut out = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = match raw.find("//") {
            Some(position) => &raw[..position],
            None => raw,
        };
        let mut chars = line.chars().peekable();
        while let Some(&first) = chars.peek() {
            if first.is_whitespace() {
                chars.next();
                continue;
            }
            let three: String = chars.clone().take(3).collect();
            let two: String = chars.clone().take(2).collect();
            let punct = if three == "|->" {
                Some("|->")
            } else if three == "|=>" {
                Some("|=>")
            } else if two == "&&" {
                Some("&&")
            } else if two == "||" {
                Some("||")
            } else if two == "##" {
                Some("##")
            } else if "()[];!:@,*+=".contains(first) {
                Some(match first {
                    '(' => "(",
                    ')' => ")",
                    '[' => "[",
                    ']' => "]",
                    ';' => ";",
                    '!' => "!",
                    ':' => ":",
                    '@' => "@",
                    ',' => ",",
                    '*' => "*",
                    '+' => "+",
                    '=' => "=",
                    _ => unreachable!(),
                })
            } else {
                None
            };
            if let Some(punct) = punct {
                let length = punct.len();
                for _ in 0..length {
                    chars.next();
                }
                out.push(Token::Punct(punct, line_number));
                continue;
            }
            if first.is_ascii_alphanumeric() || first == '_' || first == '\\' {
                let mut word = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' || next == '\\' {
                        word.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.parse::<usize>().is_ok() {
                    out.push(Token::Number(word.parse().unwrap(), line_number));
                } else {
                    out.push(Token::Word(word, line_number));
                }
                continue;
            }
            return Err(format!(
                "line {}: unexpected character '{}'",
                line_number, first
            ));
        }
    }
    Ok(out)
}

fn parse_assertion(
    tokens: &[Token],
    start: usize,
    kind: AssertionKind,
    name: Option<String>,
) -> Result<(Assertion, usize), String> {
    let line = tokens[start].line();
    expect_word(tokens, start + 1, "property", line)?;
    let mut pos = start + 2;
    let mut clock = None;
    let mut edge = None;
    let mut disable = None;
    let outer_paren = matches!(tokens.get(pos), Some(Token::Punct("(", _)));
    if outer_paren {
        pos += 1;
    }
    if matches!(tokens.get(pos), Some(Token::Punct("@", _))) {
        expect_punct(tokens, pos + 1, "(", line)?;
        let edge_word = match tokens.get(pos + 2) {
            Some(Token::Word(word, _)) if word == "posedge" || word == "negedge" => word.clone(),
            Some(token) => {
                return Err(format!(
                    "line {}: expected posedge or negedge, found {}",
                    line,
                    token.display()
                ));
            }
            None => return Err(format!("line {}: incomplete clocking event", line)),
        };
        let clock_name = match tokens.get(pos + 3) {
            Some(Token::Word(word, _)) => word.clone(),
            Some(token) => {
                return Err(format!(
                    "line {}: expected clock name, found {}",
                    line,
                    token.display()
                ));
            }
            None => return Err(format!("line {}: incomplete clocking event", line)),
        };
        expect_punct(tokens, pos + 4, ")", line)?;
        edge = Some(if edge_word == "posedge" {
            Edge::Pos
        } else {
            Edge::Neg
        });
        clock = Some(clock_name);
        pos += 5;
    }
    if matches!(tokens.get(pos), Some(Token::Word(word, _)) if word == "disable") {
        expect_word(tokens, pos + 1, "iff", line)?;
        expect_punct(tokens, pos + 2, "(", line)?;
        let (expr, end) = parse_or(tokens, pos + 3, line)?;
        expect_punct(tokens, end, ")", line)?;
        disable = Some(expr);
        pos = end + 1;
    }

    let (property, mut end) = parse_property(tokens, pos, line)?;
    if outer_paren {
        expect_punct(tokens, end, ")", line)?;
        end += 1;
    }
    expect_punct(tokens, end, ";", line)?;
    Ok((
        Assertion {
            name: name.unwrap_or_else(|| match kind {
                AssertionKind::Assert => format!("assertion_{}", start),
                AssertionKind::Assume => format!("assumption_{}", start),
                AssertionKind::Cover => format!("cover_{}", start),
            }),
            kind,
            clock,
            edge,
            disable,
            property,
        },
        end + 1,
    ))
}

fn parse_property(
    tokens: &[Token],
    start: usize,
    line: usize,
) -> Result<(Property, usize), String> {
    if let Some(Token::Word(word, _)) = tokens.get(start) {
        if word == "always" {
            let (expr, end) = parse_or(tokens, start + 1, line)?;
            return Ok((Property::Always(expr), end));
        }
        if word == "never" {
            let (expr, end) = parse_or(tokens, start + 1, line)?;
            return Ok((Property::Always(BoolExpr::Not(Box::new(expr))), end));
        }
    }
    let (antecedent, mut pos) = parse_or(tokens, start, line)?;
    if matches!(tokens.get(pos), Some(Token::Punct("##", _))) {
        let (min, max, second_start) = if matches!(tokens.get(pos + 1), Some(Token::Number(_, _))) {
            let delay = parse_number(tokens, pos + 1, line)?;
            (delay, delay, pos + 2)
        } else {
            expect_punct(tokens, pos + 1, "[", line)?;
            let min = parse_number(tokens, pos + 2, line)?;
            expect_punct(tokens, pos + 3, ":", line)?;
            let max = parse_number(tokens, pos + 4, line)?;
            expect_punct(tokens, pos + 5, "]", line)?;
            (min, max, pos + 6)
        };
        if max < min {
            return Err(format!(
                "line {}: sequence delay max {} is less than min {}",
                line, max, min
            ));
        }
        let (second, mut next) = parse_or(tokens, second_start, line)?;
        let overlapped = match tokens.get(next) {
            Some(Token::Punct("|->", _)) => true,
            Some(Token::Punct("|=>", _)) => false,
            _ => {
                return Err(format!(
                    "line {}: sequence antecedent requires |-> or |=>",
                    line
                ));
            }
        };
        next += 1;
        let (consequence, end) = parse_or(tokens, next, line)?;
        return Ok((
            Property::SequenceImplication {
                first: antecedent,
                second,
                min_delay: min,
                max_delay: max,
                overlapped,
                consequence,
            },
            end,
        ));
    }
    if matches!(tokens.get(pos), Some(Token::Punct("[", _)))
        && matches!(tokens.get(pos + 1), Some(Token::Punct("=", _)))
    {
        let min = parse_number(tokens, pos + 2, line)?;
        let (max, next) = if matches!(tokens.get(pos + 3), Some(Token::Punct(":", _))) {
            let max = parse_number(tokens, pos + 4, line)?;
            expect_punct(tokens, pos + 5, "]", line)?;
            (max, pos + 6)
        } else {
            expect_punct(tokens, pos + 3, "]", line)?;
            (min, pos + 4)
        };
        if min == 0 {
            return Err(format!(
                "line {}: zero-count nonconsecutive repetition is not supported",
                line
            ));
        }
        if max < min {
            return Err(format!(
                "line {}: nonconsecutive count max {} is less than min {}",
                line, max, min
            ));
        }
        let overlapped = match tokens.get(next) {
            Some(Token::Punct("|->", _)) => true,
            Some(Token::Punct("|=>", _)) => false,
            Some(token) => {
                return Err(format!(
                    "line {}: nonconsecutive repetition requires |-> or |=>, found {}",
                    line,
                    token.display()
                ));
            }
            None => {
                return Err(format!(
                    "line {}: nonconsecutive repetition requires |-> or |=>",
                    line
                ));
            }
        };
        let (consequence, end) = parse_or(tokens, next + 1, line)?;
        return Ok((
            Property::NonconsecutiveImplication {
                antecedent,
                min_count: min,
                max_count: max,
                overlapped,
                consequence,
            },
            end,
        ));
    }
    if matches!(tokens.get(pos), Some(Token::Punct("[", _)))
        && matches!(
            tokens.get(pos + 1),
            Some(Token::Punct("*", _)) | Some(Token::Punct("+", _))
        )
    {
        let plus = matches!(tokens.get(pos + 1), Some(Token::Punct("+", _)));
        let (min, max, unbounded, next) = if plus {
            expect_punct(tokens, pos + 2, "]", line)?;
            (1, 1, true, pos + 3)
        } else if matches!(tokens.get(pos + 2), Some(Token::Punct("]", _))) {
            (0, 0, true, pos + 3)
        } else {
            let min = parse_number(tokens, pos + 2, line)?;
            let (max, next) = if matches!(tokens.get(pos + 3), Some(Token::Punct(":", _))) {
                let max = parse_number(tokens, pos + 4, line)?;
                expect_punct(tokens, pos + 5, "]", line)?;
                (max, pos + 6)
            } else {
                expect_punct(tokens, pos + 3, "]", line)?;
                (min, pos + 4)
            };
            (min, max, false, next)
        };
        if !unbounded && min == 0 {
            return Err(format!(
                "line {}: zero-length consecutive repetition is not supported",
                line
            ));
        }
        if max < min {
            return Err(format!(
                "line {}: repetition max {} is less than min {}",
                line, max, min
            ));
        }
        let overlapped = match tokens.get(next) {
            Some(Token::Punct("|->", _)) => true,
            Some(Token::Punct("|=>", _)) => false,
            Some(token) => {
                return Err(format!(
                    "line {}: repeated antecedent requires |-> or |=>, found {}",
                    line,
                    token.display()
                ));
            }
            None => {
                return Err(format!(
                    "line {}: repeated antecedent requires |-> or |=>",
                    line
                ));
            }
        };
        let (consequence, end) = parse_or(tokens, next + 1, line)?;
        return Ok((
            Property::RepeatedImplication {
                antecedent,
                min_repeat: min,
                max_repeat: max,
                unbounded,
                overlapped,
                consequence,
            },
            end,
        ));
    }
    let (min_cycle, max_cycle) = match tokens.get(pos) {
        Some(Token::Punct("|->", _)) => (0, 0),
        Some(Token::Punct("|=>", _)) => (1, 1),
        _ => return Ok((Property::Immediate(antecedent), pos)),
    };
    pos += 1;
    if matches!(tokens.get(pos), Some(Token::Punct("##", _))) {
        expect_punct(tokens, pos + 1, "[", line)?;
        let min = parse_number(tokens, pos + 2, line)?;
        expect_punct(tokens, pos + 3, ":", line)?;
        let max = parse_number(tokens, pos + 4, line)?;
        expect_punct(tokens, pos + 5, "]", line)?;
        pos += 6;
        if max < min {
            return Err(format!(
                "line {}: temporal delay max {} is less than min {}",
                line, max, min
            ));
        }
        let (consequence, end) = parse_or(tokens, pos, line)?;
        return Ok((
            Property::Implication {
                antecedent,
                consequence,
                min_cycle: min,
                max_cycle: max,
            },
            end,
        ));
    }
    let (consequence, end) = parse_or(tokens, pos, line)?;
    Ok((
        Property::Implication {
            antecedent,
            consequence,
            min_cycle,
            max_cycle,
        },
        end,
    ))
}

fn parse_or(tokens: &[Token], start: usize, line: usize) -> Result<(BoolExpr, usize), String> {
    let (mut lhs, mut pos) = parse_and(tokens, start, line)?;
    while matches!(tokens.get(pos), Some(Token::Punct("||", _))) {
        let (rhs, next) = parse_and(tokens, pos + 1, line)?;
        lhs = BoolExpr::Or(Box::new(lhs), Box::new(rhs));
        pos = next;
    }
    Ok((lhs, pos))
}

fn parse_and(tokens: &[Token], start: usize, line: usize) -> Result<(BoolExpr, usize), String> {
    let (mut lhs, mut pos) = parse_unary(tokens, start, line)?;
    while matches!(tokens.get(pos), Some(Token::Punct("&&", _))) {
        let (rhs, next) = parse_unary(tokens, pos + 1, line)?;
        lhs = BoolExpr::And(Box::new(lhs), Box::new(rhs));
        pos = next;
    }
    Ok((lhs, pos))
}

fn parse_unary(tokens: &[Token], start: usize, line: usize) -> Result<(BoolExpr, usize), String> {
    if matches!(tokens.get(start), Some(Token::Punct("!", _))) {
        let (inner, end) = parse_unary(tokens, start + 1, line)?;
        return Ok((BoolExpr::Not(Box::new(inner)), end));
    }
    parse_primary(tokens, start, line)
}

fn parse_primary(tokens: &[Token], start: usize, line: usize) -> Result<(BoolExpr, usize), String> {
    match tokens.get(start) {
        Some(Token::Punct("(", _)) => {
            let (expr, pos) = parse_or(tokens, start + 1, line)?;
            expect_punct(tokens, pos, ")", line)?;
            Ok((expr, pos + 1))
        }
        Some(Token::Word(word, _)) if word == "true" => Ok((BoolExpr::True, start + 1)),
        Some(Token::Word(word, _)) if word == "false" => Ok((BoolExpr::False, start + 1)),
        Some(Token::Word(word, _)) => {
            let mut name = word.clone();
            let mut pos = start + 1;
            if matches!(tokens.get(pos), Some(Token::Punct("[", _)))
                && matches!(tokens.get(pos + 1), Some(Token::Number(_, _)))
                && matches!(tokens.get(pos + 2), Some(Token::Punct("]", _)))
            {
                let index = parse_number(tokens, pos + 1, line)?;
                expect_punct(tokens, pos + 2, "]", line)?;
                name = format!("{name}[{index}]");
                pos += 3;
            }
            Ok((BoolExpr::Signal(name), pos))
        }
        Some(token) => Err(format!(
            "line {}: expected Boolean expression, found {}",
            line,
            token.display()
        )),
        None => Err(format!("line {}: incomplete Boolean expression", line)),
    }
}

fn parse_number(tokens: &[Token], pos: usize, line: usize) -> Result<usize, String> {
    match tokens.get(pos) {
        Some(Token::Number(value, _)) => Ok(*value),
        Some(token) => Err(format!(
            "line {}: expected number, found {}",
            line,
            token.display()
        )),
        None => Err(format!("line {}: expected number", line)),
    }
}

fn expect_word(tokens: &[Token], pos: usize, word: &str, line: usize) -> Result<(), String> {
    match tokens.get(pos) {
        Some(Token::Word(actual, _)) if actual == word => Ok(()),
        Some(token) => Err(format!(
            "line {}: expected '{}', found {}",
            line,
            word,
            token.display()
        )),
        None => Err(format!("line {}: expected '{}'", line, word)),
    }
}

fn expect_punct(tokens: &[Token], pos: usize, punct: &str, line: usize) -> Result<(), String> {
    match tokens.get(pos) {
        Some(Token::Punct(actual, _)) if *actual == punct => Ok(()),
        Some(token) => Err(format!(
            "line {}: expected '{}', found {}",
            line,
            punct,
            token.display()
        )),
        None => Err(format!("line {}: expected '{}'", line, punct)),
    }
}

/// Human-readable assertion report.
pub fn report(results: &[AssertionResult]) -> String {
    let mut out = format!("Assertion report: {} checked\n", results.len());
    for result in results {
        let status = match (result.kind, result.status.clone()) {
            (AssertionKind::Assert, AssertionStatus::Pass) => "PASS".into(),
            (AssertionKind::Assume, AssertionStatus::Pass) => "ASSUME PASS".into(),
            (_, AssertionStatus::Pass) => "PASS".into(),
            (_, AssertionStatus::Unknown) => "UNKNOWN".into(),
            (AssertionKind::Assume, AssertionStatus::Fail { sample }) => {
                format!("ASSUME FAIL@{}", sample)
            }
            (_, AssertionStatus::Fail { sample }) => format!("FAIL@{}", sample),
            (_, AssertionStatus::CoverHit { sample }) => format!("COVER@{}", sample),
            (_, AssertionStatus::CoverMiss) => "COVER MISS".into(),
        };
        out.push_str(&format!(
            "  {}: {} ({} samples{})\n",
            result.name,
            status,
            result.samples,
            result
                .clock
                .as_ref()
                .map(|clock| format!(", clock={}", clock))
                .unwrap_or_default()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vcd() -> Vcd {
        eda_waveform::parse_vcd(
            r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 1 # ack $end
$enddefinitions $end
#0
0!
0"
0#
#10
1!
1"
0#
#20
0!
1"
0#
#30
1!
1"
1#
#40
0!
0"
1#
#50
1!
0"
1#
"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_and_checks_bounded_implication() {
        let assertions =
            parse_assertions("assert property (@(posedge clk) req |-> ##[0:2] ack);").unwrap();
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Pass);
    }

    #[test]
    fn detects_false_immediate_property() {
        let assertions =
            parse_assertions("assert property (@(posedge clk) !(req && ack));").unwrap();
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Fail { sample: 1 });
    }

    #[test]
    fn parses_and_checks_cover_properties() {
        let assertions =
            parse_assertions("cover property (@(posedge clk) req |-> ##[0:1] ack);").unwrap();
        assert_eq!(assertions[0].kind, AssertionKind::Cover);
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverHit { sample: 0 });
    }

    #[test]
    fn reports_an_unhit_immediate_cover() {
        let assertions = parse_assertions("cover property (@(posedge clk) req && !req);").unwrap();
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverMiss);
        assert!(report(&results).contains("COVER MISS"));
    }

    #[test]
    fn parses_and_checks_assume_properties() {
        let properties =
            parse_assertions("assume property (@(posedge clk) req |-> ##[0:1] ack);").unwrap();
        assert_eq!(properties[0].kind, AssertionKind::Assume);
        let results = check_vcd(&vcd(), &properties).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Pass);
        assert!(report(&results).contains("ASSUME PASS"));
    }

    #[test]
    fn parses_and_checks_repeated_antecedents() {
        let assertions =
            parse_assertions("assert property (@(posedge clk) req[*2] |-> ack);").unwrap();
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Pass);

        let bad = parse_assertions("assert property (@(posedge clk) ack[*1] |-> !ack);").unwrap();
        let results = check_vcd(&vcd(), &bad).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Fail { sample: 1 });
    }

    #[test]
    fn repeated_antecedent_cover_semantics() {
        let assertions =
            parse_assertions("cover property (@(posedge clk) req[*2] |-> ack);").unwrap();
        let results = check_vcd(&vcd(), &assertions).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverHit { sample: 0 });

        let miss = parse_assertions("cover property (@(posedge clk) req[*1] |=> !ack);").unwrap();
        let results = check_vcd(&vcd(), &miss).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverMiss);
    }

    #[test]
    fn unbounded_repetition_is_bounded_by_observation() {
        let star = parse_assertions("assert property (@(posedge clk) req[*] |-> ack);").unwrap();
        match &star[0].property {
            Property::RepeatedImplication {
                min_repeat,
                unbounded,
                ..
            } if *min_repeat == 0 && *unbounded => {}
            property => panic!("{property:?}"),
        }
        let results = check_vcd(&vcd(), &star).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Fail { sample: 0 });

        let plus = parse_assertions("cover property (@(posedge clk) req[+] |=> ack);").unwrap();
        match &plus[0].property {
            Property::RepeatedImplication {
                min_repeat,
                unbounded,
                ..
            } if *min_repeat == 1 && *unbounded => {}
            property => panic!("{property:?}"),
        }
        let results = check_vcd(&vcd(), &plus).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverHit { sample: 0 });
    }

    #[test]
    fn parses_and_checks_nonconsecutive_repetition() {
        let failure =
            parse_assertions("assert property (@(posedge clk) req[=1] |-> !ack);").unwrap();
        let results = check_vcd(&vcd(), &failure).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Fail { sample: 1 });

        let hit = parse_assertions("cover property (@(posedge clk) req[=2] |-> ack);").unwrap();
        let results = check_vcd(&vcd(), &hit).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverHit { sample: 0 });
    }

    #[test]
    fn parses_and_checks_sequence_antecedents() {
        let failure =
            parse_assertions("assert property (@(posedge clk) req ##[1:1] ack |-> !ack);").unwrap();
        let results = check_vcd(&vcd(), &failure).unwrap();
        assert_eq!(results[0].status, AssertionStatus::Fail { sample: 0 });

        let hit = parse_assertions("cover property (@(posedge clk) req ##1 ack |-> ack);").unwrap();
        let results = check_vcd(&vcd(), &hit).unwrap();
        assert_eq!(results[0].status, AssertionStatus::CoverHit { sample: 0 });
    }

    #[test]
    fn rejects_zero_and_reversed_repetition_ranges() {
        assert!(parse_assertions("assert property (a[*0] |-> b);").is_err());
        assert!(parse_assertions("assert property (a[*3:1] |-> b);").is_err());
        assert!(parse_assertions("assert property (a[=0] |-> b);").is_err());
        assert!(parse_assertions("assert property (a[=3:1] |-> b);").is_err());
    }

    #[test]
    fn rejects_bad_delay_range() {
        assert!(parse_assertions("assert property (a |-> ##[3:1] b);").is_err());
    }
}
