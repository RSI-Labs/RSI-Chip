//! Conservative signal-quality checks: transition, load capacitance, and
//! fanout design rules.

use crate::constraints::TimingObjectRef;
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityKind {
    MaxTransition,
    MaxCapacitance,
    MaxFanout,
}

impl QualityKind {
    fn code(self) -> &'static str {
        match self {
            Self::MaxTransition => "TRANS-001",
            Self::MaxCapacitance => "CAP-001",
            Self::MaxFanout => "FANOUT-001",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QualityViolation {
    pub kind: QualityKind,
    pub cell: usize,
    pub net: NetId,
    pub actual: f64,
    pub limit: f64,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QualityResult {
    pub violations: Vec<QualityViolation>,
    pub transitions_checked: usize,
    pub capacitances_checked: usize,
    pub fanouts_checked: usize,
    pub worst_transition_slack: Option<f64>,
    pub worst_capacitance_slack: Option<f64>,
}

impl QualityResult {
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct QualityOptions {
    /// Assumed transition on every primary input, in nanoseconds.
    pub input_transition_ns: f64,
    /// Design-wide transition override; per-output Liberty limits otherwise.
    pub max_transition_ns: Option<f64>,
    /// Design-wide load override; per-output Liberty limits otherwise.
    pub max_capacitance_pf: Option<f64>,
    /// Optional design-wide fanout limit.
    pub max_fanout: Option<usize>,
    /// Object-scoped SDC transition limits. The last matching rule wins.
    pub scoped_max_transition_ns: Vec<ScopedQualityLimit>,
    /// Object-scoped SDC capacitance limits. The last matching rule wins.
    pub scoped_max_capacitance_pf: Vec<ScopedQualityLimit>,
    /// Object-scoped SDC fanout limits. The last matching rule wins.
    pub scoped_max_fanout: Vec<ScopedQualityLimit>,
    /// Object-scoped SDC input transitions in nanoseconds.
    pub input_transitions: Vec<ScopedQualityLimit>,
    /// Object-scoped SDC pin/port loads in picofarads.
    pub load_capacitances: Vec<ScopedQualityLimit>,
    /// Extra parasitic capacitance by net name, typically from SPEF.
    pub extra_capacitance_pf: BTreeMap<String, f64>,
}

/// One typed SDC quality-rule object selector and its numeric limit.
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedQualityLimit {
    pub object: TimingObjectRef,
    pub limit: f64,
}

impl Default for QualityOptions {
    fn default() -> Self {
        Self {
            input_transition_ns: 0.05,
            max_transition_ns: None,
            max_capacitance_pf: None,
            max_fanout: None,
            scoped_max_transition_ns: Vec::new(),
            scoped_max_capacitance_pf: Vec::new(),
            scoped_max_fanout: Vec::new(),
            input_transitions: Vec::new(),
            load_capacitances: Vec::new(),
            extra_capacitance_pf: BTreeMap::new(),
        }
    }
}

impl QualityOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !self.input_transition_ns.is_finite() || self.input_transition_ns < 0.0 {
            return Err("input transition must be finite and nonnegative".into());
        }
        if let Some(value) = self.max_transition_ns {
            if !value.is_finite() || value < 0.0 {
                return Err("max transition must be finite and nonnegative".into());
            }
        }
        if let Some(value) = self.max_capacitance_pf {
            if !value.is_finite() || value < 0.0 {
                return Err("max capacitance must be finite and nonnegative".into());
            }
        }
        for rules in [
            &self.scoped_max_transition_ns,
            &self.scoped_max_capacitance_pf,
            &self.scoped_max_fanout,
        ] {
            if rules
                .iter()
                .any(|rule| !rule.limit.is_finite() || rule.limit <= 0.0)
            {
                return Err("scoped quality limits must be positive and finite".into());
            }
        }
        for value in self
            .input_transitions
            .iter()
            .chain(&self.load_capacitances)
            .map(|rule| rule.limit)
        {
            if !value.is_finite() || value < 0.0 {
                return Err("input transitions and loads must be finite and nonnegative".into());
            }
        }
        if self
            .extra_capacitance_pf
            .values()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err("extra capacitance values must be finite and nonnegative".into());
        }
        Ok(())
    }
}

pub fn analyze_quality(nl: &Netlist, options: &QualityOptions) -> Result<QualityResult, String> {
    options.validate()?;
    let order = nl
        .topo_cells()
        .ok_or_else(|| "quality analysis requires an acyclic combinational netlist".to_string())?;
    let mut transition = vec![None::<f64>; nl.net_count()];
    for port in &nl.ports {
        if port.dir == Dir::In {
            for &net in &port.bits {
                transition[net] = Some(
                    effective_primary_input_transition(options, net, nl)
                        .unwrap_or(options.input_transition_ns),
                );
            }
        }
    }

    let mut result = QualityResult::default();
    for &ci in &order {
        let cell = nl.cell(ci);
        match &cell.kind {
            CellKind::Const(_) => transition[cell.output] = Some(0.0),
            CellKind::Dff { .. } | CellKind::DffR { .. } => {
                transition[cell.output] = Some(0.5);
            }
            CellKind::Lib(id) => {
                let Some(library) = nl.library() else {
                    return Err(format!("cell '{}' has no attached library", cell.name));
                };
                let def = library.get(*id);
                if let Some(seq) = &def.seq {
                    transition[cell.output] = Some(seq.clk_to_q);
                } else {
                    let output = cell
                        .inputs
                        .iter()
                        .zip(&def.arcs)
                        .filter_map(|(&input, &(pin, delay))| {
                            let pin_name = def.inputs.get(pin).map(String::as_str);
                            let input_transition = pin_name.and_then(|pin_name| {
                                effective_pin_input_transition(
                                    options, &cell.name, pin_name, input, nl,
                                )
                            });
                            let input_transition = input_transition.or_else(|| transition[input]);
                            input_transition.map(|input_transition| {
                                input_transition
                                    + arc_delay(
                                        &cell.kind,
                                        def.arcs.iter().find(|&&(p, _)| p == pin).map(|&(_, d)| d),
                                        delay,
                                    )
                            })
                        })
                        .fold(None::<f64>, |best, value| match best {
                            Some(best) if best >= value => Some(best),
                            _ => Some(value),
                        });
                    transition[cell.output] = output;
                }
                let metadata = library.metadata(&def.name);
                let output_name = &def.output;
                let scoped_transition_limit = effective_scoped_limit(
                    &options.scoped_max_transition_ns,
                    &cell.name,
                    output_name,
                    cell.output,
                    nl,
                );
                let transition_limit = scoped_transition_limit
                    .or(options.max_transition_ns)
                    .or_else(|| metadata.max_transition_ns.get(output_name).copied());
                if let (Some(actual), Some(limit)) = (transition[cell.output], transition_limit) {
                    check(
                        &mut result,
                        nl,
                        ci,
                        cell.output,
                        actual,
                        limit,
                        QualityKind::MaxTransition,
                        "transition",
                        "ns",
                    );
                }
            }
            _ => {
                let output = cell
                    .inputs
                    .iter()
                    .filter_map(|&input| transition[input])
                    .fold(None::<f64>, |best, value| match best {
                        Some(best) if best >= value => Some(best),
                        _ => Some(value),
                    });
                transition[cell.output] = output.map(|value| value + 0.5);
            }
        }
    }

    for net_id in 0..nl.net_count() {
        let net = nl.net(net_id);
        let Some(&driver) = net.drivers.first() else {
            continue;
        };
        let cell = nl.cell(driver);
        let mut load = options
            .extra_capacitance_pf
            .get(&net.name)
            .copied()
            .unwrap_or(0.0);
        load += matching_pin_load(options, nl, net_id, driver);
        for &sink_id in &net.sinks {
            let sink = nl.cell(sink_id);
            if let CellKind::Lib(id) = &sink.kind {
                let Some(library) = nl.library() else {
                    return Err(format!("cell '{}' has no attached library", sink.name));
                };
                let def = library.get(*id);
                let Some(pin_index) = sink.inputs.iter().position(|&input| input == net_id) else {
                    continue;
                };
                let Some(pin_name) = def.inputs.get(pin_index) else {
                    continue;
                };
                load += library
                    .metadata(&def.name)
                    .input_capacitance_pf
                    .get(pin_name)
                    .copied()
                    .unwrap_or(0.0);
            }
        }

        let scoped_cap_limit = effective_scoped_limit(
            &options.scoped_max_capacitance_pf,
            &cell.name,
            &output_pin(nl, cell),
            net_id,
            nl,
        );
        let mut cap_limit = scoped_cap_limit.or(options.max_capacitance_pf);
        if cap_limit.is_none() {
            if let CellKind::Lib(id) = &cell.kind {
                let library = nl
                    .library()
                    .ok_or_else(|| format!("cell '{}' has no attached library", cell.name))?;
                let def = library.get(*id);
                cap_limit = library
                    .metadata(&def.name)
                    .max_capacitance_pf
                    .get(&def.output)
                    .copied();
            }
        }
        if let Some(limit) = cap_limit {
            check(
                &mut result,
                nl,
                driver,
                net_id,
                load,
                limit,
                QualityKind::MaxCapacitance,
                "load capacitance",
                "pF",
            );
        }
        let scoped_fanout_limit = effective_scoped_limit(
            &options.scoped_max_fanout,
            &cell.name,
            &output_pin(nl, cell),
            net_id,
            nl,
        );
        if let Some(limit) = scoped_fanout_limit
            .map(|value| value as usize)
            .or(options.max_fanout)
        {
            let fanout = net.sinks.len();
            if fanout > limit {
                result.violations.push(QualityViolation {
                    kind: QualityKind::MaxFanout,
                    cell: driver,
                    net: net_id,
                    actual: fanout as f64,
                    limit: limit as f64,
                    message: format!("net '{}' fanout {} exceeds {}", net.name, fanout, limit),
                });
            }
            result.fanouts_checked += 1;
        }
    }

    result.violations.sort_by(|a, b| {
        a.kind
            .code()
            .cmp(b.kind.code())
            .then(a.cell.cmp(&b.cell))
            .then(a.net.cmp(&b.net))
    });
    Ok(result)
}

fn output_pin<'a>(nl: &'a Netlist, cell: &eda_core::netlist::Cell) -> std::borrow::Cow<'a, str> {
    if let CellKind::Lib(id) = &cell.kind {
        if let Some(library) = nl.library() {
            let definition = library.get(*id);
            return std::borrow::Cow::Owned(definition.output.clone());
        }
    }
    std::borrow::Cow::Borrowed("y")
}

fn effective_primary_input_transition(
    options: &QualityOptions,
    net: NetId,
    nl: &Netlist,
) -> Option<f64> {
    options
        .input_transitions
        .iter()
        .rev()
        .find(|rule| {
            matches!(
                rule.object.kind,
                crate::constraints::TimingObjectKind::Port
                    | crate::constraints::TimingObjectKind::Net
            ) && rule.object.matches_name(&nl.net(net).name)
        })
        .map(|rule| rule.limit)
}

fn effective_pin_input_transition(
    options: &QualityOptions,
    cell_name: &str,
    pin_name: &str,
    net: NetId,
    nl: &Netlist,
) -> Option<f64> {
    let pin_name = format!("{cell_name}/{pin_name}");
    options
        .input_transitions
        .iter()
        .rev()
        .find(|rule| {
            rule.object.kind == crate::constraints::TimingObjectKind::Pin
                && rule.object.matches_name(&pin_name)
        })
        .map(|rule| rule.limit)
        .or_else(|| effective_primary_input_transition(options, net, nl))
}

fn matching_pin_load(options: &QualityOptions, nl: &Netlist, net: NetId, driver: usize) -> f64 {
    let net_ref = nl.net(net);
    let driver_cell = nl.cell(driver);
    let driver_output = output_pin(nl, driver_cell);
    let driver_pin = format!("{}/{}", driver_cell.name, driver_output);
    options
        .load_capacitances
        .iter()
        .filter_map(|rule| match rule.object.kind {
            crate::constraints::TimingObjectKind::Port
            | crate::constraints::TimingObjectKind::Net => rule
                .object
                .matches_name(&net_ref.name)
                .then_some(rule.limit),
            crate::constraints::TimingObjectKind::Pin => {
                if rule.object.matches_name(&driver_pin) {
                    return Some(rule.limit);
                }
                net_ref
                    .sinks
                    .iter()
                    .any(|&sink_id| {
                        let sink = nl.cell(sink_id);
                        let CellKind::Lib(id) = &sink.kind else {
                            return false;
                        };
                        let Some(library) = nl.library() else {
                            return false;
                        };
                        let definition = library.get(*id);
                        let Some(pin_index) = sink.inputs.iter().position(|&input| input == net)
                        else {
                            return false;
                        };
                        let Some(pin_name) = definition.inputs.get(pin_index) else {
                            return false;
                        };
                        rule.object
                            .matches_name(&format!("{}/{}", sink.name, pin_name))
                    })
                    .then_some(rule.limit)
            }
            _ => None,
        })
        .sum()
}

fn effective_scoped_limit(
    rules: &[ScopedQualityLimit],
    cell_name: &str,
    output_pin: &str,
    net: NetId,
    nl: &Netlist,
) -> Option<f64> {
    let output_pin = format!("{cell_name}/{output_pin}");
    rules
        .iter()
        .rev()
        .find(|rule| match rule.object.kind {
            crate::constraints::TimingObjectKind::Port
            | crate::constraints::TimingObjectKind::Net => {
                rule.object.matches_name(&nl.net(net).name)
            }
            crate::constraints::TimingObjectKind::Cell => rule.object.matches_name(cell_name),
            crate::constraints::TimingObjectKind::Pin => rule.object.matches_name(&output_pin),
            crate::constraints::TimingObjectKind::Clock => false,
        })
        .map(|rule| rule.limit)
}

fn arc_delay(kind: &CellKind, library_delay: Option<f64>, fallback: f64) -> f64 {
    match kind {
        CellKind::Lib(_) => library_delay.unwrap_or(fallback),
        _ => 0.5,
    }
}

#[allow(clippy::too_many_arguments)]
fn check(
    result: &mut QualityResult,
    nl: &Netlist,
    cell: usize,
    net: NetId,
    actual: f64,
    limit: f64,
    kind: QualityKind,
    quantity: &str,
    unit: &str,
) {
    let slack = limit - actual;
    let worst = match kind {
        QualityKind::MaxTransition => &mut result.worst_transition_slack,
        QualityKind::MaxCapacitance => &mut result.worst_capacitance_slack,
        QualityKind::MaxFanout => return,
    };
    *worst = Some(worst.map(|value| value.min(slack)).unwrap_or(slack));
    match kind {
        QualityKind::MaxTransition => result.transitions_checked += 1,
        QualityKind::MaxCapacitance => result.capacitances_checked += 1,
        QualityKind::MaxFanout => {}
    }
    if slack < 0.0 {
        result.violations.push(QualityViolation {
            kind,
            cell,
            net,
            actual,
            limit,
            message: format!(
                "cell '{}' output '{}' {} {:.6} {} exceeds limit {:.6} {}",
                nl.cell(cell).name,
                nl.net(net).name,
                quantity,
                actual,
                unit,
                limit,
                unit
            ),
        });
    }
}

pub fn quality_report(result: &QualityResult) -> String {
    let mut out = format!(
        "Signal-quality report: {} transition checks, {} capacitance checks, {} fanout checks, {} violation(s)\n",
        result.transitions_checked,
        result.capacitances_checked,
        result.fanouts_checked,
        result.violations.len()
    );
    for violation in &result.violations {
        out.push_str(&format!(
            "  {}: {}\n",
            violation.kind.code(),
            violation.message
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};
    use std::sync::Arc;

    #[test]
    fn reports_transition_capacitance_and_fanout() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("quality");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        nl.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(library.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        let inv = library.find("INV").unwrap();
        nl.add_cell(Cell {
            name: "i1".into(),
            kind: CellKind::Lib(inv),
            inputs: vec![y],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "i2".into(),
            kind: CellKind::Lib(inv),
            inputs: vec![y],
            output: n2,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("n1", Dir::Out, vec![n1]);
        nl.add_port("n2", Dir::Out, vec![n2]);

        let mut extra = BTreeMap::new();
        extra.insert("y".to_string(), 0.06);
        let options = QualityOptions {
            max_transition_ns: Some(0.2),
            max_capacitance_pf: Some(0.05),
            max_fanout: Some(1),
            extra_capacitance_pf: extra,
            ..Default::default()
        };
        let result = analyze_quality(&nl, &options).unwrap();
        assert_eq!(result.transitions_checked, 3);
        assert_eq!(result.capacitances_checked, 3);
        assert_eq!(result.fanouts_checked, 3);
        assert!(result
            .violations
            .iter()
            .any(|v| v.kind == QualityKind::MaxTransition));
        assert!(result
            .violations
            .iter()
            .any(|v| v.kind == QualityKind::MaxCapacitance));
        assert!(result
            .violations
            .iter()
            .any(|v| v.kind == QualityKind::MaxFanout));
        assert!(quality_report(&result).contains("TRANS-001"));
    }

    #[test]
    fn rejects_invalid_quality_options() {
        assert!(QualityOptions {
            input_transition_ns: -1.0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn object_scoped_quality_limits_override_design_wide_rules() {
        let library = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("scoped_quality");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        let and = library.find("AND2").unwrap();
        nl.add_cell(Cell {
            name: "driver".into(),
            kind: CellKind::Lib(and),
            inputs: vec![a, b],
            output: y,
        });
        let inv = library.find("INV").unwrap();
        nl.add_cell(Cell {
            name: "i1".into(),
            kind: CellKind::Lib(inv),
            inputs: vec![y],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "i2".into(),
            kind: CellKind::Lib(inv),
            inputs: vec![y],
            output: n2,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("n1", Dir::Out, vec![n1]);
        nl.add_port("n2", Dir::Out, vec![n2]);

        let transition_rule = ScopedQualityLimit {
            object: crate::constraints::TimingObjectRef::new(
                crate::constraints::TimingObjectKind::Port,
                "n1",
            ),
            limit: 0.001,
        };
        let capacitance_rule = ScopedQualityLimit {
            object: crate::constraints::TimingObjectRef::new(
                crate::constraints::TimingObjectKind::Net,
                "n1",
            ),
            limit: 0.001,
        };
        let fanout_rule = ScopedQualityLimit {
            object: crate::constraints::TimingObjectRef::new(
                crate::constraints::TimingObjectKind::Cell,
                "driver",
            ),
            limit: 1.0,
        };
        let mut extra = BTreeMap::new();
        extra.insert("n1".to_string(), 0.1);
        let options = QualityOptions {
            max_transition_ns: Some(10.0),
            max_capacitance_pf: Some(10.0),
            max_fanout: Some(16),
            scoped_max_transition_ns: vec![transition_rule],
            scoped_max_capacitance_pf: vec![capacitance_rule],
            scoped_max_fanout: vec![fanout_rule],
            extra_capacitance_pf: extra,
            ..Default::default()
        };
        let result = analyze_quality(&nl, &options).unwrap();
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.kind == QualityKind::MaxTransition && violation.net == n1));
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.kind == QualityKind::MaxCapacitance && violation.net == n1));
        assert!(result
            .violations
            .iter()
            .any(|violation| violation.kind == QualityKind::MaxFanout && violation.net == y));
    }
}
