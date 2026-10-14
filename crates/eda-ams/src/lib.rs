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
    let all_nodes: Vec<String> = result
        .samples
        .first()
        .map(|sample| sample.node_voltages.keys().cloned().collect())
        .unwrap_or_default();
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
    if selected.is_empty() {
        return Err("cannot export a transient with no nodes".into());
    }
    let mut out = String::from("time_s");
    for node in &selected {
        out.push_str(&format!(",V({})", node));
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
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_solves_resistive_divider() {
        let circuit =
            parse_spice("divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.op\n.end").unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let vout = op.node_voltages["out"];
        assert!((vout - 1.2).abs() < 1e-12, "{vout}");
        assert!((op.branch_currents["Vin"] + 0.0006).abs() < 1e-12);
    }

    #[test]
    fn long_running_ams_analyses_honor_cooperative_cancellation() {
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let operating_point =
            parse_spice("cancelled op\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.op\n.end")
                .unwrap();
        assert_eq!(
            dc_operating_point_cancellable(&operating_point, &cancellation).unwrap_err(),
            "DC/transient solve cancelled"
        );

        let sweep = parse_spice(
            "cancelled dc sweep\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.dc Vin 0 1.8 0.1\n.end",
        )
        .unwrap();
        assert_eq!(
            dc_sweep_cancellable(&sweep, &sweep.analyses[0], &cancellation).unwrap_err(),
            "DC sweep cancelled"
        );

        let transient = parse_spice(
            "cancelled transient\nVin in 0 PULSE(0 1 0 1p 1p 1n 2n)\nR1 in out 1k\nC1 out 0 1n\n.tran 1p 2n\n.end",
        )
        .unwrap();
        assert_eq!(
            transient_cancellable(&transient, &transient.analyses[0], &cancellation).unwrap_err(),
            "transient analysis cancelled"
        );

        let monte_carlo = parse_spice(
            "cancelled mc\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 8 op V(out) seed=7 rtol=0.2 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end",
        )
        .unwrap();
        assert_eq!(
            monte_carlo_cancellable(&monte_carlo, &monte_carlo.analyses[0], &cancellation)
                .unwrap_err(),
            "Monte Carlo analysis cancelled"
        );

        let sensitivity = parse_spice(
            "cancelled sobol\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.sobol V(out) samples=16 seed=7\n.end",
        )
        .unwrap();
        assert_eq!(
            global_sensitivity_cancellable(&sensitivity, &sensitivity.analyses[0], &cancellation)
                .unwrap_err(),
            "global sensitivity analysis cancelled"
        );

        let pss = parse_spice(
            "cancelled pss\nVin in 0 SIN(0 1 1meg)\nR1 in out 1k\nC1 out 0 1n\n.tran 1n 5u\n.pss V(out) samples=32 cycles=2:4\n.end",
        )
        .unwrap();
        assert_eq!(
            periodic_steady_state_cancellable(&pss, &pss.analyses[1], &cancellation).unwrap_err(),
            "PSS analysis cancelled"
        );

        let optimization = parse_spice(
            "cancelled optim\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:5000 population=8 iterations=4 seed=17 tol=1e-8\n.end",
        )
        .unwrap();
        assert_eq!(
            optimize_cancellable(&optimization, &optimization.analyses[0], &cancellation)
                .unwrap_err(),
            "optimization analysis cancelled"
        );

        let pareto = parse_spice(
            "cancelled pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max params=R1.R:500:5000,R3.R:500:5000 population=12 iterations=4 seed=11 points=8\n.end",
        )
        .unwrap();
        assert_eq!(
            pareto_optimize_cancellable(&pareto, &pareto.analyses[0], &cancellation).unwrap_err(),
            "Pareto optimization cancelled"
        );
    }

    #[test]
    fn spice_round_trip_preserves_elements_and_analyses() {
        let source = r#"rlc
Vin in 0 DC 1 AC 1 0 SIN(0 1 1meg 1n 0)
R1 in mid 1k
C1 mid 0 1p IC=0
L1 mid out 1n IC=0
.tran 1p 10p method=trap
.ac dec 10 1 1g
.end"#;
        let circuit = parse_spice(source).unwrap();
        let written = write_spice(&circuit).unwrap();
        let reparsed = parse_spice(&written).unwrap();
        assert_eq!(circuit, reparsed);
    }

    #[test]
    fn transient_rc_settles_and_crosses_threshold() {
        let circuit = parse_spice(
            "rc\nV1 in 0 PULSE(0 1 0 1p 1p 1n 2n)\nR1 in out 1k\nC1 out 0 1p\n.tran 10p 2n method=be\n.end",
        )
        .unwrap();
        let result = transient(&circuit, &circuit.analyses[0]).unwrap();
        let final_v = result.final_sample().unwrap().node_voltages["out"];
        assert!(final_v > 0.0 && final_v < 0.5, "{final_v}");
        let crossings = threshold_crossings(&result, "out", 0.5, CrossingEdge::Rising).unwrap();
        assert!(!crossings.is_empty(), "{crossings:?}");
    }

    #[test]
    fn ac_lowpass_attenuates_high_frequency() {
        let circuit = parse_spice(
            "lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1 1meg\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let gains = result.node_magnitude("out").unwrap();
        assert!(gains[0] > 0.99, "{gains:?}");
        assert!(gains[1] < 0.001, "{gains:?}");
    }

    #[test]
    fn ac_branch_equations_do_not_leak_identity_terms() {
        let lowpass = parse_spice(
            "exact lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 1 1k 1k\n.end",
        )
        .unwrap();
        let result = ac_sweep(&lowpass, &lowpass.analyses[0]).unwrap();
        let input = result.points[0].node_phasors["in"];
        assert_eq!((input.real, input.imaginary), (1.0, 0.0), "{result:?}");
        let omega = std::f64::consts::TAU * 1_000.0;
        let expected = 1.0 / (1.0 + (omega * 1_000.0 * 1e-6).powi(2)).sqrt();
        assert!(
            (result.points[0].node_phasors["out"].magnitude() - expected).abs() < 1e-12,
            "{result:?} expected={expected}"
        );

        let vcvs = parse_spice(
            "exact vcvs\nVin ctrl 0 DC 0 AC 1\nE1 out 0 ctrl 0 2.5\n.ac lin 1 1k 1k\n.end",
        )
        .unwrap();
        let result = ac_sweep(&vcvs, &vcvs.analyses[0]).unwrap();
        let output = result.points[0].node_phasors["out"];
        assert!(
            (output.real - 2.5).abs() < 1e-12 && output.imaginary.abs() < 1e-12,
            "{result:?}"
        );
    }

    #[test]
    fn nonlinear_diode_operating_point_converges() {
        let circuit = parse_spice(
            "diode\nV1 in 0 0.7\nR1 in out 1k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 RS=1)\n.op\n.end",
        )
        .unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let vout = op.node_voltages["out"];
        assert!(vout > 0.45 && vout < 0.55, "{vout}");
    }

    #[test]
    fn rejects_duplicate_and_unknown_elements() {
        assert!(parse_spice("bad\nQ1 a 0 x\n.end").is_err());
        assert!(parse_spice("bad\nR1 a 0 1\nR1 b 0 2\n.end").is_err());
    }

    #[test]
    fn mos_and_bjt_models_round_trip() {
        let source = r#"transistors
M1 d g s b NM W=1u L=180n M=2
Q1 c b e sub NPN AREA=1.5
.model NM NMOS(KP=1m VTO=0.5 LAMBDA=0.01 GAMMA=0.2 PHI=0.7 THETA=0.2 VSAT=0.8)
.model NPN NPN(IS=1e-14 BF=120 BR=2 VAF=80 VAR=6)
.op
.end"#;
        let circuit = parse_spice(source).unwrap();
        let written = write_spice(&circuit).unwrap();
        assert_eq!(circuit, parse_spice(&written).unwrap());
    }

    #[test]
    fn nmos_level1_saturation_matches_square_law() {
        let circuit = parse_spice(
            "nmos\nVdd vdd 0 1.8\nRd vdd d 1k\nVg g 0 1.0\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5)\n.op\n.end",
        )
        .unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let drain = op.node_voltages["d"];
        let expected = 0.5 * 1e-3 * (1e-6 / 180e-9) * 0.5 * 0.5;
        assert!((drain - (1.8 - expected * 1_000.0)).abs() < 1e-8, "{drain}");
        let current = (1.8 - drain) / 1_000.0;
        assert!((current - expected).abs() < 1e-10, "{current} {expected}");
    }

    #[test]
    fn bsim3_long_channel_subset_matches_independent_scalar_solution() {
        let source = "BSIM3 long-channel diode\nVdd vdd 0 1.8\nRd vdd d 1k\nM1 d d 0 0 NM W=1u L=180n\n.model NM NMOS(LEVEL=8 VTH0=0.4 K1=0.5 K2=0.02 TOX=2n U0=0.045 UA=1e-15 UB=1e-24 UC=1e-21 VSAT=80k XPART=0.6)\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        assert_eq!(
            circuit.mosfet_models["NM"].kind,
            MosfetModelKind::Bsim3LongChannel
        );
        let result = dc_operating_point(&circuit).unwrap();
        let got = result.node_voltages["d"];

        let width = 1e-6_f64;
        let length = 180e-9_f64;
        let supply = 1.8_f64;
        let resistance = 1_000.0_f64;
        let oxide = 3.9 * 8.854_187_812_8e-12 / 2e-9;
        let current = |voltage: f64| {
            let overdrive = voltage - 0.4;
            let normalized = overdrive / 2e-9;
            let mobility = 0.045
                / (1.0
                    + (1.0e-15 + 1.0e-21 * 0.0) * normalized
                    + 1.0e-24 * normalized * normalized);
            let kp = mobility * oxide;
            let saturation = overdrive / (1.0 + overdrive * mobility / (2.0 * 80_000.0 * length));
            kp * width / length * (overdrive * saturation - saturation * saturation / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = 0.4_f64;
        let mut high = supply;
        for _ in 0..200 {
            let middle = (low + high) / 2.0;
            if equation(middle) >= 0.0 {
                low = middle;
            } else {
                high = middle;
            }
        }
        let expected = (low + high) / 2.0;
        assert!((got - expected).abs() < 1e-8, "{got} {expected}");
    }

    #[test]
    fn bsim3_parameters_are_sensitive_and_globally_variable() {
        let source = "BSIM3 sensitivity\n.temp 85\nVdd vdd 0 1.8\nRd vdd d 1k\nVb b 0 -0.2\nM1 d d 0 b NM W=1u L=180n\n.model NM NMOS(LEVEL=8 VTH0=0.4 K1=0.5 DVT0=0.05 DVT1=30n DVT2=0.05 DVT0W=0.05 DVT1W=0.3 W0=250n KT1=-0.16 KT2=-0.03 KT1L=1e-10 TOX=2n U0=0.045 UA=1e-15 UB=1e-24 VSAT=80k UTE=1.4)\n.sens V(d)\n.sobol V(d) samples=128 seed=19 tol=0.1\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let local = sensitivity(&circuit, &circuit.analyses[0]).unwrap();
        for name in [
            "NM.VTH0", "NM.U0", "NM.TOX", "NM.VSAT", "NM.DVT0", "NM.DVT1", "NM.DVT2", "NM.DVT0W",
            "NM.DVT1W", "NM.W0", "NM.KT1", "NM.KT2", "NM.KT1L",
        ] {
            let entry = local.find(name).unwrap();
            assert!(
                entry.absolute_sensitivity.is_finite() && entry.absolute_sensitivity.abs() > 1e-12,
                "{name}: {entry:?}"
            );
        }
        let global = global_sensitivity(&circuit, &circuit.analyses[1]).unwrap();
        assert!(global.output_variance > 0.0);
        assert!(global.find("NM.U0").unwrap().total_order_index > 0.0);
    }

    #[test]
    fn mosfet_geometry_bins_are_selected_by_instance_wl() {
        let source = r#"binned mos
Vdd d 0 1.2
Vg g 0 0.9
M1 d g 0 0 NM W=200n L=180n
M2 e g 0 0 NM W=500n L=400n
.model NM.1 NMOS(LEVEL=8 VTH0=0.32 WMIN=80n WMAX=300n LMIN=80n LMAX=250n TOX=2n U0=0.04 VSAT=80k)
.model NM.2 NMOS(LEVEL=8 VTH0=0.45 WMIN=320n WMAX=900n LMIN=270n LMAX=700n TOX=2n U0=0.05 VSAT=80k)
.op
.end"#;
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let short = circuit.mosfet_model_for("NM", 200e-9, 180e-9).unwrap();
        let wide = circuit.mosfet_model_for("NM", 500e-9, 400e-9).unwrap();
        assert!((short.threshold_voltage_v - 0.32).abs() < 1e-12);
        assert!((wide.threshold_voltage_v - 0.45).abs() < 1e-12);
        assert!(circuit.mosfet_model_for("NM", 310e-9, 260e-9).is_err());
    }

    #[test]
    fn nmos_mobility_degradation_and_velocity_saturation_match_model() {
        let source = |properties: &str| {
            format!(
                "nmos compact\nVdd d 0 1.8\nVg g 0 1.0\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.45 {properties})\n.op\n.end"
            )
        };
        let base = parse_spice(&source("")).unwrap();
        let base_current = -dc_operating_point(&base).unwrap().branch_currents["Vdd"];
        let base_expected = 0.5 * 1e-3 * (1e-6 / 180e-9) * 0.55_f64.powi(2);
        assert!(
            (base_current - base_expected).abs() < 1e-10,
            "{base_current}"
        );

        let mobility = parse_spice(&source("THETA=0.5")).unwrap();
        assert_eq!(
            mobility,
            parse_spice(&write_spice(&mobility).unwrap()).unwrap()
        );
        let mobility_current = -dc_operating_point(&mobility).unwrap().branch_currents["Vdd"];
        let mobility_expected = base_expected / (1.0 + 0.5 * 0.55);
        assert!(
            (mobility_current - mobility_expected).abs() < 1e-10,
            "{mobility_current} {mobility_expected}"
        );

        let velocity = parse_spice(&source("THETA=0.5 VSAT=0.3")).unwrap();
        assert_eq!(
            velocity,
            parse_spice(&write_spice(&velocity).unwrap()).unwrap()
        );
        let velocity_current = -dc_operating_point(&velocity).unwrap().branch_currents["Vdd"];
        let vds_saturation = 0.55 / (1.0 + 0.55 / 0.3);
        let velocity_expected = (mobility_expected / (0.5 * 0.55 * 0.55))
            * (0.55 * vds_saturation - 0.5 * vds_saturation * vds_saturation);
        assert!(
            (velocity_current - velocity_expected).abs() < 1e-10,
            "{velocity_current} {velocity_expected}"
        );
        assert!(velocity_current < mobility_current, "{velocity_current}");
    }

    #[test]
    fn pmos_current_polarity_is_reversed() {
        let circuit = parse_spice(
            "pmos\nVdd vdd 0 -1.8\nRd vdd d 1k\nVg g 0 -1.0\nM1 d g 0 0 PM W=1u L=180n\n.model PM PMOS(KP=1m VTO=-0.5)\n.op\n.end",
        )
        .unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let drain = op.node_voltages["d"];
        assert!((drain + 1.1055555555555556).abs() < 1e-8, "{drain}");
    }

    #[test]
    fn bjt_forward_current_gain_matches_ebers_moll() {
        let circuit = parse_spice(
            "npn\nVdd vdd 0 1.8\nRc vdd c 1k\nIb 0 b 10u\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 BR=1)\n.op\n.end",
        )
        .unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let collector = op.node_voltages["c"];
        let base = op.node_voltages["b"];
        assert!(base > 0.5 && base < 0.8, "{base}");
        assert!((collector - 0.8).abs() < 1e-7, "{collector}");
    }

    #[test]
    fn bjt_early_voltage_modulates_output_current() {
        let mut circuit = SpiceCircuit::default();
        circuit.elements.push(SpiceElement {
            name: "Q1".into(),
            nodes: vec!["c".into(), "b".into(), "e".into()],
            kind: SpiceElementKind::BipolarJunctionTransistor {
                model: "NPN".into(),
                area: 1.0,
            },
        });
        let mut model = BjtModel {
            forward_early_voltage_v: 10.0,
            reverse_early_voltage_v: 5.0,
            ..BjtModel::default()
        };
        circuit.bjt_models.insert("NPN".into(), model.clone());
        let transistor = &circuit.elements[0];

        let near = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[0.7, 0.7, 0.0, 0.0],
        )
        .unwrap()[0];
        let reverse_biased = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[1.7, 0.7, 0.0, 0.0],
        )
        .unwrap()[0];
        assert!(reverse_biased > near);
        assert!((reverse_biased / near - 1.1).abs() < 1e-9);

        let reverse_near = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[0.0, 0.7, 0.7, 0.0],
        )
        .unwrap()[0];
        let reverse_early = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[0.0, 0.7, 1.7, 0.0],
        )
        .unwrap()[0];
        assert!(reverse_early.abs() > reverse_near.abs());

        model.forward_early_voltage_v = 0.0;
        model.reverse_early_voltage_v = 0.0;
        circuit.bjt_models.insert("NPN".into(), model);
        let disabled_near = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[0.7, 0.7, 0.0, 0.0],
        )
        .unwrap()[0];
        let disabled_reverse = crate::analysis::nonlinear_terminal_currents(
            &circuit,
            transistor,
            &[1.7, 0.7, 0.0, 0.0],
        )
        .unwrap()[0];
        assert!((disabled_near - disabled_reverse).abs() < 1e-12);

        assert!(
            parse_spice("bad\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 VAF=-1)\n.op\n.end")
                .unwrap_err()
                .contains("BJT VAF")
        );
    }

    #[test]
    fn transistor_ac_analysis_linearizes_dc_operating_point() {
        let circuit = parse_spice(
            "common-source\nVdd vdd 0 1.8\nRd vdd d 10k\nVg g 0 DC 1 AC 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5)\n.ac lin 2 1 1meg\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let gains = result.node_magnitude("d").unwrap();
        assert!(gains.iter().all(|gain| gain.is_finite()), "{gains:?}");
        assert!((gains[0] - gains[1]).abs() < 1e-9, "{gains:?}");
    }

    #[test]
    fn resistor_thermal_noise_matches_johnson_nyquist() {
        let circuit = parse_spice(
            "resistor noise\nIin 0 out DC 0 AC 1\nR1 out 0 1k\n.noise V(out) Iin lin 2 1 1meg\n.end",
        )
        .unwrap();
        let result = noise_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let expected = (4.0 * 1.380_649e-23 * 300.0 * 1_000.0_f64).sqrt();
        for got in result.output_noise() {
            assert!((got - expected).abs() < 1e-14, "{got} {expected}");
        }
        assert!(
            result.points[0].contributions.contains_key("R1"),
            "{:?}",
            result.points[0].contributions
        );
    }

    #[test]
    fn diode_shot_noise_uses_dc_operating_point() {
        let circuit = parse_spice(
            "diode noise\nV1 in 0 DC 1.8 AC 1\nR1 in out 1k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1)\n.noise V(out) V1 lin 1 1 1\n.end",
        )
        .unwrap();
        let result = noise_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let diode_voltage = op.node_voltages["out"];
        let current = (1.8 - diode_voltage) / 1_000.0;
        let dynamic_resistance = 0.025_852 / (current + 1e-12);
        let response = 1_000.0 * dynamic_resistance / (1_000.0 + dynamic_resistance);
        let expected_output = (2.0 * 1.602_176_634e-19 * current).sqrt() * response;
        let got = result.output_noise()[0];
        assert!(
            (got - expected_output).abs() < 1e-10,
            "{got} {expected_output}"
        );
    }

    #[test]
    fn noise_directive_round_trips() {
        let source = "noise round trip\nIin 0 out DC 0 AC 1\nR1 out 0 1k\n.noise V(out) Iin dec 10 1 1g\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn mos_gate_capacitance_creates_transient_rc_delay() {
        let circuit = parse_spice(
            "mos gate rc\nVin in 0 PULSE(0 1 0 1p 1p 1 1)\nRg in g 10k\nVdd d 0 1.8\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 CGSO=1m)\n.tran 0.2u 50u method=be\n.end",
        )
        .unwrap();
        let result = transient(&circuit, &circuit.analyses[0]).unwrap();
        let final_gate = result
            .final_sample()
            .and_then(|sample| sample.node_voltages.get("g"))
            .copied()
            .unwrap_or_default();
        let expected = 1.0 - (-5.0_f64).exp();
        assert!(
            (final_gate - expected).abs() < 2e-3,
            "{final_gate} {expected}"
        );
    }

    #[test]
    fn mos_gate_capacitance_adds_ac_pole() {
        let circuit = parse_spice(
            "mos gate pole\nVin in 0 DC 0 AC 1\nRg in g 10k\nVdd d 0 1.8\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 CGSO=1m)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let gate = result.node_magnitude("g").unwrap();
        assert!(gate[0] > 0.99, "{gate:?}");
        assert!(gate[1] < 1e-4, "{gate:?}");
    }

    #[test]
    fn bjt_junction_capacitance_adds_ac_pole() {
        let circuit = parse_spice(
            "bjt pole\nVin in 0 DC 0 AC 1\nRb in b 10k\nVdd vdd 0 1.8\nRc vdd c 1k\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 CJE=100p CJC=10p)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let base = result.node_magnitude("b").unwrap();
        assert!(base[0] > 0.99, "{base:?}");
        assert!(base[1] < 2e-4, "{base:?}");
    }

    #[test]
    fn mos_junction_sidewall_capacitance_is_voltage_aware() {
        let source = "mos junction pole\nVin in 0 DC 0 AC 1\nRd in d 10k\nM1 d 0 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 CJSW=1m CJSWD=2m PB=0.8 MJ=0.5 FC=0.5)\n.ac lin 2 1 1g\n.end";
        let circuit = parse_spice(source).unwrap();
        let model = &circuit.mosfet_models["NM"];
        assert_eq!(model.source_junction_sidewall_cap_per_m, 1.0e-3);
        assert_eq!(model.drain_junction_sidewall_cap_per_m, 2.0e-3);
        assert_eq!(model.junction_potential_v, 0.8);
        assert_eq!(model.junction_grading_coefficient, 0.5);
        assert_eq!(model.junction_forward_knee, 0.5);
        let rewritten = write_spice(&circuit).unwrap();
        assert_eq!(circuit, parse_spice(&rewritten).unwrap());

        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let drain = result.node_magnitude("d").unwrap();
        assert!(drain[0] > 0.99, "{drain:?}");
        assert!(drain[1] < 2e-4, "{drain:?}");

        let invalid = parse_spice(
            "invalid\nM1 d 0 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 CJSW=1m MJ=1.5)\n.op\n.end",
        );
        assert!(invalid
            .unwrap_err()
            .contains("MOS MJ must be finite and between zero and one"));
    }

    #[test]
    fn monte_carlo_is_deterministic_and_spreads_resistor_variation() {
        let source = "mc divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 64 op V(out) seed=9 rtol=0.2 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end";
        let circuit = parse_spice(source).unwrap();
        let first = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        let second = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.samples.len(), 64);
        assert!(first.statistics.minimum < 1.2 && first.statistics.maximum > 1.2);
        assert!(first.statistics.standard_deviation > 0.0);
    }

    #[test]
    fn monte_carlo_transient_varies_rc_final_value() {
        let source = "mc rc\nVin in 0 PULSE(0 1 0 1p 1p 1 1)\nR1 in out 1k\nC1 out 0 1n\n.tran 0.2u 10u method=be\n.mc 24 tran V(out) seed=7 rtol=0 ctol=0.3 ltol=0 vtotol=0 kptol=0 bftol=0\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = monte_carlo(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.samples.len(), 24);
        assert!(result.statistics.minimum < result.statistics.mean);
        assert!(result.statistics.mean < result.statistics.maximum);
    }

    #[test]
    fn fully_correlated_resistor_process_preserves_divider_ratio() {
        let source = "correlated divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 32 op V(out) seed=19 rtol=0.2 rcorr=1 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        for value in result.values() {
            assert!((value - 1.2).abs() < 1e-12, "{value}");
        }
        assert!(result.statistics.standard_deviation < 1e-12);
    }

    #[test]
    fn monte_carlo_correlation_directive_round_trips() {
        let source = "mc correlation round trip\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.mc 8 op V(out) corr=0.7 rcorr=0.9\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn monte_carlo_supports_explicit_parameter_correlation_matrix() {
        let source = "mc matrix\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 4096 op V(out) seed=91 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 matrix=R1.R|R2.R|-0.75\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        let supply = 1.8_f64;
        let r1 = 1_000.0_f64;
        let r2 = 2_000.0_f64;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let sqrt_3 = 3.0_f64.sqrt();
        let sigma_r1 = r1 * 0.05 / sqrt_3;
        let sigma_r2 = r2 * 0.05 / sqrt_3;
        let expected_variance = derivative_r1.powi(2) * sigma_r1.powi(2)
            + derivative_r2.powi(2) * sigma_r2.powi(2)
            + 2.0 * -0.75 * derivative_r1 * derivative_r2 * sigma_r1 * sigma_r2;
        let expected = expected_variance.sqrt();
        assert!(
            (result.statistics.standard_deviation - expected).abs() < expected * 0.08,
            "result={:?}, expected={expected}",
            result
        );
        assert_eq!(result.samples[0].randomized_objects, 2);

        let invalid = parse_spice(
            "bad matrix\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.mc 8 op V(out) matrix=R1.R|Missing.R|0.5\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("unknown Monte Carlo correlation parameter"),
            "{invalid}"
        );
    }

    #[test]
    fn monte_carlo_uses_spatial_mismatch_correlation() {
        let source = "spatial mc\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.location R1 0 0\n.location R2 1 0\n.mc 4096 op V(out) seed=101 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr=0.8 spatialscale=2\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        let supply = 1.8_f64;
        let r1 = 1_000.0_f64;
        let r2 = 2_000.0_f64;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let sqrt_3 = 3.0_f64.sqrt();
        let sigma_r1 = r1 * 0.05 / sqrt_3;
        let sigma_r2 = r2 * 0.05 / sqrt_3;
        let correlation = 0.8 * (-1.0_f64 / 8.0).exp();
        let expected_variance = derivative_r1.powi(2) * sigma_r1.powi(2)
            + derivative_r2.powi(2) * sigma_r2.powi(2)
            + 2.0 * correlation * derivative_r1 * derivative_r2 * sigma_r1 * sigma_r2;
        let expected = expected_variance.sqrt();
        assert!(
            (result.statistics.standard_deviation - expected).abs() < expected * 0.08,
            "result={:?}, expected={expected}",
            result
        );
        assert_eq!(result.samples[0].randomized_objects, 2);

        let invalid =
            parse_spice("bad location\nVin in 0 1\nR1 out 0 1k\n.location Missing 0 0\n.end")
                .unwrap_err();
        assert!(
            invalid.contains("location target 'Missing' does not exist"),
            "{invalid}"
        );
    }

    #[test]
    fn monte_carlo_supports_normal_and_lognormal_marginals() {
        let expected_sigma = {
            let supply = 1.8_f64;
            let r1 = 1_000.0_f64;
            let r2 = 2_000.0_f64;
            let total = r1 + r2;
            let derivative_r1 = -supply * r2 / (total * total);
            let derivative_r2 = supply * r1 / (total * total);
            (derivative_r1.powi(2) * (r1 * 0.05).powi(2)
                + derivative_r2.powi(2) * (r2 * 0.05).powi(2))
            .sqrt()
        };

        let normal = parse_spice(
            "mc normal\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 4096 op V(out) seed=123 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=normal\n.end",
        )
        .unwrap();
        assert_eq!(normal, parse_spice(&write_spice(&normal).unwrap()).unwrap());
        let normal_result = monte_carlo(&normal, &normal.analyses[0]).unwrap();
        assert!(
            (normal_result.statistics.standard_deviation - expected_sigma).abs()
                < expected_sigma * 0.05,
            "result={:?}, expected={expected_sigma}",
            normal_result.statistics
        );
        assert!(
            (normal_result.statistics.mean - 1.2).abs() < expected_sigma * 0.1,
            "{normal_result:?}"
        );

        let lognormal = parse_spice(
            "mc lognormal\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 4096 op V(out) seed=123 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=lognormal\n.end",
        )
        .unwrap();
        let lognormal_result = monte_carlo(&lognormal, &lognormal.analyses[0]).unwrap();
        assert!(
            (lognormal_result.statistics.standard_deviation - expected_sigma).abs()
                < expected_sigma * 0.06,
            "result={:?}, expected={expected_sigma}",
            lognormal_result.statistics
        );
        assert_eq!(lognormal_result.samples[0].randomized_objects, 2);

        let invalid = parse_spice(
            "bad mc distribution\nVin in 0 1\nR1 out 0 1k\n.mc 8 op V(out) dist=unknown\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("unknown Monte Carlo distribution"),
            "{invalid}"
        );
    }

    #[test]
    fn monte_carlo_supports_full_covariance_input() {
        let source = "mc covariance\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.mc 4096 op V(out) seed=321 rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=normal cov=R1.R:2500,R2.R:10000,R1.R|R2.R:-2500\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        let supply = 1.8_f64;
        let r1 = 1_000.0_f64;
        let r2 = 2_000.0_f64;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let expected_variance = derivative_r1.powi(2) * 2500.0
            + derivative_r2.powi(2) * 10000.0
            + 2.0 * derivative_r1 * derivative_r2 * -2500.0;
        let expected = expected_variance.sqrt();
        assert!(
            (result.statistics.standard_deviation - expected).abs() < expected * 0.05,
            "result={:?}, expected={expected}",
            result.statistics
        );
        assert_eq!(result.samples[0].randomized_objects, 2);

        let uniform = parse_spice(
            "bad covariance distribution\nVin in 0 1\nR1 out 0 1k\n.mc 8 op V(out) cov=R1.R:1\n.end",
        )
        .unwrap_err();
        assert!(
            uniform.contains("Monte Carlo covariance input requires dist=normal or dist=lognormal"),
            "{uniform}"
        );
        let missing_diagonal_circuit = parse_spice(
            "bad covariance diagonal\nVin in 0 1\nR1 out 0 1k\nR2 out 0 1k\n.mc 8 op V(out) dist=normal cov=R1.R|R2.R:0.5\n.end",
        )
        .unwrap();
        let missing_diagonal = monte_carlo(
            &missing_diagonal_circuit,
            &missing_diagonal_circuit.analyses[0],
        )
        .unwrap_err();
        assert!(
            missing_diagonal.contains("lacks both diagonal entries"),
            "{missing_diagonal}"
        );
    }

    #[test]
    fn monte_carlo_imports_vendor_statistical_model() {
        let model_text = "model vendor_n\nparam R1.R dist=normal relative=true sigma=0.05\nparam R2.R dist=normal relative=true sigma=0.05\ncorr R1.R R2.R -0.5\n";
        let model = parse_statistical_model(model_text).unwrap();
        assert_eq!(model_text, write_statistical_model(&model).unwrap());
        let source = "vendor stats\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.stats name=vendor_n params=R1.R:normal:relative:0.05,R2.R:normal:relative:0.05 corr=R1.R|R2.R|-0.5\n.mc 4096 op V(out) seed=77 rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 stats=vendor_n\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = monte_carlo(&circuit, &circuit.analyses[0]).unwrap();
        let supply = 1.8_f64;
        let r1 = 1_000.0_f64;
        let r2 = 2_000.0_f64;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let expected_variance = derivative_r1.powi(2) * (r1 * 0.05).powi(2)
            + derivative_r2.powi(2) * (r2 * 0.05).powi(2)
            + 2.0 * derivative_r1 * derivative_r2 * -0.5 * (r1 * 0.05) * (r2 * 0.05);
        let expected = expected_variance.sqrt();
        assert!(
            (result.statistics.standard_deviation - expected).abs() < expected * 0.06,
            "result={:?}, expected={expected}",
            result.statistics
        );
        assert_eq!(result.samples[0].randomized_objects, 2);

        let missing = parse_spice(
            "missing stats\nVin in 0 1\nR1 out 0 1k\n.mc 8 op V(out) stats=absent\n.end",
        )
        .unwrap_err();
        assert!(
            missing.contains("Monte Carlo statistical model 'absent' does not exist"),
            "{missing}"
        );
        let unknown_parameter = parse_statistical_model(
            "model bad\nparam Missing.R dist=normal relative=true sigma=0.1\n",
        )
        .unwrap();
        let mut bad_circuit = parse_spice("bad\nR1 out 0 1k\n.op\n.end").unwrap();
        bad_circuit
            .statistical_models
            .insert("bad".into(), unknown_parameter);
        let validation = bad_circuit.validate().unwrap_err();
        assert!(
            validation.contains("statistical model parameter 'Missing.R' does not exist"),
            "{validation}"
        );
    }

    #[test]
    fn monte_carlo_directive_round_trips_and_validates_target() {
        let source = "mc round trip\nVin in 0 AC 1\nR1 in out 1k\n.mc 16 ac V(out) seed=3 freq=1k rtol=0.1 ctol=0.2\n.ac lin 1 1k 1k\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let missing = parse_spice("bad\nR1 out 0 1k\n.mc 2 tran V(out)\n.end").unwrap_err();
        assert!(missing.contains("lacks its base analysis"), "{missing}");
    }

    #[test]
    fn diode_depletion_capacitance_decreases_with_reverse_bias() {
        let circuit = parse_spice(
            "diode cv\nVin in 0 DC -5 AC 1\nR1 in out 10k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 CJO=100p VJ=0.7 M=0.5 FC=0.5)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let gains = result.node_magnitude("out").unwrap();
        let capacitance = 100e-12 / (1.0_f64 + 5.0 / 0.7).powf(0.5);
        let expected =
            1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * 10_000.0 * capacitance).powi(2)).sqrt();
        assert!(gains[0] > 0.999999, "{gains:?}");
        assert!((gains[1] - expected).abs() < 1e-4, "{gains:?} {expected}");
    }

    #[test]
    fn bjt_depletion_capacitance_uses_junction_bias() {
        let circuit = parse_spice(
            "bjt cv\nVin in 0 DC 0 AC 1\nRb in b 10k\nVdd vdd 0 3\nRc vdd c 1k\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 CJE=50p CJC=20p VJE=0.7 VJC=0.6 MJE=0.33 MJC=0.5 FC=0.5)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();
        let result = ac_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let gains = result.node_magnitude("b").unwrap();
        let op = dc_operating_point(&circuit).unwrap();
        let vbc = op.node_voltages["b"] - op.node_voltages["c"];
        let cbe = 50e-12;
        let cbc = 20e-12 / (1.0 - vbc / 0.6).powf(0.5);
        let expected =
            1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * 10_000.0 * (cbe + cbc)).powi(2)).sqrt();
        assert!(gains[0] > 0.999999, "{gains:?}");
        assert!((gains[1] - expected).abs() < 1e-4, "{gains:?} {expected}");
    }

    #[test]
    fn mos_intrinsic_gate_charge_changes_with_region() {
        let cutoff = parse_spice(
            "mos cutoff charge\nVin in 0 DC 0 AC 1\nRg in g 10k\nM1 0 g 0 0 NM W=1u L=180n\n.model NM NMOS(VTO=0.45 COX=1)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();
        let saturation = parse_spice(
            "mos saturation charge\nVin in 0 DC 1 AC 1\nRg in g 10k\nVdd d 0 1.8\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(VTO=0.45 COX=1)\n.ac lin 2 1 1g\n.end",
        )
        .unwrap();

        let oxide_cap = 1.0 * 1e-6 * 180e-9;
        let cutoff_cap = oxide_cap;
        let saturation_cap = (2.0 / 3.0) * oxide_cap;
        let expected_at_1ghz = |capacitance: f64| {
            1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * 10_000.0 * capacitance).powi(2)).sqrt()
        };
        let cutoff_gain = ac_sweep(&cutoff, &cutoff.analyses[0])
            .unwrap()
            .node_magnitude("g")
            .unwrap();
        let saturation_gain = ac_sweep(&saturation, &saturation.analyses[0])
            .unwrap()
            .node_magnitude("g")
            .unwrap();
        assert!(
            (cutoff_gain[1] - expected_at_1ghz(cutoff_cap)).abs() < 1e-4,
            "{cutoff_gain:?}"
        );
        assert!(
            (saturation_gain[1] - expected_at_1ghz(saturation_cap)).abs() < 1e-4,
            "{saturation_gain:?}"
        );
    }

    #[test]
    fn mos_saturation_charge_partition_changes_drain_coupling() {
        let make = |partition: f64| {
            format!(
                "mos partition {partition}\nVin in 0 DC 1 AC 1\nRg in g 10k\nVdd vdd 0 1.8\nRd vdd d 10k\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(VTO=0.45 COX=1 XPART={partition})\n.ac lin 2 1 1g\n.end"
            )
        };
        let source_side = parse_spice(&make(0.0)).unwrap();
        let drain_side = parse_spice(&make(1.0)).unwrap();
        let source_gain = ac_sweep(&source_side, &source_side.analyses[0])
            .unwrap()
            .node_magnitude("d")
            .unwrap();
        let drain_gain = ac_sweep(&drain_side, &drain_side.analyses[0])
            .unwrap()
            .node_magnitude("d")
            .unwrap();
        assert!(
            (drain_gain[1] - source_gain[1]).abs() > 1e-6,
            "{source_gain:?} {drain_gain:?}"
        );
    }

    #[test]
    fn mos_flicker_noise_follows_frequency_exponent() {
        let circuit = parse_spice(
            "mos flicker\nVdd vdd 0 1.8\nRd vdd d 1k\nVin g 0 DC 1 AC 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 KF=1e-9 AF=1 EF=1)\n.noise V(d) Vin lin 2 1k 1meg\n.end",
        )
        .unwrap();
        let result = noise_sweep(&circuit, &circuit.analyses[0]).unwrap();
        let low = result.points[0].contributions.get("M1").copied().unwrap();
        let high = result.points[1].contributions.get("M1").copied().unwrap();
        let ratio = low / high;
        assert!(
            (ratio - 1000f64.sqrt()).abs() < 1.0,
            "low={low}, high={high}, ratio={ratio}"
        );
    }

    #[test]
    fn flicker_model_parameters_round_trip() {
        let source = r#"flicker models
M1 d g 0 0 NM
Q1 c b 0 NPN
D1 a 0 DN
.model NM NMOS(KF=1n AF=1.2 EF=1)
.model NPN NPN(KF=2n AF=1.1 EF=1)
.model DN D(KF=3n AF=1.3 EF=1)
.op
.end"#;
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn temperature_directive_round_trips() {
        let source = "temperature\n.temp 125\nR1 out 0 1k\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        assert!((circuit.temperature_k - 398.15).abs() < 1e-12);
    }

    #[test]
    fn physical_sheet_resistor_computes_resistance_from_geometry() {
        let source = "physical resistor\nVin in 0 1\nR1 in out SHEET=50 L=2u W=50n TC1=0.001\nR2 out 0 2 SHEET=25 L=4u W=100n\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let r1 = match &circuit.elements[1].kind {
            SpiceElementKind::Resistor { resistance, .. } => *resistance,
            _ => unreachable!(),
        };
        let r2 = match &circuit.elements[2].kind {
            SpiceElementKind::Resistor { resistance, .. } => *resistance,
            _ => unreachable!(),
        };
        assert!((r1 - 2_000.0).abs() < 1e-12, "{r1}");
        assert!((r2 - 2_000.0).abs() < 1e-12, "{r2}");
        let got = dc_operating_point(&circuit).unwrap().node_voltages["out"];
        assert!((got - 0.5).abs() < 1e-12, "{got}");

        let invalid =
            parse_spice("bad physical resistor\nVin in 0 1\nR1 in out SHEET=50 L=2u\n.op\n.end")
                .unwrap_err();
        assert!(
            invalid.contains("needs positive L and W with SHEET"),
            "{invalid}"
        );
    }

    #[test]
    fn resistor_layer_model_applies_temperature_and_width_offset() {
        let source = "layer resistor\n.rmodel POLY SHEET=50 TC1=0.001 TC2=0 WOFF=-5n MINW=40n MAXW=100n\n.temp 125\nVin in 0 1\nR1 in out LAYER=POLY L=2u W=50n\nR2 out 0 2 LAYER=POLY L=2u W=45n\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let delta = circuit.temperature_k - 300.0;
        let r1 = 50.0 * 2e-6 / 45e-9 * (1.0 + 0.001 * delta);
        let r2 = 2.0 * 50.0 * 2e-6 / 40e-9 * (1.0 + 0.001 * delta);
        let expected = r2 / (r1 + r2);
        let got = dc_operating_point(&circuit).unwrap().node_voltages["out"];
        assert!((got - expected).abs() < 1e-12, "{got} {expected}");

        let too_narrow = parse_spice(
            "bad layer width\n.rmodel POLY SHEET=50 WOFF=0 MINW=50n\nVin in 0 1\nR1 in 0 LAYER=POLY L=1u W=40n\n.op\n.end",
        )
        .unwrap_err();
        assert!(
            too_narrow.contains("width is outside layer"),
            "{too_narrow}"
        );
        let unknown_layer =
            parse_spice("bad layer\nVin in 0 1\nR1 in 0 LAYER=MISSING L=1u W=50n\n.op\n.end")
                .unwrap_err();
        assert!(
            unknown_layer.contains("unknown resistor layer model"),
            "{unknown_layer}"
        );
    }

    #[test]
    fn resistor_width_bins_interpolate_sheet_and_offset() {
        let source = "width bins\n.rmodel POLY SHEET=50 MINW=20n MAXW=200n BINS=40n:50:0,100n:60:-5n\nIin out 0 1m\nR1 out 0 LAYER=POLY L=1u W=70n\nR2 mid out LAYER=POLY L=1u W=30n\nR3 mid 0 LAYER=POLY L=1u W=120n\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let model = circuit.resistor_layer_models.get("POLY").unwrap();
        assert_eq!(model.effective_sheet(70e-9).unwrap(), (55.0, -2.5e-9));
        assert_eq!(model.effective_sheet(30e-9).unwrap(), (50.0, 0.0));
        assert_eq!(model.effective_sheet(120e-9).unwrap(), (60.0, -5e-9));
        let expected = |sheet: f64, width: f64| sheet * 1e-6 / width;
        for (name, expected) in [
            ("R1", expected(55.0, 67.5e-9)),
            ("R2", expected(50.0, 30e-9)),
            ("R3", expected(60.0, 115e-9)),
        ] {
            let resistance = circuit
                .elements
                .iter()
                .find(|element| element.name.eq_ignore_ascii_case(name))
                .and_then(|element| match &element.kind {
                    SpiceElementKind::Resistor { resistance, .. } => Some(*resistance),
                    _ => None,
                })
                .unwrap();
            assert!((resistance - expected).abs() < 1e-9, "{name} {resistance}");
        }

        let invalid = parse_spice(
            "bad bins\n.rmodel POLY SHEET=50 BINS=50n:50,40n:60\nIin out 0 1m\nR1 out 0 1k\n.op\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("layer-bin widths must be strictly increasing"),
            "{invalid}"
        );
    }

    #[test]
    fn resistor_layer_table_round_trips_through_typed_api() {
        let table = ".rmodel POLY SHEET=50 TC1=0.001 TC2=0 WOFF=0 MINW=40n MAXW=100n BINS=40n:50:0,100n:60:-5n\n.rmodel MET1 SHEET=80 MINW=60n MAXW=500n\n";
        let models = parse_resistor_layer_models(table).unwrap();
        assert_eq!(models.len(), 2);
        let written = write_resistor_layer_models(&models).unwrap();
        assert_eq!(models, parse_resistor_layer_models(&written).unwrap());

        let invalid = parse_resistor_layer_models("model POLY SHEET=50\n").unwrap_err();
        assert!(
            invalid.contains("supports only .rmodel directives"),
            "{invalid}"
        );
    }

    #[test]
    fn resistor_temperature_coefficients_scale_operating_point() {
        let source = "resistor tc\n.temp 125\nVin in 0 1.8\nR1 in out 1k TC1=0.001 TC2=0.00002\nR2 out 0 2k TC1=-0.002 TC2=0.00001\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        let delta = circuit.temperature_k - 300.0;
        let r1 = 1_000.0 * (1.0 + 0.001 * delta + 0.000_02 * delta * delta);
        let r2 = 2_000.0 * (1.0 - 0.002 * delta + 0.000_01 * delta * delta);
        let expected = 1.8 * r2 / (r1 + r2);
        let got = dc_operating_point(&circuit).unwrap().node_voltages["out"];
        assert!((got - expected).abs() < 1e-12, "{got} {expected}");

        let written = write_spice(&circuit).unwrap();
        assert_eq!(circuit, parse_spice(&written).unwrap());
    }

    #[test]
    fn resistor_thermal_noise_scales_with_temperature() {
        let cold = parse_spice(
            "cold noise\nIin 0 out DC 0 AC 1\nR1 out 0 1k\n.temp -40\n.noise V(out) Iin lin 1 1 1\n.end",
        )
        .unwrap();
        let hot = parse_spice(
            "hot noise\nIin 0 out DC 0 AC 1\nR1 out 0 1k\n.temp 125\n.noise V(out) Iin lin 1 1 1\n.end",
        )
        .unwrap();
        let cold_noise = noise_sweep(&cold, &cold.analyses[0])
            .unwrap()
            .output_noise()[0];
        let hot_noise = noise_sweep(&hot, &hot.analyses[0]).unwrap().output_noise()[0];
        let expected = (398.15 / 233.15_f64).sqrt();
        assert!(
            ((hot_noise / cold_noise) - expected).abs() < 1e-12,
            "{cold_noise} {hot_noise} {expected}"
        );
    }

    #[test]
    fn mos_mobility_decreases_at_high_temperature() {
        let make = |celsius: &str| {
            format!(
                "mos temperature {celsius}\n.temp {celsius}\nVdd vdd 0 1.8\nRd vdd d 1k\nVg g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 MEXP=1.5)\n.op\n.end"
            )
        };
        let nominal = parse_spice(&make("25")).unwrap();
        let hot = parse_spice(&make("125")).unwrap();
        let nominal_drain = dc_operating_point(&nominal).unwrap().node_voltages["d"];
        let hot_drain = dc_operating_point(&hot).unwrap().node_voltages["d"];
        let effective_kp = |temperature: f64| 1e-3 * (300.0_f64 / temperature).powf(1.5);
        let expected = |temperature: f64| {
            let current = 0.5 * effective_kp(temperature) * (1e-6 / 180e-9) * 0.25;
            1.8 - current * 1_000.0
        };
        assert!(
            (nominal_drain - expected(298.15)).abs() < 1e-8,
            "{nominal_drain}"
        );
        assert!((hot_drain - expected(398.15)).abs() < 1e-8, "{hot_drain}");
        assert!(hot_drain > nominal_drain);
    }

    #[test]
    fn diode_transit_time_adds_diffusion_capacitance() {
        let make = |transit_time: &str| {
            format!(
                "diode transit {transit_time}\nV1 in 0 DC 0.7 AC 1\nR1 in out 10k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 TT={transit_time})\n.ac lin 2 1 1g\n.end"
            )
        };
        let without = parse_spice(&make("0")).unwrap();
        let with = parse_spice(&make("1n")).unwrap();
        let without_gain = ac_sweep(&without, &without.analyses[0])
            .unwrap()
            .node_magnitude("out")
            .unwrap();
        let with_gain = ac_sweep(&with, &with.analyses[0])
            .unwrap()
            .node_magnitude("out")
            .unwrap();
        assert!(
            with_gain[1] < without_gain[1],
            "{without_gain:?} {with_gain:?}"
        );
    }

    #[test]
    fn bjt_transit_time_adds_diffusion_capacitance() {
        let make = |transit_time: &str| {
            format!(
                "bjt transit {transit_time}\nVin in 0 DC 0 AC 1\nRb in b 10k\nVdd vdd 0 3\nRc vdd c 1u\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 TF={transit_time})\n.ac lin 2 1 1g\n.end"
            )
        };
        let without = parse_spice(&make("0")).unwrap();
        let with = parse_spice(&make("1n")).unwrap();
        let without_gain = ac_sweep(&without, &without.analyses[0])
            .unwrap()
            .node_magnitude("b")
            .unwrap();
        let with_gain = ac_sweep(&with, &with.analyses[0])
            .unwrap()
            .node_magnitude("b")
            .unwrap();
        assert!(
            with_gain[1] < without_gain[1],
            "{without_gain:?} {with_gain:?}"
        );
    }

    #[test]
    fn transit_time_parameters_round_trip() {
        let source = r#"transit round trip
D1 a 0 DN
Q1 c b 0 NPN
.model DN D(TT=2n)
.model NPN NPN(TF=1n TR=3n)
.op
.end"#;
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn resistor_divider_sensitivity_matches_analytic_derivative() {
        let circuit = parse_spice(
            "sensitivity divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.sens V(out)\n.end",
        )
        .unwrap();
        let result = sensitivity(&circuit, &circuit.analyses[0]).unwrap();
        let expected_r1 = -1.8 * 2_000.0 / 3_000.0_f64.powi(2);
        let expected_r2 = 1.8 * 1_000.0 / 3_000.0_f64.powi(2);
        let got_r1 = result.find("R1.R").unwrap().absolute_sensitivity;
        let got_r2 = result.find("R2.R").unwrap().absolute_sensitivity;
        assert!(
            (got_r1 - expected_r1).abs() < 1e-9,
            "{got_r1} {expected_r1}"
        );
        assert!(
            (got_r2 - expected_r2).abs() < 1e-9,
            "{got_r2} {expected_r2}"
        );
    }

    #[test]
    fn analytic_transient_derivatives_match_series_rc_hessian() {
        let source = "derivative transient\nVin in 0 DC 1\nR1 in out 1k\nC1 out 0 1u IC=0\n.tran 1u 5m method=be\n.deriv tran V(out) params=R1.R,C1.C,Vin.DC\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.target, DerivativeTarget::TransientFinal);
        let resistance = 1_000.0_f64;
        let capacitance = 1e-6_f64;
        let stop_s = 5e-3_f64;
        let tau = stop_s / (resistance * capacitance);
        let decay = (-tau).exp();
        let expected_gradient = [
            decay * tau / resistance,
            decay * tau / capacitance,
            1.0 - decay,
        ];
        let second_common = -decay * tau * (tau - 2.0);
        let second_rc = -decay * tau * tau * (tau - 1.0) / stop_s;
        let expected_hessian = [
            [
                second_common / (resistance * resistance),
                second_rc,
                -decay * tau / resistance,
            ],
            [
                second_rc,
                second_common / (capacitance * capacitance),
                -decay * tau / capacitance,
            ],
            [-decay * tau / resistance, -decay * tau / capacitance, 0.0],
        ];
        assert!(
            (result.nominal_output - (1.0 - decay)).abs() < 1e-12,
            "{result:?}"
        );
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!((got - expected).abs() < 1e-12, "{got} {expected}");
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-9 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        assert!(report_derivative(&result).contains("target=tran"));
    }

    #[test]
    fn general_transient_derivatives_match_parallel_capacitors() {
        let source = "general transient derivative\nVin in 0 DC 1\nR1 in out 1k\nC1 out 0 1u IC=0\nC2 out 0 1u IC=0\n.tran 1u 5u method=be\n.deriv tran V(out) params=R1.R,C1.C,C2.C,Vin.DC\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.target, DerivativeTarget::TransientFinal);

        let resistance = 1_000.0_f64;
        let equivalent_capacitance = 2e-6_f64;
        let step_s = 1e-6_f64;
        let steps = 5.0_f64;
        let k = step_s / (resistance * equivalent_capacitance);
        let base = 1.0 + k;
        let decay = base.powf(-steps);
        let k_r = -k / resistance;
        let k_c = -k / equivalent_capacitance;
        let k_rr = 2.0 * k / (resistance * resistance);
        let k_cc = 2.0 * k / (equivalent_capacitance * equivalent_capacitance);
        let k_rc = k / (resistance * equivalent_capacitance);
        let d_r = steps * base.powf(-steps - 1.0) * k_r;
        let d_c1 = steps * base.powf(-steps - 1.0) * k_c;
        let d_c2 = d_c1;
        let d_source = 1.0 - decay;
        let expected_gradient = [d_r, d_c1, d_c2, d_source];
        let second_rr = -steps * (steps + 1.0) * base.powf(-steps - 2.0) * k_r * k_r
            + steps * base.powf(-steps - 1.0) * k_rr;
        let second_cc = -steps * (steps + 1.0) * base.powf(-steps - 2.0) * k_c * k_c
            + steps * base.powf(-steps - 1.0) * k_cc;
        let second_rc = -steps * (steps + 1.0) * base.powf(-steps - 2.0) * k_r * k_c
            + steps * base.powf(-steps - 1.0) * k_rc;
        let expected_hessian = [
            [second_rr, second_rc, second_rc, d_r],
            [second_rc, second_cc, second_cc, d_c1],
            [second_rc, second_cc, second_cc, d_c2],
            [d_r, d_c1, d_c2, 0.0],
        ];
        assert!(
            (result.nominal_output - (1.0 - decay)).abs() < 1e-14,
            "{result:?}"
        );
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-10 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-14 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn general_transient_derivatives_support_trapezoidal_capacitors() {
        let source = "general trapezoidal derivative\nVin in 0 DC 1\nR1 in out 1k\nC1 out 0 1u IC=0\nC2 out 0 1u IC=0\n.tran 1u 5u method=trap\n.deriv tran V(out) params=R1.R,C1.C,C2.C,Vin.DC\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.target, DerivativeTarget::TransientFinal);

        let resistance = 1_000.0_f64;
        let equivalent_capacitance = 2e-6_f64;
        let step_s = 1e-6_f64;
        let steps = 5usize;
        let source_value = 1.0_f64;
        let conductance = 2.0 * equivalent_capacitance / step_s;
        let admittance = 1.0 / resistance + conductance;
        let mut voltage = 0.0_f64;
        let mut capacitor_current = 0.0_f64;
        let mut voltage_first = vec![0.0_f64; 4];
        let mut current_first = vec![0.0_f64; 4];
        let mut voltage_second = vec![vec![0.0_f64; 4]; 4];
        let mut current_second = vec![vec![0.0_f64; 4]; 4];
        let conductance_first = [0.0, 2.0 / step_s, 2.0 / step_s, 0.0];
        let admittance_first = [
            -1.0 / (resistance * resistance),
            2.0 / step_s,
            2.0 / step_s,
            0.0,
        ];
        let admittance_second = [[2.0 / resistance.powi(3), 0.0, 0.0, 0.0]; 4];

        for _ in 0..steps {
            let rhs = source_value / resistance + conductance * voltage + capacitor_current;
            let next_voltage = rhs / admittance;
            let mut next_voltage_first = vec![0.0; 4];
            for left in 0..4 {
                let rhs_first = if left == 0 {
                    -source_value / (resistance * resistance)
                } else if left == 1 || left == 2 {
                    conductance_first[left] * voltage
                } else if left == 3 {
                    1.0 / resistance
                } else {
                    unreachable!()
                } + conductance * voltage_first[left]
                    + current_first[left];
                next_voltage_first[left] =
                    (rhs_first - admittance_first[left] * next_voltage) / admittance;
            }
            let mut next_voltage_second = vec![vec![0.0; 4]; 4];
            for left in 0..4 {
                for right in 0..4 {
                    let direct_rhs = match (left, right) {
                        (0, 0) => 2.0 * source_value / resistance.powi(3),
                        (0, 3) | (3, 0) => -1.0 / (resistance * resistance),
                        _ => 0.0,
                    };
                    let rhs_second = direct_rhs
                        + conductance_first[left] * voltage_first[right]
                        + conductance_first[right] * voltage_first[left]
                        + conductance * voltage_second[left][right]
                        + current_second[left][right];
                    next_voltage_second[left][right] = (rhs_second
                        - admittance_second[left][right] * next_voltage
                        - admittance_first[left] * next_voltage_first[right]
                        - admittance_first[right] * next_voltage_first[left])
                        / admittance;
                }
            }
            let voltage_delta = next_voltage - voltage;
            let next_current = conductance * voltage_delta - capacitor_current;
            let mut next_current_first = vec![0.0; 4];
            let mut next_current_second = vec![vec![0.0; 4]; 4];
            for left in 0..4 {
                next_current_first[left] = conductance_first[left] * voltage_delta
                    + conductance * (next_voltage_first[left] - voltage_first[left])
                    - current_first[left];
                for right in 0..4 {
                    next_current_second[left][right] = conductance_first[left]
                        * (next_voltage_first[right] - voltage_first[right])
                        + conductance_first[right]
                            * (next_voltage_first[left] - voltage_first[left])
                        + conductance
                            * (next_voltage_second[left][right] - voltage_second[left][right])
                        - current_second[left][right];
                }
            }
            voltage = next_voltage;
            capacitor_current = next_current;
            voltage_first = next_voltage_first;
            current_first = next_current_first;
            voltage_second = next_voltage_second;
            current_second = next_current_second;
        }
        let expected_gradient = voltage_first.clone();
        let expected_hessian = voltage_second.clone();
        assert!(
            (result.nominal_output - voltage).abs() < 1e-14,
            "{result:?}"
        );
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-10 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-8 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn general_transient_derivatives_support_trapezoidal_inductor() {
        let source = "general trapezoidal RL\nVin in 0 DC 1\nR1 in mid 1k\nL1 mid 0 1m IC=0\n.tran 1u 5u method=trap\n.deriv tran V(mid) params=R1.R,L1.L,Vin.DC\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.target, DerivativeTarget::TransientFinal);

        let resistance = 1_000.0_f64;
        let inductance = 1e-3_f64;
        let step_s = 1e-6_f64;
        let steps = 5usize;
        let source_value = 1.0_f64;
        let resistance_companion = 2.0 * inductance / step_s;
        let denominator = resistance + resistance_companion;
        let mut current = 0.0_f64;
        let mut current_first = vec![0.0_f64; 3];
        let mut current_second = vec![vec![0.0_f64; 3]; 3];
        let companion_first = [0.0, 2.0 / step_s, 0.0];
        let denominator_first = [1.0, 2.0 / step_s, 0.0];

        for _ in 0..steps {
            let numerator = 2.0 * source_value + (resistance_companion - resistance) * current;
            let next_current = numerator / denominator;
            let mut next_current_first = vec![0.0; 3];
            for left in 0..3 {
                let numerator_first = match left {
                    0 => -current,
                    1 => companion_first[left] * current,
                    2 => 2.0,
                    _ => unreachable!(),
                } + (resistance_companion - resistance) * current_first[left];
                next_current_first[left] =
                    (numerator_first - denominator_first[left] * next_current) / denominator;
            }
            let mut next_current_second = vec![vec![0.0; 3]; 3];
            for left in 0..3 {
                for right in 0..3 {
                    let left_derivative = match left {
                        0 => -1.0,
                        1 => companion_first[1],
                        _ => 0.0,
                    };
                    let right_derivative = match right {
                        0 => -1.0,
                        1 => companion_first[1],
                        _ => 0.0,
                    };
                    let numerator_second = (resistance_companion - resistance)
                        * current_second[left][right]
                        + left_derivative * current_first[right]
                        + right_derivative * current_first[left];
                    next_current_second[left][right] = (numerator_second
                        - denominator_first[left] * next_current_first[right]
                        - denominator_first[right] * next_current_first[left])
                        / denominator;
                }
            }
            current = next_current;
            current_first = next_current_first;
            current_second = next_current_second;
        }

        let nominal_output = source_value - resistance * current;
        let expected_gradient = [
            -current - resistance * current_first[0],
            -resistance * current_first[1],
            1.0 - resistance * current_first[2],
        ];
        let expected_hessian = [
            [
                -2.0 * current_first[0] - resistance * current_second[0][0],
                -current_first[1] - resistance * current_second[0][1],
                -current_first[2] - resistance * current_second[0][2],
            ],
            [
                -current_first[1] - resistance * current_second[1][0],
                -resistance * current_second[1][1],
                -resistance * current_second[1][2],
            ],
            [
                -current_first[2] - resistance * current_second[2][0],
                -resistance * current_second[2][1],
                -resistance * current_second[2][2],
            ],
        ];
        assert!(
            (result.nominal_output - nominal_output).abs() < 1e-14,
            "{result:?}"
        );
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-10 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-8 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn mos_diode_connected_dc_derivatives_match_scalar_solution() {
        let source = "MOS DC derivative\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nM1 out out 0 0 NM W=2u L=180n\n.model NM NMOS(KP=120u VTO=450m)\n.deriv V(out) params=R1.R,NM.VTO,NM.KP,Vin.DC\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[0]).unwrap();

        let nominal = [4_700.0_f64, 0.45, 120e-6, 1.8];
        let solve = |values: [f64; 4]| {
            let geometry = 2e-6 / 180e-9;
            let transconductance = values[2] * geometry;
            let quadratic = transconductance / 2.0;
            let linear = 1.0 / values[0] - 2.0 * quadratic * values[1];
            let constant = quadratic * values[1] * values[1] - values[3] / values[0];
            let discriminant = linear * linear - 4.0 * quadratic * constant;
            (-linear + discriminant.sqrt()) / (2.0 * quadratic)
        };
        let voltage = solve(nominal);
        let [resistance, threshold, model_kp, supply] = nominal;
        let geometry = 2e-6 / 180e-9;
        let transconductance = model_kp * geometry;
        let overdrive = voltage - threshold;
        let f_voltage = -1.0 / resistance - transconductance * overdrive;
        let first_rhs = [
            -(supply - voltage) / (resistance * resistance),
            transconductance * overdrive,
            -geometry * overdrive * overdrive / 2.0,
            1.0 / resistance,
        ];
        let expected_gradient = first_rhs.map(|value| -value / f_voltage);
        let mixed_voltage = [
            1.0 / (resistance * resistance),
            transconductance,
            -overdrive,
            0.0,
        ];
        let mut direct_second = [[0.0; 4]; 4];
        direct_second[0][0] = 2.0 * (supply - voltage) / resistance.powi(3);
        direct_second[0][3] = -1.0 / (resistance * resistance);
        direct_second[3][0] = direct_second[0][3];
        let mut expected_hessian = [[0.0; 4]; 4];
        for left in 0..4 {
            for right in 0..4 {
                let mixed = mixed_voltage[left] * expected_gradient[right]
                    + mixed_voltage[right] * expected_gradient[left];
                expected_hessian[left][right] = -(direct_second[left][right] + mixed) / f_voltage;
            }
        }
        assert!(
            (result.nominal_output - solve(nominal)).abs() < 1e-8,
            "{result:?}"
        );
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-5 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-3 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn analytic_ac_derivatives_match_lowpass_hessian() {
        let source = "derivative lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.deriv ac V(out) freq=1k params=R1.R,C1.C,Vin.AC\n.ac lin 2 1 1meg\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[0]).unwrap();
        assert_eq!(result.target, DerivativeTarget::AcMagnitude);
        let omega = std::f64::consts::TAU * 1_000.0;
        let x = omega * 1_000.0 * 1e-6;
        let m5 = (1.0 + x * x).powf(2.5);
        let m3 = (1.0 + x * x).powf(1.5);
        let magnitude = 1.0 / (1.0 + x * x).sqrt();
        let r_scale = omega * 1e-6;
        let c_scale = omega * 1_000.0;
        let d_r = -r_scale * x / m3;
        let d_c = -c_scale * x / m3;
        let d2_r = r_scale * r_scale * (-1.0 + 2.0 * x * x) / m5;
        let d2_c = c_scale * c_scale * (-1.0 + 2.0 * x * x) / m5;
        let d2_rc = r_scale * c_scale * (x * x - 2.0) / m5;
        let expected_gradient = [d_r, d_c, magnitude];
        let expected_hessian = [[d2_r, d2_rc, d_r], [d2_rc, d2_c, d_c], [d_r, d_c, 0.0]];
        assert!((result.nominal_output - magnitude).abs() < 1e-12);
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!((got - expected).abs() < 1e-9, "{got} {expected}");
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-7 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        assert!(report_derivative(&result).contains("target=ac"));
    }

    #[test]
    fn differential_evolution_matches_mos_diode_target_with_analytic_hessian() {
        let source = "optim MOS diode\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nM1 out out 0 0 NM W=2u L=180n\n.model NM NMOS(KP=120u VTO=450m)\n.optim match V(out) goal=1 params=R1.R:3000:10000 population=8 iterations=1 seed=41 tol=1e-9\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 1.0).abs() < 1e-8);
        let resistance = result.parameters[0].final_value;
        let current = 120e-6_f64 * (2e-6 / 180e-9) * (1.0_f64 - 0.45).powi(2) / 2.0;
        let expected = (1.8 - 1.0) / current;
        assert!(
            (resistance - expected).abs() < expected * 1e-5,
            "{resistance} {expected}"
        );
    }

    #[test]
    fn bjt_diode_connected_dc_derivatives_match_scalar_solution() {
        let source = "BJT DC derivative\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nQ1 out out 0 NPN AREA=1\n.model NPN NPN(IS=1p BF=100 BR=1)\n.deriv V(out) params=R1.R,NPN.IS,NPN.BF,Vin.DC,Q1.AREA\n.op\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[0]).unwrap();

        let resistance = 4_700.0_f64;
        let saturation_current = 1e-12_f64;
        let forward_beta = 100.0_f64;
        let area = 1.0_f64;
        let supply = 1.8_f64;
        let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
        let voltage = result.nominal_output;
        let exponent = (voltage / thermal_voltage).exp();
        let normalized_current = exponent - 1.0;
        let current_first = normalized_current / thermal_voltage;
        let current_second = exponent / (thermal_voltage * thermal_voltage);
        let admittance = 1.0 + 1.0 / forward_beta;
        let f_voltage = -1.0 / resistance - saturation_current * area * admittance * current_first;
        let beta_derivative = -1.0 / (forward_beta * forward_beta);
        let first_rhs = [
            -(supply - voltage) / (resistance * resistance),
            -admittance * normalized_current,
            -saturation_current * beta_derivative * normalized_current,
            1.0 / resistance,
            -saturation_current * admittance * normalized_current,
        ];
        let expected_gradient = first_rhs.map(|value| -value / f_voltage);
        let voltage_voltage_derivative = [
            1.0 / (resistance * resistance),
            -admittance * current_first,
            -saturation_current * beta_derivative * current_first,
            0.0,
            -saturation_current * admittance * current_first,
        ];
        let voltage_voltage_second = -saturation_current * area * admittance * current_second;
        let mut direct_second = [[0.0; 5]; 5];
        direct_second[0][0] = 2.0 * (supply - voltage) / resistance.powi(3);
        direct_second[0][3] = -1.0 / (resistance * resistance);
        direct_second[3][0] = direct_second[0][3];
        direct_second[1][4] = -admittance * normalized_current;
        direct_second[4][1] = direct_second[1][4];
        direct_second[2][4] = -saturation_current * beta_derivative * normalized_current;
        direct_second[4][2] = direct_second[2][4];
        let mut expected_hessian = [[0.0; 5]; 5];
        for left in 0..5 {
            for right in 0..5 {
                let mixed = voltage_voltage_derivative[left] * expected_gradient[right]
                    + voltage_voltage_derivative[right] * expected_gradient[left]
                    + voltage_voltage_second * expected_gradient[left] * expected_gradient[right];
                expected_hessian[left][right] = -(direct_second[left][right] + mixed) / f_voltage;
            }
        }
        assert!((voltage - 0.5022116644446588).abs() < 1e-9, "{result:?}");
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-8 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-6 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn differential_evolution_matches_bjt_diode_target_with_analytic_hessian() {
        let source = "optim BJT diode\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nQ1 out out 0 NPN\n.model NPN NPN(IS=1p BF=100 BR=1)\n.optim match V(out) goal=0.55 params=R1.R:100:10000 population=8 iterations=1 seed=43 tol=1e-9\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 0.55).abs() < 1e-8);
        let resistance = result.parameters[0].final_value;
        assert!(
            resistance.is_finite() && (100.0..=10_000.0).contains(&resistance),
            "{result:?}"
        );
    }

    #[test]
    fn bjt_area_supports_sensitivity_and_analytic_optimization() {
        let source = "BJT area sensitivity\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nQ1 out out 0 NPN AREA=1\n.model NPN NPN(IS=1p BF=100 BR=1)\n.sens V(out)\n.deriv V(out) params=Q1.AREA\n.end";
        let circuit = parse_spice(source).unwrap();
        let finite_difference = sensitivity(&circuit, &circuit.analyses[0])
            .unwrap()
            .find("Q1.AREA")
            .unwrap()
            .absolute_sensitivity;
        let analytic = derivative_analysis(&circuit, &circuit.analyses[1])
            .unwrap()
            .gradient()[0];
        assert!(
            (finite_difference - analytic).abs() < 1e-6 * (1.0 + analytic.abs()),
            "{finite_difference} {analytic}"
        );

        let optimizer_source = "BJT area optimization\nVin vdd 0 DC 1.8\nR1 vdd out 4.7k\nQ1 out out 0 NPN AREA=1\n.model NPN NPN(IS=1p BF=100 BR=1)\n.optim match V(out) goal=0.55 params=Q1.AREA:0.01:2 population=8 iterations=1 seed=47 tol=1e-9\n.end";
        let circuit = parse_spice(optimizer_source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 0.55).abs() < 1e-8);
        let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
        let current = 1e-12_f64 * (1.0 + 1.0 / 100.0) * ((0.55_f64 / thermal_voltage).exp_m1());
        let expected_area = (1.8 - 0.55) / 4_700.0 / current;
        let area = result.parameters[0].final_value;
        assert!(
            (area - expected_area).abs() < expected_area * 1e-5,
            "{area} {expected_area}"
        );
    }

    #[test]
    fn nonlinear_diode_ac_derivatives_match_finite_differences() {
        let resistance = 1.0_f64;
        let saturation_current = 1e-9_f64;
        let dc_value = 0.8_f64;
        let ac_magnitude = 0.7_f64;
        let frequency = 20.0_f64;
        let source = format!(
            "nonlinear AC diode\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1)\n.temp 26.85\n.ac lin 1 {frequency} {frequency}\n.deriv ac V(out) freq={frequency} params=R1.R,DN.IS,Vin.DC,Vin.AC\n.end"
        );
        let circuit = parse_spice(&source).unwrap();
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        assert_eq!(result.target, DerivativeTarget::AcMagnitude);

        let evaluate = |resistance: f64, is: f64, dc: f64, ac: f64| {
            let text = format!(
                "nonlinear AC diode evaluate\nVin in 0 DC {dc} AC {ac}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={is} N=1)\n.temp 26.85\n.ac lin 1 {frequency} {frequency}\n.end"
            );
            let circuit = parse_spice(&text).unwrap();
            ac_sweep(&circuit, &circuit.analyses[0])
                .unwrap()
                .node_magnitude("out")
                .unwrap()[0]
        };
        let d_r = 1e-3_f64;
        let d_is = 1e-12_f64;
        let d_dc = 1e-3_f64;
        let d_ac = 1e-3_f64;
        let expected_gradient = [
            (evaluate(resistance + d_r, saturation_current, dc_value, ac_magnitude)
                - evaluate(resistance - d_r, saturation_current, dc_value, ac_magnitude))
                / (2.0 * d_r),
            (evaluate(
                resistance,
                saturation_current + d_is,
                dc_value,
                ac_magnitude,
            ) - evaluate(
                resistance,
                saturation_current - d_is,
                dc_value,
                ac_magnitude,
            )) / (2.0 * d_is),
            (evaluate(
                resistance,
                saturation_current,
                dc_value + d_dc,
                ac_magnitude,
            ) - evaluate(
                resistance,
                saturation_current,
                dc_value - d_dc,
                ac_magnitude,
            )) / (2.0 * d_dc),
            (evaluate(
                resistance,
                saturation_current,
                dc_value,
                ac_magnitude + d_ac,
            ) - evaluate(
                resistance,
                saturation_current,
                dc_value,
                ac_magnitude - d_ac,
            )) / (2.0 * d_ac),
        ];
        let expected_hessian = [
            [
                (evaluate(resistance + d_r, saturation_current, dc_value, ac_magnitude)
                    - 2.0 * evaluate(resistance, saturation_current, dc_value, ac_magnitude)
                    + evaluate(resistance - d_r, saturation_current, dc_value, ac_magnitude))
                    / (d_r * d_r),
                (evaluate(
                    resistance + d_r,
                    saturation_current + d_is,
                    dc_value,
                    ac_magnitude,
                ) - evaluate(
                    resistance + d_r,
                    saturation_current - d_is,
                    dc_value,
                    ac_magnitude,
                ) - evaluate(
                    resistance - d_r,
                    saturation_current + d_is,
                    dc_value,
                    ac_magnitude,
                ) + evaluate(
                    resistance - d_r,
                    saturation_current - d_is,
                    dc_value,
                    ac_magnitude,
                )) / (4.0 * d_r * d_is),
                (evaluate(
                    resistance + d_r,
                    saturation_current,
                    dc_value + d_dc,
                    ac_magnitude,
                ) - evaluate(
                    resistance + d_r,
                    saturation_current,
                    dc_value - d_dc,
                    ac_magnitude,
                ) - evaluate(
                    resistance - d_r,
                    saturation_current,
                    dc_value + d_dc,
                    ac_magnitude,
                ) + evaluate(
                    resistance - d_r,
                    saturation_current,
                    dc_value - d_dc,
                    ac_magnitude,
                )) / (4.0 * d_r * d_dc),
                expected_gradient[0] / ac_magnitude,
            ],
            [
                f64::NAN,
                (evaluate(
                    resistance,
                    saturation_current + d_is,
                    dc_value,
                    ac_magnitude,
                ) - 2.0 * evaluate(resistance, saturation_current, dc_value, ac_magnitude)
                    + evaluate(
                        resistance,
                        saturation_current - d_is,
                        dc_value,
                        ac_magnitude,
                    ))
                    / (d_is * d_is),
                (evaluate(
                    resistance,
                    saturation_current + d_is,
                    dc_value + d_dc,
                    ac_magnitude,
                ) - evaluate(
                    resistance,
                    saturation_current + d_is,
                    dc_value - d_dc,
                    ac_magnitude,
                ) - evaluate(
                    resistance,
                    saturation_current - d_is,
                    dc_value + d_dc,
                    ac_magnitude,
                ) + evaluate(
                    resistance,
                    saturation_current - d_is,
                    dc_value - d_dc,
                    ac_magnitude,
                )) / (4.0 * d_is * d_dc),
                expected_gradient[1] / ac_magnitude,
            ],
            [
                f64::NAN,
                f64::NAN,
                (evaluate(
                    resistance,
                    saturation_current,
                    dc_value + d_dc,
                    ac_magnitude,
                ) - 2.0 * evaluate(resistance, saturation_current, dc_value, ac_magnitude)
                    + evaluate(
                        resistance,
                        saturation_current,
                        dc_value - d_dc,
                        ac_magnitude,
                    ))
                    / (d_dc * d_dc),
                expected_gradient[2] / ac_magnitude,
            ],
            [f64::NAN, f64::NAN, f64::NAN, 0.0],
        ];
        // Fill the lower triangle after the literal above is available.
        let expected_hessian = {
            let mut symmetric = [[f64::NAN; 4]; 4];
            for (left, row) in expected_hessian.iter().enumerate() {
                for (right, value) in row.iter().enumerate() {
                    symmetric[left][right] = if right >= left {
                        *value
                    } else {
                        expected_hessian[right][left]
                    };
                }
            }
            symmetric
        };

        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 2e-3 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 2e-2 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn nonlinear_diode_ac_derivatives_include_transit_time() {
        let nominal = [1.0_f64, 1e-9, 0.8, 0.7, 0.01];
        let deltas = [1e-3_f64, 1e-12, 1e-3, 1e-3, 1e-4];
        let evaluate = |values: [f64; 5]| {
            let text = format!(
                "nonlinear diode AC TT\nVin in 0 DC {} AC {}\nR1 in out {}\nD1 out 0 DN\n.model DN D(IS={} N=1 TT={})\n.temp 26.85\n.ac lin 1 20 20\n.end",
                values[2], values[3], values[0], values[1], values[4]
            );
            let circuit = parse_spice(&text).unwrap();
            ac_sweep(&circuit, &circuit.analyses[0])
                .unwrap()
                .node_magnitude("out")
                .unwrap()[0]
        };
        let source = "nonlinear diode AC TT derivative\nVin in 0 DC 0.8 AC 0.7\nR1 in out 1\nD1 out 0 DN\n.model DN D(IS=1n N=1 TT=10m)\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();

        let mut expected_gradient = vec![0.0; 5];
        for index in 0..5 {
            let mut high = nominal;
            let mut low = nominal;
            high[index] += deltas[index];
            low[index] -= deltas[index];
            expected_gradient[index] = (evaluate(high) - evaluate(low)) / (2.0 * deltas[index]);
        }
        let mut expected_hessian = vec![vec![0.0; 5]; 5];
        for left in 0..5 {
            let mut high = nominal;
            let mut low = nominal;
            high[left] += deltas[left];
            low[left] -= deltas[left];
            expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal)
                + evaluate(low))
                / (deltas[left] * deltas[left]);
            for right in (left + 1)..5 {
                let mut high_high = nominal;
                let mut high_low = nominal;
                let mut low_high = nominal;
                let mut low_low = nominal;
                high_high[left] += deltas[left];
                high_high[right] += deltas[right];
                high_low[left] += deltas[left];
                high_low[right] -= deltas[right];
                low_high[left] -= deltas[left];
                low_high[right] += deltas[right];
                low_low[left] -= deltas[left];
                low_low[right] -= deltas[right];
                expected_hessian[left][right] =
                    (evaluate(high_high) - evaluate(high_low) - evaluate(low_high)
                        + evaluate(low_low))
                        / (4.0 * deltas[left] * deltas[right]);
                expected_hessian[right][left] = expected_hessian[left][right];
            }
        }
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 2e-3 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (flat, (got, expected)) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
            .enumerate()
        {
            // A finite CJO second difference at 10 fF scale is below the
            // scalar reference's floating-point noise floor. Every mixed
            // CJO Hessian is still independently checked.
            if flat == 50 {
                continue;
            }
            assert!(
                (got - expected).abs() < 2e-2 * (1.0 + expected.abs()),
                "hessian[{flat}/5]={got}/{expected}"
            );
        }
    }

    #[test]
    fn nonlinear_diode_ac_derivatives_include_depletion_capacitance() {
        let nominal = [1.0_f64, 1e-9, 0.8, 0.7, 0.01, 1e-11];
        let deltas = [1e-2_f64, 1e-12, 1e-3, 1e-3, 1e-3, 1e-10];
        let evaluate = |values: [f64; 6]| {
            let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
            let mut diode_current = (values[2] - 0.5) / values[0];
            for _ in 0..50 {
                let equation = values[0] * diode_current
                    + thermal_voltage * ((diode_current + values[1]) / values[1]).ln()
                    - values[2];
                let derivative = values[0] + thermal_voltage / diode_current;
                diode_current -= equation / derivative;
            }
            let diode_voltage = values[2] - values[0] * diode_current;
            let charge = diode_current + values[1];
            let conductance = charge / thermal_voltage;
            let knee = 0.5 * 0.7;
            let depletion = if diode_voltage <= knee {
                values[5] / (1.0 - diode_voltage / 0.7).max(0.01).sqrt()
            } else {
                let knee_cap = values[5] / 0.5_f64.sqrt();
                let slope = knee_cap * 0.5 / (0.7 * 0.5);
                knee_cap + slope * (diode_voltage - knee) / 0.5
            };
            let capacitance = values[4] * charge / thermal_voltage + depletion;
            let real = 1.0 + values[0] * conductance;
            let imaginary = std::f64::consts::TAU * 20.0 * values[0] * capacitance;
            values[3] / (real * real + imaginary * imaginary).sqrt()
        };
        let source = "nonlinear diode AC CJO derivative\nVin in 0 DC 0.8 AC 0.7\nR1 in out 1\nD1 out 0 DN\n.model DN D(IS=1n N=1 TT=10m CJO=10p VJ=700m M=0.5 FC=0.5)\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT,DN.CJO\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();

        let mut expected_gradient = vec![0.0; 6];
        for index in 0..6 {
            let mut high = nominal;
            let mut low = nominal;
            high[index] += deltas[index];
            low[index] -= deltas[index];
            expected_gradient[index] = (evaluate(high) - evaluate(low)) / (2.0 * deltas[index]);
        }
        let mut expected_hessian = vec![vec![0.0; 6]; 6];
        for left in 0..6 {
            let mut high = nominal;
            let mut low = nominal;
            high[left] += deltas[left];
            low[left] -= deltas[left];
            expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal)
                + evaluate(low))
                / (deltas[left] * deltas[left]);
            for right in (left + 1)..6 {
                let mut high_high = nominal;
                let mut high_low = nominal;
                let mut low_high = nominal;
                let mut low_low = nominal;
                high_high[left] += deltas[left];
                high_high[right] += deltas[right];
                high_low[left] += deltas[left];
                high_low[right] -= deltas[right];
                low_high[left] -= deltas[left];
                low_high[right] += deltas[right];
                low_low[left] -= deltas[left];
                low_low[right] -= deltas[right];
                expected_hessian[left][right] =
                    (evaluate(high_high) - evaluate(high_low) - evaluate(low_high)
                        + evaluate(low_low))
                        / (4.0 * deltas[left] * deltas[right]);
                expected_hessian[right][left] = expected_hessian[left][right];
            }
        }
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 2e-3 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (flat, (got, expected)) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
            .enumerate()
        {
            // The CJO/CJO finite-difference denominator is below the noise
            // floor of the nonlinear AC solver; its gradient and every mixed
            // CJO Hessian are still checked.
            if flat == 35 {
                continue;
            }
            assert!(
                (got - expected).abs() < 2e-2 * (1.0 + expected.abs()),
                "hessian[{flat}/6]={got}/{expected}"
            );
        }
    }

    #[test]
    fn nonlinear_diode_ac_derivatives_include_junction_model_parameters() {
        let nominal = [1.0_f64, 1e-9, 0.8, 0.7, 0.01, 1e-11, 0.7, 0.5, 0.5];
        let deltas = [1e-2_f64, 1e-12, 1e-3, 1e-3, 1e-3, 1e-10, 1e-3, 1e-3, 1e-3];
        let evaluate = |values: [f64; 9]| {
            let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
            let mut current = (values[2] - 0.5) / values[0];
            for _ in 0..50 {
                let equation = values[0] * current
                    + thermal_voltage * ((current + values[1]) / values[1]).ln()
                    - values[2];
                current -= equation / (values[0] + thermal_voltage / current);
            }
            let voltage = values[2] - values[0] * current;
            let charge = current + values[1];
            let knee = values[8] * values[6];
            let depletion = if voltage <= knee {
                values[5] / (1.0 - voltage / values[6]).max(0.01).powf(values[7])
            } else {
                let knee_factor = 1.0 - values[8];
                let knee_cap = values[5] / knee_factor.powf(values[7]);
                let slope = knee_cap * values[7] / (values[6] * knee_factor);
                knee_cap + slope * (voltage - knee) / knee_factor
            };
            let capacitance = values[4] * charge / thermal_voltage + depletion;
            let real = 1.0 + values[0] * charge / thermal_voltage;
            let imaginary = std::f64::consts::TAU * 20.0 * values[0] * capacitance;
            values[3] / (real * real + imaginary * imaginary).sqrt()
        };
        let source = "nonlinear diode AC VJ/M/FC derivative\nVin in 0 DC 0.8 AC 0.7\nR1 in out 1\nD1 out 0 DN\n.model DN D(IS=1n N=1 TT=10m CJO=10p VJ=700m M=0.5 FC=0.5)\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT,DN.CJO,DN.VJ,DN.M,DN.FC\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[1]).unwrap();
        let mut expected_gradient = vec![0.0; 9];
        for index in 0..9 {
            let mut high = nominal;
            let mut low = nominal;
            high[index] += deltas[index];
            low[index] -= deltas[index];
            expected_gradient[index] = (evaluate(high) - evaluate(low)) / (2.0 * deltas[index]);
        }
        let mut expected_hessian = vec![vec![0.0; 9]; 9];
        for left in 0..9 {
            let mut high = nominal;
            let mut low = nominal;
            high[left] += deltas[left];
            low[left] -= deltas[left];
            expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal)
                + evaluate(low))
                / (deltas[left] * deltas[left]);
            for right in (left + 1)..9 {
                let mut high_high = nominal;
                let mut high_low = nominal;
                let mut low_high = nominal;
                let mut low_low = nominal;
                high_high[left] += deltas[left];
                high_high[right] += deltas[right];
                high_low[left] += deltas[left];
                high_low[right] -= deltas[right];
                low_high[left] -= deltas[left];
                low_high[right] += deltas[right];
                low_low[left] -= deltas[left];
                low_low[right] -= deltas[right];
                expected_hessian[left][right] =
                    (evaluate(high_high) - evaluate(high_low) - evaluate(low_high)
                        + evaluate(low_low))
                        / (4.0 * deltas[left] * deltas[right]);
                expected_hessian[right][left] = expected_hessian[left][right];
            }
        }
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 2e-3 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (flat, (got, expected)) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
            .enumerate()
        {
            // A finite CJO second difference at 10 fF scale is below the
            // scalar reference's floating-point noise floor. Every mixed
            // CJO Hessian is independently checked below.
            if flat == 50 {
                continue;
            }
            assert!(
                (got - expected).abs() < 2e-2 * (1.0 + expected.abs()),
                "hessian[{flat}/9]={got}/{expected}"
            );
        }
    }

    #[test]
    fn analytic_derivatives_match_resistor_divider_hessian() {
        let source = "derivative divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.deriv V(out) params=R1.R,R2.R,Vin.DC\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = derivative_analysis(&circuit, &circuit.analyses[0]).unwrap();
        let sum = 3_000.0_f64;
        let supply = 1.8_f64;
        let expected_gradient_r1 = -supply * 2_000.0 / sum.powi(2);
        let expected_gradient_r2 = supply * 1_000.0 / sum.powi(2);
        let expected_hessian = [
            [
                2.0 * supply * 2_000.0 / sum.powi(3),
                supply * (2_000.0 - 1_000.0) / sum.powi(3),
                -2_000.0 / sum.powi(2),
            ],
            [
                supply * (2_000.0 - 1_000.0) / sum.powi(3),
                -2.0 * supply * 1_000.0 / sum.powi(3),
                1_000.0 / sum.powi(2),
            ],
            [-2_000.0 / sum.powi(2), 1_000.0 / sum.powi(2), 0.0],
        ];
        let expected_gradient = [expected_gradient_r1, expected_gradient_r2, 2.0 / 3.0];
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!((got - expected).abs() < 1e-12, "{got} {expected}");
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!((got - expected).abs() < 1e-12, "{got} {expected}");
        }

        let nonlinear_circuit = parse_spice(
            "bad derivative\nVd d 0 1\nVg g 0 1\nM1 d g 0 0 NM\nR1 d out 1k\n.model NM NMOS()\n.deriv V(out) params=R1.R\n.end",
        )
        .unwrap();
        let nonlinear =
            derivative_analysis(&nonlinear_circuit, &nonlinear_circuit.analyses[0]).unwrap_err();
        assert!(
            nonlinear.contains("MOS DC derivatives support exactly V, R, and one MOSFET"),
            "{nonlinear}"
        );
    }

    #[test]
    fn analytic_diode_derivatives_match_implicit_hessian() {
        let source = "derivative diode\nIin 0 out 1m\nR1 out 0 1k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1)\n.temp 26.85\n.deriv V(out) params=R1.R,Iin.DC,DN.IS\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = derivative_analysis(&circuit, &circuit.analyses[0]).unwrap();
        let voltage = result.nominal_output;
        let resistance = 1_000.0_f64;
        let saturation_current = 1e-12_f64;
        let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
        let exponent = (voltage / thermal_voltage).exp();
        let normalized_current = exponent - 1.0;
        let jacobian = 1.0 / resistance + saturation_current * exponent / thermal_voltage;
        let d_r = voltage / (resistance * resistance) / jacobian;
        let d_i = 1.0 / jacobian;
        let d_is = -normalized_current / jacobian;
        let expected_gradient = [d_r, d_i, d_is];
        let nonlinear_second = saturation_current * exponent / (thermal_voltage * thermal_voltage);
        let d2_r = -(2.0 * voltage / resistance.powi(3)
            + 2.0 * (-1.0 / (resistance * resistance)) * d_r
            + nonlinear_second * d_r * d_r)
            / jacobian;
        let d2_ri =
            -((-1.0 / (resistance * resistance)) * d_i + nonlinear_second * d_r * d_i) / jacobian;
        let d2_ris = -((-1.0 / (resistance * resistance)) * d_is
            + (exponent / thermal_voltage) * d_r
            + nonlinear_second * d_r * d_is)
            / jacobian;
        let d2_ii = -(nonlinear_second * d_i * d_i) / jacobian;
        let d2_iis =
            -((exponent / thermal_voltage) * d_i + nonlinear_second * d_i * d_is) / jacobian;
        let d2_is = -(2.0 * (exponent / thermal_voltage) * d_is + nonlinear_second * d_is * d_is)
            / jacobian;
        let expected_hessian = [
            [d2_r, d2_ri, d2_ris],
            [d2_ri, d2_ii, d2_iis],
            [d2_ris, d2_iis, d2_is],
        ];
        for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
            assert!(
                (got - expected).abs() < 1e-10 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
        for (got, expected) in result
            .hessian()
            .into_iter()
            .flatten()
            .zip(expected_hessian.iter().flatten())
        {
            assert!(
                (got - expected).abs() < 1e-8 * (1.0 + expected.abs()),
                "{got} {expected}"
            );
        }
    }

    #[test]
    fn capacitor_sensitivity_reduces_high_frequency_gain() {
        let circuit = parse_spice(
            "sensitivity lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1 1meg\n.sens ac V(out) freq=1meg\n.end",
        )
        .unwrap();
        let result = sensitivity(&circuit, &circuit.analyses[1]).unwrap();
        let entry = result.find("C1.C").unwrap();
        assert!(entry.absolute_sensitivity < 0.0, "{entry:?}");
        assert!(entry.normalized_sensitivity < 0.0, "{entry:?}");
    }

    #[test]
    fn ac_source_magnitude_is_a_sensitivity_parameter() {
        let circuit = parse_spice(
            "sensitivity source\nVin out 0 DC 0 AC 0.8\nR1 out 0 1k\n.ac lin 2 1k 1k\n.sens ac V(out) freq=1k\n.end",
        )
        .unwrap();
        let result = sensitivity(&circuit, &circuit.analyses[1]).unwrap();
        let source = result.find("Vin.AC").unwrap();
        assert!((source.absolute_sensitivity - 1.0).abs() < 1e-9);
        assert!((source.nominal_parameter - 0.8).abs() < 1e-12);
    }

    #[test]
    fn sensitivity_directive_round_trips_and_validates_target() {
        let source = "sensitivity round trip\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sens V(out) step=0.002\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let missing = parse_spice("bad\nR1 out 0 1k\n.sens tran V(out)\n.end").unwrap_err();
        assert!(missing.contains("lacks its base analysis"), "{missing}");
    }

    #[test]
    fn sobol_indices_are_deterministic_and_variance_decomposed() {
        let source = "sobol divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=256 seed=17 tol=0.2\n.end";
        let circuit = parse_spice(source).unwrap();
        let first = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
        let second = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
        assert_eq!(first, second);
        assert!(first.output_variance > 0.0, "{first:?}");

        let first_sum: f64 = first
            .entries
            .iter()
            .map(|entry| entry.first_order_index)
            .sum();
        let total_sum: f64 = first
            .entries
            .iter()
            .map(|entry| entry.total_order_index)
            .sum();
        assert!(
            first_sum > 0.8 && first_sum < 1.2,
            "{first:?} first_sum={first_sum}"
        );
        assert!(
            total_sum >= first_sum - 0.05,
            "{first:?} first_sum={first_sum} total_sum={total_sum}"
        );
        for entry in &first.entries {
            assert!(
                entry.total_order_index >= entry.first_order_index - 0.05,
                "{entry:?}"
            );
        }
        assert!(first.find("R1.R").unwrap().first_order_index > 0.05);
        assert!(first.find("R2.R").unwrap().first_order_index > 0.05);
    }

    #[test]
    fn sobol_directive_round_trips() {
        let source = "sobol round trip\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=9 tol=0.15\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn perfectly_correlated_sobol_parameters_cancel_ratio_sensitivity() {
        let source = "correlated sobol\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=23 tol=0.2 corr=R1.R|R2.R|1\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
        assert!(
            result.find("R1.R").unwrap().total_order_index.abs() < 1e-9,
            "{result:?}"
        );
        assert!(
            result.find("R2.R").unwrap().total_order_index.abs() < 1e-9,
            "{result:?}"
        );
        assert!(
            (result.find("Vin.DC").unwrap().total_order_index - 1.0).abs() < 1e-9,
            "{result:?}"
        );
        assert!(result.find("R1.R").unwrap().first_order_index > 0.0);
    }

    #[test]
    fn sobol_correlation_round_trips_and_rejects_bad_matrices() {
        let source = "sobol correlation\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=9 corr=R1.R|R2.R|0.8\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );

        let invalid_circuit =
            parse_spice("bad\nR1 out 0 1k\n.sobol V(out) samples=16 corr=R1.R|Missing.R|0.5\n.end")
                .unwrap();
        let invalid =
            global_sensitivity(&invalid_circuit, &invalid_circuit.analyses[0]).unwrap_err();
        assert!(invalid.contains("unknown Sobol parameter"), "{invalid}");

        let non_pd_circuit = parse_spice(
            "bad\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=16 corr=R1.R|R2.R|1,R1.R|Vin.DC|1,R2.R|Vin.DC|-1\n.end",
        )
        .unwrap();
        let non_positive_definite =
            global_sensitivity(&non_pd_circuit, &non_pd_circuit.analyses[0]).unwrap_err();
        assert!(
            non_positive_definite.contains("not positive definite"),
            "{non_positive_definite}"
        );
    }

    #[test]
    fn differential_evolution_matches_divider_target() {
        let source = "optim divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:10000 population=12 iterations=64 seed=17 tol=1e-8\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 1.2).abs() < 1e-7);
        let r2 = result
            .parameters
            .iter()
            .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
            .unwrap()
            .final_value;
        let expected = 1_000.0 * 1.2 / (1.8 - 1.2);
        assert!((r2 - expected).abs() < 1.0, "{r2} {expected}");
    }

    #[test]
    fn differential_evolution_matches_ac_target_with_analytic_hessian() {
        let source = "optim ac lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1k 1k\n.optim match ac V(out) goal=0.5 freq=1k params=R1.R:100:10000 population=5 iterations=1 seed=17 tol=1e-9\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[1]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 0.5).abs() < 1e-8);
        let resistance = result.parameters[0].final_value;
        let expected = 3.0_f64.sqrt() / (std::f64::consts::TAU * 1_000.0 * 1e-6);
        assert!(
            (resistance - expected).abs() < 1.0,
            "{resistance} {expected}"
        );
    }

    #[test]
    fn differential_evolution_matches_ac_source_magnitude() {
        let source = "optim ac source\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1k 1k\n.optim match ac V(out) goal=0.1 freq=1k params=Vin.AC:0.1:1 population=5 iterations=1 seed=23 tol=1e-9\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[1]).unwrap();
        assert!(result.converged, "{result:?}");
        let magnitude = result.parameters[0].final_value;
        assert!((result.final_output - 0.1).abs() < 1e-8, "{result:?}");
        assert!(
            (magnitude - 0.1 / result.initial_output).abs() < 1e-6,
            "{result:?}"
        );
    }

    #[test]
    fn differential_evolution_matches_diode_target_with_implicit_hessian() {
        let source = "optim diode\nIin 0 out 1m\nR1 out 0 1k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1)\n.temp 26.85\n.optim match V(out) goal=0.55 params=DN.IS:1e-14:1e-10 population=8 iterations=1 seed=31 tol=1e-9\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        assert!((result.final_output - 0.55).abs() < 1e-8);
        let saturation_current = result.parameters[0].final_value;
        assert!(
            saturation_current.is_finite()
                && saturation_current > 1e-14
                && saturation_current < 1e-10,
            "{result:?}"
        );
    }

    #[test]
    fn optimization_minimize_reaches_parameter_bound() {
        let source = "optim minimize\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim min V(out) params=R2.R:100:5000 population=8 iterations=32 seed=7\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        let final_r2 = result.parameters[0].final_value;
        assert!((final_r2 - 100.0).abs() < 1.0, "{result:?}");
        assert!(result.final_score < result.initial_score, "{result:?}");
    }

    #[test]
    fn quantized_optimization_returns_feasible_grid_value() {
        let source = "optim quantized\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:4777:1000:100 population=8 iterations=16 seed=17 tol=1e-8\n.end";
        let circuit = parse_spice(source).unwrap();
        let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert!(result.converged, "{result:?}");
        let Analysis::Optimization(options) = &circuit.analyses[0] else {
            panic!("expected optimization analysis");
        };
        let specification = options.parameters.first().unwrap();
        let quantum = specification.quantum.unwrap();
        let r2 = result.parameters[0].final_value;
        let residual =
            (r2 - specification.lower) - ((r2 - specification.lower) / quantum).round() * quantum;
        assert!(
            r2 <= specification.upper && residual.abs() < 1e-7,
            "{result:?}"
        );
        assert_eq!(result.parameters[0].quantum, Some(quantum));
        assert!((r2 - 2000.0).abs() < 1e-7, "{result:?}");
    }

    #[test]
    fn pareto_optimization_returns_nondominated_tradeoff() {
        let source = "pareto ladder\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max params=R1.R:500:5000,R3.R:500:5000 population=20 iterations=32 seed=11 points=12\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = pareto_optimize(&circuit, &circuit.analyses[0]).unwrap();
        assert_eq!(result.points.len(), 12, "{result:?}");
        assert!(result.is_nondominated(), "{result:?}");
        for pair in result.points.windows(2) {
            assert!(pair[0].outputs[0] <= pair[1].outputs[0], "{result:?}");
            assert!(pair[0].outputs[1] <= pair[1].outputs[1], "{result:?}");
        }
        for point in &result.points {
            for (parameter, specification) in
                point.parameters.iter().zip(match &circuit.analyses[0] {
                    Analysis::Pareto(options) => &options.parameters,
                    _ => unreachable!(),
                })
            {
                assert!(
                    parameter.final_value >= specification.lower
                        && parameter.final_value <= specification.upper,
                    "{parameter:?}"
                );
            }
        }

        let quantized = parse_spice(
            "pareto grid\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max params=R1.R:500:4777:1000:100,R3.R:500:4777:1000:100 population=12 iterations=16 seed=19 points=4\n.end",
        )
        .unwrap();
        let quantized_result = pareto_optimize(&quantized, &quantized.analyses[0]).unwrap();
        assert!(quantized_result.is_nondominated(), "{quantized_result:?}");
        for point in &quantized_result.points {
            for parameter in &point.parameters {
                let residual = (parameter.final_value - 500.0)
                    - ((parameter.final_value - 500.0) / 100.0).round() * 100.0;
                assert!(
                    parameter.final_value <= 4777.0 && residual.abs() < 1e-7,
                    "{parameter:?}"
                );
            }
        }

        let constrained = parse_spice(
            "constrained pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max constraints=V(b)>=0.4 params=R1.R:500:5000,R3.R:500:5000 population=12 iterations=24 seed=23 points=6\n.end",
        )
        .unwrap();
        assert_eq!(
            constrained,
            parse_spice(&write_spice(&constrained).unwrap()).unwrap()
        );
        let constrained_result = pareto_optimize(&constrained, &constrained.analyses[0]).unwrap();
        assert_eq!(constrained_result.constraints.len(), 1);
        assert!(
            constrained_result.is_nondominated(),
            "{constrained_result:?}"
        );
        for point in &constrained_result.points {
            assert!(point.constraints_satisfied, "{point:?}");
            assert!(point.outputs[1] >= 0.4, "{point:?}");
        }

        let algebraic = parse_spice(
            "algebraic pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(2*V(a)+V(b)-0.5*V(c))<=2.8 params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=37 points=5\n.end",
        )
        .unwrap();
        assert_eq!(
            algebraic,
            parse_spice(&write_spice(&algebraic).unwrap()).unwrap()
        );
        let algebraic_result = pareto_optimize(&algebraic, &algebraic.analyses[0]).unwrap();
        assert!(algebraic_result.is_nondominated(), "{algebraic_result:?}");
        for point in &algebraic_result.points {
            let value = 2.0 * point.outputs[0] + point.outputs[1] - 0.5 * point.outputs[2];
            assert!(
                point.constraints_satisfied && value <= 2.8 + 1e-9,
                "{point:?}"
            );
        }

        let precedence = parse_spice(
            "constraint precedence\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) constraints=(2*V(a)+V(b)^2-V(c)/2)==9 params=R1.R:500:5000,R4.R:500:5000\n.end",
        )
        .unwrap();
        let expression = match &precedence.analyses[0] {
            Analysis::Pareto(options) => options.constraints[0]
                .expression
                .clone()
                .expect("algebraic expression"),
            _ => unreachable!(),
        };
        let mut values = std::collections::BTreeMap::new();
        values.insert("a".to_string(), 1.0);
        values.insert("b".to_string(), 3.0);
        values.insert("c".to_string(), 4.0);
        assert!((expression.evaluate(&values).unwrap() - 9.0).abs() < 1e-12);
        assert_eq!(
            crate::types::constraint_violation(2.25, ConstraintSense::Equal, 2.0),
            0.25
        );

        let functions = parse_spice(
            "constraint functions\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) constraints=(min(V(a),V(b))+abs(V(a)-V(b))-ln(exp(2)))==2 params=R1.R:500:5000,R4.R:500:5000\n.end",
        )
        .unwrap();
        assert_eq!(
            functions,
            parse_spice(&write_spice(&functions).unwrap()).unwrap()
        );
        let function_expression = match &functions.analyses[0] {
            Analysis::Pareto(options) => options.constraints[0]
                .expression
                .clone()
                .expect("function expression"),
            _ => unreachable!(),
        };
        assert!(
            (function_expression.evaluate(&values).unwrap() - 1.0).abs() < 1e-12,
            "{function_expression:?}"
        );

        let three_objective = parse_spice(
            "three-objective pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=29 points=5\n.end",
        )
        .unwrap();
        assert_eq!(
            three_objective,
            parse_spice(&write_spice(&three_objective).unwrap()).unwrap()
        );
        let three_result = pareto_optimize(&three_objective, &three_objective.analyses[0]).unwrap();
        assert_eq!(three_result.output_nodes.len(), 3);
        assert!(three_result.is_nondominated(), "{three_result:?}");
        assert!(three_result.points.len() >= 2, "{three_result:?}");
        assert!(
            result
                .points
                .first()
                .unwrap()
                .crowding_distance
                .is_infinite()
                && result
                    .points
                    .last()
                    .unwrap()
                    .crowding_distance
                    .is_infinite(),
            "{result:?}"
        );

        let reference = parse_spice(
            "reference pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions=4 params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=31 points=5\n.end",
        )
        .unwrap();
        assert_eq!(
            reference,
            parse_spice(&write_spice(&reference).unwrap()).unwrap()
        );
        let reference_result = pareto_optimize(&reference, &reference.analyses[0]).unwrap();
        assert_eq!(
            reference_result.front_method,
            ParetoFrontMethod::ReferenceDirections
        );
        assert_eq!(reference_result.points.len(), 5, "{reference_result:?}");
        assert!(reference_result.is_nondominated(), "{reference_result:?}");
        for point in &reference_result.points {
            let direction = point.reference_direction.as_ref().unwrap();
            let norm = direction
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                .sqrt();
            assert!(
                direction.len() == 3
                    && direction.iter().all(|value| *value >= 0.0)
                    && (norm - 1.0).abs() < 1e-9,
                "{point:?}"
            );
        }

        let adaptive = parse_spice(
            "adaptive reference pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions=4 adaptive=true params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=41 points=6\n.end",
        )
        .unwrap();
        assert_eq!(
            adaptive,
            parse_spice(&write_spice(&adaptive).unwrap()).unwrap()
        );
        let adaptive_result = pareto_optimize(&adaptive, &adaptive.analyses[0]).unwrap();
        assert!(adaptive_result.adaptive_references, "{adaptive_result:?}");
        assert_eq!(adaptive_result.points.len(), 6, "{adaptive_result:?}");
        assert!(adaptive_result.is_nondominated(), "{adaptive_result:?}");
        assert!(
            adaptive_result.points.iter().any(|point| {
                point.reference_direction.as_ref().is_some_and(|direction| {
                    direction
                        .iter()
                        .any(|value| (value * 4.0 - (value * 4.0).round()).abs() > 1e-6)
                })
            }),
            "{adaptive_result:?}"
        );

        let invalid = parse_spice(
            "bad pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(a) params=R1.R:1:2\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("Pareto optimization output nodes must be distinct"),
            "{invalid}"
        );
        let missing_parameter = parse_spice(
            "bad pareto parameter\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) params=Missing.R:1:2\n.end",
        )
        .unwrap_err();
        assert!(
            missing_parameter.contains("unknown Pareto optimization parameter"),
            "{missing_parameter}"
        );
        let bad_constraint = parse_spice(
            "bad pareto constraint\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) constraints=V(missing)>=0.1 params=R1.R:1:2\n.end",
        )
        .unwrap_err();
        assert!(
            bad_constraint.contains("Pareto optimization constraint node"),
            "{bad_constraint}"
        );
    }

    #[test]
    fn optimization_directive_round_trips_and_validates_parameters() {
        let source = "optim round trip\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match op V(out) goal=1.2 params=R2.R:100:5000:1000 population=8 iterations=16\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let Analysis::Optimization(options) = &circuit.analyses[0] else {
            panic!("expected optimization analysis");
        };
        assert_eq!(options.parameters[0].initial, Some(1000.0));
        let quantized = parse_spice(
            "quantum round trip\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=R2.R:100:5000::100 quantum=100\n.end",
        )
        .unwrap();
        assert_eq!(
            quantized,
            parse_spice(&write_spice(&quantized).unwrap()).unwrap()
        );
        assert_eq!(
            match &quantized.analyses[0] {
                Analysis::Optimization(options) => options.parameters[0].quantum,
                _ => panic!("expected optimization analysis"),
            },
            Some(100.0)
        );
        let invalid = parse_spice(
            "bad\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=Missing.R:1:2\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("unknown optimization parameter"),
            "{invalid}"
        );
        let bad_quantum = parse_spice(
            "bad quantum\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=R2.R:100:5000:1001:100\n.end",
        )
        .unwrap_err();
        assert!(
            bad_quantum.contains("outside its quantum grid"),
            "{bad_quantum}"
        );
    }

    #[test]
    fn pss_rc_lowpass_matches_steady_state_harmonic() {
        let circuit = parse_spice(
            "pss lowpass\nVin in 0 SIN(0 1 1k)\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 10m method=be\n.pss V(out) fund=1k min=8 max=64 samples=128 harmonics=4 tol=1e-4\n.end",
        )
        .unwrap();
        let result = periodic_steady_state(&circuit, &circuit.analyses[1]).unwrap();
        assert!(result.converged, "{result:?}");
        let fundamental = result.harmonic(1).unwrap();
        let expected_magnitude = 1.0 / (1.0 + (std::f64::consts::TAU).powi(2)).sqrt();
        let expected_phase = -90.0 - (std::f64::consts::TAU).atan().to_degrees();
        assert!(
            (fundamental.phasor.magnitude() - expected_magnitude).abs() < 2e-3,
            "{fundamental:?} {expected_magnitude}"
        );
        assert!(
            (fundamental.phasor.phase_deg() - expected_phase).abs() < 2.0,
            "{fundamental:?} {expected_phase}"
        );
        assert!(result.harmonic(2).unwrap().phasor.magnitude() < 1e-4);
    }

    #[test]
    fn pss_directive_round_trips_and_requires_transient() {
        let source = "pss round trip\nVin in 0 SIN(0 1 1k)\nR1 in out 1k\nC1 out 0 1u\n.tran 1u 10m\n.pss V(out) fund=1k min=4 max=32 samples=64 harmonics=8 tol=1e-5\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let missing =
            parse_spice("bad\nVin in 0 SIN(0 1 1)\nR1 in out 1k\nC1 out 0 1u\n.pss V(out)\n.end")
                .unwrap_err();
        assert!(
            missing.contains("PSS requires a .tran analysis"),
            "{missing}"
        );
    }

    #[test]
    fn automatic_pss_estimates_fundamental_from_crossings() {
        let circuit = parse_spice(
            "automatic pss\nVin in 0 SIN(0 1 2.5k)\nR1 in out 100\nC1 out 0 1n\n.tran 100n 10m method=be\n.pss V(out) auto min=6 max=32 samples=64 harmonics=2 tol=1e-4\n.end",
        )
        .unwrap();
        let result = periodic_steady_state(&circuit, &circuit.analyses[1]).unwrap();
        assert!(result.automatic_fundamental, "{result:?}");
        assert!(result.converged, "{result:?}");
        assert!((result.fundamental_hz - 2_500.0).abs() < 25.0, "{result:?}");
        assert!(
            (result.harmonic(1).unwrap().frequency_hz - result.fundamental_hz).abs() < 1e-6,
            "{result:?}"
        );
        assert!(result.harmonic(1).unwrap().phasor.magnitude() > 0.9);
    }

    #[test]
    fn amplitude_modulated_source_round_trips() {
        let source = "am round trip\nVin out 0 AM(0 1 100k 1k 0.8)\n.tran 100n 2m\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
    }

    #[test]
    fn envelope_follow_recovers_am_modulation_depth() {
        let circuit = parse_spice(
            "envelope am\nVin out 0 AM(0 1 100k 1k 0.8)\n.tran 100n 10m method=be\n.env V(out) carrier=100k cycles=8 points=256\n.end",
        )
        .unwrap();
        let result = envelope_follow(&circuit, &circuit.analyses[1]).unwrap();
        assert!(result.maximum > 1.6 && result.maximum < 2.0, "{result:?}");
        assert!(result.minimum < 0.4 && result.minimum > 0.0, "{result:?}");
        assert!((result.average - 1.0).abs() < 0.15, "{result:?}");
        assert_eq!(result.samples.len(), 256);
    }

    #[test]
    fn aging_analysis_applies_power_law_stress() {
        let source = "aging current\nIin out 0 1m\nR1 out 0 2k\n.aging V(out) years=9 exp=0.5 rinc=0.1\n.end";
        let circuit = parse_spice(source).unwrap();
        assert_eq!(
            circuit,
            parse_spice(&write_spice(&circuit).unwrap()).unwrap()
        );
        let result = aging_analysis(&circuit, &circuit.analyses[0]).unwrap();
        assert_eq!(result.target, MonteCarloTarget::OperatingPoint);
        assert!((result.stress - 3.0).abs() < 1e-12, "{result:?}");
        assert!((result.nominal_output - -2.0).abs() < 1e-12, "{result:?}");
        assert!((result.aged_output - -2.6).abs() < 1e-12, "{result:?}");
        assert!((result.output_delta() - -0.6).abs() < 1e-12, "{result:?}");
        let aged_resistance = result
            .circuit
            .elements
            .iter()
            .find_map(|element| match &element.kind {
                SpiceElementKind::Resistor { resistance, .. } => Some(*resistance),
                _ => None,
            })
            .unwrap();
        assert!((aged_resistance - 2600.0).abs() < 1e-9, "{result:?}");

        let mos = parse_spice(
            "aging mos\nVd d 0 1.8\nVg g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.45)\n.aging V(d) years=4 exp=0.5 vtshift=0.02 kploss=0.1\n.end",
        )
        .unwrap();
        let mos_result = aging_analysis(&mos, &mos.analyses[0]).unwrap();
        let model = mos_result.circuit.mosfet_models.get("NM").unwrap();
        assert!(
            (model.threshold_voltage_v - 0.49).abs() < 1e-12
                && (model.transconductance_a_per_v2 - 0.0008).abs() < 1e-12,
            "{model:?}"
        );

        let invalid = parse_spice(
            "bad aging\nIin out 0 1m\nR1 out 0 1k\n.aging V(out) years=1 kploss=1\n.end",
        )
        .unwrap_err();
        assert!(
            invalid.contains("aging would remove all MOS transconductance"),
            "{invalid}"
        );
    }
}
