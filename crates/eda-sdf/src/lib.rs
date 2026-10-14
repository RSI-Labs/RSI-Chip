//! # eda-sdf — SDF delay back-annotation.
//!
//! Parses a practical SDF subset and applies absolute `IOPATH`,
//! `INTERCONNECT`, setup, and hold annotations to a mapped netlist. Timing
//! values are converted to nanoseconds using the SDF timescale.

use eda_core::library::{CellLibrary, SeqTiming};
use eda_core::netlist::{CellKind, Netlist};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfDelay {
    pub rise: f64,
    pub fall: f64,
    pub transition: f64,
}

impl SdfDelay {
    fn conservative(&self) -> f64 {
        self.rise.max(self.fall).max(self.transition)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Iopath {
    pub instance: Option<String>,
    pub cell_type: Option<String>,
    pub from_pin: String,
    pub to_pin: String,
    pub delay: SdfDelay,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interconnect {
    pub instance: Option<String>,
    pub source_net: String,
    pub target_net: String,
    pub delay: SdfDelay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingCheckKind {
    Setup,
    Hold,
    Width,
    Recovery,
    Removal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SdfTimingCheck {
    pub instance: Option<String>,
    pub cell_type: Option<String>,
    pub kind: TimingCheckKind,
    pub data_pin: String,
    /// Empty for WIDTH checks, which have a single clock/port pin.
    pub clock_pin: String,
    pub delay: SdfDelay,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sdf {
    pub version: String,
    pub design: String,
    pub timescale_ns: f64,
    pub iopaths: Vec<Iopath>,
    pub interconnects: Vec<Interconnect>,
    pub timing_checks: Vec<SdfTimingCheck>,
}

#[derive(Clone, Debug)]
enum Tok {
    Word(String, usize),
    Number(String, usize),
    Punct(&'static str, usize),
    String(String, usize),
}

impl Tok {
    fn line(&self) -> usize {
        match self {
            Tok::Word(_, line)
            | Tok::Number(_, line)
            | Tok::Punct(_, line)
            | Tok::String(_, line) => *line,
        }
    }

    fn text(&self) -> String {
        match self {
            Tok::Word(word, _) | Tok::Number(word, _) | Tok::String(word, _) => word.clone(),
            Tok::Punct(value, _) => (*value).to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Node {
    kind: String,
    args: Vec<String>,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, kind: &str) -> Option<&Node> {
        self.children.iter().find(|child| child.kind == kind)
    }

    fn descendants(&self, kind: &str) -> Vec<&Node> {
        let mut out = Vec::new();
        fn walk<'a>(node: &'a Node, kind: &str, out: &mut Vec<&'a Node>) {
            if node.kind == kind {
                out.push(node);
            }
            for child in &node.children {
                walk(child, kind, out);
            }
        }
        walk(self, kind, &mut out);
        out
    }
}

/// Parse an SDF 3.x subset.
pub fn parse_sdf(source: &str) -> Result<Sdf, String> {
    let tokens = lex(source)?;
    let mut pos = 0;
    let nodes = parse_nodes(&tokens, &mut pos, 0)?;
    if pos != tokens.len() {
        return Err("unexpected trailing SDF tokens".into());
    }
    let root = nodes
        .iter()
        .find(|node| node.kind == "DELAYFILE")
        .ok_or_else(|| "SDF file has no DELAYFILE".to_string())?;
    let mut sdf = Sdf {
        timescale_ns: 1.0,
        ..Default::default()
    };
    sdf.version = root
        .child("SDFVERSION")
        .and_then(|node| node.args.first().cloned())
        .unwrap_or_else(|| "3.0".into());
    sdf.design = root
        .child("DESIGN")
        .and_then(|node| node.args.first().cloned())
        .ok_or_else(|| "SDF file has no DESIGN".to_string())?;
    if let Some(timescale) = root.child("TIMESCALE") {
        let text = timescale
            .args
            .first()
            .ok_or_else(|| "SDF TIMESCALE has no value".to_string())?;
        sdf.timescale_ns = parse_timescale(text)?;
    }

    for cell in root.descendants("CELL") {
        let cell_type = cell
            .child("CELLTYPE")
            .and_then(|node| node.args.first().cloned());
        let instance = cell.child("INSTANCE").and_then(|node| {
            let value = node.args.first().cloned().unwrap_or_default();
            if value.is_empty() || value == "*" {
                None
            } else {
                Some(value)
            }
        });
        for iopath in cell.descendants("IOPATH") {
            if iopath.args.len() != 2 {
                return Err("IOPATH requires two pins".into());
            }
            let delay = parse_delay_node(iopath, sdf.timescale_ns)?;
            sdf.iopaths.push(Iopath {
                instance: instance.clone(),
                cell_type: cell_type.clone(),
                from_pin: iopath.args[0].clone(),
                to_pin: iopath.args[1].clone(),
                delay,
            });
        }
        for interconnect in cell.descendants("INTERCONNECT") {
            if interconnect.args.len() != 2 {
                return Err("INTERCONNECT requires source and target nets".into());
            }
            let delay = parse_delay_node(interconnect, sdf.timescale_ns)?;
            sdf.interconnects.push(Interconnect {
                instance: instance.clone(),
                source_net: interconnect.args[0].clone(),
                target_net: interconnect.args[1].clone(),
                delay,
            });
        }
        for check in cell.descendants("TIMINGCHECK") {
            for kind in ["SETUP", "HOLD", "WIDTH", "RECOVERY", "REMOVAL"] {
                let Some(node) = check.child(kind) else {
                    continue;
                };
                let expected_args = if kind == "WIDTH" { 1 } else { 2 };
                if node.args.len() != expected_args {
                    return Err(format!("{} requires {} pin(s)", kind, expected_args));
                }
                let delay = parse_delay_node(node, sdf.timescale_ns)?;
                sdf.timing_checks.push(SdfTimingCheck {
                    instance: instance.clone(),
                    cell_type: cell_type.clone(),
                    kind: match kind {
                        "SETUP" => TimingCheckKind::Setup,
                        "HOLD" => TimingCheckKind::Hold,
                        "WIDTH" => TimingCheckKind::Width,
                        "RECOVERY" => TimingCheckKind::Recovery,
                        _ => TimingCheckKind::Removal,
                    },
                    data_pin: node.args[0].clone(),
                    clock_pin: node.args.get(1).cloned().unwrap_or_default(),
                    delay,
                });
            }
        }
    }

    if sdf.iopaths.is_empty() && sdf.interconnects.is_empty() && sdf.timing_checks.is_empty() {
        return Err("SDF file contains no timing annotations".into());
    }
    Ok(sdf)
}

fn lex(source: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = match raw.find("//") {
            Some(position) => &raw[..position],
            None => raw,
        };
        let mut chars = line.chars().peekable();
        while let Some(&first) = chars.peek() {
            match first {
                c if c.is_whitespace() => {
                    chars.next();
                }
                '(' | ')' | ',' | ';' | ':' => {
                    chars.next();
                    let punct = match first {
                        '(' => "(",
                        ')' => ")",
                        ',' => ",",
                        ';' => ";",
                        _ => ":",
                    };
                    // SDF uses `::` inside triple-value numbers. Keep it as
                    // one token so float parsing can replace it with ':'.
                    if punct == ":" && chars.peek() == Some(&':') {
                        chars.next();
                        out.push(Tok::Punct("::", line_number));
                    } else {
                        out.push(Tok::Punct(punct, line_number));
                    }
                }
                '"' => {
                    chars.next();
                    let mut word = String::new();
                    let mut closed = false;
                    for c in chars.by_ref() {
                        if c == '"' {
                            closed = true;
                            break;
                        }
                        word.push(c);
                    }
                    if !closed {
                        return Err(format!("line {}: unterminated string", line_number));
                    }
                    out.push(Tok::String(word, line_number));
                }
                _ => {
                    let mut word = String::new();
                    while let Some(&next) = chars.peek() {
                        if next.is_whitespace() || matches!(next, '(' | ')' | ',' | ';' | '"' | ':')
                        {
                            break;
                        }
                        word.push(next);
                        chars.next();
                    }
                    if word.is_empty() {
                        return Err(format!(
                            "line {}: unexpected character '{}'",
                            line_number, first
                        ));
                    }
                    if word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        out.push(Tok::Number(word, line_number));
                    } else {
                        out.push(Tok::Word(word, line_number));
                    }
                }
            }
        }
    }
    Ok(out)
}

fn parse_nodes(tokens: &[Tok], pos: &mut usize, depth: usize) -> Result<Vec<Node>, String> {
    let _ = depth;
    let mut out = Vec::new();
    while *pos < tokens.len() {
        if !matches!(tokens[*pos], Tok::Punct("(", _)) {
            let line = tokens[*pos].line();
            return Err(format!(
                "line {}: expected '(' in SDF, found {}",
                line,
                tokens[*pos].text()
            ));
        }
        out.push(parse_group(tokens, pos)?);
    }
    Ok(out)
}

fn parse_group(tokens: &[Tok], pos: &mut usize) -> Result<Node, String> {
    let open_line = tokens[*pos].line();
    *pos += 1;
    let mut node = match tokens.get(*pos) {
        Some(Tok::Word(kind, _)) => {
            let kind = kind.clone();
            *pos += 1;
            Node {
                kind,
                ..Default::default()
            }
        }
        Some(Tok::Number(first, _)) => {
            let first = first.clone();
            *pos += 1;
            let mut node = Node {
                kind: "__VALUE__".into(),
                ..Default::default()
            };
            node.args.push(first);
            node
        }
        Some(token) => {
            return Err(format!(
                "line {}: expected SDF group name, found {}",
                open_line,
                token.text()
            ));
        }
        None => return Err(format!("line {}: unterminated SDF group", open_line)),
    };

    while *pos < tokens.len() {
        match tokens[*pos].clone() {
            Tok::Punct("(", _) => {
                let child = parse_group(tokens, pos)?;
                node.children.push(child);
            }
            Tok::Punct(")", _) => {
                *pos += 1;
                return Ok(node);
            }
            token => {
                node.args.push(token.text());
                *pos += 1;
            }
        }
    }
    Err(format!("line {}: unterminated SDF group", open_line))
}

fn parse_timescale(text: &str) -> Result<f64, String> {
    let cleaned = text.trim().trim_end_matches(';');
    let (number, scale) = if let Some(value) = cleaned.strip_suffix("fs") {
        (value, 1e-6)
    } else if let Some(value) = cleaned.strip_suffix("ps") {
        (value, 1e-3)
    } else if let Some(value) = cleaned.strip_suffix("ns") {
        (value, 1.0)
    } else if let Some(value) = cleaned.strip_suffix("us") {
        (value, 1000.0)
    } else {
        return Err(format!("unsupported SDF timescale '{}'", cleaned));
    };
    let number: f64 = number
        .trim()
        .parse()
        .map_err(|_| format!("invalid SDF timescale '{}'", cleaned))?;
    let result = number * scale;
    if !result.is_finite() || result <= 0.0 {
        return Err("SDF timescale must be finite and positive".into());
    }
    Ok(result)
}

fn parse_delay_node(node: &Node, scale: f64) -> Result<SdfDelay, String> {
    // Values are child groups such as `(0.1::0.2::0.3)`, represented by a
    // NUMBER node followed by an argument list. Accept one, two, or three
    // values; missing values default to the first value.
    let mut values = Vec::new();
    for child in &node.children {
        let joined = child.args.join("");
        for value in joined.split("::") {
            if value.is_empty() {
                continue;
            }
            let parsed: f64 = value
                .parse()
                .map_err(|_| format!("invalid SDF delay value '{}'", value))?;
            values.push(parsed * scale);
        }
    }
    if values.is_empty() {
        return Err(format!("{} has no delay value", node.kind));
    }
    let first = values[0];
    let second = values.get(1).copied().unwrap_or(first);
    let third = values.get(2).copied().unwrap_or(first);
    if [first, second, third]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("SDF delay values must be finite and nonnegative".into());
    }
    Ok(SdfDelay {
        rise: first,
        fall: second,
        transition: third,
    })
}

#[derive(Clone, Debug, Default)]
struct CellTimingPatch {
    input_delays: HashMap<String, f64>,
    /// Incremental INTERCONNECT delays. Unlike absolute IOPATH annotations,
    /// these delays add to the original library input arc.
    interconnect_input_delays: HashMap<String, f64>,
    clk_to_q: Option<f64>,
    setup: Option<f64>,
    hold: Option<f64>,
    recovery: Option<f64>,
    removal: Option<f64>,
    min_pulse_width: Option<f64>,
}

/// Apply absolute SDF timing annotations and return a new netlist.
pub fn annotate_sdf(nl: &Netlist, sdf: &Sdf) -> Result<Netlist, String> {
    let original_lib: CellLibrary = match nl.library() {
        Some(lib) => (**lib).clone(),
        None => return Err("SDF annotation requires a library-backed netlist".into()),
    };
    let mut patches: HashMap<usize, CellTimingPatch> = HashMap::new();

    for path in &sdf.iopaths {
        for ci in matching_cells(nl, path.instance.as_deref(), path.cell_type.as_deref())? {
            let patch = patches.entry(ci).or_default();
            let cell = nl.cell(ci);
            let CellKind::Lib(lib_id) = cell.kind else {
                return Err(format!("cell '{}' is not a library cell", cell.name));
            };
            let def = original_lib.get(lib_id);
            if def
                .inputs
                .iter()
                .any(|pin| pin.eq_ignore_ascii_case(&path.from_pin))
            {
                patch.input_delays.insert(
                    path.from_pin.to_ascii_uppercase(),
                    path.delay.conservative(),
                );
            } else if def.seq.is_some()
                && path.from_pin.eq_ignore_ascii_case(
                    def.inputs
                        .get(def.seq.as_ref().unwrap().clk_pin)
                        .map(String::as_str)
                        .unwrap_or("clk"),
                )
                && path.to_pin.eq_ignore_ascii_case(&def.output)
            {
                patch.clk_to_q = Some(path.delay.conservative());
            } else {
                return Err(format!(
                    "SDF IOPATH pins {} -> {} do not match cell '{}'",
                    path.from_pin, path.to_pin, cell.name
                ));
            }
        }
    }

    for check in &sdf.timing_checks {
        for ci in matching_cells(nl, check.instance.as_deref(), check.cell_type.as_deref())? {
            let patch = patches.entry(ci).or_default();
            match check.kind {
                TimingCheckKind::Setup => {
                    patch.setup = Some(check.delay.conservative());
                }
                TimingCheckKind::Hold => {
                    patch.hold = Some(check.delay.conservative());
                }
                TimingCheckKind::Width => {
                    patch.min_pulse_width = Some(check.delay.conservative());
                }
                TimingCheckKind::Recovery => {
                    patch.recovery = Some(check.delay.conservative());
                }
                TimingCheckKind::Removal => {
                    patch.removal = Some(check.delay.conservative());
                }
            }
        }
    }

    for interconnect in &sdf.interconnects {
        let Some(target) = nl.net_by_name(&interconnect.target_net) else {
            return Err(format!(
                "SDF INTERCONNECT target net '{}' not found",
                interconnect.target_net
            ));
        };
        for &sink in &nl.net(target).sinks {
            let cell = nl.cell(sink);
            let CellKind::Lib(lib_id) = cell.kind else {
                continue;
            };
            let def = original_lib.get(lib_id);
            for (pin_index, input) in cell.inputs.iter().enumerate() {
                if *input != target {
                    continue;
                }
                let Some(pin) = def.inputs.get(pin_index) else {
                    continue;
                };
                patches
                    .entry(sink)
                    .or_default()
                    .interconnect_input_delays
                    .insert(pin.to_ascii_uppercase(), interconnect.delay.conservative());
            }
        }
    }

    let mut annotated_lib = original_lib.clone();
    annotated_lib.name = format!("{}_sdf", original_lib.name);
    let mut out = nl.clone();
    for (ci, patch) in patches {
        let cell = out.cell(ci);
        let CellKind::Lib(old_id) = cell.kind else {
            return Err(format!("cell '{}' is not a library cell", cell.name));
        };
        let mut def = original_lib.get(old_id).clone();
        let original_name = def.name.clone();
        let annotated_name = format!(
            "{}__SDF__{}",
            def.name,
            cell.name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        def.name = annotated_name.clone();
        if let Some(clk_to_q) = patch.clk_to_q {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming { clk_to_q, ..seq });
        }
        if let Some(setup) = patch.setup {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming {
                setup,
                setup_tables: None,
                ..seq
            });
        }
        if let Some(hold) = patch.hold {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming {
                hold,
                hold_tables: None,
                ..seq
            });
        }
        if let Some(width) = patch.min_pulse_width {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming {
                min_pulse_high: width,
                min_pulse_low: width,
                ..seq
            });
        }
        if let Some(recovery) = patch.recovery {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming {
                recovery,
                recovery_tables: None,
                ..seq
            });
        }
        if let Some(removal) = patch.removal {
            let seq = def.seq.clone().unwrap_or(SeqTiming {
                clk_pin: 0,
                clk_to_q: 0.0,
                setup: 0.0,
                hold: 0.0,
                setup_tables: None,
                hold_tables: None,
                recovery_tables: None,
                removal_tables: None,
                recovery: 0.0,
                removal: 0.0,
                min_pulse_high: 0.0,
                min_pulse_low: 0.0,
                reset_pin: None,
            });
            def.seq = Some(SeqTiming {
                removal,
                removal_tables: None,
                ..seq
            });
        }
        for (pin, delay) in patch.input_delays {
            let index = def
                .inputs
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&pin))
                .ok_or_else(|| format!("SDF pin '{}' not found", pin))?;
            if let Some(arc) = def.arcs.iter_mut().find(|(arc_pin, _)| *arc_pin == index) {
                arc.1 = delay;
            } else {
                def.arcs.push((index, delay));
            }
        }
        for (pin, delay) in patch.interconnect_input_delays {
            let index = def
                .inputs
                .iter()
                .position(|name| name.eq_ignore_ascii_case(&pin))
                .ok_or_else(|| format!("SDF pin '{}' not found", pin))?;
            if let Some(arc) = def.arcs.iter_mut().find(|(arc_pin, _)| *arc_pin == index) {
                arc.1 += delay;
            } else {
                def.arcs.push((index, delay));
            }
        }
        let new_id = annotated_lib.add(def);
        // Built-in scalar libraries intentionally have no optional metadata;
        // metadata lookup then correctly falls back to an empty record.
        let _ = annotated_lib.copy_metadata(&original_name, &annotated_name);
        out.cells_mut()[ci].kind = CellKind::Lib(new_id);
    }
    out.set_library(Arc::new(annotated_lib));
    Ok(out)
}

fn matching_cells(
    nl: &Netlist,
    instance: Option<&str>,
    cell_type: Option<&str>,
) -> Result<Vec<usize>, String> {
    let lib = nl
        .library()
        .ok_or_else(|| "SDF annotation requires a library-backed netlist".to_string())?;
    let mut out = Vec::new();
    for ci in 0..nl.cell_count() {
        let cell = nl.cell(ci);
        let actual_type = if let CellKind::Lib(id) = cell.kind {
            lib.get(id).name.clone()
        } else {
            format!("{:?}", cell.kind)
        };
        let instance_matches = instance.map(|name| name == cell.name).unwrap_or(true);
        let type_matches = cell_type
            .map(|kind| kind.eq_ignore_ascii_case(&actual_type))
            .unwrap_or(true);
        if instance_matches && type_matches {
            out.push(ci);
        }
    }
    if instance.is_some() && out.is_empty() {
        return Err(format!(
            "SDF instance '{}' not found",
            instance.unwrap_or_default()
        ));
    }
    Ok(out)
}

/// Export current library timing as an SDF 3.0 file.
pub fn write_sdf(nl: &Netlist) -> Result<String, String> {
    let lib = nl
        .library()
        .ok_or_else(|| "SDF export requires a library-backed netlist".to_string())?;
    let mut out = String::new();
    out.push_str("(DELAYFILE\n");
    out.push_str("  (SDFVERSION \"3.0\")\n");
    out.push_str(&format!("  (DESIGN \"{}\")\n", nl.name));
    out.push_str("  (TIMESCALE 1ns)\n");
    for (ci, cell) in nl.cells().iter().enumerate() {
        let CellKind::Lib(lib_id) = cell.kind else {
            continue;
        };
        let def = lib.get(lib_id);
        out.push_str("  (CELL\n");
        out.push_str(&format!("    (CELLTYPE \"{}\")\n", def.name));
        out.push_str(&format!("    (INSTANCE {})\n", cell.name));
        if !def.arcs.is_empty() {
            out.push_str("    (DELAY (ABSOLUTE\n");
            for (pin, delay) in &def.arcs {
                let Some(pin_name) = def.inputs.get(*pin) else {
                    continue;
                };
                out.push_str(&format!(
                    "      (IOPATH {} {} ({:.6}::{:.6}))\n",
                    pin_name, def.output, delay, delay
                ));
            }
            out.push_str("    ))\n");
        }
        if let Some(seq) = &def.seq {
            let clock = def
                .inputs
                .get(seq.clk_pin)
                .cloned()
                .unwrap_or_else(|| "CLK".into());
            let data = def.inputs.get(1).cloned().unwrap_or_else(|| "D".into());
            let reset = seq
                .reset_pin
                .and_then(|pin| def.inputs.get(pin).cloned())
                .unwrap_or_else(|| "RST".into());
            out.push_str("    (TIMINGCHECK\n");
            out.push_str(&format!(
                "      (SETUP {} {} ({:.6}::{:.6}))\n",
                data, clock, seq.setup, seq.setup
            ));
            out.push_str(&format!(
                "      (HOLD {} {} ({:.6}::{:.6}))\n",
                data, clock, seq.hold, seq.hold
            ));
            out.push_str(&format!(
                "      (WIDTH {} ({:.6}::{:.6}))\n",
                clock,
                seq.min_pulse_high.max(seq.min_pulse_low),
                seq.min_pulse_high.max(seq.min_pulse_low)
            ));
            if seq.reset_pin.is_some() {
                out.push_str(&format!(
                    "      (RECOVERY {} {} ({:.6}::{:.6}))\n",
                    reset, clock, seq.recovery, seq.recovery
                ));
                out.push_str(&format!(
                    "      (REMOVAL {} {} ({:.6}::{:.6}))\n",
                    reset, clock, seq.removal, seq.removal
                ));
            }
            out.push_str("    )\n");
        }
        let _ = ci;
        out.push_str("  )\n");
    }
    out.push_str(")\n");
    Ok(out)
}

/// Serialize an in-memory SDF object, preserving its normalized nanosecond
/// timing values and original timescale.
///
/// This is distinct from [`write_sdf`], which exports nominal timing from a
/// library-backed netlist. In particular, this writer preserves physical
/// `INTERCONNECT` annotations generated after parasitic extraction.
pub fn write_delay_file(sdf: &Sdf) -> Result<String, String> {
    if sdf.design.is_empty() {
        return Err("SDF export requires a design name".into());
    }
    if !sdf.timescale_ns.is_finite() || sdf.timescale_ns <= 0.0 {
        return Err("SDF export requires a finite positive timescale".into());
    }
    if sdf.iopaths.is_empty() && sdf.interconnects.is_empty() && sdf.timing_checks.is_empty() {
        return Err("SDF export requires at least one timing annotation".into());
    }

    #[derive(Default)]
    struct CellGroup<'a> {
        iopaths: Vec<&'a Iopath>,
        interconnects: Vec<&'a Interconnect>,
        timing_checks: Vec<&'a SdfTimingCheck>,
    }

    let mut order: Vec<(Option<String>, Option<String>)> = Vec::new();
    let mut groups: HashMap<(Option<String>, Option<String>), CellGroup> = HashMap::new();

    for path in &sdf.iopaths {
        validate_delay(&path.delay)?;
        validate_identifier(&path.from_pin, "IOPATH source pin")?;
        validate_identifier(&path.to_pin, "IOPATH destination pin")?;
        if let Some(instance) = &path.instance {
            validate_identifier(instance, "IOPATH instance")?;
        }
        if let Some(cell_type) = &path.cell_type {
            validate_quoted_string(cell_type, "IOPATH cell type")?;
        }
        let key = (path.instance.clone(), path.cell_type.clone());
        if !order.contains(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().iopaths.push(path);
    }
    for interconnect in &sdf.interconnects {
        validate_delay(&interconnect.delay)?;
        validate_identifier(&interconnect.source_net, "INTERCONNECT source net")?;
        validate_identifier(&interconnect.target_net, "INTERCONNECT target net")?;
        if let Some(instance) = &interconnect.instance {
            validate_identifier(instance, "INTERCONNECT instance")?;
        }
        let key = (interconnect.instance.clone(), None);
        if !order.contains(&key) {
            order.push(key.clone());
        }
        groups
            .entry(key)
            .or_default()
            .interconnects
            .push(interconnect);
    }
    for check in &sdf.timing_checks {
        validate_delay(&check.delay)?;
        validate_identifier(&check.data_pin, "timing-check data pin")?;
        if !check.clock_pin.is_empty() {
            validate_identifier(&check.clock_pin, "timing-check clock pin")?;
        }
        if let Some(instance) = &check.instance {
            validate_identifier(instance, "timing-check instance")?;
        }
        if let Some(cell_type) = &check.cell_type {
            validate_quoted_string(cell_type, "timing-check cell type")?;
        }
        let key = (check.instance.clone(), check.cell_type.clone());
        if !order.contains(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().timing_checks.push(check);
    }

    let (timescale_number, timescale_unit) = format_timescale(sdf.timescale_ns);
    validate_quoted_string(&sdf.version, "version")?;
    validate_quoted_string(&sdf.design, "design")?;
    let mut out = String::new();
    out.push_str("(DELAYFILE\n");
    out.push_str(&format!("  (SDFVERSION \"{}\")\n", sdf.version));
    out.push_str(&format!("  (DESIGN \"{}\")\n", sdf.design));
    out.push_str(&format!(
        "  (TIMESCALE {}{})\n",
        timescale_number, timescale_unit
    ));

    for key in order {
        let group = &groups[&key];
        out.push_str("  (CELL\n");
        if let Some(cell_type) = &key.1 {
            out.push_str(&format!("    (CELLTYPE \"{}\")\n", cell_type));
        }
        out.push_str(&format!(
            "    (INSTANCE {})\n",
            key.0.as_deref().unwrap_or("*")
        ));
        if !group.iopaths.is_empty() || !group.interconnects.is_empty() {
            out.push_str("    (DELAY (ABSOLUTE\n");
            for path in &group.iopaths {
                out.push_str(&format!(
                    "      (IOPATH {} {} {})\n",
                    path.from_pin,
                    path.to_pin,
                    format_delay(&path.delay, sdf.timescale_ns)
                ));
            }
            for interconnect in &group.interconnects {
                out.push_str(&format!(
                    "      (INTERCONNECT {} {} {})\n",
                    interconnect.source_net,
                    interconnect.target_net,
                    format_delay(&interconnect.delay, sdf.timescale_ns)
                ));
            }
            out.push_str("    ))\n");
        }
        if !group.timing_checks.is_empty() {
            out.push_str("    (TIMINGCHECK\n");
            for check in &group.timing_checks {
                let kind = match check.kind {
                    TimingCheckKind::Setup => "SETUP",
                    TimingCheckKind::Hold => "HOLD",
                    TimingCheckKind::Width => "WIDTH",
                    TimingCheckKind::Recovery => "RECOVERY",
                    TimingCheckKind::Removal => "REMOVAL",
                };
                if check.clock_pin.is_empty() {
                    out.push_str(&format!(
                        "      ({} {} {})\n",
                        kind,
                        check.data_pin,
                        format_delay(&check.delay, sdf.timescale_ns)
                    ));
                } else {
                    out.push_str(&format!(
                        "      ({} {} {} {})\n",
                        kind,
                        check.data_pin,
                        check.clock_pin,
                        format_delay(&check.delay, sdf.timescale_ns)
                    ));
                }
            }
            out.push_str("    )\n");
        }
        out.push_str("  )\n");
    }
    out.push_str(")\n");
    Ok(out)
}

fn validate_delay(delay: &SdfDelay) -> Result<(), String> {
    if [delay.rise, delay.fall, delay.transition]
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("SDF delay values must be finite and nonnegative".into());
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value == "*"
        || value
            .contains(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '"' | ',' | ';' | ':'))
        || value.contains("//")
    {
        return Err(format!("invalid SDF {}: '{}'", field, value));
    }
    Ok(())
}

fn validate_quoted_string(value: &str, field: &str) -> Result<(), String> {
    if value.contains('"') || value.contains('\\') || value.contains('\n') {
        return Err(format!("invalid SDF {}: '{}'", field, value));
    }
    Ok(())
}

fn format_delay(delay: &SdfDelay, timescale_ns: f64) -> String {
    format!(
        "({}::{}::{})",
        format_number(delay.rise / timescale_ns),
        format_number(delay.fall / timescale_ns),
        format_number(delay.transition / timescale_ns)
    )
}

fn format_number(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        format!("{}", value)
    }
}

fn format_timescale(timescale_ns: f64) -> (String, &'static str) {
    if timescale_ns >= 1.0 {
        (format_number(timescale_ns), "ns")
    } else if timescale_ns >= 1e-3 {
        (format_number(timescale_ns / 1e-3), "ps")
    } else {
        (format_number(timescale_ns / 1e-6), "fs")
    }
}

/// Human-readable SDF summary.
pub fn report(sdf: &Sdf) -> String {
    format!(
        "SDF report: design '{}', timescale {} ns, {} IOPATH, {} INTERCONNECT, {} timing check\n",
        sdf.design,
        sdf.timescale_ns,
        sdf.iopaths.len(),
        sdf.interconnects.len(),
        sdf.timing_checks.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};

    fn mapped_and() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("m");
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

    fn mapped_dff() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("ff");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl
    }

    fn mapped_dffr() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("ffr");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let rst = nl.add_net("rst");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("DFFR").unwrap()),
            inputs: vec![clk, d, rst],
            output: q,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("rst", Dir::In, vec![rst]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl
    }

    #[test]
    fn parse_write_annotate_round_trip() {
        let nl = mapped_and();
        let text = write_sdf(&nl).unwrap();
        let sdf = parse_sdf(&text).unwrap();
        assert_eq!(sdf.design, "m");
        assert_eq!(sdf.iopaths.len(), 2);
        let annotated = annotate_sdf(&nl, &sdf).unwrap();
        let old_kind = &nl.cells()[0].kind;
        let CellKind::Lib(old_id) = old_kind else {
            panic!("expected mapped cell");
        };
        let new_kind = &annotated.cells()[0].kind;
        let CellKind::Lib(new_id) = new_kind else {
            panic!("expected annotated mapped cell");
        };
        let old = nl.library().unwrap().get(*old_id).clone();
        let new = annotated.library().unwrap().get(*new_id).clone();
        assert_eq!(old.arcs, new.arcs);
    }

    #[test]
    fn absolute_annotation_updates_sta_delay() {
        let nl = mapped_and();
        let sdf_text = r#"
(DELAYFILE
  (SDFVERSION "3.0")
  (DESIGN "m")
  (TIMESCALE 1ps)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (500::600::700))
      (IOPATH B Y (800))
    ))
  )
)
"#;
        let sdf = parse_sdf(sdf_text).unwrap();
        let annotated = annotate_sdf(&nl, &sdf).unwrap();
        let id = annotated.library().unwrap().find("AND2__SDF__u1").unwrap();
        let def = annotated.library().unwrap().get(id);
        assert!((def.arcs[0].1 - 0.7).abs() < 1e-12);
        assert!((def.arcs[1].1 - 0.8).abs() < 1e-12);
    }

    #[test]
    fn interconnect_annotation_adds_to_absolute_iopath() {
        let nl = mapped_and();
        let text = r#"
(DELAYFILE
  (SDFVERSION "3.0")
  (DESIGN "m")
  (TIMESCALE 1ns)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (0.1::0.1::0.1))
      (INTERCONNECT a a (0.2::0.2::0.2))
    ))
  )
)
"#;
        let sdf = parse_sdf(text).unwrap();
        assert_eq!(sdf.interconnects.len(), 1);
        let annotated = annotate_sdf(&nl, &sdf).unwrap();
        let id = annotated.library().unwrap().find("AND2__SDF__u1").unwrap();
        let def = annotated.library().unwrap().get(id);
        assert!((def.arcs[0].1 - 0.3).abs() < 1e-12);
        assert!((def.arcs[1].1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn width_annotation_updates_min_pulse_requirements() {
        let nl = mapped_dff();
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "ff") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFF") (INSTANCE *)
    (TIMINGCHECK (WIDTH CLK (0.31::0.33)))
  )
)
"#;
        let sdf = parse_sdf(text).unwrap();
        assert_eq!(sdf.timing_checks[0].kind, TimingCheckKind::Width);
        let annotated = annotate_sdf(&nl, &sdf).unwrap();
        let id = annotated.library().unwrap().find("DFF__SDF__u1").unwrap();
        let seq = annotated.library().unwrap().get(id).seq.clone().unwrap();
        assert!((seq.min_pulse_high - 0.33).abs() < 1e-12);
        assert!((seq.min_pulse_low - 0.33).abs() < 1e-12);
    }

    #[test]
    fn recovery_removal_annotations_update_async_checks() {
        let nl = mapped_dffr();
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "ffr") (TIMESCALE 1ns)
  (CELL (CELLTYPE "DFFR") (INSTANCE *)
    (TIMINGCHECK
      (RECOVERY RST CLK (0.41::0.43))
      (REMOVAL RST CLK (0.16::0.18))
    )
  )
)
"#;
        let sdf = parse_sdf(text).unwrap();
        assert_eq!(sdf.timing_checks[0].kind, TimingCheckKind::Recovery);
        assert_eq!(sdf.timing_checks[1].kind, TimingCheckKind::Removal);
        let annotated = annotate_sdf(&nl, &sdf).unwrap();
        let id = annotated.library().unwrap().find("DFFR__SDF__u1").unwrap();
        let seq = annotated.library().unwrap().get(id).seq.clone().unwrap();
        assert!((seq.recovery - 0.43).abs() < 1e-12);
        assert!((seq.removal - 0.18).abs() < 1e-12);
    }

    #[test]
    fn write_delay_file_round_trips_all_annotation_kinds() {
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ps)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (100.5::110.25::120.125))
      (INTERCONNECT a b (10::11::12))
    ))
    (TIMINGCHECK
      (SETUP D CLK (50::51::52))
      (HOLD D CLK (10::11::12))
      (WIDTH CLK (20::21::22))
      (RECOVERY RST CLK (30::31::32))
      (REMOVAL RST CLK (5::6::7))
    )
  )
)
"#;
        let original = parse_sdf(text).unwrap();
        let exported = write_delay_file(&original).unwrap();
        let reparsed = parse_sdf(&exported).unwrap();
        assert_eq!(original, reparsed);
        assert!(exported.contains("(INTERCONNECT a b"));
    }

    #[test]
    fn serialized_interconnect_still_adds_to_iopath() {
        let nl = mapped_and();
        let text = r#"
(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
  (CELL (CELLTYPE "AND2") (INSTANCE u1)
    (DELAY (ABSOLUTE
      (IOPATH A Y (0.1::0.1::0.1))
      (INTERCONNECT a a (0.2::0.2::0.2))
    ))
  )
)
"#;
        let original = parse_sdf(text).unwrap();
        let exported = write_delay_file(&original).unwrap();
        let reparsed = parse_sdf(&exported).unwrap();
        let annotated = annotate_sdf(&nl, &reparsed).unwrap();
        let id = annotated.library().unwrap().find("AND2__SDF__u1").unwrap();
        let def = annotated.library().unwrap().get(id);
        assert!((def.arcs[0].1 - 0.3).abs() < 1e-12);
    }

    #[test]
    fn write_delay_file_rejects_invalid_values() {
        let mut sdf = parse_sdf(
            r#"(DELAYFILE (SDFVERSION "3.0") (DESIGN "m") (TIMESCALE 1ns)
               (CELL (INSTANCE *) (DELAY (ABSOLUTE (IOPATH a b (1::1)))))
              )"#,
        )
        .unwrap();
        sdf.iopaths[0].delay.rise = -1.0;
        assert!(write_delay_file(&sdf).is_err());
        sdf.iopaths[0].delay.rise = f64::NAN;
        assert!(write_delay_file(&sdf).is_err());
        sdf.iopaths.clear();
        assert!(write_delay_file(&sdf).is_err());
    }
}
