//! Minimal Liberty `.lib` reader.
//!
//! This is intentionally dependency-free and supports the subset needed by
//! the tool's synthesis/STA/PnR models: library units, cell area/leakage,
//! input/output pins, Boolean functions, timing arcs, and single-clock
//! sequential cells.

use crate::library::{
    CellLibrary, LibCellDef, LibConstraintTables, LibFunc, LibTimingTables, LibertyLut, SeqTiming,
};
use crate::logic::LogicValue;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct LibCellMetadata {
    pub area: f64,
    pub leakage_power: f64,
    pub input_capacitance_pf: HashMap<String, f64>,
    /// Per-output maximum slew in nanoseconds.
    pub max_transition_ns: HashMap<String, f64>,
    /// Per-output maximum lumped load in picofarads.
    pub max_capacitance_pf: HashMap<String, f64>,
    /// State-dependent leakage rows. A matching row overrides the base value.
    pub leakage_states: Vec<LibLeakageState>,
    /// Scalar path-dependent internal switching energy, in pJ.
    pub internal_power: Vec<LibInternalPower>,
}

/// One Liberty `leakage_power () { when; value; }` state row.
#[derive(Clone, Debug)]
pub struct LibLeakageState {
    pub when: LibFunc,
    pub power_w: f64,
}

/// One scalar pin-to-pin internal-power arc.
#[derive(Clone, Debug)]
pub struct LibInternalPower {
    pub related_pin: String,
    pub when: Option<LibFunc>,
    pub rise_power_pj: f64,
    pub fall_power_pj: f64,
}

#[derive(Clone, Debug)]
enum Tok {
    Word(String, usize),
    Punct(char),
}

#[derive(Clone, Debug, Default)]
struct Node {
    kind: String,
    args: Vec<String>,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn children<'a>(&'a self, kind: &str) -> Vec<&'a Node> {
        self.children
            .iter()
            .filter(|child| child.kind == kind)
            .collect()
    }

    fn first_child(&self, kind: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.kind == kind)
    }
}

/// Parse a Liberty library and construct the tool's cell-library model.
pub fn parse_liberty(source: &str) -> Result<CellLibrary, String> {
    let tokens = lex(source)?;
    let mut pos = 0;
    let mut nodes = Vec::new();
    while pos < tokens.len() {
        let (node, next) = parse_node(&tokens, pos)?;
        if let Some(node) = node {
            nodes.push(node);
        }
        pos = next;
    }
    let root = nodes
        .iter()
        .find(|node| node.kind == "library")
        .ok_or_else(|| "Liberty file has no library group".to_string())?;
    let name = root
        .args
        .first()
        .cloned()
        .ok_or_else(|| "library group has no name".to_string())?;
    let time_scale = root
        .attr("time_unit")
        .map(parse_time_scale)
        .transpose()?
        .unwrap_or(1.0);
    let legacy_capacitance_scale = root
        .attr("capacitance_unit")
        .map(parse_capacitance_scale)
        .transpose()?;
    let capacitive_load_units = root.children("capacitive_load_unit");
    if capacitive_load_units.len() > 1 {
        return Err("duplicate Liberty capacitive_load_unit".into());
    }
    let standard_capacitance_scale = capacitive_load_units
        .first()
        .map(|node| parse_capacitive_load_unit(node))
        .transpose()?;
    let capacitance_scale = match (legacy_capacitance_scale, standard_capacitance_scale) {
        (Some(legacy), Some(standard)) if (legacy - standard).abs() > f64::EPSILON => {
            return Err("conflicting Liberty capacitance_unit and capacitive_load_unit".into());
        }
        (Some(value), _) | (_, Some(value)) => value,
        (None, None) => 1.0,
    };
    let leakage_power_scale = root
        .attr("leakage_power_unit")
        .map(parse_leakage_power_scale)
        .transpose()?
        .unwrap_or(1.0);

    let mut lib = CellLibrary::new(name);
    for cell in root.children("cell") {
        let (def, metadata) = parse_cell(cell, time_scale, capacitance_scale, leakage_power_scale)?;
        lib.metadata.insert(def.name.clone(), metadata);
        lib.add(def);
    }
    if lib.cells.is_empty() {
        return Err("Liberty library contains no cells".into());
    }
    Ok(lib)
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    let mut line = 1usize;
    while let Some(&c) = chars.peek() {
        match c {
            '\n' => {
                chars.next();
                line += 1;
            }
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' if chars.clone().nth(1) == Some('/') => {
                while let Some(c) = chars.next_if(|&c| c != '\n') {
                    let _ = c;
                }
            }
            '/' if chars.clone().nth(1) == Some('*') => {
                chars.next();
                chars.next();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\n' {
                        line += 1;
                    }
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(format!("line {}: unterminated block comment", line));
                }
            }
            '"' => {
                chars.next();
                let mut word = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '\n' {
                        line += 1;
                    }
                    if c == '"' {
                        closed = true;
                        break;
                    }
                    word.push(c);
                }
                if !closed {
                    return Err(format!("line {}: unterminated string", line));
                }
                out.push(Tok::Word(word, line));
            }
            '{' | '}' | '(' | ')' | ':' | ';' | ',' | '[' | ']' | '=' => {
                chars.next();
                out.push(Tok::Punct(c));
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace()
                        || matches!(c, '{' | '}' | '(' | ')' | ':' | ';' | ',' | '[' | ']' | '=')
                    {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                if word.is_empty() {
                    return Err(format!("line {}: unexpected character '{}'", line, c));
                }
                out.push(Tok::Word(word, line));
            }
        }
    }
    Ok(out)
}

fn parse_node(tokens: &[Tok], start: usize) -> Result<(Option<Node>, usize), String> {
    let Some(Tok::Word(kind, line)) = tokens.get(start) else {
        return Ok((None, start));
    };
    let (kind, line) = (kind.clone(), *line);
    let mut pos = start + 1;
    let mut node = Node {
        kind,
        ..Default::default()
    };

    if matches!(tokens.get(pos), Some(Tok::Punct('('))) {
        let (args, next) = parse_parens(tokens, pos)?;
        node.args = args;
        pos = next;
    } else {
        let _ = line;
    }

    match tokens.get(pos) {
        Some(Tok::Punct('{')) => {
            pos += 1;
            loop {
                if matches!(tokens.get(pos), Some(Tok::Punct('}'))) {
                    pos += 1;
                    break;
                }
                if pos >= tokens.len() {
                    return Err(format!("line {}: unterminated {{}} group", line));
                }
                let (child, next) = parse_node(tokens, pos)?;
                pos = next;
                if let Some(child) = child {
                    if child.kind.is_empty() {
                        let Some((key, value)) = child.attrs.into_iter().next() else {
                            return Err("internal error: empty attribute".into());
                        };
                        node.attrs.push((key, value));
                    } else {
                        node.children.push(child);
                    }
                }
            }
            Ok((Some(node), pos))
        }
        Some(Tok::Punct(';')) => {
            pos += 1;
            Ok((Some(node), pos))
        }
        Some(Tok::Punct(':')) => {
            pos += 1;
            let mut value = String::new();
            while let Some(token) = tokens.get(pos) {
                if matches!(token, Tok::Punct(';')) {
                    pos += 1;
                    break;
                }
                if !value.is_empty() {
                    value.push(' ');
                }
                match token {
                    Tok::Word(word, _) => value.push_str(word),
                    Tok::Punct(ch) => value.push(*ch),
                }
                pos += 1;
            }
            node.attrs.push((node.kind.clone(), value));
            node.kind = String::new();
            Ok((Some(node), pos))
        }
        _ => Err(format!(
            "line {}: expected '{{', ':' or ';' after '{}'",
            line, node.kind
        )),
    }
}

fn parse_parens(tokens: &[Tok], open: usize) -> Result<(Vec<String>, usize), String> {
    if !matches!(tokens.get(open), Some(Tok::Punct('('))) {
        return Err("expected '('".into());
    }
    let mut depth = 1usize;
    let mut pos = open + 1;
    let mut args = Vec::new();
    while pos < tokens.len() {
        match &tokens[pos] {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return Ok((args, pos + 1));
                }
            }
            Tok::Punct(',') if depth == 1 => {
                pos += 1;
                continue;
            }
            Tok::Word(word, _) => args.push(word.clone()),
            Tok::Punct(ch) => {
                if let Some(last) = args.last_mut() {
                    last.push(*ch);
                }
            }
        }
        pos += 1;
    }
    Err("unterminated '('".into())
}

fn parse_time_scale(text: &str) -> Result<f64, String> {
    let cleaned = text.trim().trim_matches('"');
    let suffix = if cleaned.ends_with("ps") {
        0.001
    } else if cleaned.ends_with("ns") {
        1.0
    } else if cleaned.ends_with("us") {
        1000.0
    } else {
        return Err(format!("unsupported Liberty time unit '{}'", cleaned));
    };
    let number = cleaned
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .parse::<f64>()
        .map_err(|_| format!("invalid Liberty time unit '{}'", cleaned))?;
    Ok(number * suffix)
}

fn parse_capacitance_scale(text: &str) -> Result<f64, String> {
    let cleaned = text.trim().trim_matches('"').to_ascii_lowercase();
    let suffix = if cleaned.ends_with("ff") {
        0.001
    } else if cleaned.ends_with("pf") {
        1.0
    } else if cleaned.ends_with("nf") {
        1000.0
    } else {
        return Err(format!(
            "unsupported Liberty capacitance unit '{}'",
            text.trim().trim_matches('"')
        ));
    };
    let number = cleaned
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .parse::<f64>()
        .map_err(|_| format!("invalid Liberty capacitance unit '{}'", cleaned))?;
    let scale = number * suffix;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(format!("invalid Liberty capacitance unit '{}'", cleaned));
    }
    Ok(scale)
}

fn parse_capacitive_load_unit(node: &Node) -> Result<f64, String> {
    if node.args.len() != 2 {
        return Err("capacitive_load_unit requires <value> and <unit>".into());
    }
    parse_capacitance_scale(&format!("{}{}", node.args[0], node.args[1])).map_err(|error| {
        format!(
            "invalid Liberty capacitive_load_unit '{}': {error}",
            node.args.join(", ")
        )
    })
}

fn parse_leakage_power_scale(text: &str) -> Result<f64, String> {
    let cleaned = text.trim().trim_matches('"').to_ascii_lowercase();
    let number_text = cleaned.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let unit = &cleaned[number_text.len()..];
    let suffix = match unit {
        "kw" => 1000.0,
        "mw" => 0.001,
        "uw" => 1.0e-6,
        "nw" => 1.0e-9,
        "pw" => 1.0e-12,
        "w" => 1.0,
        _ => {
            return Err(format!(
                "unsupported Liberty leakage power unit '{}'",
                text.trim().trim_matches('"')
            ));
        }
    };
    let number = number_text
        .parse::<f64>()
        .map_err(|_| format!("invalid Liberty leakage power unit '{}'", cleaned))?;
    let scale = number * suffix;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(format!("invalid Liberty leakage power unit '{}'", cleaned));
    }
    Ok(scale)
}

fn number(text: &str) -> Result<f64, String> {
    text.trim()
        .trim_matches('"')
        .split(',')
        .next()
        .unwrap_or("")
        .parse::<f64>()
        .map_err(|_| format!("invalid number '{}'", text))
}

fn group_number(node: &Node) -> Result<f64, String> {
    if let Some(child) = node.first_child("values") {
        let text = child.args.join(" ");
        return number(&text);
    }
    if node.args.first().map(String::as_str) != Some("values") {
        return match node.attr("values") {
            Some(text) => number(text),
            None => Err("missing power value".into()),
        };
    }
    let value = if node.args.first().map(String::as_str) == Some("values") {
        node.args.get(1)
    } else {
        node.args.first()
    };
    match value {
        Some(text) => number(text),
        None => Err("missing power value".into()),
    }
}

fn numbers(text: &str) -> Result<Vec<f64>, String> {
    let normalized = text.trim().trim_matches('"').replace(',', " ");
    let mut out = Vec::new();
    for token in normalized.split_whitespace() {
        let value = token
            .parse::<f64>()
            .map_err(|_| format!("invalid number list entry '{}'", token))?;
        if !value.is_finite() {
            return Err(format!("non-finite number list entry '{}'", token));
        }
        out.push(value);
    }
    Ok(out)
}

fn parse_lut(
    node: &Node,
    time_scale: f64,
    capacitance_scale: f64,
    capacitance_index_2: bool,
) -> Result<LibertyLut, String> {
    let raw_values = if node.args.first().map(String::as_str) == Some("values") {
        node.args.get(1).cloned()
    } else {
        node.attr("values").map(str::to_string)
    }
    .or_else(|| node.first_child("values").map(|child| child.args.join(" ")))
    .ok_or("Liberty table has no values")?;
    let values = numbers(&raw_values)?
        .into_iter()
        .map(|value| value * time_scale)
        .collect::<Vec<_>>();
    let index_1_text = node.attr("index_1").map(str::to_string).or_else(|| {
        node.first_child("index_1")
            .map(|child| child.args.join(" "))
    });
    let index_1 = match index_1_text {
        Some(text) => numbers(&text)?,
        None if values.len() == 1 => vec![0.0],
        None => return Err("Liberty table has no index_1".into()),
    }
    .into_iter()
    .map(|value| value * time_scale)
    .collect::<Vec<_>>();
    let index_2 = match node.attr("index_2").map(str::to_string).or_else(|| {
        node.first_child("index_2")
            .map(|child| child.args.join(" "))
    }) {
        Some(text) => Some(
            numbers(&text)?
                .into_iter()
                .map(|value| {
                    if capacitance_index_2 {
                        value * capacitance_scale
                    } else {
                        value * time_scale
                    }
                })
                .collect::<Vec<_>>(),
        ),
        None => None,
    };
    if index_1
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || index_2.as_ref().is_some_and(|values| {
            values
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        })
    {
        return Err("Liberty LUT indices must be finite and nonnegative".into());
    }
    let expected = match &index_2 {
        None => index_1.len(),
        Some(index_2) => index_1.len() * index_2.len(),
    };
    if index_1.is_empty() || values.len() != expected {
        return Err(format!(
            "Liberty table dimensions mismatch: index1={}, index2={:?}, values={}",
            index_1.len(),
            index_2.as_ref().map(Vec::len),
            values.len()
        ));
    }
    Ok(LibertyLut {
        index_1,
        index_2,
        values,
    })
}

struct PinInfo {
    name: String,
    input: bool,
    output: bool,
    function: Option<String>,
    capacitance: f64,
    max_transition: Option<f64>,
    max_capacitance: Option<f64>,
    timings: Vec<TimingInfo>,
}

struct TimingInfo {
    related_pin: String,
    timing_type: String,
    delay: f64,
    rise_table: Option<LibertyLut>,
    fall_table: Option<LibertyLut>,
    rise_transition_table: Option<LibertyLut>,
    fall_transition_table: Option<LibertyLut>,
    rise_constraint_table: Option<LibertyLut>,
    fall_constraint_table: Option<LibertyLut>,
}

fn parse_cell(
    cell: &Node,
    time_scale: f64,
    capacitance_scale: f64,
    leakage_power_scale: f64,
) -> Result<(LibCellDef, LibCellMetadata), String> {
    let name = cell
        .args
        .first()
        .cloned()
        .ok_or_else(|| "cell group has no name".to_string())?;
    let area = cell
        .attr("area")
        .map(number)
        .transpose()?
        .filter(|v| v.is_finite() && *v >= 0.0)
        .ok_or_else(|| format!("cell '{}' has no valid area", name))?;
    let leakage = cell
        .attr("cell_leakage_power")
        .map(number)
        .transpose()?
        .unwrap_or(0.0)
        * leakage_power_scale;
    if !leakage.is_finite() {
        return Err(format!("cell '{}' has invalid leakage power", name));
    }

    let mut pins = Vec::new();
    for pin in cell.children("pin") {
        let pin_name = pin
            .args
            .first()
            .cloned()
            .ok_or_else(|| format!("cell '{}' has anonymous pin", name))?;
        let direction = pin
            .attr("direction")
            .unwrap_or("input")
            .to_ascii_lowercase();
        let capacitance = pin
            .attr("capacitance")
            .map(number)
            .transpose()?
            .unwrap_or(0.0)
            * capacitance_scale;
        if !capacitance.is_finite() || capacitance < 0.0 {
            return Err(format!(
                "cell '{}' pin '{}' has invalid capacitance",
                name, pin_name
            ));
        }
        let max_transition = pin
            .attr("max_transition")
            .map(number)
            .transpose()?
            .filter(|value| value.is_finite() && *value >= 0.0);
        let max_capacitance = pin
            .attr("max_capacitance")
            .map(number)
            .transpose()?
            .filter(|value| value.is_finite() && *value >= 0.0);
        let mut timings = Vec::new();
        for timing in pin.children("timing") {
            let related = timing
                .attr("related_pin")
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "cell '{}' pin '{}' timing has no related_pin",
                        name, pin_name
                    )
                })?;
            let timing_type = timing
                .attr("timing_type")
                .unwrap_or("combinational")
                .to_string();
            let rise_table = timing
                .first_child("cell_rise")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, true))
                .transpose()?;
            let fall_table = timing
                .first_child("cell_fall")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, true))
                .transpose()?;
            let rise_transition_table = timing
                .first_child("rise_transition")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, true))
                .transpose()?;
            let fall_transition_table = timing
                .first_child("fall_transition")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, true))
                .transpose()?;
            let rise_constraint_table = timing
                .first_child("rise_constraint")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, false))
                .transpose()?;
            let fall_constraint_table = timing
                .first_child("fall_constraint")
                .filter(|table| {
                    table.attr("index_1").is_some() || table.first_child("index_1").is_some()
                })
                .map(|table| parse_lut(table, time_scale, capacitance_scale, false))
                .transpose()?;
            let mut delay = None;
            for arc in [
                "cell_rise",
                "cell_fall",
                "rise_constraint",
                "fall_constraint",
            ] {
                if let Some(values) = timing.first_child(arc) {
                    // Liberty commonly writes `cell_rise(values("1.2,1.3"))`.
                    // The generic parenthesis reader exposes both nested
                    // tokens; select the quoted vector rather than its name.
                    let value = if values.args.first().map(String::as_str) == Some("values") {
                        values.args.get(1)
                    } else {
                        None
                    };
                    if let Some(first) = value {
                        delay = Some(number(first)? * time_scale);
                        break;
                    }
                    if !values.args.is_empty() {
                        delay = Some(group_number(values)? * time_scale);
                        break;
                    }
                }
            }
            if delay.is_none() {
                if let Some(table) = rise_table.as_ref().or(fall_table.as_ref()) {
                    delay = table.values.first().copied();
                }
            }
            if delay.is_none() {
                if let Some(table) = rise_constraint_table
                    .as_ref()
                    .or(fall_constraint_table.as_ref())
                {
                    delay = table.values.first().copied();
                }
            }
            timings.push(TimingInfo {
                related_pin: related,
                timing_type,
                delay: delay.unwrap_or(0.0),
                rise_table,
                fall_table,
                rise_transition_table,
                fall_transition_table,
                rise_constraint_table,
                fall_constraint_table,
            });
        }
        pins.push(PinInfo {
            name: pin_name,
            input: direction == "input",
            output: direction == "output",
            function: pin.attr("function").map(str::to_string),
            capacitance,
            max_transition,
            max_capacitance,
            timings,
        });
    }

    let input_names: Vec<String> = pins
        .iter()
        .filter(|pin| pin.input)
        .map(|pin| pin.name.clone())
        .collect();
    if let Some(ff) = cell.first_child("ff") {
        let clock_name = ff.attr("clocked_on").unwrap_or("clk");
        let data_name = ff.attr("next_state").unwrap_or("d");
        let mut ordered_inputs = vec![clock_name.to_string(), data_name.to_string()];
        if let Some(reset) = ff.attr("clear").or_else(|| ff.attr("preset")) {
            ordered_inputs.push(reset.to_string());
        }
        for pin in &input_names {
            if !ordered_inputs.contains(pin) {
                ordered_inputs.push(pin.clone());
            }
        }
        let find_pin = |name: &str| -> Result<usize, String> {
            pins.iter()
                .position(|pin| pin.input && pin.name == name)
                .ok_or_else(|| format!("sequential cell '{}' lacks pin '{}'", name, name))
        };
        find_pin(clock_name)?;
        find_pin(data_name)?;
        let reset = ff
            .attr("clear")
            .or_else(|| ff.attr("preset"))
            .map(String::from)
            .map(|reset| -> Result<usize, String> {
                find_pin(&reset)?;
                Ok(ordered_inputs
                    .iter()
                    .position(|pin| *pin == reset)
                    .expect("validated reset pin"))
            })
            .transpose()?;
        let output_pin = pins
            .iter()
            .find(|pin| pin.output)
            .ok_or_else(|| format!("sequential cell '{}' has no output", name))?;
        let mut setup = 0.0f64;
        let mut hold = 0.0f64;
        let mut setup_tables = LibConstraintTables::default();
        let mut hold_tables = LibConstraintTables::default();
        let mut recovery_tables = LibConstraintTables::default();
        let mut removal_tables = LibConstraintTables::default();
        let mut recovery = 0.0f64;
        let mut removal = 0.0f64;
        let mut min_pulse_high = 0.0f64;
        let mut min_pulse_low = 0.0f64;
        let mut clk_to_q = 0.0f64;
        for pin in &pins {
            for timing in &pin.timings {
                if timing.related_pin == clock_name && timing.timing_type.contains("setup") {
                    setup = setup.max(timing.delay);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("hold") {
                    hold = hold.max(timing.delay);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("setup") {
                    setup_tables.rise = timing.rise_constraint_table.clone().or(setup_tables.rise);
                    setup_tables.fall = timing.fall_constraint_table.clone().or(setup_tables.fall);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("hold") {
                    hold_tables.rise = timing.rise_constraint_table.clone().or(hold_tables.rise);
                    hold_tables.fall = timing.fall_constraint_table.clone().or(hold_tables.fall);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("recovery") {
                    recovery_tables.rise = timing
                        .rise_constraint_table
                        .clone()
                        .or(recovery_tables.rise);
                    recovery_tables.fall = timing
                        .fall_constraint_table
                        .clone()
                        .or(recovery_tables.fall);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("removal") {
                    removal_tables.rise =
                        timing.rise_constraint_table.clone().or(removal_tables.rise);
                    removal_tables.fall =
                        timing.fall_constraint_table.clone().or(removal_tables.fall);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("rising_edge") {
                    clk_to_q = clk_to_q.max(timing.delay);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("recovery") {
                    recovery = recovery.max(timing.delay);
                }
                if timing.related_pin == clock_name && timing.timing_type.contains("removal") {
                    removal = removal.max(timing.delay);
                }
                if timing.timing_type.contains("min_high_pulse_width") {
                    min_pulse_high = min_pulse_high.max(timing.delay);
                }
                if timing.timing_type.contains("min_low_pulse_width") {
                    min_pulse_low = min_pulse_low.max(timing.delay);
                }
                if timing.timing_type.contains("min_pulse_width") {
                    min_pulse_high = min_pulse_high.max(timing.delay);
                    min_pulse_low = min_pulse_low.max(timing.delay);
                }
            }
        }
        let def = LibCellDef {
            name: name.clone(),
            inputs: ordered_inputs.clone(),
            output: output_pin.name.clone(),
            func: LibFunc::Id(
                ordered_inputs
                    .iter()
                    .position(|p| p == data_name)
                    .unwrap_or(1),
            ),
            width_sites: area.ceil().max(1.0) as usize,
            arcs: Vec::new(),
            timing_tables: Vec::new(),
            seq: Some(SeqTiming {
                clk_pin: 0,
                clk_to_q,
                setup,
                hold,
                setup_tables: (setup_tables.rise.is_some() || setup_tables.fall.is_some())
                    .then_some(setup_tables),
                hold_tables: (hold_tables.rise.is_some() || hold_tables.fall.is_some())
                    .then_some(hold_tables),
                recovery_tables: (recovery_tables.rise.is_some() || recovery_tables.fall.is_some())
                    .then_some(recovery_tables),
                removal_tables: (removal_tables.rise.is_some() || removal_tables.fall.is_some())
                    .then_some(removal_tables),
                recovery,
                removal,
                min_pulse_high,
                min_pulse_low,
                reset_pin: reset,
            }),
        };
        let mut caps = HashMap::new();
        let mut max_transition_ns = HashMap::new();
        let mut max_capacitance_pf = HashMap::new();
        for pin in &pins {
            if pin.input {
                caps.insert(pin.name.clone(), pin.capacitance);
            }
            if pin.output {
                if let Some(limit) = pin.max_transition {
                    max_transition_ns.insert(pin.name.clone(), limit * time_scale);
                }
                if let Some(limit) = pin.max_capacitance {
                    max_capacitance_pf.insert(pin.name.clone(), limit * capacitance_scale);
                }
            }
        }
        let leakage_states =
            parse_leakage_states(cell, &name, &ordered_inputs, leakage_power_scale)?;
        let internal_power = parse_internal_power(cell, &name, &ordered_inputs)?;
        return Ok((
            def,
            LibCellMetadata {
                area,
                leakage_power: leakage,
                input_capacitance_pf: caps,
                max_transition_ns,
                max_capacitance_pf,
                leakage_states,
                internal_power,
            },
        ));
    }

    let output_pin = pins
        .iter()
        .find(|pin| pin.output && pin.function.is_some())
        .ok_or_else(|| format!("combinational cell '{}' has no output function", name))?;
    let function = output_pin.function.clone().unwrap();
    let func = parse_boolean_function(&function, &input_names)
        .map_err(|e| format!("cell '{}': {}", name, e))?;
    let mut arcs = Vec::new();
    let mut timing_tables = Vec::new();
    for timing in pins.iter().flat_map(|pin| &pin.timings) {
        if timing.timing_type.contains("setup") {
            continue;
        }
        let index = input_names
            .iter()
            .position(|pin| *pin == timing.related_pin)
            .ok_or_else(|| {
                format!(
                    "cell '{}' output '{}' references unknown related pin '{}'",
                    name, output_pin.name, timing.related_pin
                )
            })?;
        arcs.push((index, timing.delay));
        if timing.rise_table.is_some()
            || timing.fall_table.is_some()
            || timing.rise_transition_table.is_some()
            || timing.fall_transition_table.is_some()
        {
            timing_tables.push(LibTimingTables {
                input_pin: index,
                rise: timing.rise_table.clone(),
                fall: timing.fall_table.clone(),
                rise_transition: timing.rise_transition_table.clone(),
                fall_transition: timing.fall_transition_table.clone(),
            });
        }
    }
    let def = LibCellDef {
        name: name.clone(),
        inputs: input_names.clone(),
        output: output_pin.name.clone(),
        func,
        width_sites: area.ceil().max(1.0) as usize,
        arcs,
        timing_tables,
        seq: None,
    };
    let mut caps = HashMap::new();
    let mut max_transition_ns = HashMap::new();
    let mut max_capacitance_pf = HashMap::new();
    for pin in &pins {
        if pin.input {
            caps.insert(pin.name.clone(), pin.capacitance);
        }
        if pin.output {
            if let Some(limit) = pin.max_transition {
                max_transition_ns.insert(pin.name.clone(), limit * time_scale);
            }
            if let Some(limit) = pin.max_capacitance {
                max_capacitance_pf.insert(pin.name.clone(), limit * capacitance_scale);
            }
        }
    }
    let leakage_states = parse_leakage_states(cell, &name, &input_names, leakage_power_scale)?;
    let internal_power = parse_internal_power(cell, &name, &input_names)?;
    Ok((
        def,
        LibCellMetadata {
            area,
            leakage_power: leakage,
            input_capacitance_pf: caps,
            max_transition_ns,
            max_capacitance_pf,
            leakage_states,
            internal_power,
        },
    ))
}

fn parse_leakage_states(
    cell: &Node,
    cell_name: &str,
    inputs: &[String],
    leakage_power_scale: f64,
) -> Result<Vec<LibLeakageState>, String> {
    let mut states = Vec::new();
    let mut conditions = std::collections::BTreeSet::new();
    for group in cell.children("leakage_power") {
        let when = group
            .attr("when")
            .ok_or_else(|| format!("cell '{}' leakage_power has no when", cell_name))?;
        if !conditions.insert(when.to_string()) {
            return Err(format!(
                "cell '{}' has duplicate leakage state '{}'",
                cell_name, when
            ));
        }
        let function = parse_boolean_function(when, inputs)
            .map_err(|e| format!("cell '{}': {}", cell_name, e))?;
        let power = group
            .attr("value")
            .map(number)
            .transpose()?
            .ok_or_else(|| format!("cell '{}' leakage state has no value", cell_name))?;
        if !power.is_finite() || power < 0.0 {
            return Err(format!(
                "cell '{}' leakage state has invalid power {}",
                cell_name, power
            ));
        }
        states.push(LibLeakageState {
            when: function,
            power_w: power * leakage_power_scale,
        });
    }
    Ok(states)
}

fn parse_internal_power(
    cell: &Node,
    cell_name: &str,
    inputs: &[String],
) -> Result<Vec<LibInternalPower>, String> {
    let mut out = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    for pin in cell.children("pin") {
        for group in pin.children("internal_power") {
            let related = group
                .attr("related_pin")
                .map(str::to_string)
                .ok_or_else(|| format!("cell '{}' internal_power has no related_pin", cell_name))?;
            if !inputs.contains(&related) {
                return Err(format!(
                    "cell '{}' internal_power references unknown related pin '{}'",
                    cell_name, related
                ));
            }
            let when = group.attr("when");
            let function = when
                .map(|text| {
                    parse_boolean_function(text, inputs)
                        .map_err(|e| format!("cell '{}': {}", cell_name, e))
                })
                .transpose()?;
            let key = format!("{}:{:?}", related, when);
            if !keys.insert(key) {
                return Err(format!(
                    "cell '{}' has duplicate internal-power arc for pin '{}'",
                    cell_name, related
                ));
            }
            let rise = group
                .first_child("rise_power")
                .map(group_number)
                .transpose()?
                .ok_or_else(|| {
                    format!(
                        "cell '{}' internal_power for '{}' has no rise_power",
                        cell_name, related
                    )
                })?;
            let fall = group
                .first_child("fall_power")
                .map(group_number)
                .transpose()?
                .ok_or_else(|| {
                    format!(
                        "cell '{}' internal_power for '{}' has no fall_power",
                        cell_name, related
                    )
                })?;
            for (name, value) in [("rise_power", rise), ("fall_power", fall)] {
                if !value.is_finite() || value < 0.0 {
                    return Err(format!(
                        "cell '{}' internal_power {} for '{}' is invalid",
                        cell_name, name, related
                    ));
                }
            }
            out.push(LibInternalPower {
                related_pin: related,
                when: function,
                rise_power_pj: rise,
                fall_power_pj: fall,
            });
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
enum FTok {
    Ident(String),
    Not,
    And,
    Or,
    Xor,
    LParen,
    RParen,
    Const(LogicValue),
}

fn lex_function(text: &str) -> Result<Vec<FTok>, String> {
    let normalized = text.replace('&', "*").replace('|', "+").replace('~', "!");
    let mut out = Vec::new();
    let mut chars = normalized.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '!' => out.push(FTok::Not),
            '*' => out.push(FTok::And),
            '+' => out.push(FTok::Or),
            '^' => out.push(FTok::Xor),
            '(' => out.push(FTok::LParen),
            ')' => out.push(FTok::RParen),
            '0' => out.push(FTok::Const(LogicValue::Zero)),
            '1' => out.push(FTok::Const(LogicValue::One)),
            c if c.is_ascii_alphanumeric() || c == '_' || c == '\\' => {
                let mut ident = String::new();
                ident.push(c);
                while let Some(&next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || next == '_' {
                        ident.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push(FTok::Ident(ident));
            }
            other => return Err(format!("unexpected function character '{}'", other)),
        }
    }
    Ok(out)
}

pub(crate) fn parse_boolean_function(text: &str, inputs: &[String]) -> Result<LibFunc, String> {
    let tokens = lex_function(text)?;
    let mut pos = 0;
    let func = parse_function_or(&tokens, inputs, &mut pos)?;
    if pos != tokens.len() {
        return Err("unexpected trailing tokens in Boolean function".into());
    }
    Ok(func)
}

fn parse_function_or(
    tokens: &[FTok],
    inputs: &[String],
    pos: &mut usize,
) -> Result<LibFunc, String> {
    let mut terms = vec![parse_function_xor(tokens, inputs, pos)?];
    while matches!(tokens.get(*pos), Some(FTok::Or)) {
        *pos += 1;
        terms.push(parse_function_xor(tokens, inputs, pos)?);
    }
    if terms.len() == 1 {
        Ok(terms.pop().unwrap())
    } else {
        Ok(LibFunc::Or(terms))
    }
}

fn parse_function_xor(
    tokens: &[FTok],
    inputs: &[String],
    pos: &mut usize,
) -> Result<LibFunc, String> {
    let mut terms = vec![parse_function_and(tokens, inputs, pos)?];
    while matches!(tokens.get(*pos), Some(FTok::Xor)) {
        *pos += 1;
        terms.push(parse_function_and(tokens, inputs, pos)?);
    }
    if terms.len() == 1 {
        Ok(terms.pop().unwrap())
    } else {
        Ok(LibFunc::Xor(terms))
    }
}

fn parse_function_and(
    tokens: &[FTok],
    inputs: &[String],
    pos: &mut usize,
) -> Result<LibFunc, String> {
    let mut terms = vec![parse_function_unary(tokens, inputs, pos)?];
    while matches!(tokens.get(*pos), Some(FTok::And)) {
        *pos += 1;
        terms.push(parse_function_unary(tokens, inputs, pos)?);
    }
    if terms.len() == 1 {
        Ok(terms.pop().unwrap())
    } else {
        Ok(LibFunc::And(terms))
    }
}

fn parse_function_unary(
    tokens: &[FTok],
    inputs: &[String],
    pos: &mut usize,
) -> Result<LibFunc, String> {
    if matches!(tokens.get(*pos), Some(FTok::Not)) {
        *pos += 1;
        return Ok(LibFunc::Not(Box::new(parse_function_unary(
            tokens, inputs, pos,
        )?)));
    }
    parse_function_primary(tokens, inputs, pos)
}

fn parse_function_primary(
    tokens: &[FTok],
    inputs: &[String],
    pos: &mut usize,
) -> Result<LibFunc, String> {
    match tokens.get(*pos) {
        Some(FTok::LParen) => {
            *pos += 1;
            let value = parse_function_or(tokens, inputs, pos)?;
            if !matches!(tokens.get(*pos), Some(FTok::RParen)) {
                return Err("missing ')' in Boolean function".into());
            }
            *pos += 1;
            Ok(value)
        }
        Some(FTok::Const(value)) => {
            let value = *value;
            *pos += 1;
            Ok(LibFunc::Const(value))
        }
        Some(FTok::Ident(name)) => {
            let name = name.clone();
            *pos += 1;
            let index = inputs
                .iter()
                .position(|input| input.eq_ignore_ascii_case(name.as_str()))
                .ok_or_else(|| format!("function references unknown pin '{}'", name))?;
            Ok(LibFunc::Id(index))
        }
        _ => Err("expected Boolean function term".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = r#"
        library (test) {
            time_unit : "1ns";
            cell (INV) {
                area : 1.2;
                cell_leakage_power : 0.03;
                pin (A) { direction : input; capacitance : 0.01; }
                pin (Y) {
                    direction : output;
                    function : "!A";
                    timing () { related_pin : "A"; cell_rise(values("0.35,0.42")); }
                }
            }
            cell (AND2) {
                area : 3.1;
                pin (A) { direction : input; }
                pin (B) { direction : input; }
                pin (Y) {
                    direction : output;
                    function : "(A*B)";
                    timing () { related_pin : "A"; cell_rise(values("1.05")); }
                    timing () { related_pin : "B"; cell_rise(values("1.10")); }
                }
            }
            cell (DFF) {
                area : 5.0;
                ff (IQ, IQN) {
                    clocked_on : "CLK";
                    next_state : "D";
                    clear : "RST";
                }
                pin (D) {
                    direction : input;
                    timing () { related_pin : "CLK"; timing_type : setup_rising; rise_constraint(values("0.22")); }
                    timing () { related_pin : "CLK"; timing_type : hold_rising; rise_constraint(values("0.08")); }
                }
                pin (RST) {
                    direction : input;
                    timing () { related_pin : "CLK"; timing_type : recovery_rising; rise_constraint(values("0.19")); }
                    timing () { related_pin : "CLK"; timing_type : removal_rising; rise_constraint(values("0.11")); }
                }
                pin (CLK) {
                    direction : input;
                    timing () { related_pin : "CLK"; timing_type : min_pulse_width; rise_constraint(values("0.12")); }
                }
                pin (Q) { direction : output; timing () { related_pin : "CLK"; timing_type : rising_edge; cell_rise(values("0.77")); } }
            }
        }
    "#;

    #[test]
    fn parses_combinational_and_sequential_cells() {
        let lib = parse_liberty(LIB).unwrap();
        assert_eq!(lib.name, "test");
        assert_eq!(lib.cells.len(), 3);
        let inv = lib.get(lib.find("INV").unwrap());
        assert_eq!(inv.width_sites, 2);
        assert_eq!(inv.arcs, vec![(0, 0.35)]);
        assert_eq!(lib.metadata("INV").leakage_power, 0.03);
        let dff = lib.get(lib.find("DFF").unwrap());
        assert_eq!(dff.inputs, vec!["CLK", "D", "RST"]);
        let seq = dff.seq.as_ref().unwrap();
        assert_eq!(seq.reset_pin, Some(2));
        assert_eq!(seq.clk_to_q, 0.77);
        assert_eq!(seq.setup, 0.22);
        assert_eq!(seq.hold, 0.08);
        assert_eq!(seq.recovery, 0.19);
        assert_eq!(seq.removal, 0.11);
        assert_eq!(seq.min_pulse_high, 0.12);
        assert_eq!(seq.min_pulse_low, 0.12);
    }

    #[test]
    fn rejects_unknown_pin_and_missing_area() {
        assert!(parse_liberty(
            r#"library(x){cell(A){area:1;pin(Y){direction:output;function:"Z";}}} "#
        )
        .is_err());
        assert!(
            parse_liberty(r#"library(x){cell(A){pin(Y){direction:output;function:"1";}}} "#)
                .is_err()
        );
    }
}
