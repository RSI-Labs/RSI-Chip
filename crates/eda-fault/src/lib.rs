//! # eda-fault — gate-level stuck-at fault simulation and pattern generation.
//!
//! Fault grading evaluates every net stuck-at-0/1 under deterministic input
//! patterns and records which patterns observe each fault at a primary output.
//! The current engine is combinational; sequential designs should first be
//! represented in scan-shift form or checked with a sequential ATPG engine.

use eda_core::logic::LogicValue;
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;

pub type Pattern = HashMap<String, u64>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StuckAt {
    Zero,
    One,
}

impl StuckAt {
    pub fn value(self) -> LogicValue {
        match self {
            StuckAt::Zero => LogicValue::Zero,
            StuckAt::One => LogicValue::One,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub index: usize,
    pub net: NetId,
    pub net_name: String,
    pub stuck_at: StuckAt,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultDetection {
    pub fault: Fault,
    pub detecting_patterns: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FaultCoverageResult {
    pub patterns: Vec<Pattern>,
    pub exhaustive: bool,
    pub total_faults: usize,
    pub detected_faults: usize,
    pub undetected_faults: Vec<Fault>,
    pub detections: Vec<FaultDetection>,
    pub coverage_percent: f64,
}

#[derive(Clone, Debug)]
pub struct PatternOptions {
    pub max_random_patterns: usize,
    pub exhaustive_input_limit: usize,
    pub random_seed: u64,
}

impl Default for PatternOptions {
    fn default() -> Self {
        PatternOptions {
            max_random_patterns: 1024,
            exhaustive_input_limit: 16,
            random_seed: 0x5eed_cafe_f00d,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StilPattern {
    pub inputs: Pattern,
    pub outputs: HashMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StilPatternSet {
    pub name: String,
    pub input_signals: Vec<String>,
    pub output_signals: Vec<String>,
    pub vectors: Vec<StilPattern>,
}

/// Generate deterministic patterns and grade all single net stuck-at faults.
pub fn grade_automatic(
    nl: &Netlist,
    options: &PatternOptions,
) -> Result<FaultCoverageResult, String> {
    let patterns = automatic_patterns(nl, options)?;
    grade_patterns(nl, &patterns)
}

/// Generate exhaustive patterns when practical, otherwise a deterministic
/// zero, all-ones, and pseudo-random sequence.
pub fn automatic_patterns(nl: &Netlist, options: &PatternOptions) -> Result<Vec<Pattern>, String> {
    if options.max_random_patterns < 2 {
        return Err("fault pattern generation requires at least two random patterns".into());
    }
    if options.exhaustive_input_limit > 24 {
        return Err("exhaustive fault-pattern input limit must be at most 24 bits".into());
    }
    validate(nl)?;
    let inputs: Vec<(String, usize)> = nl
        .ports
        .iter()
        .filter(|port| port.dir == Dir::In)
        .map(|port| (port.name.clone(), port.bits.len()))
        .collect();
    let input_bits: usize = inputs.iter().map(|(_, width)| *width).sum();
    if input_bits <= options.exhaustive_input_limit {
        let combinations = 1usize << input_bits;
        let mut out = Vec::with_capacity(combinations);
        for mask in 0..combinations {
            out.push(pattern_from_mask(&inputs, mask));
        }
        return Ok(out);
    }

    let mut out = Vec::with_capacity(options.max_random_patterns.max(2));
    out.push(pattern_from_mask(&inputs, 0));
    let all_ones = if input_bits >= u64::BITS as usize {
        u64::MAX
    } else {
        (1u64 << input_bits) - 1
    };
    out.push(pattern_from_mask(&inputs, all_ones as usize));
    let mut seed = options.random_seed | 1;
    while out.len() < options.max_random_patterns {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        out.push(pattern_from_mask(&inputs, seed as usize));
    }
    Ok(out)
}

/// Export fault patterns and good-machine primary outputs as a compact STIL
/// subset. Signal assignments use binary text.
pub fn write_stil(
    nl: &Netlist,
    patterns: &[Pattern],
    pattern_set_name: &str,
) -> Result<String, String> {
    validate(nl)?;
    if patterns.is_empty() {
        return Err("STIL export requires at least one pattern".into());
    }
    if pattern_set_name.is_empty()
        || !pattern_set_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("STIL pattern set name must be a nonempty ASCII identifier".into());
    }
    let order = nl
        .topo_cells()
        .ok_or_else(|| "cannot export STIL: combinational loop".to_string())?;
    let inputs: Vec<&eda_core::netlist::Port> =
        nl.ports.iter().filter(|port| port.dir == Dir::In).collect();
    let outputs: Vec<&eda_core::netlist::Port> = nl
        .ports
        .iter()
        .filter(|port| port.dir == Dir::Out)
        .collect();

    let mut out = String::new();
    out.push_str("STIL 1.0;\n\n");
    out.push_str(&format!("Ann {{* Pattern set: {} *}}\n", pattern_set_name));
    out.push_str("\nSignalGroups {\n");
    out.push_str(&format!(
        "  inputs = '{}';\n",
        inputs
            .iter()
            .map(|port| port.name.as_str())
            .collect::<Vec<_>>()
            .join("+")
    ));
    out.push_str(&format!(
        "  outputs = '{}';\n",
        outputs
            .iter()
            .map(|port| port.name.as_str())
            .collect::<Vec<_>>()
            .join("+")
    ));
    out.push_str("}\n\n");
    out.push_str(&format!(
        "PatternExec {{ PatternBurst \"{pattern_set_name}\"; }}\n\n"
    ));
    out.push_str(&format!(
        "PatternBurst \"{pattern_set_name}\" {{ Pattern \"vectors\"; }}\n\n"
    ));
    out.push_str("Pattern \"vectors\" {\n");
    for pattern in patterns {
        let good = evaluate_pattern(nl, &order, pattern, None)?;
        out.push_str("  V {");
        let assignments = inputs.len() + outputs.len();
        for (index, port) in inputs.iter().enumerate() {
            let Some(value) = pattern.get(&port.name) else {
                return Err(format!("pattern lacks input '{}'", port.name));
            };
            let word = eda_core::logic::LogicWord::from_u64(port.bits.len(), *value);
            out.push_str(&format!("{}={}", port.name, word.to_bin_str()));
            if index + 1 < assignments {
                out.push_str("; ");
            }
        }
        for (index, port) in outputs.iter().enumerate() {
            let mut bits = String::new();
            for &net in port.bits.iter() {
                let value = good.get(&net).copied().unwrap_or(LogicValue::X);
                if !value.is_01() {
                    return Err(format!("STIL export requires known output '{}'", port.name));
                }
                bits.push(value.to_char());
            }
            out.push_str(&format!("{}={}", port.name, bits));
            if index + 1 < outputs.len() {
                out.push_str("; ");
            }
        }
        out.push_str("; }\n");
    }
    out.push_str("}\n");
    Ok(out)
}

/// Parse the compact one-signal-per-assignment STIL subset emitted by
/// [`write_stil`].
pub fn parse_stil(source: &str) -> Result<StilPatternSet, String> {
    let mut result = StilPatternSet::default();
    let mut in_signal_groups = false;
    let mut in_pattern = false;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = match raw.find("//") {
            Some(position) => &raw[..position],
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("Ann ") {
            if let Some(position) = line.find("Pattern set: ") {
                let rest = &line[position + "Pattern set: ".len()..];
                result.name = rest
                    .trim()
                    .trim_start_matches('*')
                    .trim_end_matches("*}")
                    .trim()
                    .to_string();
            }
            continue;
        }
        if line.starts_with("SignalGroups") {
            in_signal_groups = true;
            continue;
        }
        if in_signal_groups {
            if line.starts_with('}') {
                in_signal_groups = false;
                continue;
            }
            let Some((group, expression)) = parse_signal_group_line(line) else {
                return Err(format!("line {}: malformed signal group", line_number));
            };
            if group == "inputs" {
                result.input_signals = expression;
            } else if group == "outputs" {
                result.output_signals = expression;
            } else {
                return Err(format!(
                    "line {}: unsupported signal group '{}'",
                    line_number, group
                ));
            }
            continue;
        }
        if line.starts_with("Pattern \"") {
            if result.input_signals.is_empty() || result.output_signals.is_empty() {
                return Err(format!(
                    "line {}: STIL pattern before signal groups",
                    line_number
                ));
            }
            in_pattern = true;
            continue;
        }
        if in_pattern && line.starts_with('}') {
            in_pattern = false;
            continue;
        }
        if in_pattern && line.starts_with('V') {
            let mut vector = parse_stil_vector(line, line_number)?;
            let input_names = result.input_signals.clone();
            let output_names = result.output_signals.clone();
            let all = vector.inputs.clone();
            vector.outputs.clear();
            for (name, value) in all {
                if input_names.iter().any(|signal| signal == &name) {
                    vector.inputs.insert(name, value);
                } else if output_names.iter().any(|signal| signal == &name) {
                    vector.inputs.remove(&name);
                    vector.outputs.insert(name, value);
                } else {
                    return Err(format!(
                        "line {}: STIL signal '{}' is not in a group",
                        line_number, name
                    ));
                }
            }
            result.vectors.push(vector);
        }
    }
    if result.input_signals.is_empty() || result.output_signals.is_empty() {
        return Err("STIL input/output signal groups are missing".into());
    }
    if result.vectors.is_empty() {
        return Err("STIL pattern contains no vectors".into());
    }
    Ok(result)
}

fn parse_signal_group_line(line: &str) -> Option<(String, Vec<String>)> {
    let (left, right) = line.split_once('=')?;
    let group = left.trim().to_string();
    let right = right.trim().trim_end_matches(';').trim();
    let right = right.strip_prefix('\'')?.strip_suffix('\'')?;
    let signals = right
        .split('+')
        .map(str::trim)
        .filter(|signal| !signal.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if signals.is_empty() {
        return None;
    }
    Some((group, signals))
}

fn parse_stil_vector(line: &str, line_number: usize) -> Result<StilPattern, String> {
    let start = line
        .find('{')
        .ok_or_else(|| format!("line {}: STIL vector lacks '{{'", line_number))?;
    let end = line
        .rfind('}')
        .ok_or_else(|| format!("line {}: STIL vector lacks '}}'", line_number))?;
    if end <= start {
        return Err(format!("line {}: malformed STIL vector", line_number));
    }
    let mut assignments = HashMap::new();
    for assignment in line[start + 1..end].split(';') {
        let assignment = assignment.trim();
        if assignment.is_empty() {
            continue;
        }
        let Some((name, value)) = assignment.split_once('=') else {
            return Err(format!("line {}: malformed STIL assignment", line_number));
        };
        let name = name.trim();
        let value = value.trim();
        if value.is_empty() || !value.chars().all(|c| c == '0' || c == '1') {
            return Err(format!(
                "line {}: STIL value for '{}' must be binary",
                line_number, name
            ));
        }
        let number = u64::from_str_radix(value, 2)
            .map_err(|_| format!("line {}: STIL value is too wide", line_number))?;
        assignments.insert(name.to_string(), number);
    }
    Ok(StilPattern {
        inputs: assignments,
        outputs: HashMap::new(),
    })
}

fn pattern_from_mask(inputs: &[(String, usize)], mut mask: usize) -> Pattern {
    let mut out = Pattern::new();
    for (name, width) in inputs {
        let value = if *width >= usize::BITS as usize {
            mask as u64
        } else {
            (mask & ((1usize << width) - 1)) as u64
        };
        out.insert(name.clone(), value);
        mask = if *width >= usize::BITS as usize {
            0
        } else {
            mask >> *width
        };
    }
    out
}

/// Grade every single stuck-at net fault against caller-supplied patterns.
pub fn grade_patterns(nl: &Netlist, patterns: &[Pattern]) -> Result<FaultCoverageResult, String> {
    validate(nl)?;
    if patterns.is_empty() {
        return Err("fault simulation requires at least one input pattern".into());
    }
    let order = nl
        .topo_cells()
        .ok_or_else(|| "cannot grade faults: combinational loop".to_string())?;
    let output_nets: Vec<NetId> = nl
        .ports
        .iter()
        .filter(|port| port.dir == Dir::Out)
        .flat_map(|port| port.bits.iter().copied())
        .collect();
    if output_nets.is_empty() {
        return Err("fault simulation requires at least one primary output".into());
    }

    let faults = fault_universe(nl);
    let mut detections = Vec::with_capacity(faults.len());
    let mut undetected = Vec::new();
    let mut good_values_per_pattern = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        good_values_per_pattern.push(evaluate_pattern(nl, &order, pattern, None)?);
    }

    for fault in faults {
        let mut detecting = Vec::new();
        for (pattern_index, pattern) in patterns.iter().enumerate() {
            let faulty = evaluate_pattern(nl, &order, pattern, Some(&fault))?;
            let observed = output_nets.iter().enumerate().any(|(bit_index, &net)| {
                let good = good_values_per_pattern[pattern_index]
                    .get(&net)
                    .copied()
                    .unwrap_or(LogicValue::X);
                let bad = faulty.get(&net).copied().unwrap_or(LogicValue::X);
                // Unknown values cannot certify fault detection.
                good != bad && good.is_01() && bad.is_01() && {
                    let _ = bit_index;
                    true
                }
            });
            if observed {
                detecting.push(pattern_index);
            }
        }
        if detecting.is_empty() {
            undetected.push(fault.clone());
        }
        detections.push(FaultDetection {
            fault,
            detecting_patterns: detecting,
        });
    }

    let total_faults = detections.len();
    let detected_faults = total_faults - undetected.len();
    let coverage_percent = if total_faults == 0 {
        0.0
    } else {
        100.0 * detected_faults as f64 / total_faults as f64
    };
    let exhaustive_input_bits: usize = nl
        .ports
        .iter()
        .filter(|port| port.dir == Dir::In)
        .map(|port| port.bits.len())
        .sum();
    Ok(FaultCoverageResult {
        patterns: patterns.to_vec(),
        exhaustive: exhaustive_input_bits <= 16
            && patterns.len() == (1usize << exhaustive_input_bits),
        total_faults,
        detected_faults,
        undetected_faults: undetected,
        detections,
        coverage_percent,
    })
}

/// Enumerate testable net faults. Constant-driver nets are omitted because
/// their stuck-at values are structurally equivalent to redundant constants.
pub fn fault_universe(nl: &Netlist) -> Vec<Fault> {
    let mut out = Vec::new();
    for net in 0..nl.net_count() {
        let constant = nl
            .net(net)
            .drivers
            .iter()
            .any(|&cell| matches!(nl.cell(cell).kind, CellKind::Const(_)));
        if constant {
            continue;
        }
        for stuck_at in [StuckAt::Zero, StuckAt::One] {
            out.push(Fault {
                index: out.len(),
                net,
                net_name: nl.net(net).name.clone(),
                stuck_at,
            });
        }
    }
    out
}

fn evaluate_pattern(
    nl: &Netlist,
    order: &[usize],
    pattern: &Pattern,
    fault: Option<&Fault>,
) -> Result<HashMap<NetId, LogicValue>, String> {
    let mut vals = HashMap::new();
    for port in &nl.ports {
        if port.dir != Dir::In {
            continue;
        }
        let Some(value) = pattern.get(&port.name) else {
            return Err(format!("pattern lacks input '{}'", port.name));
        };
        let word = eda_core::logic::LogicWord::from_u64(port.bits.len(), *value);
        // Port bits are MSB-first: bit k in the public value maps to
        // bits[width-1-k].
        for k in 0..port.bits.len() {
            vals.insert(port.bits[port.bits.len() - 1 - k], word.bit(k));
        }
    }
    if let Some(fault) = fault {
        if nl.net(fault.net).drivers.is_empty() {
            vals.insert(fault.net, fault.stuck_at.value());
        }
    }
    for &cell_id in order {
        let cell = nl.cell(cell_id);
        if nl.cell_is_sequential(&cell.kind) {
            continue;
        }
        let value = nl.eval_cell(cell_id, &vals);
        vals.insert(cell.output, value);
        if let Some(fault) = fault {
            if fault.net == cell.output {
                vals.insert(fault.net, fault.stuck_at.value());
            }
        }
    }
    Ok(vals)
}

fn validate(nl: &Netlist) -> Result<(), String> {
    if nl.cell_count() == 0 {
        return Err("cannot grade an empty netlist".into());
    }
    if nl
        .cells()
        .iter()
        .any(|cell| nl.cell_is_sequential(&cell.kind))
    {
        return Err(
            "sequential stuck-at grading is unsupported; use scan-shift form or sequential ATPG"
                .into(),
        );
    }
    if !nl.ports.iter().any(|port| port.dir == Dir::In) {
        return Err("fault simulation requires at least one primary input".into());
    }
    Ok(())
}

pub fn report(result: &FaultCoverageResult) -> String {
    let mut out = format!(
        "Fault report: {:.2}% coverage, {}/{} detected, {} pattern(s){}\n",
        result.coverage_percent,
        result.detected_faults,
        result.total_faults,
        result.patterns.len(),
        if result.exhaustive {
            " (exhaustive)"
        } else {
            ""
        }
    );
    for fault in result.undetected_faults.iter().take(30) {
        out.push_str(&format!(
            "  UNDETECTED net={} stuck-{}\n",
            fault.net_name,
            match fault.stuck_at {
                StuckAt::Zero => 0,
                StuckAt::One => 1,
            }
        ));
    }
    if result.undetected_faults.len() > 30 {
        out.push_str(&format!(
            "  ... and {} more undetected faults\n",
            result.undetected_faults.len() - 30
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::netlist::Cell;

    fn and_netlist() -> Netlist {
        let mut nl = Netlist::new("and");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u_and".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl
    }

    #[test]
    fn exhaustive_and_gate_reaches_full_coverage() {
        let design = eda_hdl::compile(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )
        .unwrap();
        let nl = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )
        .unwrap()
        .netlist;
        let result = grade_automatic(&nl, &PatternOptions::default()).unwrap();
        assert!(result.exhaustive);
        assert_eq!(result.patterns.len(), 4);
        assert_eq!(
            result.coverage_percent, 100.0,
            "undetected={:?}",
            result.undetected_faults
        );
    }

    #[test]
    fn stil_round_trip_preserves_patterns_and_outputs() {
        let design = eda_hdl::compile(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )
        .unwrap();
        let nl = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: true,
                map_lib: true,
            },
        )
        .unwrap()
        .netlist;
        let patterns = vec![
            HashMap::from([("a".to_string(), 0), ("b".to_string(), 0)]),
            HashMap::from([("a".to_string(), 1), ("b".to_string(), 1)]),
        ];
        let text = write_stil(&nl, &patterns, "and_patterns").unwrap();
        let parsed = parse_stil(&text).unwrap();
        assert_eq!(parsed.name, "and_patterns");
        assert_eq!(parsed.input_signals, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(parsed.output_signals, vec!["y".to_string()]);
        assert_eq!(parsed.vectors[0].inputs["a"], 0);
        assert_eq!(parsed.vectors[0].outputs["y"], 0);
        assert_eq!(parsed.vectors[1].inputs["b"], 1);
        assert_eq!(parsed.vectors[1].outputs["y"], 1);
    }

    #[test]
    fn exhaustive_pattern_generation_enumerates_input_values_in_order() {
        let patterns = automatic_patterns(&and_netlist(), &PatternOptions::default()).unwrap();
        assert_eq!(
            patterns,
            vec![
                HashMap::from([("a".to_string(), 0), ("b".to_string(), 0)]),
                HashMap::from([("a".to_string(), 1), ("b".to_string(), 0)]),
                HashMap::from([("a".to_string(), 0), ("b".to_string(), 1)]),
                HashMap::from([("a".to_string(), 1), ("b".to_string(), 1)]),
            ]
        );
    }

    #[test]
    fn single_pattern_grades_exact_stuck_at_coverage() {
        let patterns = vec![HashMap::from([("a".to_string(), 1), ("b".to_string(), 1)])];
        let result = grade_patterns(&and_netlist(), &patterns).unwrap();
        assert_eq!(result.total_faults, 6);
        assert_eq!(result.detected_faults, 3);
        assert_eq!(result.undetected_faults.len(), 3);
        assert!((result.coverage_percent - 50.0).abs() < 1e-12);
        assert!(!result.exhaustive);
        assert_eq!(result.patterns.len(), 1);
        assert!(report(&result).contains("50.00% coverage"));
        assert!(result
            .detections
            .iter()
            .filter(|detection| !detection.detecting_patterns.is_empty())
            .all(|detection| detection.detecting_patterns == vec![0]));
    }

    #[test]
    fn deterministic_random_patterns_are_reproducible_beyond_exhaustive_limit() {
        let mut nl = Netlist::new("wide");
        let mut bits = Vec::new();
        for index in 0..17 {
            bits.push(nl.add_net(format!("a{index}")));
        }
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u_not".into(),
            kind: CellKind::Not,
            inputs: vec![bits[0]],
            output: y,
        });
        nl.add_port("a", Dir::In, bits);
        nl.add_port("y", Dir::Out, vec![y]);
        let options = PatternOptions {
            max_random_patterns: 5,
            exhaustive_input_limit: 16,
            random_seed: 0x1234_5678_9abc_def0,
        };
        let first = automatic_patterns(&nl, &options).unwrap();
        let second = automatic_patterns(&nl, &options).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 5);
        assert_eq!(first[0]["a"], 0);
        assert_eq!(first[1]["a"], (1u64 << 17) - 1);
    }

    #[test]
    fn rejects_invalid_pattern_options_and_netlist_shape() {
        let options = PatternOptions {
            max_random_patterns: 1,
            ..PatternOptions::default()
        };
        assert_eq!(
            automatic_patterns(&and_netlist(), &options).unwrap_err(),
            "fault pattern generation requires at least two random patterns"
        );
        let options = PatternOptions {
            exhaustive_input_limit: 25,
            ..PatternOptions::default()
        };
        assert_eq!(
            automatic_patterns(&and_netlist(), &options).unwrap_err(),
            "exhaustive fault-pattern input limit must be at most 24 bits"
        );

        let mut sequential = Netlist::new("sequential");
        let clk = sequential.add_net("clk");
        let d = sequential.add_net("d");
        let q = sequential.add_net("q");
        sequential.add_cell(Cell {
            name: "u_dff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, d],
            output: q,
        });
        sequential.add_port("clk", Dir::In, vec![clk]);
        sequential.add_port("d", Dir::In, vec![d]);
        sequential.add_port("q", Dir::Out, vec![q]);
        assert!(automatic_patterns(&sequential, &PatternOptions::default()).is_err());

        let mut no_inputs = Netlist::new("constant");
        let y = no_inputs.add_net("y");
        no_inputs.add_cell(Cell {
            name: "tie".into(),
            kind: CellKind::Const(LogicValue::One),
            inputs: Vec::new(),
            output: y,
        });
        no_inputs.add_port("y", Dir::Out, vec![y]);
        assert!(automatic_patterns(&no_inputs, &PatternOptions::default()).is_err());

        let mut no_outputs = and_netlist();
        no_outputs.ports.retain(|port| port.dir != Dir::Out);
        let patterns = vec![HashMap::from([("a".to_string(), 0), ("b".to_string(), 0)])];
        assert_eq!(
            grade_patterns(&no_outputs, &patterns).unwrap_err(),
            "fault simulation requires at least one primary output"
        );
    }

    #[test]
    fn rejects_missing_pattern_input_and_invalid_stil() {
        let patterns = vec![HashMap::from([("a".to_string(), 0)])];
        assert_eq!(
            grade_patterns(&and_netlist(), &patterns).unwrap_err(),
            "pattern lacks input 'b'"
        );
        assert_eq!(
            write_stil(&and_netlist(), &[], "valid").unwrap_err(),
            "STIL export requires at least one pattern"
        );
        assert!(write_stil(&and_netlist(), &patterns, "bad-name")
            .unwrap_err()
            .contains("identifier"));

        let source = r#"
STIL 1.0;
Ann {* Pattern set: bad *}
SignalGroups {
  inputs = 'a';
  outputs = 'y';
}
Pattern "vectors" {
  V {a=0;unknown=0;}
}
"#;
        assert!(parse_stil(source).unwrap_err().contains("not in a group"));
        let source = source.replace("unknown=0", "unknown");
        assert!(parse_stil(&source)
            .unwrap_err()
            .contains("malformed STIL assignment"));
    }
}
