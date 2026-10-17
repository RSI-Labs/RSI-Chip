//! # eda-upf — power-intent subset and static checks.
//!
//! The reader accepts a compact UPF 2.x command subset and checks it against
//! an elaborated design: domains, supply nets/ports, primary supplies, power
//! switches, isolation, retention, and level shifting.
//! Named power modes can describe a complete switch-state table for
//! multi-mode static power and IR analysis.

use eda_core::ir::Design;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PowerDomain {
    pub name: String,
    pub elements: Vec<String>,
    pub primary_power_net: Option<String>,
    pub primary_ground_net: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupplyPort {
    pub name: String,
    pub domain: String,
    pub direction: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupplyNet {
    pub name: String,
    pub domain: String,
    pub connected_ports: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PowerSwitch {
    pub name: String,
    pub domain: String,
    pub input_supply: String,
    pub output_supply: String,
    pub controls: Vec<String>,
    /// Optional on-state resistance in ohms.
    ///
    /// A switch without an explicit resistance is structurally valid but is
    /// not yet included in the electrical IR network.
    pub on_resistance_ohm: Option<f64>,
    /// Optional off-state leakage current in amperes.
    pub off_leakage_a: Option<f64>,
    /// Explicit switch state: `true` is on, `false` is off.
    ///
    /// `None` means the mode was not specified; static IR treats it as on.
    pub state: Option<bool>,
}

/// One named multi-switch operating mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerMode {
    pub name: String,
    pub switch_states: BTreeMap<String, bool>,
    /// Per-domain dynamic-activity multipliers for this mode.
    pub domain_activity: BTreeMap<String, f64>,
    /// Mode overrides for strategy static currents.
    ///
    /// Keys are created by [`power_mode_strategy_key`].
    pub strategy_currents: BTreeMap<String, f64>,
}

/// Stable key for a mode-specific isolation/retention/level-shifter current.
pub fn power_mode_strategy_key(kind: &str, name: &str) -> String {
    format!("{}:{}", kind, name)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Isolation {
    pub name: String,
    pub domain: String,
    pub signal: String,
    pub isolation_power_net: Option<String>,
    pub isolation_ground_net: Option<String>,
    /// Optional static current drawn by the isolation cell, in amperes.
    pub static_current_a: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Retention {
    pub name: String,
    pub domain: String,
    pub signal: String,
    pub retention_power_net: Option<String>,
    pub retention_ground_net: Option<String>,
    /// Optional always-on retention current, in amperes.
    pub static_current_a: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LevelShifter {
    pub name: String,
    pub domain: String,
    pub input_supply: String,
    pub output_supply: String,
    pub location: String,
    /// Optional static current drawn by the level shifter, in amperes.
    pub static_current_a: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Upf {
    pub version: String,
    pub design_top: Option<String>,
    pub domains: BTreeMap<String, PowerDomain>,
    pub supply_ports: BTreeMap<String, SupplyPort>,
    pub supply_nets: BTreeMap<String, SupplyNet>,
    pub switches: BTreeMap<String, PowerSwitch>,
    pub power_modes: BTreeMap<String, PowerMode>,
    pub active_power_mode: Option<String>,
    pub isolations: BTreeMap<String, Isolation>,
    pub retentions: BTreeMap<String, Retention>,
    pub level_shifters: BTreeMap<String, LevelShifter>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpfFinding {
    pub code: String,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpfResult {
    pub domain_count: usize,
    pub supply_net_count: usize,
    pub switch_count: usize,
    pub power_mode_count: usize,
    pub isolation_count: usize,
    pub retention_count: usize,
    pub level_shifter_count: usize,
    pub findings: Vec<UpfFinding>,
}

impl UpfResult {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Error)
    }
}

/// Parse a compact UPF command subset. Commands may be separated by newlines
/// or semicolons; `{ ... }` lists are treated as a single argument.
pub fn parse_upf(source: &str) -> Result<Upf, String> {
    let tokens = lex(source)?;
    let mut upf = Upf::default();
    let mut pos = 0;
    while pos < tokens.len() {
        let Token::Word(command, line) = tokens[pos].clone() else {
            if matches!(&tokens[pos], Token::Punct(p, _) if p == ";") {
                pos += 1;
                continue;
            }
            return Err(format!("line {}: expected UPF command", tokens[pos].line()));
        };
        pos += 1;
        let mut end = pos;
        while end < tokens.len()
            && !tokens[end].is_command_start()
            && !matches!(&tokens[end], Token::Word(word, _) if is_upf_command(word))
        {
            end += 1;
        }
        let args = &tokens[pos..end];
        apply_command(&mut upf, &command, args, line)?;
        pos = end;
    }
    if upf.domains.is_empty() {
        return Err("UPF has no create_power_domain command".into());
    }
    Ok(upf)
}

fn apply_command(upf: &mut Upf, command: &str, args: &[Token], line: usize) -> Result<(), String> {
    match command {
        "upf_version" => {
            upf.version = arg_word(args, 0, line)?;
            Ok(())
        }
        "set_design_top" => {
            upf.design_top = Some(arg_word(args, 0, line)?);
            Ok(())
        }
        "create_power_domain" => {
            let name = arg_word(args, 0, line)?;
            let elements = opt_list(args, "-elements");
            let mut unique_elements = BTreeSet::new();
            for element in &elements {
                if !unique_elements.insert(element.as_str()) {
                    return Err(format!(
                        "line {}: power domain '{}' repeats element '{}'",
                        line, name, element
                    ));
                }
            }
            let domain = PowerDomain {
                name: name.clone(),
                elements,
                ..Default::default()
            };
            if upf.domains.insert(name.clone(), domain).is_some() {
                return Err(format!("line {}: duplicate power domain '{}'", line, name));
            }
            Ok(())
        }
        "create_supply_port" => {
            let name = arg_word(args, 0, line)?;
            let domain = option_word(args, "-domain")
                .ok_or_else(|| format!("line {}: supply port requires -domain", line))?;
            let direction = option_word(args, "-direction");
            if let Some(direction) = &direction {
                if !matches!(direction.as_str(), "in" | "out" | "inout") {
                    return Err(format!(
                        "line {}: supply port direction must be in, out, or inout",
                        line
                    ));
                }
            }
            let port = SupplyPort {
                name: name.clone(),
                domain,
                direction,
            };
            if upf.supply_ports.insert(name.clone(), port).is_some() {
                return Err(format!("line {}: duplicate supply port '{}'", line, name));
            }
            Ok(())
        }
        "create_supply_net" => {
            let name = arg_word(args, 0, line)?;
            let domain = option_word(args, "-domain")
                .ok_or_else(|| format!("line {}: supply net requires -domain", line))?;
            let net = SupplyNet {
                name: name.clone(),
                domain,
                connected_ports: Vec::new(),
            };
            if upf.supply_nets.insert(name.clone(), net).is_some() {
                return Err(format!("line {}: duplicate supply net '{}'", line, name));
            }
            Ok(())
        }
        "connect_supply_net" => {
            let name = arg_word(args, 0, line)?;
            let ports = opt_list(args, "-ports");
            let net = upf
                .supply_nets
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown supply net '{}'", line, name))?;
            net.connected_ports.extend(ports);
            net.connected_ports.sort();
            net.connected_ports.dedup();
            Ok(())
        }
        "set_domain_supply_net" => {
            let domain_name = arg_word(args, 0, line)?;
            let domain = upf
                .domains
                .get_mut(&domain_name)
                .ok_or_else(|| format!("line {}: unknown power domain '{}'", line, domain_name))?;
            domain.primary_power_net = option_word(args, "-primary_power_net");
            domain.primary_ground_net = option_word(args, "-primary_ground_net");
            Ok(())
        }
        "create_power_switch" => {
            let name = arg_word(args, 0, line)?;
            let domain = option_word(args, "-domain")
                .ok_or_else(|| format!("line {}: switch requires -domain", line))?;
            let input_supply = option_word(args, "-input_supply_port")
                .ok_or_else(|| format!("line {}: switch requires -input_supply_port", line))?;
            let output_supply = option_word(args, "-output_supply_port")
                .ok_or_else(|| format!("line {}: switch requires -output_supply_port", line))?;
            let switch = PowerSwitch {
                name: name.clone(),
                domain,
                input_supply,
                output_supply,
                controls: opt_list(args, "-control_port"),
                on_resistance_ohm: None,
                off_leakage_a: None,
                state: None,
            };
            if upf.switches.insert(name.clone(), switch).is_some() {
                return Err(format!("line {}: duplicate power switch '{}'", line, name));
            }
            Ok(())
        }
        "set_power_switch_on_resistance" => {
            let name = arg_word(args, 0, line)?;
            let resistance_text = arg_word(args, 1, line)?;
            let resistance = resistance_text.parse::<f64>().map_err(|_| {
                format!(
                    "line {}: invalid switch resistance '{}'",
                    line, resistance_text
                )
            })?;
            if !resistance.is_finite() || resistance <= 0.0 {
                return Err(format!(
                    "line {}: switch resistance must be finite and positive",
                    line
                ));
            }
            let switch = upf
                .switches
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown power switch '{}'", line, name))?;
            if switch.on_resistance_ohm.replace(resistance).is_some() {
                return Err(format!(
                    "line {}: power switch '{}' resistance already set",
                    line, name
                ));
            }
            Ok(())
        }
        "set_power_switch_state" => {
            let name = arg_word(args, 0, line)?;
            let state_text = arg_word(args, 1, line)?;
            let state = match state_text.as_str() {
                "on" => true,
                "off" => false,
                _ => {
                    return Err(format!(
                        "line {}: power switch state must be 'on' or 'off', found '{}'",
                        line, state_text
                    ))
                }
            };
            let switch = upf
                .switches
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown power switch '{}'", line, name))?;
            if switch.state.replace(state).is_some() {
                return Err(format!(
                    "line {}: power switch '{}' state already set",
                    line, name
                ));
            }
            Ok(())
        }
        "set_power_switch_off_leakage" => {
            let name = arg_word(args, 0, line)?;
            let leakage_text = arg_word(args, 1, line)?;
            let leakage = leakage_text
                .parse::<f64>()
                .map_err(|_| format!("line {}: invalid switch leakage '{}'", line, leakage_text))?;
            if !leakage.is_finite() || leakage < 0.0 {
                return Err(format!(
                    "line {}: switch leakage must be finite and nonnegative",
                    line
                ));
            }
            let switch = upf
                .switches
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown power switch '{}'", line, name))?;
            if switch.off_leakage_a.replace(leakage).is_some() {
                return Err(format!(
                    "line {}: power switch '{}' leakage already set",
                    line, name
                ));
            }
            Ok(())
        }
        "create_power_mode" => {
            let name = arg_word(args, 0, line)?;
            if upf
                .power_modes
                .insert(
                    name.clone(),
                    PowerMode {
                        name: name.clone(),
                        switch_states: BTreeMap::new(),
                        domain_activity: BTreeMap::new(),
                        strategy_currents: BTreeMap::new(),
                    },
                )
                .is_some()
            {
                return Err(format!("line {}: duplicate power mode '{}'", line, name));
            }
            Ok(())
        }
        "set_power_mode_switch" => {
            let mode_name = arg_word(args, 0, line)?;
            let switch_name = arg_word(args, 1, line)?;
            let state_text = arg_word(args, 2, line)?;
            let state = match state_text.as_str() {
                "on" => true,
                "off" => false,
                _ => {
                    return Err(format!(
                        "line {}: power-mode switch state must be 'on' or 'off', found '{}'",
                        line, state_text
                    ))
                }
            };
            if !upf.switches.contains_key(&switch_name) {
                return Err(format!(
                    "line {}: power mode '{}' references unknown switch '{}'",
                    line, mode_name, switch_name
                ));
            }
            let Some(mode) = upf.power_modes.get_mut(&mode_name) else {
                return Err(format!("line {}: unknown power mode '{}'", line, mode_name));
            };
            if mode
                .switch_states
                .insert(switch_name.clone(), state)
                .is_some()
            {
                return Err(format!(
                    "line {}: power mode '{}' sets switch '{}' more than once",
                    line, mode_name, switch_name
                ));
            }
            Ok(())
        }
        "set_power_mode_domain_activity" => {
            let mode_name = arg_word(args, 0, line)?;
            let domain_name = arg_word(args, 1, line)?;
            let factor_text = arg_word(args, 2, line)?;
            let factor = factor_text
                .parse::<f64>()
                .map_err(|_| format!("line {}: invalid activity factor '{}'", line, factor_text))?;
            if !factor.is_finite() || factor < 0.0 {
                return Err(format!(
                    "line {}: activity factor must be finite and nonnegative",
                    line
                ));
            }
            if !upf.domains.contains_key(&domain_name) {
                return Err(format!(
                    "line {}: power mode '{}' references unknown domain '{}'",
                    line, mode_name, domain_name
                ));
            }
            let Some(mode) = upf.power_modes.get_mut(&mode_name) else {
                return Err(format!("line {}: unknown power mode '{}'", line, mode_name));
            };
            if mode
                .domain_activity
                .insert(domain_name.clone(), factor)
                .is_some()
            {
                return Err(format!(
                    "line {}: power mode '{}' sets activity for domain '{}' more than once",
                    line, mode_name, domain_name
                ));
            }
            Ok(())
        }
        "set_power_mode_strategy_current" => {
            let mode_name = arg_word(args, 0, line)?;
            let kind = arg_word(args, 1, line)?;
            let strategy_name = arg_word(args, 2, line)?;
            let current = parse_strategy_current(arg_word(args, 3, line)?, line)?;
            let exists = match kind.as_str() {
                "isolation" => upf.isolations.contains_key(&strategy_name),
                "retention" => upf.retentions.contains_key(&strategy_name),
                "level_shifter" => upf.level_shifters.contains_key(&strategy_name),
                _ => {
                    return Err(format!(
                        "line {}: strategy kind must be isolation, retention, or level_shifter, found '{}'",
                        line, kind
                    ))
                }
            };
            if !exists {
                return Err(format!(
                    "line {}: unknown {} '{}'",
                    line, kind, strategy_name
                ));
            }
            let Some(mode) = upf.power_modes.get_mut(&mode_name) else {
                return Err(format!("line {}: unknown power mode '{}'", line, mode_name));
            };
            let key = power_mode_strategy_key(&kind, &strategy_name);
            if mode.strategy_currents.insert(key, current).is_some() {
                return Err(format!(
                    "line {}: power mode '{}' sets {} '{}' current more than once",
                    line, mode_name, kind, strategy_name
                ));
            }
            Ok(())
        }
        "set_active_power_mode" => {
            let name = arg_word(args, 0, line)?;
            if !upf.power_modes.contains_key(&name) {
                return Err(format!("line {}: unknown power mode '{}'", line, name));
            }
            if upf.active_power_mode.replace(name).is_some() {
                return Err(format!("line {}: active power mode already set", line));
            }
            Ok(())
        }
        "set_isolation" => {
            let name = arg_word(args, 0, line)?;
            let isolation = Isolation {
                name: name.clone(),
                domain: option_word(args, "-domain")
                    .ok_or_else(|| format!("line {}: isolation requires -domain", line))?,
                signal: option_word(args, "-signal")
                    .ok_or_else(|| format!("line {}: isolation requires -signal", line))?,
                isolation_power_net: option_word(args, "-isolation_power_net"),
                isolation_ground_net: option_word(args, "-isolation_ground_net"),
                static_current_a: None,
            };
            if upf.isolations.insert(name.clone(), isolation).is_some() {
                return Err(format!("line {}: duplicate isolation '{}'", line, name));
            }
            Ok(())
        }
        "set_retention" => {
            let name = arg_word(args, 0, line)?;
            let retention = Retention {
                name: name.clone(),
                domain: option_word(args, "-domain")
                    .ok_or_else(|| format!("line {}: retention requires -domain", line))?,
                signal: option_word(args, "-signal")
                    .ok_or_else(|| format!("line {}: retention requires -signal", line))?,
                retention_power_net: option_word(args, "-retention_power_net"),
                retention_ground_net: option_word(args, "-retention_ground_net"),
                static_current_a: None,
            };
            if upf.retentions.insert(name.clone(), retention).is_some() {
                return Err(format!("line {}: duplicate retention '{}'", line, name));
            }
            Ok(())
        }
        "set_level_shifter" => {
            let name = arg_word(args, 0, line)?;
            let shifter = LevelShifter {
                name: name.clone(),
                domain: option_word(args, "-domain")
                    .ok_or_else(|| format!("line {}: level shifter requires -domain", line))?,
                input_supply: option_word(args, "-input")
                    .ok_or_else(|| format!("line {}: level shifter requires -input", line))?,
                output_supply: option_word(args, "-output")
                    .ok_or_else(|| format!("line {}: level shifter requires -output", line))?,
                location: option_word(args, "-location").unwrap_or_else(|| "self".into()),
                static_current_a: None,
            };
            if upf.level_shifters.insert(name.clone(), shifter).is_some() {
                return Err(format!("line {}: duplicate level shifter '{}'", line, name));
            }
            Ok(())
        }
        "set_isolation_static_current" => {
            let name = arg_word(args, 0, line)?;
            let current = parse_strategy_current(arg_word(args, 1, line)?, line)?;
            let strategy = upf
                .isolations
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown isolation '{}'", line, name))?;
            if strategy.static_current_a.replace(current).is_some() {
                return Err(format!(
                    "line {}: isolation '{}' current already set",
                    line, name
                ));
            }
            Ok(())
        }
        "set_retention_static_current" => {
            let name = arg_word(args, 0, line)?;
            let current = parse_strategy_current(arg_word(args, 1, line)?, line)?;
            let strategy = upf
                .retentions
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown retention '{}'", line, name))?;
            if strategy.static_current_a.replace(current).is_some() {
                return Err(format!(
                    "line {}: retention '{}' current already set",
                    line, name
                ));
            }
            Ok(())
        }
        "set_level_shifter_static_current" => {
            let name = arg_word(args, 0, line)?;
            let current = parse_strategy_current(arg_word(args, 1, line)?, line)?;
            let strategy = upf
                .level_shifters
                .get_mut(&name)
                .ok_or_else(|| format!("line {}: unknown level shifter '{}'", line, name))?;
            if strategy.static_current_a.replace(current).is_some() {
                return Err(format!(
                    "line {}: level shifter '{}' current already set",
                    line, name
                ));
            }
            Ok(())
        }
        other => Err(format!(
            "line {}: unsupported UPF command '{}'",
            line, other
        )),
    }
}

fn parse_strategy_current(text: String, line: usize) -> Result<f64, String> {
    let current = text
        .parse::<f64>()
        .map_err(|_| format!("line {}: invalid strategy current '{}'", line, text))?;
    if !current.is_finite() || current < 0.0 {
        return Err(format!(
            "line {}: strategy current must be finite and nonnegative",
            line
        ));
    }
    Ok(current)
}

/// Check UPF objects and references against an elaborated design.
pub fn check(design: &Design, upf: &Upf) -> UpfResult {
    let mut findings = Vec::new();
    if let Some(top) = &upf.design_top {
        if *top != design.name {
            findings.push(error(
                "UPF-TOP",
                format!("UPF top '{}' does not match design '{}'", top, design.name),
            ));
        }
    }
    let design_names: BTreeSet<String> = design
        .ports
        .iter()
        .map(|port| port.name.clone())
        .chain(design.net_decls.iter().map(|net| net.name.clone()))
        .chain([design.name.clone()])
        .chain(
            design
                .hierarchy
                .iter()
                .map(|instance| instance.path.clone()),
        )
        .collect();

    for domain in upf.domains.values() {
        for element in &domain.elements {
            if !design_names.contains(element) {
                findings.push(error(
                    "UPF-ELEMENT",
                    format!(
                        "domain '{}' references unknown element '{}'",
                        domain.name, element
                    ),
                ));
            }
        }
        if domain.primary_power_net.is_none() {
            findings.push(error(
                "UPF-PWR",
                format!("domain '{}' has no primary power net", domain.name),
            ));
        }
        if domain.primary_ground_net.is_none() {
            findings.push(error(
                "UPF-GND",
                format!("domain '{}' has no primary ground net", domain.name),
            ));
        }
        for net in [
            domain.primary_power_net.clone(),
            domain.primary_ground_net.clone(),
        ]
        .into_iter()
        .flatten()
        {
            if !upf.supply_nets.contains_key(&net) {
                findings.push(error(
                    "UPF-SUPPLY",
                    format!(
                        "domain '{}' references unknown supply net '{}'",
                        domain.name, net
                    ),
                ));
            } else if upf.supply_nets[&net].domain != domain.name {
                findings.push(error(
                    "UPF-DOMAIN",
                    format!(
                        "domain '{}' primary supply '{}' belongs to domain '{}'",
                        domain.name, net, upf.supply_nets[&net].domain
                    ),
                ));
            }
        }
    }
    for port in upf.supply_ports.values() {
        if !upf.domains.contains_key(&port.domain) {
            findings.push(error(
                "UPF-DOMAIN",
                format!(
                    "supply port '{}' references unknown domain '{}'",
                    port.name, port.domain
                ),
            ));
        }
    }
    for net in upf.supply_nets.values() {
        if !upf.domains.contains_key(&net.domain) {
            findings.push(error(
                "UPF-DOMAIN",
                format!(
                    "supply net '{}' references unknown domain '{}'",
                    net.name, net.domain
                ),
            ));
        }
        for connected in &net.connected_ports {
            if !upf.supply_ports.contains_key(connected) {
                findings.push(error(
                    "UPF-PORT",
                    format!(
                        "supply net '{}' connects unknown port '{}'",
                        net.name, connected
                    ),
                ));
            } else if upf.supply_ports[connected].domain != net.domain {
                findings.push(error(
                    "UPF-DOMAIN",
                    format!(
                        "supply net '{}' in domain '{}' connects port '{}' in domain '{}'",
                        net.name, net.domain, connected, upf.supply_ports[connected].domain
                    ),
                ));
            }
        }
    }
    let mut nets_by_connected_port: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for net in upf.supply_nets.values() {
        for connected in &net.connected_ports {
            nets_by_connected_port
                .entry(connected.as_str())
                .or_default()
                .push(net.name.as_str());
        }
    }
    for (port, nets) in nets_by_connected_port {
        if nets.len() > 1 {
            findings.push(error(
                "UPF-PORT",
                format!(
                    "supply port '{}' connects multiple supply nets: {}",
                    port,
                    nets.join(",")
                ),
            ));
        }
    }
    for switch in upf.switches.values() {
        if !upf.domains.contains_key(&switch.domain) {
            findings.push(error(
                "UPF-DOMAIN",
                format!(
                    "switch '{}' references unknown domain '{}'",
                    switch.name, switch.domain
                ),
            ));
        }
        for net in [&switch.input_supply, &switch.output_supply] {
            if !upf.supply_nets.contains_key(net) {
                findings.push(error(
                    "UPF-SUPPLY",
                    format!(
                        "switch '{}' references unknown supply net '{}'",
                        switch.name, net
                    ),
                ));
            } else if upf.supply_nets[net].domain != switch.domain {
                findings.push(error(
                    "UPF-DOMAIN",
                    format!(
                        "switch '{}' in domain '{}' uses supply '{}' in domain '{}'",
                        switch.name, switch.domain, net, upf.supply_nets[net].domain
                    ),
                ));
            }
        }
        for control in &switch.controls {
            if !design_names.contains(control) {
                findings.push(error(
                    "UPF-CONTROL",
                    format!(
                        "switch '{}' references unknown control '{}'",
                        switch.name, control
                    ),
                ));
            }
        }
    }
    for mode in upf.power_modes.values() {
        for switch_name in mode.switch_states.keys() {
            if !upf.switches.contains_key(switch_name) {
                findings.push(error(
                    "UPF-MODE-SWITCH",
                    format!(
                        "power mode '{}' references unknown switch '{}'",
                        mode.name, switch_name
                    ),
                ));
            }
        }
        for switch_name in upf.switches.keys() {
            if !mode.switch_states.contains_key(switch_name) {
                findings.push(error(
                    "UPF-MODE-SWITCH",
                    format!(
                        "power mode '{}' does not define state for switch '{}'",
                        mode.name, switch_name
                    ),
                ));
            }
        }
        for (domain_name, factor) in &mode.domain_activity {
            if !factor.is_finite() || *factor < 0.0 {
                findings.push(error(
                    "UPF-MODE-ACTIVITY",
                    format!(
                        "power mode '{}' has invalid activity factor {} for domain '{}'",
                        mode.name, factor, domain_name
                    ),
                ));
            }
            if !upf.domains.contains_key(domain_name) {
                findings.push(error(
                    "UPF-MODE-ACTIVITY",
                    format!(
                        "power mode '{}' references unknown activity domain '{}'",
                        mode.name, domain_name
                    ),
                ));
            }
        }
        for (key, current) in &mode.strategy_currents {
            if !current.is_finite() || *current < 0.0 {
                findings.push(error(
                    "UPF-MODE-STRATEGY",
                    format!(
                        "power mode '{}' has invalid strategy current {} for '{}'",
                        mode.name, current, key
                    ),
                ));
            }
            let Some((kind, name)) = key.split_once(':') else {
                findings.push(error(
                    "UPF-MODE-STRATEGY",
                    format!(
                        "power mode '{}' has malformed strategy key '{}'",
                        mode.name, key
                    ),
                ));
                continue;
            };
            let exists = match kind {
                "isolation" => upf.isolations.contains_key(name),
                "retention" => upf.retentions.contains_key(name),
                "level_shifter" => upf.level_shifters.contains_key(name),
                _ => false,
            };
            if !exists {
                findings.push(error(
                    "UPF-MODE-STRATEGY",
                    format!(
                        "power mode '{}' references unknown strategy '{}'",
                        mode.name, key
                    ),
                ));
            }
        }
    }
    if let Some(active) = &upf.active_power_mode {
        if !upf.power_modes.contains_key(active) {
            findings.push(error(
                "UPF-MODE",
                format!("active power mode '{}' does not exist", active),
            ));
        }
    }
    for isolation in upf.isolations.values() {
        check_strategy(
            design,
            upf,
            "isolation",
            &isolation.name,
            &isolation.domain,
            &isolation.signal,
            &isolation.isolation_power_net,
            &isolation.isolation_ground_net,
            &mut findings,
        );
    }
    for retention in upf.retentions.values() {
        check_strategy(
            design,
            upf,
            "retention",
            &retention.name,
            &retention.domain,
            &retention.signal,
            &retention.retention_power_net,
            &retention.retention_ground_net,
            &mut findings,
        );
    }
    for shifter in upf.level_shifters.values() {
        if !upf.domains.contains_key(&shifter.domain) {
            findings.push(error(
                "UPF-DOMAIN",
                format!(
                    "level shifter '{}' references unknown domain '{}'",
                    shifter.name, shifter.domain
                ),
            ));
        }
        for net in [&shifter.input_supply, &shifter.output_supply] {
            if !upf.supply_nets.contains_key(net) {
                findings.push(error(
                    "UPF-SUPPLY",
                    format!(
                        "level shifter '{}' references unknown supply net '{}'",
                        shifter.name, net
                    ),
                ));
            }
        }
    }
    findings.sort_by(|a, b| {
        (&a.severity, &a.code, &a.message).cmp(&(&b.severity, &b.code, &b.message))
    });
    UpfResult {
        domain_count: upf.domains.len(),
        supply_net_count: upf.supply_nets.len(),
        switch_count: upf.switches.len(),
        power_mode_count: upf.power_modes.len(),
        isolation_count: upf.isolations.len(),
        retention_count: upf.retentions.len(),
        level_shifter_count: upf.level_shifters.len(),
        findings,
    }
}

#[allow(clippy::too_many_arguments)]
fn check_strategy(
    design: &Design,
    upf: &Upf,
    kind: &str,
    name: &str,
    domain: &str,
    signal: &str,
    power: &Option<String>,
    ground: &Option<String>,
    findings: &mut Vec<UpfFinding>,
) {
    if !upf.domains.contains_key(domain) {
        findings.push(error(
            "UPF-DOMAIN",
            format!("{} '{}' references unknown domain '{}'", kind, name, domain),
        ));
    }
    if design.port(signal).is_none() && !design.net_decls.iter().any(|n| n.name == signal) {
        findings.push(error(
            "UPF-SIGNAL",
            format!("{} '{}' references unknown signal '{}'", kind, name, signal),
        ));
    }
    for net in [power.clone(), ground.clone()].into_iter().flatten() {
        if !upf.supply_nets.contains_key(&net) {
            findings.push(error(
                "UPF-SUPPLY",
                format!(
                    "{} '{}' references unknown supply net '{}'",
                    kind, name, net
                ),
            ));
        }
    }
}

fn error(code: &str, message: String) -> UpfFinding {
    UpfFinding {
        code: code.into(),
        severity: Severity::Error,
        message,
    }
}

#[derive(Clone, Debug)]
enum Token {
    Word(String, usize),
    List(Vec<String>, usize),
    Punct(String, usize),
}

impl Token {
    fn line(&self) -> usize {
        match self {
            Token::Word(_, line) | Token::List(_, line) | Token::Punct(_, line) => *line,
        }
    }

    fn is_command_start(&self) -> bool {
        matches!(self, Token::Punct(p, _) if p == ";")
    }
}

fn is_upf_command(word: &str) -> bool {
    matches!(
        word,
        "upf_version"
            | "set_design_top"
            | "create_power_domain"
            | "create_supply_port"
            | "create_supply_net"
            | "connect_supply_net"
            | "set_domain_supply_net"
            | "create_power_switch"
            | "set_power_switch_on_resistance"
            | "set_power_switch_state"
            | "set_power_switch_off_leakage"
            | "create_power_mode"
            | "set_power_mode_switch"
            | "set_power_mode_domain_activity"
            | "set_power_mode_strategy_current"
            | "set_active_power_mode"
            | "set_isolation"
            | "set_retention"
            | "set_level_shifter"
            | "set_isolation_static_current"
            | "set_retention_static_current"
            | "set_level_shifter_static_current"
    )
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
            } else if first == ';' {
                chars.next();
                out.push(Token::Punct(";".into(), line_number));
            } else if first == '{' {
                chars.next();
                let mut words = Vec::new();
                let mut closed = false;
                let mut word = String::new();
                for character in chars.by_ref() {
                    if character == '}' {
                        closed = true;
                        break;
                    }
                    if character.is_whitespace() {
                        if !word.is_empty() {
                            words.push(std::mem::take(&mut word));
                        }
                    } else {
                        word.push(character);
                    }
                }
                if !word.is_empty() {
                    words.push(word);
                }
                if !closed {
                    return Err(format!("line {}: unterminated UPF list", line_number));
                }
                out.push(Token::List(words, line_number));
            } else {
                let mut word = String::new();
                while let Some(&next) = chars.peek() {
                    if next.is_whitespace() || matches!(next, '{' | '}' | ';') {
                        break;
                    }
                    word.push(next);
                    chars.next();
                }
                if word.is_empty() {
                    return Err(format!("line {}: illegal UPF character", line_number));
                }
                out.push(Token::Word(word, line_number));
            }
        }
    }
    Ok(out)
}

fn arg_word(args: &[Token], index: usize, line: usize) -> Result<String, String> {
    match args.get(index) {
        Some(Token::Word(word, _)) => Ok(word.clone()),
        Some(token) => Err(format!(
            "line {}: expected identifier at argument {}",
            token.line(),
            index
        )),
        None => Err(format!("line {}: missing argument {}", line, index)),
    }
}

fn option_word(args: &[Token], option: &str) -> Option<String> {
    let position = args
        .iter()
        .position(|token| matches!(token, Token::Word(word, _) if word == option))?;
    match args.get(position + 1) {
        Some(Token::Word(word, _)) => Some(word.clone()),
        _ => None,
    }
}

fn opt_list(args: &[Token], option: &str) -> Vec<String> {
    let Some(position) = args
        .iter()
        .position(|token| matches!(token, Token::Word(word, _) if word == option))
    else {
        return Vec::new();
    };
    match args.get(position + 1) {
        Some(Token::List(values, _)) => values.clone(),
        Some(Token::Word(word, _)) => vec![word.clone()],
        _ => Vec::new(),
    }
}

pub fn report(result: &UpfResult) -> String {
    let mut out = format!(
        "UPF report: {} - {} domain(s), {} supply net(s), {} switch(es), {} power mode(s), {} isolation(s), {} retention(s), {} level shifter(s), {} error(s)\n",
        if result.has_errors() { "FAIL" } else { "PASS" },
        result.domain_count,
        result.supply_net_count,
        result.switch_count,
        result.power_mode_count,
        result.isolation_count,
        result.retention_count,
        result.level_shifter_count,
        result.findings.iter().filter(|f| f.severity == Severity::Error).count()
    );
    for finding in &result.findings {
        let severity = match finding.severity {
            Severity::Warning => "WARN",
            Severity::Error => "ERROR",
        };
        out.push_str(&format!(
            "  {} [{}] {}\n",
            severity, finding.code, finding.message
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::ir::{Dir, NetDecl, PortDecl};

    fn design() -> Design {
        let net_decls = vec![
            NetDecl {
                name: "a".into(),
                width: 1,
                is_reg: false,
                is_signed: false,
                base: 0,
            },
            NetDecl {
                name: "q".into(),
                width: 1,
                is_reg: true,
                is_signed: false,
                base: 1,
            },
            NetDecl {
                name: "sleep".into(),
                width: 1,
                is_reg: false,
                is_signed: false,
                base: 2,
            },
        ];
        Design {
            name: "m".into(),
            net_decls,
            memories: Vec::new(),
            n_nets: 3,
            ports: vec![
                PortDecl {
                    name: "a".into(),
                    dir: Dir::In,
                    base: 0,
                    width: 1,
                },
                PortDecl {
                    name: "q".into(),
                    dir: Dir::Out,
                    base: 1,
                    width: 1,
                },
                PortDecl {
                    name: "sleep".into(),
                    dir: Dir::In,
                    base: 2,
                    width: 1,
                },
            ],
            assigns: Vec::new(),
            processes: Vec::new(),
            initials: Vec::new(),
            hierarchy: Vec::new(),
        }
    }

    #[test]
    fn parses_power_intent() {
        let upf = parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {vdd}
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
"#,
        )
        .unwrap();
        assert_eq!(upf.design_top.as_deref(), Some("m"));
        assert_eq!(upf.supply_nets["vdd"].connected_ports, vec!["vdd"]);
        assert!(parse_upf("create_power_domain TOP -elements {m m}")
            .unwrap_err()
            .contains("repeats element"));
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_supply_port p -domain TOP -direction both"
        )
        .unwrap_err()
        .contains("direction must be"));
    }

    #[test]
    fn parses_power_switch_on_resistance() {
        let upf = parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 0.025
set_power_switch_state sw off
set_power_switch_off_leakage sw 1e-9
"#,
        )
        .unwrap();
        assert_eq!(upf.switches["sw"].on_resistance_ohm, Some(0.025));
        assert_eq!(upf.switches["sw"].state, Some(false));
        assert_eq!(upf.switches["sw"].off_leakage_a, Some(1e-9));
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\nset_power_switch_on_resistance sw 0"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\nset_power_switch_on_resistance other 1"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\nset_power_switch_state sw open"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\nset_power_switch_off_leakage sw -1"
        )
        .is_err());
    }

    #[test]
    fn parses_power_modes() {
        let upf = parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw0 -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_switch sw1 -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode active
set_power_mode_switch active sw0 on
set_power_mode_switch active sw1 on
set_power_mode_domain_activity active TOP 1.0
create_power_mode sleep
set_power_mode_switch sleep sw0 on
set_power_mode_switch sleep sw1 off
set_power_mode_domain_activity sleep TOP 0.25
set_active_power_mode sleep
"#,
        )
        .unwrap();
        assert_eq!(upf.power_modes.len(), 2);
        assert_eq!(upf.active_power_mode.as_deref(), Some("sleep"));
        assert!(!upf.power_modes["sleep"].switch_states["sw1"]);
        assert_eq!(upf.power_modes["sleep"].domain_activity["TOP"], 0.25);
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_power_mode_switch m sw on\nset_power_mode_switch m sw off"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_power_mode_switch m ghost on"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_active_power_mode m\nset_active_power_mode m"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_power_mode_domain_activity m ghost 1"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_power_mode_domain_activity m TOP -1"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\ncreate_power_switch sw -domain TOP -input_supply_port v -output_supply_port v\ncreate_power_mode m\nset_power_mode_domain_activity m TOP 1\nset_power_mode_domain_activity m TOP 2"
        )
        .is_err());
    }

    #[test]
    fn parses_strategy_static_currents() {
        let upf = parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_power_domain AUX -elements {}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
create_supply_net vddaux -domain AUX
create_supply_net vssaux -domain AUX
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
set_domain_supply_net AUX -primary_power_net vddaux -primary_ground_net vssaux
set_isolation iso -domain TOP -signal a -isolation_power_net vddaux
set_isolation_static_current iso 1e-9
set_retention ret -domain TOP -signal q -retention_power_net vddaux
set_retention_static_current ret 2e-9
set_level_shifter shift -domain TOP -input vdd -output vddaux
set_level_shifter_static_current shift 3e-9
create_power_mode active
set_power_mode_strategy_current active isolation iso 4e-9
create_power_mode sleep
set_power_mode_strategy_current sleep retention ret 5e-9
"#,
        )
        .unwrap();
        assert_eq!(upf.isolations["iso"].static_current_a, Some(1e-9));
        assert_eq!(upf.retentions["ret"].static_current_a, Some(2e-9));
        assert_eq!(upf.level_shifters["shift"].static_current_a, Some(3e-9));
        assert_eq!(
            upf.power_modes["active"].strategy_currents
                [power_mode_strategy_key("isolation", "iso").as_str()],
            4e-9
        );
        assert_eq!(
            upf.power_modes["sleep"].strategy_currents
                [power_mode_strategy_key("retention", "ret").as_str()],
            5e-9
        );
        assert!(parse_upf(
            "create_power_domain TOP\nset_isolation iso -domain TOP -signal a\nset_isolation_static_current iso -1"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\nset_retention ret -domain TOP -signal q\nset_retention_static_current ghost 1"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\nset_level_shifter ls -domain TOP -input v -output v\nset_level_shifter_static_current ls 1\nset_level_shifter_static_current ls 2"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\nset_isolation iso -domain TOP -signal a\ncreate_power_mode m\nset_power_mode_strategy_current m isolation iso 1\nset_power_mode_strategy_current m isolation iso 2"
        )
        .is_err());
        assert!(parse_upf(
            "create_power_domain TOP\nset_isolation iso -domain TOP -signal a\ncreate_power_mode m\nset_power_mode_strategy_current m buffer iso 1"
        )
        .is_err());
    }

    #[test]
    fn valid_power_intent_checks_cleanly() {
        let upf = parse_upf(
            r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP -direction in
create_supply_port vss -domain TOP -direction out
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {vdd}
connect_supply_net vss -ports {vss}
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd -control_port {sleep}
set_power_switch_on_resistance sw 0.025
create_power_mode active
set_power_mode_switch active sw on
set_power_mode_domain_activity active TOP 1.0
set_active_power_mode active
set_isolation iso -domain TOP -signal a -isolation_power_net vdd -isolation_ground_net vss
set_retention ret -domain TOP -signal q -retention_power_net vdd -retention_ground_net vss
set_level_shifter shift -domain TOP -input vdd -output vss
"#,
        )
        .unwrap();
        let result = check(&design(), &upf);
        assert!(!result.has_errors(), "{:?}", result.findings);
        assert_eq!(result.domain_count, 1);
        assert_eq!(result.supply_net_count, 2);
        assert_eq!(result.switch_count, 1);
        assert_eq!(result.power_mode_count, 1);
        assert_eq!(result.isolation_count, 1);
        assert_eq!(result.retention_count, 1);
        assert_eq!(result.level_shifter_count, 1);
        let text = report(&result);
        assert!(text.contains("UPF report: PASS"), "{text}");
    }

    #[test]
    fn cross_domain_supplies_and_shared_ports_are_errors() {
        let upf = parse_upf(
            r#"
create_power_domain TOP -elements {m}
create_power_domain AUX
create_supply_port auxp -domain AUX
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
create_supply_net aux -domain AUX
connect_supply_net vdd -ports {auxp}
set_domain_supply_net TOP -primary_power_net aux -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port aux -output_supply_port vss
create_power_mode m
set_power_mode_switch m sw on
"#,
        )
        .unwrap();
        let result = check(&design(), &upf);
        assert!(result.has_errors());
        let codes: Vec<&str> = result
            .findings
            .iter()
            .map(|finding| finding.code.as_str())
            .collect();
        for expected in ["UPF-DOMAIN", "UPF-DOMAIN", "UPF-DOMAIN", "UPF-DOMAIN"] {
            assert!(codes.contains(&expected), "{:?}", result.findings);
        }
        let text = report(&result);
        assert!(
            text.contains("primary supply 'aux' belongs to domain 'AUX'"),
            "{text}"
        );
        assert!(
            text.contains("connects port 'auxp' in domain 'AUX'"),
            "{text}"
        );
        assert!(text.contains("uses supply 'aux' in domain 'AUX'"), "{text}");

        let shared = parse_upf(
            r#"
create_power_domain TOP
create_supply_port p -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {p}
connect_supply_net vss -ports {p}
"#,
        )
        .unwrap();
        let result = check(&design(), &shared);
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.code == "UPF-PORT"
                && finding.message.contains("connects multiple supply nets")));
    }

    #[test]
    fn check_reports_top_element_signal_and_mode_errors() {
        let upf = parse_upf(
            r#"
set_design_top wrong
create_power_domain TOP -elements {ghost}
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
create_power_mode only
set_power_mode_switch only sw on
set_isolation iso -domain TOP -signal missing -isolation_power_net vdd
"#,
        )
        .unwrap();
        let result = check(&design(), &upf);
        let messages = result
            .findings
            .iter()
            .map(|finding| finding.message.as_str())
            .collect::<Vec<_>>();
        assert!(messages
            .iter()
            .any(|message| message.contains("does not match design")));
        assert!(messages
            .iter()
            .any(|message| message.contains("unknown element 'ghost'")));
        assert!(messages
            .iter()
            .any(|message| message.contains("unknown signal 'missing'")));
        assert!(result.has_errors());
    }
}
