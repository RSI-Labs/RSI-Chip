//! # eda-power — gate-level switching-power analysis
//!
//! The analyzer runs an event-driven simulation over user-supplied input
//! vectors, records per-net switching activity, and combines that activity
//! with standard-cell site counts to estimate dynamic and leakage power.

use eda_core::logic::{LogicValue, LogicWord};
use eda_core::netlist::{CellKind, NetId, Netlist};
use eda_core::to_design::netlist_to_design;
use eda_sim::Simulator;
use eda_waveform::Vcd;
use std::collections::{BTreeMap, HashMap};

/// One simulation stimulus. Keys are primary-input port names.
/// 每端口活动字（支持任意位宽输入）。
pub type InputVector = HashMap<String, eda_core::logic::LogicWord>;

#[derive(Clone, Debug)]
pub struct PowerOptions {
    /// Operating frequency in MHz.
    pub frequency_mhz: f64,
    /// Base internal-switching energy per cell transition, in pJ.
    pub base_cell_energy_pj: f64,
    /// Incremental energy per placement site per transition, in pJ.
    pub energy_per_site_pj: f64,
    /// Energy per picofarad of switched net capacitance, in pJ.
    ///
    /// This remains zero for legacy pre-layout estimates and is enabled by
    /// post-route analysis after RC extraction.
    pub capacitive_energy_per_pf_pj: f64,
    /// Leakage per placement site, in nW.
    pub leakage_per_site_nw: f64,
}

impl Default for PowerOptions {
    fn default() -> Self {
        PowerOptions {
            frequency_mhz: 100.0,
            base_cell_energy_pj: 0.002,
            energy_per_site_pj: 0.0015,
            capacitive_energy_per_pf_pj: 0.0,
            leakage_per_site_nw: 2.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SwitchingActivity {
    pub transitions: u64,
    pub known_samples: u64,
    pub one_samples: u64,
}

/// Per-net activity plus correlated full-state snapshots.
#[derive(Clone, Debug)]
struct PowerActivity {
    nets: Vec<SwitchingActivity>,
    snapshots: Vec<Vec<LogicValue>>,
}

impl SwitchingActivity {
    pub fn toggle_probability(&self, intervals: u64) -> f64 {
        if intervals == 0 {
            0.0
        } else {
            self.transitions as f64 / intervals as f64
        }
    }

    pub fn static_probability(&self) -> f64 {
        if self.known_samples == 0 {
            0.0
        } else {
            self.one_samples as f64 / self.known_samples as f64
        }
    }
}

#[derive(Clone, Debug)]
pub struct CellPower {
    pub cell: usize,
    pub name: String,
    pub width_sites: usize,
    pub activity: f64,
    pub dynamic_power_uw: f64,
}

#[derive(Clone, Debug)]
pub struct PowerReport {
    pub frequency_mhz: f64,
    pub vectors: usize,
    pub intervals: usize,
    pub dynamic_power_uw: f64,
    pub static_power_uw: f64,
    pub total_power_uw: f64,
    pub activity_by_net: BTreeMap<String, SwitchingActivity>,
    pub cells: Vec<CellPower>,
}

/// Estimate gate-level power. Dynamic energy is charged when any cell input
/// or its output toggles; leakage is estimated from total placement sites.
pub fn analyze(
    nl: &Netlist,
    vectors: &[InputVector],
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    if vectors.is_empty() {
        return Err("power analysis requires at least one input vector".into());
    }
    validate_options(opts)?;
    let activity = activity_from_vectors(nl, vectors)?;
    let intervals = (vectors.len() - 1).max(1) as u64;
    build_report(nl, &activity, vectors.len(), intervals as usize, None, opts)
}

/// Estimate power with explicit per-net switched capacitance.
///
/// Capacitance is keyed by stable net name. Effective switched coupling
/// capacitance is normally grounded capacitance plus half of the coupling
/// capacitance. Each net load is charged to its driving cell.
pub fn analyze_with_capacitances(
    nl: &Netlist,
    vectors: &[InputVector],
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    if vectors.is_empty() {
        return Err("power analysis requires at least one input vector".into());
    }
    validate_options(opts)?;
    if capacitance_pf
        .values()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("switched capacitance must be finite and nonnegative".into());
    }
    let activity = activity_from_vectors(nl, vectors)?;
    let intervals = (vectors.len() - 1).max(1) as u64;
    build_report(
        nl,
        &activity,
        vectors.len(),
        intervals as usize,
        Some(capacitance_pf),
        opts,
    )
}

/// Estimate power from a gate-level VCD. The VCD may contain hierarchical or
/// short names; indexed netlist names such as `q[2]` are matched to the base
/// vector declaration when necessary.
pub fn analyze_waveform(
    nl: &Netlist,
    waveform: &Vcd,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    validate_options(opts)?;
    let times = waveform.time_points();
    if times.is_empty() {
        return Err("VCD power analysis requires at least one timestamp".into());
    }
    let activity = activity_from_waveform(nl, waveform, &times)?;
    let intervals = (times.len() - 1).max(1);
    build_report(nl, &activity, times.len(), intervals, None, opts)
}

/// Estimate waveform-driven power with extracted switched capacitance.
pub fn analyze_waveform_with_capacitances(
    nl: &Netlist,
    waveform: &Vcd,
    capacitance_pf: &BTreeMap<String, f64>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    validate_options(opts)?;
    if capacitance_pf
        .values()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        return Err("switched capacitance must be finite and nonnegative".into());
    }
    let times = waveform.time_points();
    if times.is_empty() {
        return Err("VCD power analysis requires at least one timestamp".into());
    }
    let activity = activity_from_waveform(nl, waveform, &times)?;
    let intervals = (times.len() - 1).max(1);
    build_report(
        nl,
        &activity,
        times.len(),
        intervals,
        Some(capacitance_pf),
        opts,
    )
}

fn activity_from_waveform(
    nl: &Netlist,
    waveform: &Vcd,
    times: &[u64],
) -> Result<PowerActivity, String> {
    let mut activity = vec![SwitchingActivity::default(); nl.net_count()];
    let mut snapshots = Vec::with_capacity(times.len());
    let mut previous: Option<Vec<LogicValue>> = None;
    for &time in times {
        let mut current = Vec::with_capacity(nl.net_count());
        for net in 0..nl.net_count() {
            let value = match waveform_value(waveform, &nl.net(net).name, time) {
                Some(word) if word.width() == 1 => word.bit(0),
                Some(word) => {
                    return Err(format!(
                        "VCD signal for scalar net '{}' has width {}",
                        nl.net(net).name,
                        word.width()
                    ));
                }
                None => LogicValue::X,
            };
            current.push(value);
        }
        for (net, &value) in current.iter().enumerate() {
            if value.is_01() {
                activity[net].known_samples += 1;
                if value == LogicValue::One {
                    activity[net].one_samples += 1;
                }
            }
            if let Some(prev) = previous.as_ref().and_then(|values| values.get(net)) {
                if prev.is_01() && value.is_01() && *prev != value {
                    activity[net].transitions += 1;
                }
            }
        }
        snapshots.push(current.clone());
        previous = Some(current);
    }
    Ok(PowerActivity {
        nets: activity,
        snapshots,
    })
}

fn waveform_value(waveform: &Vcd, net_name: &str, time: u64) -> Option<LogicWord> {
    if let Some(signal) = waveform.signal(net_name) {
        return signal.value_at(time);
    }
    if let Some(index) = net_name.rfind('[') {
        let bit = net_name[index + 1..]
            .trim_end_matches(']')
            .parse::<usize>()
            .ok()?;
        let base = &net_name[..index];
        let signal = waveform.signal(base)?;
        signal.value_at(time).map(|word| LogicWord {
            bits: vec![word.bit(bit)],
        })
    } else {
        None
    }
}

fn library_inputs(nl: &Netlist, ci: usize) -> Option<(eda_core::library::LibCellId, Vec<usize>)> {
    let CellKind::Lib(id) = &nl.cell(ci).kind else {
        return None;
    };
    let library = nl.library()?;
    let definition = library.get(*id);
    let nets = definition
        .inputs
        .iter()
        .zip(nl.cell(ci).inputs.iter())
        .map(|(_, net)| *net)
        .collect::<Vec<_>>();
    Some((*id, nets))
}

fn average_library_leakage_w(
    nl: &Netlist,
    ci: usize,
    snapshots: &[Vec<LogicValue>],
) -> Option<f64> {
    let library = nl.library()?;
    let CellKind::Lib(id) = &nl.cell(ci).kind else {
        return None;
    };
    let definition = library.get(*id);
    let metadata = library.metadata(&definition.name);
    if metadata.leakage_power == 0.0 && metadata.leakage_states.is_empty() {
        return None;
    }
    let (_, input_nets) = library_inputs(nl, ci)?;
    let mut sum_w = 0.0_f64;
    for snapshot in snapshots {
        let inputs = input_nets
            .iter()
            .map(|&net| snapshot.get(net).copied())
            .collect::<Option<Vec<_>>>()?;
        let state = metadata
            .leakage_states
            .iter()
            .find(|state| state.when.eval(&inputs).as_bool().unwrap_or(false));
        sum_w += state.map_or(metadata.leakage_power, |state| state.power_w);
    }
    Some(sum_w / snapshots.len().max(1) as f64)
}

fn library_internal_energy_pj(
    nl: &Netlist,
    ci: usize,
    snapshots: &[Vec<LogicValue>],
    intervals: usize,
) -> Option<f64> {
    let library = nl.library()?;
    let CellKind::Lib(id) = &nl.cell(ci).kind else {
        return None;
    };
    let definition = library.get(*id);
    let metadata = library.metadata(&definition.name);
    if metadata.internal_power.is_empty() {
        return None;
    }
    let (_, input_nets) = library_inputs(nl, ci)?;
    let mut energy_pj = 0.0_f64;
    for pair in snapshots.windows(2) {
        let (previous, current) = (pair[0].clone(), pair[1].clone());
        for arc in &metadata.internal_power {
            let Some(pin_index) = definition
                .inputs
                .iter()
                .position(|pin| *pin == arc.related_pin)
            else {
                continue;
            };
            let Some(&net) = input_nets.get(pin_index) else {
                continue;
            };
            let (Some(from), Some(to)) = (previous.get(net).copied(), current.get(net).copied())
            else {
                continue;
            };
            if !from.is_01() || !to.is_01() || from == to {
                continue;
            }
            let inputs = input_nets
                .iter()
                .map(|&net| previous.get(net).copied())
                .collect::<Option<Vec<_>>>()?;
            if arc
                .when
                .as_ref()
                .is_some_and(|when| !when.eval(&inputs).as_bool().unwrap_or(false))
            {
                continue;
            }
            energy_pj += if from == LogicValue::Zero {
                arc.rise_power_pj
            } else {
                arc.fall_power_pj
            };
        }
    }
    Some(energy_pj / intervals.max(1) as f64)
}

fn build_report(
    nl: &Netlist,
    activity: &PowerActivity,
    samples: usize,
    intervals: usize,
    capacitance_pf: Option<&BTreeMap<String, f64>>,
    opts: &PowerOptions,
) -> Result<PowerReport, String> {
    let mut cells = Vec::with_capacity(nl.cell_count());
    let mut dynamic_power_uw = 0.0;
    let mut static_power_uw = 0.0;

    for ci in 0..nl.cell_count() {
        let cell = nl.cell(ci);
        let width = width_sites(nl, ci);
        let mut transitions = activity.nets[cell.output].transitions;
        for &input in &cell.inputs {
            transitions = transitions.max(activity.nets[input].transitions);
        }
        let cell_activity = transitions as f64 / intervals as f64;
        let switched_cap_pf = capacitance_pf
            .and_then(|caps| caps.get(&nl.net(cell.output).name))
            .copied()
            .unwrap_or(0.0);
        let library_internal_pj =
            library_internal_energy_pj(nl, ci, &activity.snapshots, intervals);
        let energy_pj = library_internal_pj
            .unwrap_or(opts.base_cell_energy_pj + width as f64 * opts.energy_per_site_pj)
            + switched_cap_pf * opts.capacitive_energy_per_pf_pj;
        let power_activity = if library_internal_pj.is_some() {
            1.0
        } else {
            cell_activity
        };
        // pJ × MHz = µW.
        let power_uw = power_activity * energy_pj * opts.frequency_mhz;
        dynamic_power_uw += power_uw;
        cells.push(CellPower {
            cell: ci,
            name: cell.name.clone(),
            width_sites: width,
            activity: cell_activity,
            dynamic_power_uw: power_uw,
        });
        if let Some(leakage_w) = average_library_leakage_w(nl, ci, &activity.snapshots) {
            static_power_uw += leakage_w * 1_000_000.0;
        } else {
            static_power_uw += width as f64 * opts.leakage_per_site_nw / 1000.0;
        }
    }
    let activity_by_net = (0..nl.net_count())
        .map(|net: NetId| (nl.net(net).name.clone(), activity.nets[net]))
        .collect();

    Ok(PowerReport {
        frequency_mhz: opts.frequency_mhz,
        vectors: samples,
        intervals,
        dynamic_power_uw,
        static_power_uw,
        total_power_uw: dynamic_power_uw + static_power_uw,
        activity_by_net,
        cells,
    })
}

fn activity_from_vectors(nl: &Netlist, vectors: &[InputVector]) -> Result<PowerActivity, String> {
    let design = netlist_to_design(nl);
    let mut sim = Simulator::new(&design)?;
    let mut activity = vec![SwitchingActivity::default(); nl.net_count()];
    let mut snapshots = Vec::with_capacity(vectors.len());
    let mut previous: Option<Vec<eda_core::logic::LogicValue>> = None;

    for (vi, vector) in vectors.iter().enumerate() {
        for port in &nl.ports {
            if port.dir != eda_core::netlist::Dir::In {
                continue;
            }
            let word = vector
                .get(&port.name)
                .ok_or_else(|| format!("vector {}: missing input '{}'", vi, port.name))?;
            sim.set_input(&port.name, word)?;
        }

        let current: Vec<_> = (0..nl.net_count())
            .map(|net| sim.read_net_bit(net))
            .collect();
        snapshots.push(current.clone());
        for net in 0..nl.net_count() {
            let value = current[net];
            if value.is_01() {
                activity[net].known_samples += 1;
                if value == eda_core::logic::LogicValue::One {
                    activity[net].one_samples += 1;
                }
            }
            if let Some(prev) = previous.as_ref().and_then(|p| p.get(net)) {
                if prev.is_01() && value.is_01() && *prev != value {
                    activity[net].transitions += 1;
                }
            }
        }
        previous = Some(current);
    }
    Ok(PowerActivity {
        nets: activity,
        snapshots,
    })
}

fn validate_options(opts: &PowerOptions) -> Result<(), String> {
    let fields = [
        ("frequency", opts.frequency_mhz),
        ("base cell energy", opts.base_cell_energy_pj),
        ("site energy", opts.energy_per_site_pj),
        ("capacitive energy", opts.capacitive_energy_per_pf_pj),
        ("leakage per site", opts.leakage_per_site_nw),
    ];
    for (name, value) in fields {
        if !value.is_finite() || value < 0.0 {
            return Err(format!("invalid power {} {}", name, value));
        }
    }
    if opts.frequency_mhz == 0.0 {
        return Err("power frequency must be positive".into());
    }
    Ok(())
}

fn width_sites(nl: &Netlist, ci: usize) -> usize {
    match &nl.cell(ci).kind {
        CellKind::Lib(id) => nl
            .library()
            .map(|lib| lib.get(*id).width_sites)
            .unwrap_or(2)
            .max(1),
        CellKind::Const(_) => 1,
        _ => 2,
    }
}

/// Human-readable power report.
pub fn report(r: &PowerReport) -> String {
    format!(
        "Power report: {:.6} uW total ({:.6} uW dynamic, {:.6} uW leakage), {} vectors, {:.3} MHz\n",
        r.total_power_uw,
        r.dynamic_power_uw,
        r.static_power_uw,
        r.vectors,
        r.frequency_mhz
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};
    use std::sync::Arc;

    fn inv_netlist() -> Netlist {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("inv");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        nl
    }

    fn vector(a: u64) -> InputVector {
        [("a".to_string(), LogicWord::from_u64(1, a))]
            .into_iter()
            .collect()
    }

    #[test]
    fn toggling_has_dynamic_power() {
        let nl = inv_netlist();
        let r = analyze(&nl, &[vector(0), vector(1)], &PowerOptions::default()).unwrap();
        assert_eq!(r.intervals, 1);
        assert_eq!(r.activity_by_net["a"].transitions, 1);
        assert_eq!(r.activity_by_net["y"].transitions, 1);
        assert!(r.dynamic_power_uw > 0.0);
        assert!(r.static_power_uw > 0.0);
    }

    #[test]
    fn constant_activity_has_leakage_only() {
        let nl = inv_netlist();
        let r = analyze(&nl, &[vector(1), vector(1)], &PowerOptions::default()).unwrap();
        assert_eq!(r.dynamic_power_uw, 0.0);
        assert!(r.static_power_uw > 0.0);
    }

    #[test]
    fn missing_vector_input_is_rejected() {
        let nl = inv_netlist();
        assert!(analyze(&nl, &[HashMap::new()], &PowerOptions::default()).is_err());
    }

    #[test]
    fn vcd_activity_drives_power() {
        let nl = inv_netlist();
        let vcd = eda_waveform::parse_vcd(
            r#"
$timescale 1ns $end
$scope module top $end
$var wire 1 ! a $end
$var wire 1 " y $end
$upscope $end
$enddefinitions $end
#0
0!
1"
#10
1!
0"
"#,
        )
        .unwrap();
        let r = analyze_waveform(&nl, &vcd, &PowerOptions::default()).unwrap();
        assert_eq!(r.intervals, 1);
        assert_eq!(r.activity_by_net["a"].transitions, 1);
        assert_eq!(r.activity_by_net["y"].transitions, 1);
        assert!(r.dynamic_power_uw > 0.0);
    }

    #[test]
    fn switched_capacitance_adds_dynamic_power() {
        let nl = inv_netlist();
        let vectors = vec![vector(0), vector(1)];
        let options = PowerOptions {
            capacitive_energy_per_pf_pj: 2.0,
            ..PowerOptions::default()
        };
        let capacitance = [("y".to_string(), 0.75)].into_iter().collect();
        let without_cap = analyze(&nl, &vectors, &options).unwrap();
        let with_cap = analyze_with_capacitances(&nl, &vectors, &capacitance, &options).unwrap();
        assert!(
            (with_cap.dynamic_power_uw - without_cap.dynamic_power_uw - 150.0).abs() < 1e-12,
            "{} versus {}",
            with_cap.dynamic_power_uw,
            without_cap.dynamic_power_uw
        );
    }

    #[test]
    fn switched_capacitance_is_validated() {
        let nl = inv_netlist();
        let vectors = vec![vector(0), vector(1)];
        let capacitance = [("y".to_string(), -1.0)].into_iter().collect();
        assert!(
            analyze_with_capacitances(&nl, &vectors, &capacitance, &PowerOptions::default())
                .is_err()
        );
    }
}
