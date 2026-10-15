//! SDC tokenizer, command parser, units normalization, and typed object lists.

use super::*;

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

pub(super) fn upsert_scoped_value(
    values: &mut Vec<ScopedQualityLimit>,
    object: TimingObjectRef,
    value: f64,
) {
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
    let setup = tokens.iter().any(|token| token == "-setup");
    let hold = tokens.iter().any(|token| token == "-hold");
    if setup && hold {
        return Err("path exception cannot select both -setup and -hold".into());
    }
    let check = if setup {
        PathExceptionCheck::Setup
    } else if hold {
        PathExceptionCheck::Hold
    } else {
        PathExceptionCheck::Both
    };
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
            check,
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
            check,
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

pub(super) fn validate_derate(
    early: Option<f64>,
    late: Option<f64>,
    what: &str,
) -> Result<(), String> {
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
