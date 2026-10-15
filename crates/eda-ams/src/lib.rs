//! # eda-ams — compact analog and mixed-signal simulation.
//!
//! This crate implements a dependency-free SPICE subset useful for
//! agent-driven mixed-signal exploration:
//!
//! * netlist parse/write round trips;
//! * linear DC operating point and sweeps;
//! * nonlinear diode DC solutions with Newton iteration;
//! * AC phasor sweeps with R/C/L and controlled sources;
//! * small-signal noise sweeps with thermal, shot, and MOS thermal sources;
//! * transient integration with backward-Euler or trapezoidal companions;
//! * CSV waveform export and threshold-crossing queries.

pub mod analysis;
pub mod numeric;
pub mod parser;
pub mod types;

pub use analysis::{
    ac_sweep, aging_analysis, dc_operating_point, dc_operating_point_cancellable, dc_sweep,
    dc_sweep_cancellable, derivative_analysis, envelope_follow, global_sensitivity,
    global_sensitivity_cancellable, monte_carlo, monte_carlo_cancellable, noise_sweep, optimize,
    optimize_cancellable, pareto_optimize, pareto_optimize_cancellable, periodic_steady_state,
    periodic_steady_state_cancellable, sensitivity, transient, transient_cancellable,
};
pub use eda_core::cancellation::FlowCancellation;
pub use parser::{
    format_number, parse_number, parse_resistor_layer_models, parse_spice, parse_statistical_model,
    write_resistor_layer_models, write_spice, write_statistical_model,
};
pub use types::*;

#[derive(Clone, Debug, PartialEq)]
pub enum AmsResult {
    OperatingPoint(DcOperatingPoint),
    DcSweep(DcSweepResult),
    Transient(TransientResult),
    AcSweep(AcResult),
    NoiseSweep(NoiseResult),
    MonteCarlo(MonteCarloResult),
    Sensitivity(SensitivityResult),
    GlobalSensitivity(GlobalSensitivityResult),
    Derivative(DerivativeResult),
    Optimization(OptimizationResult),
    Pareto(ParetoResult),
    PeriodicSteadyState(PssResult),
    EnvelopeFollow(EnvelopeResult),
    Aging(AgingResult),
}

/// Run one parsed analysis and return its typed result.
pub fn run_analysis(circuit: &SpiceCircuit, analysis: &Analysis) -> Result<AmsResult, String> {
    match analysis {
        Analysis::OperatingPoint => Ok(AmsResult::OperatingPoint(dc_operating_point(circuit)?)),
        Analysis::DcSweep { .. } => Ok(AmsResult::DcSweep(dc_sweep(circuit, analysis)?)),
        Analysis::Transient { .. } => Ok(AmsResult::Transient(transient(circuit, analysis)?)),
        Analysis::AcSweep { .. } => Ok(AmsResult::AcSweep(ac_sweep(circuit, analysis)?)),
        Analysis::NoiseSweep { .. } => Ok(AmsResult::NoiseSweep(noise_sweep(circuit, analysis)?)),
        Analysis::MonteCarlo(_) => Ok(AmsResult::MonteCarlo(monte_carlo(circuit, analysis)?)),
        Analysis::Sensitivity(_) => Ok(AmsResult::Sensitivity(sensitivity(circuit, analysis)?)),
        Analysis::Derivative(_) => Ok(AmsResult::Derivative(derivative_analysis(
            circuit, analysis,
        )?)),
        Analysis::GlobalSensitivity(_) => Ok(AmsResult::GlobalSensitivity(global_sensitivity(
            circuit, analysis,
        )?)),
        Analysis::Optimization(_) => Ok(AmsResult::Optimization(optimize(circuit, analysis)?)),
        Analysis::Pareto(_) => Ok(AmsResult::Pareto(pareto_optimize(circuit, analysis)?)),
        Analysis::PeriodicSteadyState(_) => Ok(AmsResult::PeriodicSteadyState(
            periodic_steady_state(circuit, analysis)?,
        )),
        Analysis::EnvelopeFollow(_) => Ok(AmsResult::EnvelopeFollow(envelope_follow(
            circuit, analysis,
        )?)),
        Analysis::Aging(_) => Ok(AmsResult::Aging(aging_analysis(circuit, analysis)?)),
    }
}

/// Run all directives in circuit order.
pub fn run_all(circuit: &SpiceCircuit) -> Result<Vec<AmsResult>, String> {
    circuit
        .analyses
        .iter()
        .map(|analysis| run_analysis(circuit, analysis))
        .collect()
}

pub fn report(result: &AmsResult) -> String {
    match result {
        AmsResult::OperatingPoint(op) => report_operating_point(op),
        AmsResult::DcSweep(sweep) => report_dc_sweep(sweep),
        AmsResult::Transient(tran) => report_transient(tran),
        AmsResult::AcSweep(ac) => report_ac(ac),
        AmsResult::NoiseSweep(noise) => report_noise(noise),
        AmsResult::MonteCarlo(mc) => report_monte_carlo(mc),
        AmsResult::Sensitivity(sens) => report_sensitivity(sens),
        AmsResult::Derivative(derivative) => report_derivative(derivative),
        AmsResult::GlobalSensitivity(global) => report_global_sensitivity(global),
        AmsResult::Optimization(optimization) => report_optimization(optimization),
        AmsResult::Pareto(pareto) => report_pareto(pareto),
        AmsResult::PeriodicSteadyState(pss) => report_pss(pss),
        AmsResult::EnvelopeFollow(envelope) => report_envelope(envelope),
        AmsResult::Aging(aging) => report_aging(aging),
    }
}

pub fn report_all(results: &[AmsResult]) -> String {
    results.iter().map(report).collect::<Vec<_>>().join("\n")
}

pub fn report_operating_point(op: &DcOperatingPoint) -> String {
    let mut out = format!(
        "DC operating point: converged={}, iterations={}\n",
        op.converged, op.iterations
    );
    for (node, voltage) in &op.node_voltages {
        out.push_str(&format!("V({}) = {:.9} V\n", node, voltage));
    }
    for (branch, current) in &op.branch_currents {
        out.push_str(&format!("I({}) = {:.9} A\n", branch, current));
    }
    out
}

pub fn report_dc_sweep(result: &DcSweepResult) -> String {
    let mut out = format!(
        "DC sweep source {}: {} points\n",
        result.source,
        result.points.len()
    );
    for point in &result.points {
        out.push_str(&format!("{:.9}:", point.source_value));
        for (node, voltage) in &point.operating_point.node_voltages {
            out.push_str(&format!(" V({})={:.9}", node, voltage));
        }
        out.push('\n');
    }
    out
}

pub fn report_transient(result: &TransientResult) -> String {
    let mut out = format!(
        "Transient: method={}, step={}s, stop={}s, samples={}, convergence_failures={}\n",
        result.method.as_str(),
        result.step_s,
        result.stop_s,
        result.samples.len(),
        result.convergence_failures
    );
    for sample in &result.samples {
        out.push_str(&format!("t={:.9}s:", sample.time_s));
        for (node, voltage) in &sample.node_voltages {
            out.push_str(&format!(" V({})={:.9}", node, voltage));
        }
        for (element, temperature) in &sample.device_temperatures {
            out.push_str(&format!(" T({})={:.9}K", element, temperature));
        }
        out.push('\n');
    }
    out
}

pub fn report_ac(result: &AcResult) -> String {
    let mut out = format!(
        "AC sweep: {} points ({})\n",
        result.points.len(),
        result.kind.as_str()
    );
    for point in &result.points {
        out.push_str(&format!("f={:.9}Hz:", point.frequency_hz));
        for (node, phasor) in &point.node_phasors {
            out.push_str(&format!(
                " V({})={:.9}/{:.9}deg",
                node,
                phasor.magnitude(),
                phasor.phase_deg()
            ));
        }
        out.push('\n');
    }
    out
}

pub fn report_noise(result: &NoiseResult) -> String {
    let mut out = format!(
        "Noise analysis: output=V({}), input={}, points={}\n",
        result.output_node,
        result.input_source,
        result.points.len()
    );
    for point in &result.points {
        out.push_str(&format!(
            "f={:.9}Hz: output={:.9} V/sqrtHz, input-referred={:.9}/sqrtHz\n",
            point.frequency_hz, point.output_v_per_sqrt_hz, point.input_referred_per_sqrt_hz
        ));
    }
    out
}

pub fn report_monte_carlo(result: &MonteCarloResult) -> String {
    let mut out = format!(
        "Monte Carlo: target={}, output=V({}), samples={}\n",
        result.target.as_str(),
        result.output_node,
        result.samples.len()
    );
    out.push_str(&format!(
        "statistics: min={:.9}, max={:.9}, mean={:.9}, sigma={:.9}\n",
        result.statistics.minimum,
        result.statistics.maximum,
        result.statistics.mean,
        result.statistics.standard_deviation
    ));
    for sample in result.samples.iter().take(20) {
        out.push_str(&format!(
            "sample {}: {:.9} ({} randomized objects)\n",
            sample.index, sample.value, sample.randomized_objects
        ));
    }
    if result.samples.len() > 20 {
        out.push_str(&format!(
            "... and {} more samples\n",
            result.samples.len() - 20
        ));
    }
    out
}

pub fn report_sensitivity(result: &SensitivityResult) -> String {
    let mut out = format!(
        "Sensitivity: target={}, output=V({}), nominal={:.9}\n",
        result.target.as_str(),
        result.output_node,
        result.nominal_output
    );
    for entry in &result.entries {
        out.push_str(&format!(
            "{} ({}): dV/dP={:.9}, normalized={:.9}, nominalP={}, delta={}\n",
            entry.parameter,
            entry.object,
            entry.absolute_sensitivity,
            entry.normalized_sensitivity,
            format_number(entry.nominal_parameter),
            format_number(entry.perturbation)
        ));
    }
    out
}

pub fn report_derivative(result: &DerivativeResult) -> String {
    let mut out = match result.target {
        DerivativeTarget::AcMagnitude => format!(
            "Analytic derivatives: target=ac, frequency={} Hz, output=V({}), nominal magnitude={:.9}\n",
            format_number(result.frequency_hz),
            result.output_node,
            result.nominal_output
        ),
        DerivativeTarget::TransientFinal => format!(
            "Analytic derivatives: target=tran, output=V({}), final transient={:.9}\n",
            result.output_node, result.nominal_output
        ),
        DerivativeTarget::Dc => format!(
            "Analytic derivatives: output=V({}), nominal={:.9}\n",
            result.output_node, result.nominal_output
        ),
    };
    for entry in &result.entries {
        let hessian = entry
            .hessian_row
            .iter()
            .map(|value| format!("{value:.9}"))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!(
            "{}: nominalP={}, gradient={:.9}, hessian=[{}]\n",
            entry.parameter,
            format_number(entry.nominal_parameter),
            entry.gradient,
            hessian
        ));
    }
    out
}

pub fn report_global_sensitivity(result: &GlobalSensitivityResult) -> String {
    let mut out = format!(
        "Global sensitivity: target={}, output=V({}), variance={:.9}\n",
        result.target.as_str(),
        result.output_node,
        result.output_variance
    );
    for entry in &result.entries {
        out.push_str(&format!(
            "{} ({}): first-order={:.9}, total-order={:.9}\n",
            entry.parameter, entry.object, entry.first_order_index, entry.total_order_index
        ));
    }
    out
}

pub fn report_optimization(result: &OptimizationResult) -> String {
    let mut out = format!(
        "Optimization: objective={}, target={}, output=V({}), converged={}\n",
        result.objective.as_str(),
        result.target.as_str(),
        result.output_node,
        result.converged
    );
    out.push_str(&format!(
        "initial={:.9}, final={:.9}, goal={:.9}, score={:.9}, evaluations={}\n",
        result.initial_output,
        result.final_output,
        result.goal_value,
        result.final_score,
        result.evaluations
    ));
    for parameter in &result.parameters {
        out.push_str(&format!(
            "{}: initial={}, final={}, bounds=[{},{}], quantum={}\n",
            parameter.parameter,
            format_number(parameter.initial),
            format_number(parameter.final_value),
            format_number(parameter.lower),
            format_number(parameter.upper),
            parameter
                .quantum
                .map(format_number)
                .unwrap_or_else(|| "continuous".into())
        ));
    }
    out
}

pub fn report_pareto(result: &ParetoResult) -> String {
    let outputs = result
        .output_nodes
        .iter()
        .map(|node| format!("V({node})"))
        .collect::<Vec<_>>()
        .join(",");
    let senses = result
        .senses
        .iter()
        .map(|sense| sense.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let constraints = if result.constraints.is_empty() {
        "none".to_string()
    } else {
        result
            .constraints
            .iter()
            .map(|constraint| {
                format!(
                    "{}{}{}",
                    constraint
                        .expression
                        .as_ref()
                        .map(|expression| expression.format())
                        .unwrap_or_else(|| format!("V({})", constraint.node)),
                    constraint.sense.as_str(),
                    format_number(constraint.bound)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut out = format!(
        "Pareto optimization: target={}, outputs={}, senses={}, front={}, adaptive={}, constraints={}, points={}, evaluations={}\n",
        result.target.as_str(),
        outputs,
        senses,
        result.front_method.as_str(),
        result.adaptive_references,
        constraints,
        result.points.len(),
        result.evaluations
    );
    for (index, point) in result.points.iter().enumerate() {
        let outputs = point
            .outputs
            .iter()
            .map(|value| format!("{value:.9}"))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&format!("point {index}: outputs=({outputs})"));
        out.push_str(&format!(
            ", constraint_violation={:.9}, crowding={}",
            point.constraint_violation,
            match point.crowding_distance {
                value if value.is_infinite() => "inf".to_string(),
                value => format!("{value:.9}"),
            }
        ));
        for parameter in &point.parameters {
            out.push_str(&format!(
                ", {}={}",
                parameter.parameter,
                format_number(parameter.final_value)
            ));
        }
        out.push('\n');
    }
    out
}

pub fn report_pss(result: &PssResult) -> String {
    let mut out = format!(
        "PSS: output=V({}), fundamental={:.9} Hz, cycles={}, converged={}, residual={:.9}\n",
        result.output_node, result.fundamental_hz, result.cycles, result.converged, result.residual
    );
    for harmonic in &result.harmonics {
        out.push_str(&format!(
            "harmonic {}: frequency={:.9} Hz, magnitude={:.9}, phase={:.9} deg\n",
            harmonic.index,
            harmonic.frequency_hz,
            harmonic.phasor.magnitude(),
            harmonic.phasor.phase_deg()
        ));
    }
    out
}

pub fn report_envelope(result: &EnvelopeResult) -> String {
    let mut out = format!(
        "Envelope: output=V({}), carrier={:.9} Hz, window={:.9}s, min={:.9}, max={:.9}, average={:.9}\n",
        result.output_node,
        result.carrier_hz,
        result.window_s,
        result.minimum,
        result.maximum,
        result.average
    );
    for sample in result.samples.iter().take(20) {
        out.push_str(&format!(
            "t={:.9}s: envelope={:.9}, phase={:.9} rad\n",
            sample.time_s, sample.envelope, sample.phase_rad
        ));
    }
    if result.samples.len() > 20 {
        out.push_str(&format!(
            "... and {} more envelope samples\n",
            result.samples.len() - 20
        ));
    }
    out
}

pub fn report_aging(result: &AgingResult) -> String {
    format!(
        "Aging: target={}, output=V({}), years={:.9}, exponent={:.9}, stress={:.9}, nominal={:.9}, aged={:.9}, delta={:.9}\n",
        result.target.as_str(),
        result.output_node,
        result.age_years,
        result.time_exponent,
        result.stress,
        result.nominal_output,
        result.aged_output,
        result.output_delta()
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossingEdge {
    Rising,
    Falling,
    Any,
}

/// Return threshold crossing times using linear interpolation between samples.
pub fn threshold_crossings(
    result: &TransientResult,
    node: &str,
    threshold: f64,
    edge: CrossingEdge,
) -> Result<Vec<f64>, String> {
    let values = result.node_values(node)?;
    if !threshold.is_finite() || values.iter().any(|value| !value.is_finite()) {
        return Err("threshold crossings require finite values".into());
    }
    let mut out = Vec::new();
    for (sample_pair, value_pair) in result.samples.windows(2).zip(values.windows(2)) {
        let (previous, next) = (&sample_pair[0], &sample_pair[1]);
        let (old_value, new_value) = (value_pair[0], value_pair[1]);
        if old_value == new_value {
            continue;
        }
        let rising = old_value < threshold && new_value >= threshold;
        let falling = old_value > threshold && new_value <= threshold;
        let wanted = match edge {
            CrossingEdge::Rising => rising,
            CrossingEdge::Falling => falling,
            CrossingEdge::Any => rising || falling,
        };
        if wanted {
            let fraction = (threshold - old_value) / (new_value - old_value);
            if (0.0..=1.0).contains(&fraction) {
                out.push(previous.time_s + fraction * (next.time_s - previous.time_s));
            }
        }
    }
    Ok(out)
}

/// Export selected node waveforms as CSV. Empty selection exports all nodes.
pub fn write_transient_csv(result: &TransientResult, nodes: &[&str]) -> Result<String, String> {
    write_transient_csv_with_temperatures(result, nodes, &[])
}

/// Export node waveforms and optional lumped device temperatures as CSV.
///
/// Empty node/device selections export every available quantity.
pub fn write_transient_csv_with_temperatures(
    result: &TransientResult,
    nodes: &[&str],
    thermal_devices: &[&str],
) -> Result<String, String> {
    let all_nodes: Vec<String> = result
        .samples
        .first()
        .map(|sample| sample.node_voltages.keys().cloned().collect())
        .unwrap_or_default();
    let all_devices = result.device_names();
    let selected: Vec<String> = if nodes.is_empty() {
        all_nodes
    } else {
        nodes
            .iter()
            .map(|node| {
                if all_nodes.iter().any(|existing| existing == node) {
                    Ok(node.to_string())
                } else {
                    Err(format!("unknown node '{}'", node))
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let selected_devices: Vec<String> = if thermal_devices.is_empty() {
        all_devices
    } else {
        thermal_devices
            .iter()
            .map(|device| {
                if all_devices.iter().any(|existing| existing == device) {
                    Ok(device.to_string())
                } else {
                    Err(format!("unknown thermal device '{}'", device))
                }
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    if selected.is_empty() && selected_devices.is_empty() {
        return Err("cannot export a transient with no nodes or thermal devices".into());
    }
    let mut out = String::from("time_s");
    for node in &selected {
        out.push_str(&format!(",V({})", node));
    }
    for device in &selected_devices {
        out.push_str(&format!(",T({})", device));
    }
    out.push('\n');
    for sample in &result.samples {
        out.push_str(&format!("{:.12}", sample.time_s));
        for node in &selected {
            let value = sample
                .node_voltages
                .get(node)
                .copied()
                .ok_or_else(|| format!("sample has no node '{}'", node))?;
            out.push_str(&format!(",{:.12}", value));
        }
        for device in &selected_devices {
            let value = sample
                .device_temperatures
                .get(device)
                .copied()
                .ok_or_else(|| format!("sample has no thermal device '{}'", device))?;
            out.push_str(&format!(",{:.12}", value));
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
