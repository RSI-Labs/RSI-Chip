//! # eda-spef — parasitic extraction exchange and RC estimation.
//!
//! Parses the practical single-line SPEF subset used by extraction tools:
//! header units, `D_NET`, connections, grounded/coupling capacitance, and
//! resistance. The analyzer reports per-net RC and can generate SDF
//! `INTERCONNECT` annotations for an existing mapped netlist.

use eda_core::netlist::Netlist;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpefUnits {
    pub time_seconds_per_unit: f64,
    pub capacitance_farads_per_unit: f64,
    pub resistance_ohms_per_unit: f64,
    pub inductance_henries_per_unit: f64,
}

impl Default for SpefUnits {
    fn default() -> Self {
        SpefUnits {
            time_seconds_per_unit: 1e-9,
            capacitance_farads_per_unit: 1e-12,
            resistance_ohms_per_unit: 1.0,
            inductance_henries_per_unit: 1e-12,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpefConnection {
    pub direction: String,
    pub instance: String,
    pub pin: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpefCap {
    pub id: usize,
    pub node1: String,
    pub node2: Option<String>,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpefRes {
    pub id: usize,
    pub node1: String,
    pub node2: String,
    pub value: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpefNet {
    pub name: String,
    pub total_capacitance: f64,
    pub connections: Vec<SpefConnection>,
    pub caps: Vec<SpefCap>,
    pub resistors: Vec<SpefRes>,
}

impl SpefNet {
    pub fn grounded_capacitance(&self, units: SpefUnits) -> f64 {
        self.caps
            .iter()
            .filter(|cap| cap.node2.is_none())
            .map(|cap| cap.value)
            .sum::<f64>()
            * units.capacitance_farads_per_unit
    }

    pub fn coupling_capacitance(&self, units: SpefUnits) -> f64 {
        self.caps
            .iter()
            .filter(|cap| cap.node2.is_some())
            .map(|cap| cap.value)
            .sum::<f64>()
            * units.capacitance_farads_per_unit
    }

    pub fn total_resistance(&self, units: SpefUnits) -> f64 {
        self.resistors
            .iter()
            .map(|resistor| resistor.value)
            .sum::<f64>()
            * units.resistance_ohms_per_unit
    }

    /// Conservative lumped RC estimate. `0.69 R C` is the first-order RC
    /// threshold delay; it is intentionally pessimistic for tree branches.
    pub fn estimated_delay_seconds(&self, units: SpefUnits) -> f64 {
        let capacitance = self.grounded_capacitance(units) + 0.5 * self.coupling_capacitance(units);
        0.69 * self.total_resistance(units) * capacitance
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spef {
    pub version: String,
    pub design: String,
    pub units: SpefUnits,
    pub nets: BTreeMap<String, SpefNet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetParasitics {
    pub net: String,
    pub grounded_cap_pf: f64,
    pub coupling_cap_pf: f64,
    pub resistance_ohm: f64,
    pub estimated_delay_ns: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParasiticReport {
    pub design: String,
    pub net_count: usize,
    pub unmatched_netlist_nets: Vec<String>,
    pub nets: Vec<NetParasitics>,
    pub total_grounded_cap_pf: f64,
    pub total_coupling_cap_pf: f64,
    pub total_resistance_ohm: f64,
    pub worst_delay_ns: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    None,
    Conn,
    Cap,
    Res,
}

pub fn parse_spef(source: &str) -> Result<Spef, String> {
    let mut spef = Spef {
        units: SpefUnits::default(),
        ..Default::default()
    };
    let mut current: Option<SpefNet> = None;
    let mut section = Section::None;

    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let keyword = fields[0];

        match keyword {
            "*SPEF" | "*VERSION" => {
                let value = join_value(&fields[1..]);
                spef.version = value;
            }
            "*DESIGN" => spef.design = unquote(&join_value(&fields[1..])),
            "*T_UNIT" => {
                spef.units.time_seconds_per_unit =
                    parse_unit_scale(fields.get(1), fields.get(2), line_number)?;
            }
            "*C_UNIT" => {
                spef.units.capacitance_farads_per_unit =
                    parse_unit_scale(fields.get(1), fields.get(2), line_number)?;
            }
            "*R_UNIT" => {
                spef.units.resistance_ohms_per_unit =
                    parse_unit_scale(fields.get(1), fields.get(2), line_number)?;
            }
            "*L_UNIT" => {
                spef.units.inductance_henries_per_unit =
                    parse_unit_scale(fields.get(1), fields.get(2), line_number)?;
            }
            "*D_NET" => {
                finish_net(&mut spef, current.take())?;
                if fields.len() != 3 {
                    return Err(format!("line {}: malformed *D_NET", line_number));
                }
                let total = parse_number(Some(&fields[2]), line_number)?;
                current = Some(SpefNet {
                    name: unquote(fields[1]),
                    total_capacitance: total,
                    ..Default::default()
                });
                section = Section::None;
            }
            "*CONN" => section = Section::Conn,
            "*CAP" => section = Section::Cap,
            "*RES" => section = Section::Res,
            "*END" => {
                finish_net(&mut spef, current.take())?;
                section = Section::None;
            }
            _ => parse_section_line(&mut current, section, &fields, line_number)?,
        }
    }
    finish_net(&mut spef, current.take())?;

    if spef.design.is_empty() {
        return Err("SPEF has no *DESIGN statement".into());
    }
    if spef.nets.is_empty() {
        return Err("SPEF has no *D_NET sections".into());
    }
    Ok(spef)
}

fn parse_section_line(
    current: &mut Option<SpefNet>,
    section: Section,
    fields: &[&str],
    line: usize,
) -> Result<(), String> {
    let Some(net) = current.as_mut() else {
        return Err(format!("line {}: parasitic entry outside *D_NET", line));
    };
    match section {
        Section::None => {
            // Header entries not needed by the analyzer are ignored.
            Ok(())
        }
        Section::Conn => {
            // *I instance:pin I *C x y *L 0 : D
            if fields.len() < 3 {
                return Err(format!("line {}: malformed *CONN entry", line));
            }
            let node = fields[1];
            let (instance, pin) = split_node(node, line)?;
            let mut x = None;
            let mut y = None;
            for (index, field) in fields.iter().enumerate() {
                if *field == "*C" {
                    x = Some(parse_number(fields.get(index + 1), line)?);
                    y = Some(parse_number(fields.get(index + 2), line)?);
                }
            }
            net.connections.push(SpefConnection {
                direction: fields[2].to_string(),
                instance,
                pin,
                x,
                y,
            });
            Ok(())
        }
        Section::Cap => {
            // id node1 [node2] value
            if fields.len() != 3 && fields.len() != 4 {
                return Err(format!("line {}: malformed capacitance entry", line));
            }
            let id = parse_index(fields[0], line)?;
            let value = parse_number(Some(&fields[fields.len() - 1]), line)?;
            if !value.is_finite() || value < 0.0 {
                return Err(format!(
                    "line {}: capacitance value must be finite and nonnegative",
                    line
                ));
            }
            let node2 = if fields.len() == 4 {
                Some(unquote(fields[2]))
            } else {
                None
            };
            net.caps.push(SpefCap {
                id,
                node1: unquote(fields[1]),
                node2,
                value,
            });
            Ok(())
        }
        Section::Res => {
            if fields.len() != 4 {
                return Err(format!("line {}: malformed resistance entry", line));
            }
            net.resistors.push(SpefRes {
                id: parse_index(fields[0], line)?,
                node1: unquote(fields[1]),
                node2: unquote(fields[2]),
                value: parse_number(Some(&fields[3]), line)?,
            });
            let resistor = net.resistors.last().expect("resistor just inserted");
            if !resistor.value.is_finite() || resistor.value < 0.0 {
                return Err(format!(
                    "line {}: resistance value must be finite and nonnegative",
                    line
                ));
            }
            Ok(())
        }
    }
}

fn finish_net(spef: &mut Spef, net: Option<SpefNet>) -> Result<(), String> {
    let Some(net) = net else {
        return Ok(());
    };
    if net.name.is_empty() {
        return Err("SPEF net has no name".into());
    }
    if !net.total_capacitance.is_finite() || net.total_capacitance < 0.0 {
        return Err(format!(
            "SPEF net '{}' has invalid total capacitance",
            net.name
        ));
    }
    let mut capacitor_ids = BTreeSet::new();
    for capacitor in &net.caps {
        if capacitor.node1.trim().is_empty()
            || capacitor.node2.as_deref().is_some_and(str::is_empty)
        {
            return Err(format!(
                "SPEF net '{}' has an empty capacitor node",
                net.name
            ));
        }
        if !capacitor_ids.insert(capacitor.id) {
            return Err(format!(
                "SPEF net '{}' repeats capacitor id {}",
                net.name, capacitor.id
            ));
        }
    }
    let mut resistor_ids = BTreeSet::new();
    for resistor in &net.resistors {
        if resistor.node1.trim().is_empty() || resistor.node2.trim().is_empty() {
            return Err(format!(
                "SPEF net '{}' has an empty resistor node",
                net.name
            ));
        }
        if !resistor_ids.insert(resistor.id) {
            return Err(format!(
                "SPEF net '{}' repeats resistor id {}",
                net.name, resistor.id
            ));
        }
    }
    let name = net.name.clone();
    if spef.nets.insert(name.clone(), net).is_some() {
        return Err(format!("duplicate SPEF net '{}'", name));
    }
    Ok(())
}

pub fn analyze(nl: &Netlist, spef: &Spef) -> Result<ParasiticReport, String> {
    let mut nets = Vec::new();
    let mut unmatched = Vec::new();
    for ni in 0..nl.net_count() {
        let name = &nl.net(ni).name;
        let Some(parasitic) = spef.nets.get(name) else {
            unmatched.push(name.clone());
            continue;
        };
        let grounded = clean_zero(farads_to_pf(parasitic.grounded_capacitance(spef.units)));
        let coupling = clean_zero(farads_to_pf(parasitic.coupling_capacitance(spef.units)));
        let resistance = clean_zero(parasitic.total_resistance(spef.units));
        let delay = clean_zero(seconds_to_ns(parasitic.estimated_delay_seconds(spef.units)));
        nets.push(NetParasitics {
            net: name.clone(),
            grounded_cap_pf: grounded,
            coupling_cap_pf: coupling,
            resistance_ohm: resistance,
            estimated_delay_ns: delay,
        });
    }
    nets.sort_by(|a, b| {
        a.net
            .cmp(&b.net)
            .then(a.estimated_delay_ns.total_cmp(&b.estimated_delay_ns))
    });
    let total_grounded = nets.iter().map(|net| net.grounded_cap_pf).sum();
    let total_coupling = nets.iter().map(|net| net.coupling_cap_pf).sum();
    let total_resistance = nets.iter().map(|net| net.resistance_ohm).sum();
    let worst = nets
        .iter()
        .map(|net| net.estimated_delay_ns)
        .fold(0.0, f64::max);
    Ok(ParasiticReport {
        design: spef.design.clone(),
        net_count: nets.len(),
        unmatched_netlist_nets: unmatched,
        nets,
        total_grounded_cap_pf: total_grounded,
        total_coupling_cap_pf: total_coupling,
        total_resistance_ohm: total_resistance,
        worst_delay_ns: worst,
    })
}

/// Serialize a SPEF object using the practical subset understood by the parser.
pub fn write_spef(spef: &Spef) -> Result<String, String> {
    if spef.design.trim().is_empty() {
        return Err("cannot write SPEF without a design name".into());
    }
    let finite = |value: f64| value.is_finite();
    if !finite(spef.units.time_seconds_per_unit)
        || !finite(spef.units.capacitance_farads_per_unit)
        || !finite(spef.units.resistance_ohms_per_unit)
        || !finite(spef.units.inductance_henries_per_unit)
    {
        return Err("cannot write SPEF with non-finite units".into());
    }
    if spef.nets.is_empty() {
        return Err("cannot write SPEF without nets".into());
    }
    if spef.units.time_seconds_per_unit <= 0.0
        || spef.units.capacitance_farads_per_unit <= 0.0
        || spef.units.resistance_ohms_per_unit <= 0.0
        || spef.units.inductance_henries_per_unit <= 0.0
    {
        return Err("cannot write SPEF with nonpositive units".into());
    }
    let capacitance_output_scale = spef.units.capacitance_farads_per_unit / 1e-12;
    let resistance_output_scale = spef.units.resistance_ohms_per_unit;
    let capacitance_to_pf = |value: f64| value * capacitance_output_scale;
    let resistance_to_ohm = |value: f64| value * resistance_output_scale;

    let mut out = String::new();
    out.push_str("*SPEF \"IEEE 1481-1998\"\n");
    out.push_str(&format!("*DESIGN \"{}\"\n", escape_quote(&spef.design)));
    out.push_str("*T_UNIT 1 NS\n");
    out.push_str("*C_UNIT 1 PF\n");
    out.push_str("*R_UNIT 1 OHM\n");
    out.push_str("*L_UNIT 1 HENRY\n\n");

    for net in spef.nets.values() {
        if net.name.trim().is_empty() {
            return Err("cannot write unnamed SPEF net".into());
        }
        if !finite(net.total_capacitance)
            || net.caps.iter().any(|cap| !finite(cap.value))
            || net.resistors.iter().any(|resistor| !finite(resistor.value))
        {
            return Err(format!("net '{}' has non-finite parasitics", net.name));
        }
        out.push_str(&format!(
            "*D_NET \"{}\" {:.12}\n",
            escape_quote(&net.name),
            capacitance_to_pf(net.total_capacitance)
        ));
        if !net.connections.is_empty() {
            out.push_str("*CONN\n");
            for connection in &net.connections {
                let node = if connection.instance.trim().is_empty() {
                    connection.pin.clone()
                } else {
                    format!("{}:{}", connection.instance, connection.pin)
                };
                out.push_str(&format!(
                    "*I {} {}",
                    escape_quote(&node),
                    connection.direction
                ));
                if let (Some(x), Some(y)) = (connection.x, connection.y) {
                    if !finite(x) || !finite(y) {
                        return Err(format!(
                            "net '{}' has non-finite connection coordinates",
                            net.name
                        ));
                    }
                    out.push_str(&format!(" *C {x:.12} {y:.12}"));
                }
                out.push('\n');
            }
        }
        if !net.caps.is_empty() {
            out.push_str("*CAP\n");
            for cap in &net.caps {
                match &cap.node2 {
                    Some(node2) => out.push_str(&format!(
                        "{} {} {} {:.12}\n",
                        cap.id,
                        quote(&cap.node1),
                        quote(node2),
                        capacitance_to_pf(cap.value)
                    )),
                    None => out.push_str(&format!(
                        "{} {} {:.12}\n",
                        cap.id,
                        quote(&cap.node1),
                        capacitance_to_pf(cap.value)
                    )),
                }
            }
        }
        if !net.resistors.is_empty() {
            out.push_str("*RES\n");
            for resistor in &net.resistors {
                out.push_str(&format!(
                    "{} {} {} {:.12}\n",
                    resistor.id,
                    quote(&resistor.node1),
                    quote(&resistor.node2),
                    resistance_to_ohm(resistor.value)
                ));
            }
        }
        out.push_str("*END\n\n");
    }
    Ok(out)
}

fn escape_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn quote(value: &str) -> String {
    format!("\"{}\"", escape_quote(value))
}

pub fn report(result: &ParasiticReport) -> String {
    let mut out = format!(
        "SPEF report: design '{}', {} parasitic net(s), {:.6} pF grounded, {:.6} pF coupling, {:.3} ohm, worst RC {:.6} ns\n",
        result.design,
        result.net_count,
        result.total_grounded_cap_pf,
        result.total_coupling_cap_pf,
        result.total_resistance_ohm,
        result.worst_delay_ns
    );
    for net in result.nets.iter().take(30) {
        out.push_str(&format!(
            "  NET {} Cg={:.6}pF Cc={:.6}pF R={:.3}ohm RC={:.6}ns\n",
            net.net,
            net.grounded_cap_pf,
            net.coupling_cap_pf,
            net.resistance_ohm,
            net.estimated_delay_ns
        ));
    }
    if !result.unmatched_netlist_nets.is_empty() {
        out.push_str(&format!(
            "  unmatched netlist nets: {}\n",
            result.unmatched_netlist_nets.join(",")
        ));
    }
    out
}

/// Generate SDF `INTERCONNECT` annotations from conservative SPEF RC delays.
pub fn write_sdf(nl: &Netlist, spef: &Spef) -> Result<String, String> {
    let mut out = String::new();
    out.push_str("(DELAYFILE\n");
    out.push_str("  (SDFVERSION \"3.0\")\n");
    out.push_str(&format!("  (DESIGN \"{}\")\n", nl.name));
    out.push_str("  (TIMESCALE 1ns)\n");
    let mut entries = 0usize;
    for ni in 0..nl.net_count() {
        let name = &nl.net(ni).name;
        let Some(parasitic) = spef.nets.get(name) else {
            continue;
        };
        let delay = seconds_to_ns(parasitic.estimated_delay_seconds(spef.units));
        if delay <= 0.0 {
            continue;
        }
        for &sink in &nl.net(ni).sinks {
            for &input in &nl.cell(sink).inputs {
                if input == ni {
                    out.push_str(&format!(
                        "  (CELL (INSTANCE *) (DELAY (ABSOLUTE (INTERCONNECT {name} {name} ({:.9}::{:.9})))))\n",
                        delay, delay
                    ));
                    entries += 1;
                }
            }
        }
    }
    if entries == 0 {
        return Err("SPEF produced no positive interconnect delays".into());
    }
    out.push_str(")\n");
    Ok(out)
}

fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(position) => &line[..position],
        None => line,
    }
}

fn join_value(fields: &[&str]) -> String {
    fields.join(" ")
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let Some(inner) = trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return trimmed.trim_matches('"').to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut escaped = false;
    for character in inner.chars() {
        if escaped {
            out.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            out.push(character);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

fn split_node(node: &str, line: usize) -> Result<(String, String), String> {
    match node.rfind(':') {
        Some(position) => Ok((unquote(&node[..position]), unquote(&node[position + 1..]))),
        None => {
            let _ = line;
            Ok((String::new(), unquote(node)))
        }
    }
}

fn parse_number(value: Option<&&str>, line: usize) -> Result<f64, String> {
    let Some(value) = value else {
        return Err(format!("line {}: missing numeric value", line));
    };
    value
        .parse::<f64>()
        .map_err(|_| format!("line {}: invalid number '{}'", line, value))
}

fn parse_index(value: &str, line: usize) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("line {}: invalid index '{}'", line, value))
}

fn parse_unit_scale(
    magnitude: Option<&&str>,
    unit: Option<&&str>,
    line: usize,
) -> Result<f64, String> {
    let magnitude = parse_number(magnitude, line)?;
    let Some(unit) = unit else {
        return Err(format!("line {}: unit statement lacks scale", line));
    };
    let multiplier = match *unit {
        "FS" | "fs" => 1e-15,
        "PS" | "ps" => 1e-12,
        "NS" | "ns" => 1e-9,
        "US" | "us" => 1e-6,
        "MS" | "ms" => 1e-3,
        "S" | "s" => 1.0,
        "FF" | "ff" => 1e-15,
        "PF" | "pf" => 1e-12,
        "NF" | "nf" => 1e-9,
        "UF" | "uf" => 1e-6,
        "MF" | "mf" => 1e-3,
        "F" | "f" => 1.0,
        "MOHM" | "mohm" => 1e6,
        "KOHM" | "kohm" => 1e3,
        "OHM" | "ohm" => 1.0,
        "NH" | "nh" => 1e-9,
        "UH" | "uh" => 1e-6,
        "MH" | "mh" => 1e-3,
        "HENRY" | "henry" => 1.0,
        other => return Err(format!("line {}: unsupported unit '{}'", line, other)),
    };
    let value = magnitude * multiplier;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("line {}: unit scale must be positive", line));
    }
    Ok(value)
}

fn farads_to_pf(value: f64) -> f64 {
    value * 1e12
}

fn seconds_to_ns(value: f64) -> f64 {
    value * 1e9
}

fn clean_zero(value: f64) -> f64 {
    if value.abs() < 1e-15 {
        0.0
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::netlist::{Cell, CellKind, Dir};

    const SPEF: &str = r#"
*SPEF "IEEE 1481-1998"
*DESIGN "top"
*T_UNIT 1 NS
*C_UNIT 1 PF
*R_UNIT 1 OHM
*D_NET a 0.012
*CONN
*P a I
*CAP
1 a 0.010
*RES
1 a a:1 12.5
*END
*D_NET y 0.006
*CONN
*I u1:Y O
*CAP
1 y 0.006
2 y u2:A 0.004
*RES
1 y u1:Y 25.0
*END
"#;

    #[test]
    fn parses_units_nets_and_parasitics() {
        let spef = parse_spef(SPEF).unwrap();
        assert_eq!(spef.design, "top");
        assert_eq!(spef.nets.len(), 2);
        assert_eq!(spef.nets["a"].resistors.len(), 1);
        assert_eq!(spef.nets["y"].caps.len(), 2);
    }

    #[test]
    fn estimates_rc_delay() {
        let spef = parse_spef(SPEF).unwrap();
        let net = &spef.nets["a"];
        let delay = net.estimated_delay_seconds(spef.units);
        assert!((delay - 0.69 * 12.5 * 0.010e-12).abs() < 1e-18);
    }

    #[test]
    fn writes_and_re_parses_parasitics() {
        let original = parse_spef(SPEF).unwrap();
        let text = write_spef(&original).unwrap();
        let reparsed = parse_spef(&text).unwrap();
        assert_eq!(reparsed.design, original.design);
        assert_eq!(reparsed.nets.len(), original.nets.len());
        for (name, original_net) in &original.nets {
            let reparsed_net = &reparsed.nets[name];
            assert_eq!(
                reparsed_net.connections.len(),
                original_net.connections.len()
            );
            assert_eq!(reparsed_net.caps.len(), original_net.caps.len());
            assert_eq!(reparsed_net.resistors.len(), original_net.resistors.len());
            assert!(
                (reparsed_net.grounded_capacitance(reparsed.units)
                    - original_net.grounded_capacitance(original.units))
                .abs()
                    < 1e-24
            );
            assert!(
                (reparsed_net.total_resistance(reparsed.units)
                    - original_net.total_resistance(original.units))
                .abs()
                    < 1e-9
            );
        }
    }

    #[test]
    fn writing_nondefault_units_converts_physical_quantities() {
        let source = r#"
*SPEF "IEEE 1481-1998"
*DESIGN "units"
*T_UNIT 1 PS
*C_UNIT 2 FF
*R_UNIT 2 KOHM
*D_NET converted 6
*CONN
*I u:A I *C 1.25 -2.5
*CAP
1 converted 2
2 converted neighbour 4
*RES
1 converted u:A 5
*END
"#;
        let original = parse_spef(source).unwrap();
        assert_eq!(original.units.capacitance_farads_per_unit, 2e-15);
        assert_eq!(original.units.resistance_ohms_per_unit, 2e3);
        assert_eq!(original.nets["converted"].connections[0].x, Some(1.25));
        assert_eq!(original.nets["converted"].connections[0].y, Some(-2.5));

        let text = write_spef(&original).unwrap();
        assert!(text.contains("*T_UNIT 1 NS"), "{text}");
        assert!(text.contains("*C_UNIT 1 PF"), "{text}");
        assert!(text.contains("*R_UNIT 1 OHM"), "{text}");
        let round = parse_spef(&text).unwrap();
        let net = &round.nets["converted"];
        assert_eq!(net.total_capacitance, 0.012);
        assert_eq!(net.caps[0].value, 0.004);
        assert_eq!(net.caps[1].value, 0.008);
        assert_eq!(net.resistors[0].value, 10_000.0);
        assert!((net.grounded_capacitance(round.units) - 4e-15).abs() < 1e-24);
        assert!((net.coupling_capacitance(round.units) - 8e-15).abs() < 1e-24);
        assert_eq!(net.total_resistance(round.units), 10_000.0);
        assert_eq!(net.connections[0].x, Some(1.25));
        assert_eq!(net.connections[0].y, Some(-2.5));
        assert!(
            (net.estimated_delay_seconds(round.units)
                - original.nets["converted"].estimated_delay_seconds(original.units))
            .abs()
                < 1e-21
        );
    }

    #[test]
    fn escaped_design_and_net_names_round_trip() {
        let mut spef = Spef {
            version: "test".into(),
            design: "top\\one".into(),
            units: SpefUnits::default(),
            nets: BTreeMap::new(),
        };
        spef.nets.insert(
            "bus\\a".into(),
            SpefNet {
                name: "bus\\a".into(),
                total_capacitance: 0.001,
                connections: Vec::new(),
                caps: vec![SpefCap {
                    id: 1,
                    node1: "bus\\a".into(),
                    node2: None,
                    value: 0.001,
                }],
                resistors: Vec::new(),
            },
        );
        let round = parse_spef(&write_spef(&spef).unwrap()).unwrap();
        assert_eq!(round.design, "top\\one");
        assert!(round.nets.contains_key("bus\\a"));
        assert_eq!(round.nets["bus\\a"].caps[0].node1, "bus\\a");
    }

    #[test]
    fn analyze_reports_totals_unmatched_nets_and_worst_delay() {
        let mut nl = Netlist::new("top");
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

        let spef = parse_spef(SPEF).unwrap();
        let result = analyze(&nl, &spef).unwrap();
        assert_eq!(result.design, "top");
        assert_eq!(result.net_count, 2);
        assert_eq!(result.unmatched_netlist_nets, vec!["b".to_string()]);
        assert_eq!(result.nets.len(), 2);
        assert!((result.total_grounded_cap_pf - 0.016).abs() < 1e-12);
        assert!((result.total_coupling_cap_pf - 0.004).abs() < 1e-12);
        assert!((result.total_resistance_ohm - 37.5).abs() < 1e-12);
        assert!((result.worst_delay_ns - 0.000138).abs() < 1e-12);
        let text = report(&result);
        assert!(text.contains("unmatched netlist nets: b"), "{text}");
        assert!(text.contains("NET a"), "{text}");
    }

    #[test]
    fn writes_sdf_interconnect_annotations_for_spef_nets() {
        let mut nl = Netlist::new("top");
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
        let spef = parse_spef(SPEF).unwrap();
        let sdf = write_sdf(&nl, &spef).unwrap();
        assert!(sdf.contains("(DELAYFILE"), "{sdf}");
        assert!(sdf.contains("(TIMESCALE 1ns)"), "{sdf}");
        assert!(sdf.contains("(INTERCONNECT a a ("), "{sdf}");
    }

    #[test]
    fn rejects_malformed_or_inconsistent_spef() {
        assert_eq!(
            parse_spef("*D_NET a 1\n*END\n").unwrap_err(),
            "SPEF has no *DESIGN statement"
        );
        assert_eq!(
            parse_spef("*DESIGN top\n").unwrap_err(),
            "SPEF has no *D_NET sections"
        );
        assert!(parse_spef("*DESIGN top\nnoise\n*D_NET a 1\n*END\n")
            .unwrap_err()
            .contains("outside *D_NET"));
        assert!(parse_spef("*DESIGN top\n*D_NET a NaN\n*END\n")
            .unwrap_err()
            .contains("invalid total capacitance"));
        assert!(parse_spef("*DESIGN top\n*D_NET a 1\n*CAP\n1 a -1\n*END\n")
            .unwrap_err()
            .contains("capacitance value"));
        assert!(
            parse_spef("*DESIGN top\n*D_NET a 1\n*RES\n1 a b -1\n*END\n")
                .unwrap_err()
                .contains("resistance value")
        );
        assert!(
            parse_spef("*DESIGN top\n*D_NET a 1\n*END\n*D_NET a 1\n*END\n")
                .unwrap_err()
                .contains("duplicate SPEF net")
        );
        assert!(
            parse_spef("*DESIGN top\n*D_NET a 1\n*CAP\n1 a 1\n1 a 1\n*END\n")
                .unwrap_err()
                .contains("repeats capacitor id")
        );
        assert!(
            parse_spef("*DESIGN top\n*D_NET a 1\n*RES\n1 a b 1\n1 a b 1\n*END\n")
                .unwrap_err()
                .contains("repeats resistor id")
        );
        assert!(parse_spef("*DESIGN top\n*T_UNIT 0 NS\n*D_NET a 1\n*END\n")
            .unwrap_err()
            .contains("unit scale must be positive"));
        assert!(
            parse_spef("*DESIGN top\n*T_UNIT 1 YEARS\n*D_NET a 1\n*END\n")
                .unwrap_err()
                .contains("unsupported unit")
        );
    }

    #[test]
    fn rejects_invalid_spef_objects_before_writing() {
        let mut spef = parse_spef(SPEF).unwrap();
        spef.design = " ".into();
        assert_eq!(
            write_spef(&spef).unwrap_err(),
            "cannot write SPEF without a design name"
        );

        let mut spef = parse_spef(SPEF).unwrap();
        spef.nets.clear();
        assert_eq!(
            write_spef(&spef).unwrap_err(),
            "cannot write SPEF without nets"
        );

        let mut spef = parse_spef(SPEF).unwrap();
        spef.units.capacitance_farads_per_unit = 0.0;
        assert_eq!(
            write_spef(&spef).unwrap_err(),
            "cannot write SPEF with nonpositive units"
        );

        let mut spef = parse_spef(SPEF).unwrap();
        spef.nets.get_mut("a").unwrap().caps[0].value = f64::NAN;
        assert!(write_spef(&spef)
            .unwrap_err()
            .contains("non-finite parasitics"));
    }
}
