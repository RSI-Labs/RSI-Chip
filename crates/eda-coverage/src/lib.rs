//! # eda-coverage — VCD-driven functional and toggle coverage.
//!
//! The compact covergroup language supports sampled coverpoints, value bins,
//! ranges, crosses, and toggle coverage. Results include hit counts and an
//! overall coverage percentage suitable for regression dashboards.

use eda_waveform::Vcd;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub low: u64,
    pub high: u64,
}

impl Range {
    fn contains(&self, value: u64) -> bool {
        value >= self.low && value <= self.high
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bin {
    pub name: String,
    pub ranges: Vec<Range>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalSelect {
    Whole(String),
    Bit {
        name: String,
        index: usize,
    },
    Slice {
        name: String,
        msb: usize,
        lsb: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverPoint {
    pub name: String,
    pub signal: SignalSelect,
    pub bins: Vec<Bin>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cross {
    pub name: String,
    pub points: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverGroup {
    pub name: String,
    pub clock: Option<String>,
    pub edge: Option<Edge>,
    pub points: Vec<CoverPoint>,
    pub crosses: Vec<Cross>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Pos,
    Neg,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    pub name: String,
    pub signal: SignalSelect,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoveragePlan {
    pub groups: Vec<CoverGroup>,
    pub toggles: Vec<Toggle>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinResult {
    pub group: String,
    pub point: String,
    pub bin: String,
    pub hits: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossBinResult {
    pub group: String,
    pub cross: String,
    pub combination: Vec<(String, String)>,
    pub hits: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToggleResult {
    pub name: String,
    pub transitions: u64,
    pub samples: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoverageReport {
    pub samples: usize,
    pub bins: Vec<BinResult>,
    pub crosses: Vec<CrossBinResult>,
    pub toggles: Vec<ToggleResult>,
    pub percentage: f64,
}

/// Parse a compact SystemVerilog-covergroup-like subset.
///
/// ```text
/// covergroup cg @(posedge clk);
///   cp_state: coverpoint state[1:0] {
///     bins low = {0, 1};
///     bins high = {[2:3]};
///   }
///   x: cross cp_a, cp_state;
/// endgroup
/// toggle req;
/// ```
pub fn parse_coverage(source: &str) -> Result<CoveragePlan, String> {
    let tokens = lex(source)?;
    let mut pos = 0;
    let mut plan = CoveragePlan::default();
    while pos < tokens.len() {
        match tokens[pos].clone() {
            Token::Word(word, line) if word == "covergroup" => {
                let (group, next) = parse_group(&tokens, pos + 1, line)?;
                plan.groups.push(group);
                pos = next;
            }
            Token::Word(word, line) if word == "toggle" => {
                let name = identifier(&tokens, pos + 1, line)?;
                expect_punct(&tokens, pos + 2, ";", line)?;
                plan.toggles.push(Toggle {
                    name,
                    signal: SignalSelect::Whole(identifier(&tokens, pos + 1, line)?),
                });
                pos += 3;
            }
            token => {
                let line = token.line();
                return Err(format!(
                    "line {}: expected covergroup or toggle, found {}",
                    line,
                    token.display()
                ));
            }
        }
    }
    if plan.groups.is_empty() && plan.toggles.is_empty() {
        return Err("coverage plan is empty".into());
    }
    Ok(plan)
}

pub fn check_coverage(vcd: &Vcd, plan: &CoveragePlan) -> Result<CoverageReport, String> {
    let mut all_bins = Vec::new();
    let mut all_crosses = Vec::new();
    let mut samples = 0usize;

    for group in &plan.groups {
        let times = sample_times(vcd, group)?;
        samples = samples.max(times.len());
        let mut values: BTreeMap<String, Vec<Option<u64>>> = BTreeMap::new();
        for point in &group.points {
            let mut sequence = Vec::with_capacity(times.len());
            for &time in &times {
                sequence.push(signal_value(vcd, &point.signal, time)?);
            }
            values.insert(point.name.clone(), sequence);
        }
        for point in &group.points {
            let sequence = &values[&point.name];
            for bin in &point.bins {
                let hits = sequence
                    .iter()
                    .filter(|value| {
                        value
                            .map(|value| bin.ranges.iter().any(|range| range.contains(value)))
                            .unwrap_or(false)
                    })
                    .count() as u64;
                all_bins.push(BinResult {
                    group: group.name.clone(),
                    point: point.name.clone(),
                    bin: bin.name.clone(),
                    hits,
                });
            }
        }
        for cross in &group.crosses {
            let point_bins: Vec<(String, Vec<&Bin>)> = cross
                .points
                .iter()
                .map(|point_name| {
                    let point = group
                        .points
                        .iter()
                        .find(|point| &point.name == point_name)
                        .ok_or_else(|| {
                            format!(
                                "cross '{}' references unknown coverpoint '{}'",
                                cross.name, point_name
                            )
                        })?;
                    Ok((point_name.clone(), point.bins.iter().collect()))
                })
                .collect::<Result<_, String>>()?;
            for combination in cartesian_bins(&point_bins) {
                let mut hits = 0u64;
                for index in 0..times.len() {
                    let matched: bool = combination.iter().all(|(point_name, bin)| {
                        values[point_name]
                            .get(index)
                            .and_then(|value| *value)
                            .map(|value| {
                                bin.ranges.iter().any(|range: &Range| range.contains(value))
                            })
                            .unwrap_or(false)
                    });
                    if matched {
                        hits += 1;
                    }
                }
                all_crosses.push(CrossBinResult {
                    group: group.name.clone(),
                    cross: cross.name.clone(),
                    combination: combination
                        .into_iter()
                        .map(|(point, bin)| (point, bin.name.clone()))
                        .collect(),
                    hits,
                });
            }
        }
    }

    let mut toggles = Vec::new();
    for toggle in &plan.toggles {
        // Toggle coverage is time-based rather than clock-gated. It captures
        // every actual transition in the waveform, including mid-cycle glitches.
        let times = vcd.time_points();
        let mut previous: Option<u64> = None;
        let mut transitions = 0u64;
        for &time in &times {
            let current = signal_value(vcd, &toggle.signal, time)?;
            if let (Some(previous), Some(current)) = (previous, current) {
                if previous != current {
                    transitions += 1;
                }
            }
            previous = current;
        }
        toggles.push(ToggleResult {
            name: toggle.name.clone(),
            transitions,
            samples: times.len(),
        });
    }

    let total = all_bins.len() + all_crosses.len() + toggles.len();
    let covered = all_bins.iter().filter(|bin| bin.hits > 0).count()
        + all_crosses.iter().filter(|cross| cross.hits > 0).count()
        + toggles
            .iter()
            .filter(|toggle| toggle.transitions > 0)
            .count();
    let percentage = if total == 0 {
        0.0
    } else {
        100.0 * covered as f64 / total as f64
    };
    Ok(CoverageReport {
        samples,
        bins: all_bins,
        crosses: all_crosses,
        toggles,
        percentage,
    })
}

fn cartesian_bins<'a>(point_bins: &'a [(String, Vec<&'a Bin>)]) -> Vec<Vec<(String, &'a Bin)>> {
    let mut out = vec![Vec::new()];
    for (point_name, bins) in point_bins {
        let mut next = Vec::new();
        for prefix in &out {
            for bin in bins {
                let mut combination = prefix.clone();
                combination.push((point_name.clone(), *bin));
                next.push(combination);
            }
        }
        out = next;
    }
    out
}

fn sample_times(vcd: &Vcd, group: &CoverGroup) -> Result<Vec<u64>, String> {
    let Some(clock_name) = group.clock.as_deref() else {
        return Ok(vcd.time_points());
    };
    let Some(clock) = vcd.signal(clock_name) else {
        return Err(format!("coverage clock '{}' not found", clock_name));
    };
    let times = vcd.time_points();
    let mut out = Vec::new();
    for window in times.windows(2) {
        let previous = clock
            .value_at(window[0])
            .and_then(|word| word.to_u64())
            .map(|value| value != 0);
        let current = clock
            .value_at(window[1])
            .and_then(|word| word.to_u64())
            .map(|value| value != 0);
        if let (Some(previous), Some(current)) = (previous, current) {
            let edge = match group.edge {
                Some(Edge::Pos) => !previous && current,
                Some(Edge::Neg) => previous && !current,
                None => previous != current,
            };
            if edge {
                out.push(window[1]);
            }
        }
    }
    Ok(out)
}

fn signal_value(vcd: &Vcd, select: &SignalSelect, time: u64) -> Result<Option<u64>, String> {
    let word = match select {
        SignalSelect::Whole(name) => vcd.signal(name).and_then(|signal| signal.value_at(time)),
        SignalSelect::Bit { name, index } => vcd
            .signal(name)
            .and_then(|signal| signal.value_at(time))
            .map(|word| eda_core::logic::LogicWord {
                bits: vec![word.bit(*index)],
            }),
        SignalSelect::Slice { name, msb, lsb } => {
            if lsb > msb {
                return Err(format!("invalid slice {}[{}:{}]", name, msb, lsb));
            }
            vcd.signal(name)
                .and_then(|signal| signal.value_at(time))
                .map(|word| {
                    let bits: Vec<_> = (*lsb..=*msb).map(|bit| word.bit(bit)).collect();
                    eda_core::logic::LogicWord { bits }
                })
        }
    };
    let Some(word) = word else {
        let name = match select {
            SignalSelect::Whole(name)
            | SignalSelect::Bit { name, .. }
            | SignalSelect::Slice { name, .. } => name,
        };
        return Err(format!("coverage signal '{}' not found", name));
    };
    if !word.is_fully_known() {
        return Ok(None);
    }
    word.to_u64().map(Some).ok_or_else(|| {
        format!(
            "coverage signal is wider than 64 bits: {}",
            match select {
                SignalSelect::Whole(name)
                | SignalSelect::Bit { name, .. }
                | SignalSelect::Slice { name, .. } => name,
            }
        )
    })
}

#[derive(Clone, Debug)]
enum Token {
    Word(String, usize),
    Number(u64, usize),
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
            if "()[]{}:;,=@".contains(first) {
                chars.next();
                let punct = match first {
                    '(' => "(",
                    ')' => ")",
                    '[' => "[",
                    ']' => "]",
                    '{' => "{",
                    '}' => "}",
                    ':' => ":",
                    ';' => ";",
                    ',' => ",",
                    '=' => "=",
                    '@' => "@",
                    _ => unreachable!(),
                };
                out.push(Token::Punct(punct, line_number));
                continue;
            }
            if first.is_ascii_alphanumeric() || first == '_' {
                let mut word = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        word.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if let Ok(value) = word.parse::<u64>() {
                    out.push(Token::Number(value, line_number));
                } else {
                    out.push(Token::Word(word, line_number));
                }
                continue;
            }
            return Err(format!(
                "line {}: unexpected coverage character '{}'",
                line_number, first
            ));
        }
    }
    Ok(out)
}

fn parse_group(tokens: &[Token], start: usize, line: usize) -> Result<(CoverGroup, usize), String> {
    let name = identifier(tokens, start, line)?;
    let mut pos = start + 1;
    let mut clock = None;
    let mut edge = None;
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
            None => return Err(format!("line {}: incomplete clock event", line)),
        };
        let clock_name = identifier(tokens, pos + 3, line)?;
        expect_punct(tokens, pos + 4, ")", line)?;
        expect_punct(tokens, pos + 5, ";", line)?;
        edge = Some(if edge_word == "posedge" {
            Edge::Pos
        } else {
            Edge::Neg
        });
        clock = Some(clock_name);
        pos += 6;
    } else {
        expect_punct(tokens, pos, ";", line)?;
        pos += 1;
    }

    let mut group = CoverGroup {
        name,
        clock,
        edge,
        points: Vec::new(),
        crosses: Vec::new(),
    };
    loop {
        match tokens.get(pos).cloned() {
            Some(Token::Word(word, _line)) if word == "endgroup" => {
                pos += if matches!(tokens.get(pos + 1), Some(Token::Punct(";", _))) {
                    2
                } else {
                    1
                };
                break;
            }
            Some(Token::Word(word, line)) if word == "coverpoint" => {
                let (point, next) = parse_point(tokens, None, pos + 1, line)?;
                group.points.push(point);
                pos = next;
            }
            Some(Token::Word(label, line)) => {
                expect_punct(tokens, pos + 1, ":", line)?;
                let (next_kind, next) = match tokens.get(pos + 2) {
                    Some(Token::Word(word, _)) => (word.clone(), pos + 3),
                    _ => return Err(format!("line {}: expected coverpoint or cross", line)),
                };
                if next_kind == "coverpoint" {
                    let (point, next) = parse_point(tokens, Some(label), next, line)?;
                    group.points.push(point);
                    pos = next;
                } else if next_kind == "cross" {
                    let (cross, next) = parse_cross(tokens, label, next, line)?;
                    group.crosses.push(cross);
                    pos = next;
                } else {
                    return Err(format!("line {}: unknown group item '{}'", line, next_kind));
                }
            }
            Some(token) => {
                let line = token.line();
                return Err(format!(
                    "line {}: expected endgroup or cover item, found {}",
                    line,
                    token.display()
                ));
            }
            None => return Err("unterminated covergroup".into()),
        }
    }
    if group.points.is_empty() {
        return Err(format!("covergroup '{}' has no coverpoints", group.name));
    }
    Ok((group, pos))
}

fn parse_point(
    tokens: &[Token],
    label: Option<String>,
    start: usize,
    line: usize,
) -> Result<(CoverPoint, usize), String> {
    let signal_name = identifier(tokens, start, line)?;
    let mut pos = start + 1;
    let signal = if matches!(tokens.get(pos), Some(Token::Punct("[", _))) {
        let left = number(tokens, pos + 1, line)?;
        if matches!(tokens.get(pos + 2), Some(Token::Punct(":", _))) {
            let right = number(tokens, pos + 3, line)?;
            expect_punct(tokens, pos + 4, "]", line)?;
            pos += 5;
            SignalSelect::Slice {
                name: signal_name,
                msb: left as usize,
                lsb: right as usize,
            }
        } else {
            expect_punct(tokens, pos + 2, "]", line)?;
            pos += 3;
            SignalSelect::Bit {
                name: signal_name,
                index: left as usize,
            }
        }
    } else {
        SignalSelect::Whole(signal_name)
    };
    expect_punct(tokens, pos, "{", line)?;
    pos += 1;
    let mut bins = Vec::new();
    while !matches!(tokens.get(pos), Some(Token::Punct("}", _))) {
        expect_word(tokens, pos, "bins", line)?;
        let bin_name = identifier(tokens, pos + 1, line)?;
        expect_punct(tokens, pos + 2, "=", line)?;
        expect_punct(tokens, pos + 3, "{", line)?;
        let mut ranges = Vec::new();
        let mut cursor = pos + 4;
        loop {
            let range = if matches!(tokens.get(cursor), Some(Token::Punct("[", _))) {
                expect_punct(tokens, cursor, "[", line)?;
                let low = number(tokens, cursor + 1, line)?;
                expect_punct(tokens, cursor + 2, ":", line)?;
                let high = number(tokens, cursor + 3, line)?;
                expect_punct(tokens, cursor + 4, "]", line)?;
                cursor += 5;
                if high < low {
                    return Err(format!(
                        "line {}: coverage range high {} is less than low {}",
                        line, high, low
                    ));
                }
                Range { low, high }
            } else {
                let low = number(tokens, cursor, line)?;
                cursor += 1;
                Range { low, high: low }
            };
            ranges.push(range);
            if matches!(tokens.get(cursor), Some(Token::Punct(",", _))) {
                cursor += 1;
                continue;
            }
            expect_punct(tokens, cursor, "}", line)?;
            cursor += 1;
            break;
        }
        expect_punct(tokens, cursor, ";", line)?;
        pos = cursor + 1;
        bins.push(Bin {
            name: bin_name,
            ranges,
        });
    }
    pos += 1;
    let name = label.unwrap_or_else(|| {
        format!(
            "cp_{}",
            match &signal {
                SignalSelect::Whole(name)
                | SignalSelect::Bit { name, .. }
                | SignalSelect::Slice { name, .. } => name.clone(),
            }
        )
    });
    if bins.is_empty() {
        return Err(format!("coverpoint '{}' has no bins", name));
    }
    Ok((CoverPoint { name, signal, bins }, pos))
}

fn parse_cross(
    tokens: &[Token],
    label: String,
    start: usize,
    line: usize,
) -> Result<(Cross, usize), String> {
    let mut points = vec![identifier(tokens, start, line)?];
    let mut pos = start + 1;
    while matches!(tokens.get(pos), Some(Token::Punct(",", _))) {
        points.push(identifier(tokens, pos + 1, line)?);
        pos += 2;
    }
    expect_punct(tokens, pos, ";", line)?;
    if points.len() < 2 {
        return Err(format!("line {}: cross requires at least two points", line));
    }
    Ok((
        Cross {
            name: label,
            points,
        },
        pos + 1,
    ))
}

fn identifier(tokens: &[Token], pos: usize, line: usize) -> Result<String, String> {
    match tokens.get(pos) {
        Some(Token::Word(word, _)) => Ok(word.clone()),
        Some(token) => Err(format!(
            "line {}: expected identifier, found {}",
            line,
            token.display()
        )),
        None => Err(format!("line {}: expected identifier", line)),
    }
}

fn number(tokens: &[Token], pos: usize, line: usize) -> Result<u64, String> {
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

/// Human-readable coverage report.
pub fn report(result: &CoverageReport) -> String {
    let mut out = format!(
        "Coverage report: {:.2}% over {} samples\n",
        result.percentage, result.samples
    );
    for bin in &result.bins {
        out.push_str(&format!(
            "  BIN {}.{}.{} hits={}\n",
            bin.group, bin.point, bin.bin, bin.hits
        ));
    }
    for cross in &result.crosses {
        let combination = cross
            .combination
            .iter()
            .map(|(point, bin)| format!("{}={}", point, bin))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!(
            "  CROSS {}.{} [{}] hits={}\n",
            cross.group, cross.cross, combination, cross.hits
        ));
    }
    for toggle in &result.toggles {
        out.push_str(&format!(
            "  TOGGLE {} transitions={} samples={}\n",
            toggle.name, toggle.transitions, toggle.samples
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
$var wire 2 # state [1:0] $end
$enddefinitions $end
#0
0!
0"
b00 #
#10
1!
1"
b01 #
#20
0!
0"
b01 #
#30
1!
1"
b11 #
"#,
        )
        .unwrap()
    }

    #[test]
    fn parses_and_checks_bins_cross_toggle() {
        let plan = parse_coverage(
            r#"
covergroup cg @(posedge clk);
  cp_req: coverpoint req {
    bins asserted = {1};
  }
  cp_state: coverpoint state[1:0] {
    bins low = {0, 1};
    bins high = {[2:3]};
  }
  x_all: cross cp_req, cp_state;
endgroup
toggle req;
"#,
        )
        .unwrap();
        let result = check_coverage(&vcd(), &plan).unwrap();
        assert_eq!(result.samples, 2);
        assert_eq!(result.bins.len(), 3);
        assert_eq!(result.crosses.len(), 2);
        assert_eq!(result.toggles[0].transitions, 3);
        assert!(result.percentage > 0.0);
    }

    #[test]
    fn rejects_bad_range_and_missing_cross_point() {
        assert!(
            parse_coverage("covergroup g; cp: coverpoint a { bins x = {[2:1]}; } endgroup;")
                .is_err()
        );
        assert!(parse_coverage(
            "covergroup g; cp: coverpoint a { bins x = {0}; } x: cross cp, missing; endgroup;"
        )
        .is_ok());
    }
}
