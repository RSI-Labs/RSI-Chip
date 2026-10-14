//! # eda-waveform — VCD reader and waveform queries.
//!
//! The reader accepts the common structural subset emitted by commercial
//! simulators and [`eda_sim::VcdWriter`]: definitions, scopes, variables,
//! timestamps, scalar changes, binary vectors, X/Z values, `$dumpvars`, and
//! `$dumpall`.

use eda_core::logic::{LogicValue, LogicWord};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VcdChange {
    pub time: u64,
    pub value: LogicValue,
}

#[derive(Clone, Debug)]
pub struct VcdSignal {
    /// Full hierarchical name, e.g. `top.core.a`.
    pub name: String,
    /// Last component of the hierarchical name, e.g. `a`.
    pub short_name: String,
    pub width: usize,
    pub id: String,
    /// Changes in nondecreasing timestamp order.
    pub changes: Vec<(u64, LogicWord)>,
}

impl VcdSignal {
    pub fn value_at(&self, time: u64) -> Option<LogicWord> {
        let idx = self
            .changes
            .partition_point(|(t, _)| *t <= time)
            .checked_sub(1)?;
        Some(self.changes[idx].1.clone())
    }

    pub fn initial(&self) -> Option<LogicWord> {
        self.changes.first().map(|(_, value)| value.clone())
    }

    pub fn final_value(&self) -> Option<LogicWord> {
        self.changes.last().map(|(_, value)| value.clone())
    }

    pub fn transition_count(&self) -> usize {
        self.changes
            .windows(2)
            .filter(|pair| {
                let (_, a) = &pair[0];
                let (_, b) = &pair[1];
                a.is_fully_known()
                    && b.is_fully_known()
                    && a.to_u64().is_some()
                    && b.to_u64().is_some()
                    && a != b
            })
            .count()
    }
}

#[derive(Clone, Debug)]
pub struct Vcd {
    pub timescale_ns: f64,
    signals: BTreeMap<String, VcdSignal>,
    aliases: BTreeMap<String, String>,
    times: BTreeSet<u64>,
}

impl Vcd {
    pub fn signal(&self, name: &str) -> Option<&VcdSignal> {
        let canonical = self.aliases.get(name).map(String::as_str).unwrap_or(name);
        self.signals.get(canonical)
    }

    pub fn signals(&self) -> impl Iterator<Item = &VcdSignal> {
        self.signals.values()
    }

    pub fn time_points(&self) -> Vec<u64> {
        self.times.iter().copied().collect()
    }

    pub fn duration(&self) -> Option<(u64, u64)> {
        self.times
            .iter()
            .next()
            .copied()
            .zip(self.times.iter().last().copied())
    }

    pub fn value_at(&self, name: &str, time: u64) -> Result<LogicWord, String> {
        self.signal(name)
            .ok_or_else(|| format!("unknown VCD signal '{}'", name))?
            .value_at(time)
            .ok_or_else(|| format!("signal '{}' has no value before time {}", name, time))
    }

    pub fn edges(&self, name: &str, direction: EdgeDirection) -> Result<Vec<WaveformEdge>, String> {
        self.signal(name)
            .ok_or_else(|| format!("unknown VCD signal '{}'", name))?
            .edges(direction)
    }

    pub fn clock_metrics(&self, name: &str) -> Result<ClockMetrics, String> {
        let signal = self
            .signal(name)
            .ok_or_else(|| format!("unknown VCD signal '{}'", name))?;
        ClockMetrics::from_signal(signal).map(|metrics| metrics.scale_ns(self.timescale_ns))
    }

    pub fn pulse_metrics(&self, name: &str) -> Result<PulseMetrics, String> {
        let signal = self
            .signal(name)
            .ok_or_else(|| format!("unknown VCD signal '{}'", name))?;
        PulseMetrics::from_signal(signal).map(|metrics| metrics.scale_ns(self.timescale_ns))
    }

    pub fn compare_signals(&self, left: &str, right: &str) -> Result<SignalComparison, String> {
        let a = self
            .signal(left)
            .ok_or_else(|| format!("unknown VCD signal '{}'", left))?;
        let b = self
            .signal(right)
            .ok_or_else(|| format!("unknown VCD signal '{}'", right))?;
        SignalComparison::from_signals(a, b, &self.times)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeDirection {
    Rising,
    Falling,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaveformEdge {
    pub time: u64,
    pub from: u64,
    pub to: u64,
    pub direction: EdgeDirection,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClockMetrics {
    pub rise_count: usize,
    pub fall_count: usize,
    pub period_count: usize,
    pub min_period_ns: f64,
    pub max_period_ns: f64,
    pub average_period_ns: f64,
    pub min_high_ns: f64,
    pub max_high_ns: f64,
    pub average_high_ns: f64,
    pub min_low_ns: f64,
    pub max_low_ns: f64,
    pub average_low_ns: f64,
    pub frequency_mhz: f64,
    pub duty_cycle_percent: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PulseMetrics {
    pub high_count: usize,
    pub low_count: usize,
    pub min_high_ns: f64,
    pub max_high_ns: f64,
    pub average_high_ns: f64,
    pub min_low_ns: f64,
    pub max_low_ns: f64,
    pub average_low_ns: f64,
    pub duty_cycle_percent: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignalComparison {
    pub sample_count: usize,
    pub matching_samples: usize,
    pub differing_samples: usize,
    pub unknown_samples: usize,
    pub hamming_distance: usize,
}

impl VcdSignal {
    pub fn edges(&self, direction: EdgeDirection) -> Result<Vec<WaveformEdge>, String> {
        if self.width == 0 {
            return Err(format!("signal '{}' has zero width", self.name));
        }
        let mut out = Vec::new();
        for pair in self.changes.windows(2) {
            let (_, previous) = &pair[0];
            let (next_time, value) = &pair[1];
            let (Some(from), Some(to)) = (previous.to_u64(), value.to_u64()) else {
                continue;
            };
            if from == to {
                continue;
            }
            let actual = if self.width == 1 && from == 0 && to == 1 {
                EdgeDirection::Rising
            } else if self.width == 1 && from == 1 && to == 0 {
                EdgeDirection::Falling
            } else {
                EdgeDirection::Any
            };
            if direction == EdgeDirection::Any || direction == actual {
                out.push(WaveformEdge {
                    time: *next_time,
                    from,
                    to,
                    direction: actual,
                });
            }
        }
        Ok(out)
    }
}

fn alternating_scalar_edges(signal: &VcdSignal) -> Result<Vec<WaveformEdge>, String> {
    if signal.width != 1 {
        return Err(format!(
            "clock/pulse signal '{}' must be scalar, not width {}",
            signal.name, signal.width
        ));
    }
    let edges = signal.edges(EdgeDirection::Any)?;
    if edges.is_empty()
        || edges
            .windows(2)
            .any(|pair| pair[0].direction == pair[1].direction)
    {
        return Err(format!(
            "clock/pulse signal '{}' must alternate known 0/1 edges",
            signal.name
        ));
    }
    if edges[0].direction != EdgeDirection::Rising {
        return Err(format!(
            "clock/pulse signal '{}' must begin with a rising edge",
            signal.name
        ));
    }
    Ok(edges)
}

fn finite_range(values: &[u64]) -> (u64, u64, f64) {
    let min = values.iter().min().copied().unwrap_or(0);
    let max = values.iter().max().copied().unwrap_or(0);
    let sum = values.iter().map(|&value| value as f64).sum::<f64>();
    (min, max, sum / values.len().max(1) as f64)
}

impl ClockMetrics {
    fn from_signal(signal: &VcdSignal) -> Result<Self, String> {
        let edges = alternating_scalar_edges(signal)?;
        let rising: Vec<_> = edges
            .iter()
            .filter(|edge| edge.direction == EdgeDirection::Rising)
            .collect();
        let falling: Vec<_> = edges
            .iter()
            .filter(|edge| edge.direction == EdgeDirection::Falling)
            .collect();
        if rising.len() < 2 || falling.is_empty() {
            return Err(format!(
                "clock '{}' needs at least two rising and one falling edge",
                signal.name
            ));
        }
        let periods: Vec<_> = rising
            .windows(2)
            .map(|pair| pair[1].time - pair[0].time)
            .collect();
        let high: Vec<_> = rising
            .iter()
            .zip(&falling)
            .map(|(rise, fall)| fall.time.saturating_sub(rise.time))
            .collect();
        let low: Vec<_> = falling
            .iter()
            .zip(rising.iter().skip(1))
            .map(|(fall, rise)| rise.time.saturating_sub(fall.time))
            .collect();
        let (min_period, max_period, average_period) = finite_range(&periods);
        let (min_high, max_high, average_high) = finite_range(&high);
        let (min_low, max_low, average_low) = finite_range(&low);
        Ok(Self {
            rise_count: rising.len(),
            fall_count: falling.len(),
            period_count: periods.len(),
            min_period_ns: min_period as f64,
            max_period_ns: max_period as f64,
            average_period_ns: average_period,
            min_high_ns: min_high as f64,
            max_high_ns: max_high as f64,
            average_high_ns: average_high,
            min_low_ns: min_low as f64,
            max_low_ns: max_low as f64,
            average_low_ns: average_low,
            frequency_mhz: 1_000.0 / average_period,
            duty_cycle_percent: 100.0 * average_high / average_period,
        })
    }

    fn scale_ns(mut self, timescale_ns: f64) -> Self {
        for value in [
            &mut self.min_period_ns,
            &mut self.max_period_ns,
            &mut self.average_period_ns,
            &mut self.min_high_ns,
            &mut self.max_high_ns,
            &mut self.average_high_ns,
            &mut self.min_low_ns,
            &mut self.max_low_ns,
            &mut self.average_low_ns,
        ] {
            *value *= timescale_ns;
        }
        self.frequency_mhz /= timescale_ns;
        self
    }
}

impl PulseMetrics {
    fn from_signal(signal: &VcdSignal) -> Result<Self, String> {
        let edges = alternating_scalar_edges(signal)?;
        let high: Vec<_> = edges
            .chunks(2)
            .filter(|pair| pair.len() == 2)
            .map(|pair| pair[1].time - pair[0].time)
            .collect();
        let low: Vec<_> = edges
            .windows(2)
            .filter(|pair| {
                pair[0].direction == EdgeDirection::Falling
                    && pair[1].direction == EdgeDirection::Rising
            })
            .map(|pair| pair[1].time - pair[0].time)
            .collect();
        if high.is_empty() || low.is_empty() {
            return Err(format!(
                "pulse signal '{}' needs at least one high and one low pulse",
                signal.name
            ));
        }
        let (min_high, max_high, average_high) = finite_range(&high);
        let (min_low, max_low, average_low) = finite_range(&low);
        Ok(Self {
            high_count: high.len(),
            low_count: low.len(),
            min_high_ns: min_high as f64,
            max_high_ns: max_high as f64,
            average_high_ns: average_high,
            min_low_ns: min_low as f64,
            max_low_ns: max_low as f64,
            average_low_ns: average_low,
            duty_cycle_percent: 100.0 * average_high / (average_high + average_low),
        })
    }

    fn scale_ns(mut self, timescale_ns: f64) -> Self {
        for value in [
            &mut self.min_high_ns,
            &mut self.max_high_ns,
            &mut self.average_high_ns,
            &mut self.min_low_ns,
            &mut self.max_low_ns,
            &mut self.average_low_ns,
        ] {
            *value *= timescale_ns;
        }
        self
    }
}

impl SignalComparison {
    fn from_signals(
        left: &VcdSignal,
        right: &VcdSignal,
        times: &BTreeSet<u64>,
    ) -> Result<Self, String> {
        if left.width != right.width {
            return Err(format!(
                "cannot compare '{}' width {} with '{}' width {}",
                left.name, left.width, right.name, right.width
            ));
        }
        let mut out = Self::default();
        for &time in times {
            let unknown = LogicWord::xes(left.width);
            let a = left.value_at(time).unwrap_or(unknown.clone());
            let b = right.value_at(time).unwrap_or(unknown);
            out.sample_count += 1;
            if !a.is_fully_known() || !b.is_fully_known() {
                out.unknown_samples += 1;
                continue;
            }
            let distance = a.bits.iter().zip(&b.bits).filter(|(x, y)| x != y).count();
            out.hamming_distance += distance;
            if distance == 0 {
                out.matching_samples += 1;
            } else {
                out.differing_samples += 1;
            }
        }
        Ok(out)
    }
}

pub fn parse_vcd(source: &str) -> Result<Vcd, String> {
    let mut declarations: Vec<Decl> = Vec::new();
    let mut scopes: Vec<String> = Vec::new();
    let mut timescale = 1.0f64;
    let mut changes: Vec<(u64, String, LogicWord)> = Vec::new();
    let mut times = BTreeSet::new();
    let mut now = 0u64;
    let mut in_definitions = true;

    let lines = source.lines().enumerate().peekable();
    for (line_no, raw) in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }

        if let Some(rest) = line.strip_prefix("$timescale") {
            let text = rest.trim().trim_end_matches("$end").trim();
            timescale =
                parse_timescale(text).map_err(|e| format!("line {}: {}", line_no + 1, e))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("$scope") {
            let mut parts = rest.trim().trim_end_matches("$end").split_whitespace();
            let _kind = parts.next();
            let name = parts
                .next()
                .ok_or_else(|| format!("line {}: $scope has no name", line_no + 1))?;
            scopes.push(name.to_string());
            continue;
        }
        if line.starts_with("$upscope") {
            scopes.pop();
            continue;
        }
        if let Some(rest) = line.strip_prefix("$var") {
            let text = rest.trim().trim_end_matches("$end");
            let parts: Vec<_> = text.split_whitespace().collect();
            if parts.len() < 4 {
                return Err(format!("line {}: malformed $var", line_no + 1));
            }
            let width: usize = parts[1]
                .parse()
                .map_err(|_| format!("line {}: invalid variable width", line_no + 1))?;
            let id = parts[2].to_string();
            let leaf = parts[3].trim_matches(|c| c == '[' || c == ']');
            let leaf = leaf.split(':').next().unwrap_or(leaf);
            let mut full = scopes.clone();
            full.push(leaf.to_string());
            declarations.push(Decl {
                name: full.join("."),
                short_name: leaf.to_string(),
                width,
                id,
            });
            continue;
        }
        if line.starts_with("$enddefinitions") {
            in_definitions = false;
            continue;
        }
        if in_definitions {
            // Date/version/comment/etc. contain no value changes.
            continue;
        }

        if let Some(rest) = line.strip_prefix('#') {
            now = rest
                .trim()
                .parse()
                .map_err(|_| format!("line {}: invalid timestamp", line_no + 1))?;
            times.insert(now);
            continue;
        }
        if line.starts_with('$') {
            continue;
        }

        let (id, value) = if let Some(bits) = line.strip_prefix('b').or(line.strip_prefix('B')) {
            let mut parts = bits.split_whitespace();
            let bits = parts
                .next()
                .ok_or_else(|| format!("line {}: vector change has no value", line_no + 1))?;
            let id = parts
                .next()
                .ok_or_else(|| format!("line {}: vector change has no id", line_no + 1))?
                .to_string();
            (id, parse_bits(bits)?)
        } else {
            let mut chars = line.chars();
            let value = chars
                .next()
                .ok_or_else(|| format!("line {}: empty value change", line_no + 1))?;
            let id = chars.as_str().to_string();
            let value = LogicValue::from_char(value.to_ascii_lowercase())
                .ok_or_else(|| format!("line {}: invalid scalar value '{}'", line_no + 1, value))?;
            (id, LogicWord { bits: vec![value] })
        };
        changes.push((now, id, value));
    }

    if declarations.is_empty() {
        return Err("VCD has no variable declarations".into());
    }
    let mut signals = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for decl in declarations {
        let mut signal_changes = changes
            .iter()
            .filter(|(_, id, _)| *id == decl.id)
            .map(|(time, _, value)| (*time, value.clone()))
            .collect::<Vec<_>>();
        signal_changes.sort_unstable_by_key(|(time, _)| *time);
        if signal_changes.is_empty() {
            continue;
        }
        // A later declaration for the same ID supersedes an earlier alias.
        aliases.insert(decl.name.clone(), decl.name.clone());
        if !decl.short_name.is_empty() {
            aliases
                .entry(decl.short_name.clone())
                .or_insert_with(|| decl.name.clone());
        }
        // Some flattened netlist views emit duplicate declaration names with
        // distinct IDs. Preserve the first declaration as the canonical query
        // target so stable API names do not move to an arbitrary duplicate.
        signals
            .entry(decl.name.clone())
            .or_insert_with(|| VcdSignal {
                name: decl.name,
                short_name: decl.short_name,
                width: decl.width,
                id: decl.id,
                changes: signal_changes,
            });
    }
    if signals.is_empty() {
        return Err("VCD has no value changes".into());
    }
    Ok(Vcd {
        timescale_ns: timescale,
        signals,
        aliases,
        times,
    })
}

/// Serialize already-extracted signal histories back to a VCD 1.0 subset.
///
/// The writer emits one variable per `VcdSignal`, preserving each signal's
/// width and X/Z values. Signals may start at different timestamps; before a
/// signal's first change it is simply absent/unknown in VCD semantics.
pub fn write_vcd(signals: &[VcdSignal]) -> Result<String, String> {
    write_vcd_with_timescale(signals, 1.0)
}

pub fn write_vcd_with_timescale(
    signals: &[VcdSignal],
    timescale_ns: f64,
) -> Result<String, String> {
    if signals.is_empty() {
        return Err("VCD export requires at least one signal".into());
    }
    if !timescale_ns.is_finite() || timescale_ns <= 0.0 {
        return Err("VCD timescale must be finite and positive".into());
    }

    let mut names = BTreeSet::new();
    let mut times = BTreeSet::new();
    for (index, signal) in signals.iter().enumerate() {
        if signal.width == 0 || signal.changes.is_empty() {
            return Err(format!("signal '{}' has no width or changes", signal.name));
        }
        if !names.insert(signal.name.as_str()) {
            return Err(format!("duplicate VCD signal name '{}'", signal.name));
        }
        let mut previous_time = None;
        for (time, value) in &signal.changes {
            if value.width() != signal.width {
                return Err(format!(
                    "signal '{}' has width {} but a change has width {}",
                    signal.name,
                    signal.width,
                    value.width()
                ));
            }
            if let Some(previous) = previous_time {
                if *time == previous {
                    return Err(format!(
                        "signal '{}' has multiple changes at time {}",
                        signal.name, previous
                    ));
                }
                if *time < previous {
                    return Err(format!(
                        "signal '{}' changes are not sorted by time",
                        signal.name
                    ));
                }
            }
            previous_time = Some(*time);
            times.insert(*time);
        }
        let _ = index;
    }

    let mut out = String::new();
    out.push_str("$date\n  eda-waveform\n$end\n");
    out.push_str("$version\n  eda-waveform 0.1\n$end\n");
    out.push_str(&format!(
        "$timescale {}ns $end\n",
        format_timescale(timescale_ns)
    ));
    out.push_str("$scope module top $end\n");
    let mut scope_depth = 0usize;
    for (index, signal) in signals.iter().enumerate() {
        let components: Vec<_> = signal.name.split('.').collect();
        let components: Vec<_> = if components.first() == Some(&"top") {
            components[1..].to_vec()
        } else {
            components
        };
        let Some(leaf) = components.last() else {
            return Err(format!("invalid empty VCD signal name '{}'", signal.name));
        };
        while scope_depth > 0 {
            out.push_str("$upscope $end\n");
            scope_depth -= 1;
        }
        for scope in &components[..components.len() - 1] {
            out.push_str(&format!("$scope module {scope} $end\n"));
            scope_depth += 1;
        }
        let id = export_id(index);
        if signal.width == 1 {
            out.push_str(&format!("$var wire 1 {} {} $end\n", id, leaf));
        } else {
            out.push_str(&format!(
                "$var wire {} {} {} [{}:0] $end\n",
                signal.width,
                id,
                leaf,
                signal.width - 1
            ));
        }
    }
    while scope_depth > 0 {
        out.push_str("$upscope $end\n");
        scope_depth -= 1;
    }
    out.push_str("$upscope $end\n$enddefinitions $end\n");

    for time in times {
        out.push_str(&format!("#{time}\n"));
        for (index, signal) in signals.iter().enumerate() {
            if let Some((change_time, value)) = signal
                .changes
                .iter()
                .find(|(change_time, _)| change_time == &time)
            {
                let _ = change_time;
                let id = export_id(index);
                if signal.width == 1 {
                    out.push_str(&format!("{}{}\n", value.bit(0).to_char(), id));
                } else {
                    let bits: String = (0..signal.width)
                        .rev()
                        .map(|bit| value.bit(bit).to_char())
                        .collect();
                    out.push_str(&format!("b{} {}\n", bits, id));
                }
            }
        }
    }
    Ok(out)
}

fn format_timescale(value: f64) -> String {
    if (value - value.round()).abs() < 1e-12 {
        format!("{}", value.round() as u64)
    } else {
        format!("{value}")
    }
}

fn export_id(mut index: usize) -> String {
    let mut out = String::new();
    loop {
        out.insert(0, char::from_u32(33 + (index % 94) as u32).unwrap());
        index /= 94;
        if index == 0 {
            break;
        }
        index -= 1;
    }
    out
}

struct Decl {
    name: String,
    short_name: String,
    width: usize,
    id: String,
}

fn parse_timescale(text: &str) -> Result<f64, String> {
    let text = text.trim();
    let scale = if let Some(v) = text.strip_suffix("fs") {
        v.trim()
            .parse::<f64>()
            .map_err(|_| "invalid fs timescale".to_string())?
            * 1e-6
    } else if let Some(v) = text.strip_suffix("ps") {
        v.trim()
            .parse::<f64>()
            .map_err(|_| "invalid ps timescale".to_string())?
            * 1e-3
    } else if let Some(v) = text.strip_suffix("ns") {
        v.trim()
            .parse::<f64>()
            .map_err(|_| "invalid ns timescale".to_string())?
    } else if let Some(v) = text.strip_suffix("us") {
        v.trim()
            .parse::<f64>()
            .map_err(|_| "invalid us timescale".to_string())?
            * 1000.0
    } else {
        return Err(format!("unsupported VCD timescale '{}'", text));
    };
    if !scale.is_finite() || scale <= 0.0 {
        return Err("timescale must be finite and positive".into());
    }
    Ok(scale)
}

fn parse_bits(text: &str) -> Result<LogicWord, String> {
    let mut bits = Vec::with_capacity(text.len());
    for (i, ch) in text.chars().rev().enumerate() {
        let value = LogicValue::from_char(ch.to_ascii_lowercase())
            .ok_or_else(|| format!("invalid bit '{}' at {}", ch, i))?;
        bits.push(value);
    }
    Ok(LogicWord { bits })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VCD: &str = r#"
$date today $end
$timescale 1ps $end
$scope module top $end
$var wire 1 ! a $end
$var wire 4 " q [3:0] $end
$upscope $end
$enddefinitions $end
#0
0!
b0000 "
#10
1!
#20
b1010 "
#30
bxxxx "
#40
b1111 "
"#;

    #[test]
    fn parses_and_queries_values() {
        let vcd = parse_vcd(VCD).unwrap();
        assert_eq!(vcd.timescale_ns, 0.001);
        assert_eq!(vcd.signal("top.a").unwrap().transition_count(), 1);
        assert_eq!(
            vcd.signal("q").unwrap().value_at(20).unwrap().to_u64(),
            Some(10)
        );
        assert_eq!(
            vcd.signal("top.q").unwrap().value_at(30).unwrap().to_u64(),
            None
        );
        assert_eq!(vcd.duration(), Some((0, 40)));
    }

    #[test]
    fn rejects_malformed_values() {
        assert!(parse_vcd(
            "$timescale 1ns $end\n$var wire 1 ! a $end\n$enddefinitions $end\n#0\nq!\n"
        )
        .is_err());
    }

    #[test]
    fn writes_round_trip_signals() {
        let source = parse_vcd(VCD).unwrap();
        let signals: Vec<_> = source.signals().cloned().collect();
        let exported = write_vcd(&signals).unwrap();
        let parsed = parse_vcd(&exported).unwrap();
        assert_eq!(parsed.timescale_ns, 1.0);
        assert_eq!(
            parsed.signal("top.a").unwrap().value_at(20).unwrap(),
            LogicWord::from_u64(1, 1)
        );
        assert_eq!(
            parsed
                .signal("top.q")
                .unwrap()
                .value_at(20)
                .unwrap()
                .to_u64(),
            Some(10)
        );
    }

    #[test]
    fn rejects_invalid_export_signals() {
        let source = parse_vcd(VCD).unwrap();
        let mut signals: Vec<_> = source.signals().cloned().collect();
        signals.push(signals[0].clone());
        assert!(write_vcd(&signals).is_err());
    }

    #[test]
    fn measures_clock_edges_periods_and_duty() {
        let source = r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$enddefinitions $end
#0
0!
#4
1!
#9
0!
#14
1!
#19
0!
#25
1!
"#;
        let vcd = parse_vcd(source).unwrap();
        let rising = vcd.edges("clk", EdgeDirection::Rising).unwrap();
        let falling = vcd.edges("clk", EdgeDirection::Falling).unwrap();
        let metrics = vcd.clock_metrics("clk").unwrap();
        assert_eq!(
            rising.iter().map(|edge| edge.time).collect::<Vec<_>>(),
            [4, 14, 25]
        );
        assert_eq!(
            falling.iter().map(|edge| edge.time).collect::<Vec<_>>(),
            [9, 19]
        );
        assert_eq!(metrics.rise_count, 3);
        assert_eq!(metrics.fall_count, 2);
        assert_eq!(metrics.min_period_ns, 10.0);
        assert_eq!(metrics.max_period_ns, 11.0);
        assert_eq!(metrics.min_high_ns, 5.0);
        assert_eq!(metrics.min_low_ns, 5.0);
        assert_eq!(metrics.max_low_ns, 6.0);
        assert!((metrics.duty_cycle_percent - 47.61904761904762).abs() < 1e-10);
        assert!((metrics.frequency_mhz - 95.23809523809524).abs() < 1e-10);
    }

    #[test]
    fn clock_measure_rejects_nonclock_shapes() {
        let glued = parse_vcd(
            "$timescale 1ns $end\n$var wire 1 ! a $end\n$enddefinitions $end\n#0\n0!\n#1\n1!\n#2\n1!\n#3\n0!\n",
        )
        .unwrap();
        assert!(glued.clock_metrics("a").is_err());
        let vector = parse_vcd(
            "$timescale 1ns $end\n$var wire 2 ! a $end\n$enddefinitions $end\n#0\nb00 !\n#1\nb01 !\n#2\nb00 !\n",
        )
        .unwrap();
        assert!(vector.clock_metrics("a").is_err());
        assert_eq!(
            vector
                .edges("a", EdgeDirection::Any)
                .unwrap()
                .iter()
                .map(|edge| edge.to)
                .collect::<Vec<_>>(),
            [1, 0]
        );
    }

    #[test]
    fn pulse_metrics_are_scaled_to_vcd_time_units() {
        let source = r#"
$timescale 10ps $end
$var wire 1 ! pulse $end
$enddefinitions $end
#0
0!
#2
1!
#5
0!
#9
1!
#13
0!
"#;
        let vcd = parse_vcd(source).unwrap();
        let metrics = vcd.pulse_metrics("pulse").unwrap();
        assert_eq!(metrics.high_count, 2);
        assert_eq!(metrics.low_count, 1);
        assert_eq!(metrics.min_high_ns, 0.03);
        assert_eq!(metrics.max_high_ns, 0.04);
        assert_eq!(metrics.min_low_ns, 0.04);
        assert_eq!(metrics.max_low_ns, 0.04);
        assert!((metrics.duty_cycle_percent - 46.666666666666664).abs() < 1e-12);
    }

    #[test]
    fn compares_signals_and_counts_unknowns() {
        let source = r#"
$timescale 1ns $end
$var wire 2 ! a $end
$var wire 2 " b $end
$enddefinitions $end
#0
b00 !
b00 "
#1
b01 !
b11 "
#2
b1x !
b01 "
"#;
        let vcd = parse_vcd(source).unwrap();
        let comparison = vcd.compare_signals("a", "b").unwrap();
        assert_eq!(comparison.sample_count, 3);
        assert_eq!(comparison.matching_samples, 1);
        assert_eq!(comparison.differing_samples, 1);
        assert_eq!(comparison.unknown_samples, 1);
        assert_eq!(comparison.hamming_distance, 1);
    }
}
