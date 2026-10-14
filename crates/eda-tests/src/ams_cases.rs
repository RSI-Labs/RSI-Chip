//! Data-driven analog/mixed-signal circuits.
//!
//! These cases deliberately span a different parameter/circuit space from the
//! digital suite: resistive DC networks, RC/RL transient charging, AC filters,
//! time-domain source semantics, controlled sources, and nonlinear diodes.

use crate::CaseList;
use eda_api::{
    aging_analysis, dc_operating_point, derivative_analysis, envelope_follow, global_sensitivity,
    monte_carlo, noise_sweep, optimize, pareto_optimize, parse_spice, parse_statistical_model,
    periodic_steady_state, sensitivity, sweep_ac, transient, write_spice, Analysis,
    DerivativeTarget, MonteCarloDistribution, ParetoFrontMethod, SpiceElementKind,
};

pub const AMS_CASE_COUNT: usize = 3110;

pub fn register(cases: &mut CaseList) {
    resistor_dividers(cases);
    physical_resistor_cases(cases);
    resistor_layer_model_cases(cases);
    resistor_width_bin_cases(cases);
    rc_transients(cases);
    rl_transients(cases);
    ac_low_pass_filters(cases);
    source_waveforms(cases);
    voltage_controlled_sources(cases);
    nonlinear_diodes(cases);
    compact_transistors(cases);
    bsim3_long_channel_cases(cases);
    bsim3_short_channel_cases(cases);
    bsim3_narrow_width_cases(cases);
    bsim3_temperature_cases(cases);
    bsim3_geometry_binning_cases(cases);
    advanced_mos_mobility_cases(cases);
    noise_sweeps(cases);
    transistor_charge_cases(cases);
    monte_carlo_cases(cases);
    correlated_monte_carlo_cases(cases);
    correlated_matrix_monte_carlo_cases(cases);
    spatial_monte_carlo_cases(cases);
    distribution_monte_carlo_cases(cases);
    covariance_monte_carlo_cases(cases);
    vendor_statistical_model_cases(cases);
    aging_cases(cases);
    junction_capacitance_cases(cases);
    mos_charge_partition_cases(cases);
    flicker_noise_cases(cases);
    temperature_cases(cases);
    transit_charge_cases(cases);
    sensitivity_cases(cases);
    analytic_derivative_cases(cases);
    global_sensitivity_cases(cases);
    correlated_global_sensitivity_cases(cases);
    resistor_temperature_cases(cases);
    optimization_cases(cases);
    quantized_optimization_cases(cases);
    pareto_optimization_cases(cases);
    constrained_pareto_cases(cases);
    algebraic_constraint_pareto_cases(cases);
    function_constraint_pareto_cases(cases);
    multi_objective_pareto_cases(cases);
    reference_direction_pareto_cases(cases);
    adaptive_reference_pareto_cases(cases);
    periodic_steady_state_cases(cases);
    automatic_pss_cases(cases);
    autonomous_pss_cases(cases);
    envelope_cases(cases);
}

fn trapezoidal_rc_derivatives(
    resistance: f64,
    equivalent_capacitance: f64,
    step_s: f64,
    steps: usize,
    source_value: f64,
) -> (f64, Vec<f64>, Vec<Vec<f64>>) {
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
            let rhs_first = match left {
                0 => -source_value / (resistance * resistance),
                1 | 2 => conductance_first[left] * voltage,
                3 => 1.0 / resistance,
                _ => unreachable!(),
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
                    + conductance_first[right] * (next_voltage_first[left] - voltage_first[left])
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
    (voltage, voltage_first, voltage_second)
}

fn nonlinear_diode_ac_magnitude(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
) -> f64 {
    let text = format!(
        "nonlinear diode AC reference\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1 TT={transit_time} CJO={junction_capacitance} VJ=700m M=0.5 FC=0.5)\n.temp 26.85\n.ac lin 1 20 20\n.end"
    );
    let circuit = parse_spice(&text).unwrap();
    sweep_ac(&circuit, &circuit.analyses[0])
        .unwrap()
        .node_magnitude("out")
        .unwrap()[0]
}

fn nonlinear_diode_ac_reference(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
) -> (Vec<f64>, Vec<Vec<f64>>) {
    let deltas = [
        resistance * 1e-2,
        saturation_current * 1e-3,
        dc_value * 1e-3,
        ac_magnitude * 1e-3,
        transit_time * 1e-3,
        junction_capacitance * 1e-2,
    ];
    let point = |values: [f64; 6]| {
        nonlinear_diode_ac_magnitude(
            values[0], values[1], values[2], values[3], values[4], values[5],
        )
    };
    let nominal = [
        resistance,
        saturation_current,
        dc_value,
        ac_magnitude,
        transit_time,
        junction_capacitance,
    ];
    let mut gradient = Vec::with_capacity(6);
    for index in 0..6 {
        let mut high = nominal;
        let mut low = nominal;
        high[index] += deltas[index];
        low[index] -= deltas[index];
        gradient.push((point(high) - point(low)) / (2.0 * deltas[index]));
    }
    let mut hessian = vec![vec![0.0; 6]; 6];
    for left in 0..6 {
        let mut high = nominal;
        let mut low = nominal;
        high[left] += deltas[left];
        low[left] -= deltas[left];
        hessian[left][left] =
            (point(high) - 2.0 * point(nominal) + point(low)) / (deltas[left] * deltas[left]);
    }
    for left in 0..6 {
        for right in (left + 1)..6 {
            let mut high_low = nominal;
            let mut low_high = nominal;
            let mut high_high = nominal;
            let mut low_low = nominal;
            high_high[left] += deltas[left];
            high_high[right] += deltas[right];
            high_low[left] += deltas[left];
            high_low[right] -= deltas[right];
            low_high[left] -= deltas[left];
            low_high[right] += deltas[right];
            low_low[left] -= deltas[left];
            low_low[right] -= deltas[right];
            hessian[left][right] = (point(high_high) - point(high_low) - point(low_high)
                + point(low_low))
                / (4.0 * deltas[left] * deltas[right]);
            hessian[right][left] = hessian[left][right];
        }
    }
    (gradient, hessian)
}

#[allow(clippy::too_many_arguments)]
fn nonlinear_diode_junction_reference(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
    junction_potential: f64,
    grading_coefficient: f64,
    forward_knee: f64,
) -> (Vec<f64>, Vec<Vec<f64>>) {
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
    let nominal = [
        resistance,
        saturation_current,
        dc_value,
        ac_magnitude,
        transit_time,
        junction_capacitance,
        junction_potential,
        grading_coefficient,
        forward_knee,
    ];
    let deltas = [
        resistance * 1e-2,
        saturation_current * 1e-3,
        dc_value * 1e-3,
        ac_magnitude * 1e-3,
        transit_time * 1e-3,
        1e-10,
        junction_potential * 1e-3,
        grading_coefficient * 1e-3,
        forward_knee * 1e-3,
    ];
    let mut gradient = vec![0.0; 9];
    for index in 0..9 {
        let mut high = nominal;
        let mut low = nominal;
        high[index] += deltas[index];
        low[index] -= deltas[index];
        gradient[index] = (evaluate(high) - evaluate(low)) / (2.0 * deltas[index]);
    }
    let mut hessian = vec![vec![0.0; 9]; 9];
    for left in 0..9 {
        let mut high = nominal;
        let mut low = nominal;
        high[left] += deltas[left];
        low[left] -= deltas[left];
        hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal) + evaluate(low))
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
            hessian[left][right] = (evaluate(high_high) - evaluate(high_low) - evaluate(low_high)
                + evaluate(low_low))
                / (4.0 * deltas[left] * deltas[right]);
            hessian[right][left] = hessian[left][right];
        }
    }
    (gradient, hessian)
}

fn resistor_dividers(cases: &mut CaseList) {
    for idx in 0..301 {
        let r1 = 100.0 + idx as f64 * 37.0;
        let r2 = 1_000.0 + idx as f64 * 53.0;
        let vin = 0.5 + (idx % 17) as f64 * 0.1;
        let expected = vin * r2 / (r1 + r2);
        let source =
            format!("divider {idx}\nVin in 0 {vin}\nR1 in out {r1}\nR2 out 0 {r2}\n.op\n.end");
        cases.add(format!("ams_resistor_divider_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let got = op
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing out node".to_string())?;
            if (got - expected).abs() > 1e-9 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn physical_resistor_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 0.8 + (idx % 8) as f64 * 0.1;
        let sheet1 = 25.0 + (idx % 5) as f64 * 10.0;
        let sheet2 = 40.0 + (idx % 7) as f64 * 15.0;
        let length1 = 500e-9 + (idx % 6) as f64 * 100e-9;
        let width1 = 40e-9 + (idx % 5) as f64 * 10e-9;
        let length2 = 800e-9 + (idx % 4) as f64 * 150e-9;
        let width2 = 60e-9 + (idx % 3) as f64 * 20e-9;
        let multiplier2 = 1.0 + (idx % 4) as f64;
        let r1 = sheet1 * length1 / width1;
        let r2 = multiplier2 * sheet2 * length2 / width2;
        let expected = supply * r2 / (r1 + r2);
        let source = format!(
            "physical resistor {idx}\nVin in 0 {supply}\nR1 in out SHEET={sheet1} L={length1} W={width1}\nR2 out 0 {multiplier2} SHEET={sheet2} L={length2} W={width2}\n.op\n.end"
        );
        cases.add(format!("ams_physical_sheet_resistor_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            assert_eq!(
                circuit,
                parse_spice(&write_spice(&circuit).unwrap()).unwrap()
            );
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("out")
                .copied()
                .ok_or("missing out node")?;
            if (got - expected).abs() > 1e-9 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn resistor_layer_model_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 0.8 + (idx % 8) as f64 * 0.1;
        let sheet = 35.0 + (idx % 6) as f64 * 8.0;
        let tc1 = 0.0005 + (idx % 5) as f64 * 0.0004;
        let tc2 = (idx % 3) as f64 * 0.000002;
        let width_offset = -2e-9 + (idx % 4) as f64 * 1e-9;
        let length1 = 600e-9 + (idx % 5) as f64 * 120e-9;
        let width1 = 55e-9 + (idx % 6) as f64 * 8e-9;
        let length2 = 800e-9 + (idx % 4) as f64 * 140e-9;
        let width2 = 65e-9 + (idx % 3) as f64 * 10e-9;
        let multiplier2 = 1.0 + (idx % 3) as f64;
        let celsius = 25.0 + (idx % 7) as f64 * 15.0;
        let temperature = celsius + 273.15;
        let delta = temperature - 300.0;
        let factor = 1.0 + tc1 * delta + tc2 * delta * delta;
        let r1 = sheet * length1 / (width1 + width_offset) * factor;
        let r2 = multiplier2 * sheet * length2 / (width2 + width_offset) * factor;
        let expected = supply * r2 / (r1 + r2);
        let source = format!(
            "resistor layer model {idx}\n.rmodel M{idx} SHEET={sheet} TC1={tc1} TC2={tc2} WOFF={width_offset} MINW=40n MAXW=200n\n.temp {celsius}\nVin in 0 {supply}\nR1 in out LAYER=M{idx} L={length1} W={width1}\nR2 out 0 {multiplier2} LAYER=M{idx} L={length2} W={width2}\n.op\n.end"
        );
        cases.add(format!("ams_resistor_layer_model_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            assert_eq!(
                circuit,
                parse_spice(&write_spice(&circuit).unwrap()).unwrap()
            );
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("out")
                .copied()
                .ok_or("missing out node")?;
            if (got - expected).abs() > 1e-9 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn resistor_width_bin_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 0.8 + (idx % 8) as f64 * 0.1;
        let celsius = 25.0 + (idx % 7) as f64 * 15.0;
        let temperature = celsius + 273.15;
        let delta = temperature - 300.0;
        let tc1 = 0.0005 + (idx % 5) as f64 * 0.0003;
        let tc2 = (idx % 3) as f64 * 0.000002;
        let temperature_factor = 1.0 + tc1 * delta + tc2 * delta * delta;
        let width1 = 55e-9 + (idx % 10) as f64 * 5e-9;
        let width2 = 85e-9 + (idx % 6) as f64 * 10e-9;
        let interpolate = |width: f64| {
            let (sheet, offset) = if width <= 50e-9 {
                (50.0, 0.0)
            } else if width >= 150e-9 {
                (72.0, -4e-9)
            } else if width <= 100e-9 {
                let alpha = (width - 50e-9) / 50e-9;
                (50.0 + alpha * 10.0, -alpha * 2e-9)
            } else {
                let alpha = (width - 100e-9) / 50e-9;
                (60.0 + alpha * 12.0, -2e-9 - alpha * 2e-9)
            };
            sheet * 1e-6 / (width + offset) * temperature_factor
        };
        let r1 = interpolate(width1);
        let r2 = 2.0 * interpolate(width2);
        let expected = supply * r2 / (r1 + r2);
        let source = format!(
            "resistor width bins {idx}\n.rmodel BINS SHEET=50 TC1={tc1} TC2={tc2} WOFF=0 MINW=40n MAXW=200n BINS=50n:50:0,100n:60:-2n,150n:72:-4n\n.temp {celsius}\nVin in 0 {supply}\nR1 in out LAYER=BINS L=1u W={width1}\nR2 out 0 2 LAYER=BINS L=1u W={width2}\n.op\n.end"
        );
        cases.add(format!("ams_resistor_width_bin_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            assert_eq!(
                circuit,
                parse_spice(&write_spice(&circuit).unwrap()).unwrap()
            );
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("out")
                .copied()
                .ok_or("missing out node")?;
            if (got - expected).abs() > 1e-9 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn rc_transients(cases: &mut CaseList) {
    for idx in 0..251 {
        let resistance = 500.0 + idx as f64 * 13.0;
        let capacitance = 1e-10 + (idx % 19) as f64 * 1e-11;
        let tau = resistance * capacitance;
        let step = tau / 10.0;
        let stop = tau * 5.0;
        let expected = 1.0 - (-5.0_f64).exp();
        let source = format!(
            "rc transient {idx}\nVin in 0 DC 1\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.tran {step} {stop} method=be\n.end"
        );
        cases.add(format!("ams_rc_transient_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = transient(&circuit, &circuit.analyses[0])?;
            let got = result
                .final_sample()
                .and_then(|sample| sample.node_voltages.get("out"))
                .copied()
                .ok_or_else(|| "missing final V(out)".to_string())?;
            if (got - expected).abs() > 2e-3 {
                Err(format!("final V(out)={got}, expected={expected}"))
            } else if result.convergence_failures != 0 {
                Err(format!(
                    "{} convergence failures",
                    result.convergence_failures
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn rl_transients(cases: &mut CaseList) {
    for idx in 0..150 {
        let resistance = 250.0 + idx as f64 * 29.0;
        let inductance = 1e-7 + (idx % 11) as f64 * 2e-8;
        let tau = inductance / resistance;
        let step = tau / 10.0;
        let stop = tau * 5.0;
        let expected = (1.0 - (-5.0_f64).exp()) / resistance;
        let source = format!(
            "rl transient {idx}\nVin in 0 DC 1\nR1 in out {resistance}\nL1 out 0 {inductance} IC=0\n.tran {step} {stop} method=be\n.end"
        );
        cases.add(format!("ams_rl_transient_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = transient(&circuit, &circuit.analyses[0])?;
            let got = result
                .final_sample()
                .and_then(|sample| sample.branch_currents.get("L1"))
                .copied()
                .ok_or_else(|| "missing final I(L1)".to_string())?;
            if (got - expected).abs() > 2e-3 {
                Err(format!("I(L1)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn ac_low_pass_filters(cases: &mut CaseList) {
    for idx in 0..100 {
        let resistance = 100.0 + idx as f64 * 17.0;
        let capacitance = 1e-8 + (idx % 13) as f64 * 1e-9;
        let source = format!(
            "low-pass {idx}\nVin in 0 DC 0 AC 1\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.ac lin 2 1 1g\n.end"
        );
        cases.add(format!("ams_ac_low_pass_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = sweep_ac(&circuit, &circuit.analyses[0])?;
            let gains = result.node_magnitude("out")?;
            if gains.len() != 2 || gains[0] <= gains[1] || gains[1] >= 1e-3 {
                Err(format!("unexpected gains {gains:?}"))
            } else {
                Ok(())
            }
        });
    }
}

fn source_waveforms(cases: &mut CaseList) {
    for idx in 0..100 {
        let kind = idx % 4;
        let (source, expected) = match kind {
            0 => {
                let offset = (idx % 9) as f64 * 0.1;
                let amplitude = 0.2 + (idx % 7) as f64 * 0.1;
                let frequency = 1e3 + (idx % 11) as f64 * 250.0;
                let delay = (idx % 5) as f64 * 1e-6;
                let time = delay + 0.25 / frequency;
                let value = offset + amplitude;
                (
                    format!("sine {idx}\nV1 n 0 SIN({offset} {amplitude} {frequency} {delay} 0)\nR1 n 0 1k\n.op\n.end"),
                    (time, value),
                )
            }
            1 => {
                let initial = (idx % 3) as f64 * 0.2;
                let pulsed = 1.0 + (idx % 5) as f64 * 0.1;
                let delay = (idx % 4) as f64 * 2e-9;
                (
                    format!("pulse {idx}\nV1 n 0 PULSE({initial} {pulsed} {delay} 1p 1p 1n 2n)\nR1 n 0 1k\n.op\n.end"),
                    (delay + 1e-12 + 0.5e-9, pulsed),
                )
            }
            2 => {
                let v0 = (idx % 5) as f64 * 0.1;
                let v1 = 0.8 + (idx % 8) as f64 * 0.1;
                let expected = v0 + (v1 - v0) * 0.5;
                (
                    format!("pwl {idx}\nV1 n 0 PWL(0 {v0} 1 {v1} 2 {v0})\nR1 n 0 1k\n.op\n.end"),
                    (0.5, expected),
                )
            }
            _ => {
                let value = 0.1 + (idx % 23) as f64 * 0.2;
                (
                    format!("dc {idx}\nV1 n 0 {value}\nR1 n 0 1k\n.op\n.end"),
                    (123.0, value),
                )
            }
        };
        cases.add(format!("ams_source_waveform_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let element = circuit
                .element("V1")
                .ok_or_else(|| "missing V1".to_string())?;
            let got = match &element.kind {
                SpiceElementKind::VoltageSource { waveform, .. } => waveform.value_at(expected.0),
                _ => return Err("V1 is not a voltage source".into()),
            };
            if (got - expected.1).abs() > 1e-9 {
                Err(format!("V1(t)={got}, expected={}", expected.1))
            } else {
                Ok(())
            }
        });
    }
}

fn voltage_controlled_sources(cases: &mut CaseList) {
    for idx in 0..50 {
        let vin = 0.3 + (idx % 12) as f64 * 0.1;
        let resistance = 500.0 + idx as f64 * 91.0;
        let gain = -2.0 + idx as f64 * 0.25;
        let expected = vin * 0.5 * gain;
        let source = format!(
            "vcvs {idx}\nV1 in 0 {vin}\nR1 in ctrl {resistance}\nR2 ctrl 0 {resistance}\nE1 out 0 ctrl 0 {gain}\n.op\n.end"
        );
        cases.add(format!("ams_voltage_controlled_source_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let got = op
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing out node".to_string())?;
            if (got - expected).abs() > 1e-9 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..10 {
        let target = 0.25 + idx as f64 * 0.05;
        let capacitance = 0.5e-6 + (idx % 4) as f64 * 0.1e-6;
        let frequency = 750.0 + idx as f64 * 431.0;
        let expected_resistance = (1.0 / (target * target) - 1.0).sqrt()
            / (std::f64::consts::TAU * frequency * capacitance);
        let nominal = expected_resistance * 2.0;
        let lower = 10.0;
        let upper = expected_resistance * 6.0 + 100.0;
        let source = format!(
            "optim ac lowpass {idx}\nVin in 0 DC 0 AC 1\nR1 in out {nominal}\nC1 out 0 {capacitance}\n.ac lin 2 {frequency} {frequency}\n.optim match ac V(out) goal={target} freq={frequency} params=R1.R:{lower}:{upper} population=8 iterations=1 seed={} tol=1e-9\n.end",
            4300 + idx
        );
        cases.add(format!("ams_optimization_ac_lowpass_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = optimize(&circuit, &circuit.analyses[1])?;
            let resistance = result
                .parameters
                .iter()
                .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R1.R"))
                .ok_or_else(|| "missing optimized AC resistor".to_string())?
                .final_value;
            if !result.converged
                || (result.final_output - target).abs() > 1e-7
                || (resistance - expected_resistance).abs() > expected_resistance * 1e-4
            {
                Err(format!(
                    "target={target}, expected_resistance={expected_resistance}, result={result:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn nonlinear_diodes(cases: &mut CaseList) {
    for idx in 0..48 {
        let supply = 0.45 + idx as f64 * 0.012;
        let resistance = 1_000.0 + idx as f64 * 137.0;
        let exponent = 13.0 + (idx % 5) as f64;
        let saturation = 10f64.powf(-exponent);
        let source = format!(
            "diode {idx}\nV1 in 0 {supply}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation} N=1)\n.op\n.end"
        );
        cases.add(format!("ams_nonlinear_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let vout = op
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing out node".to_string())?;
            let current = (supply - vout) / resistance;
            let expected = saturation * (vout / 0.025_852).exp_m1();
            if (current - expected).abs() > 1e-7 + 1e-6 * expected.abs() {
                Err(format!(
                    "KCL mismatch: resistor current={current}, diode current={expected}, V(out)={vout}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn compact_transistors(cases: &mut CaseList) {
    // NMOS Level-1 saturation: choose a load that fixes V(D)=1.2 V, well
    // above each device's overdrive voltage.
    for idx in 0..40 {
        let kp = 5e-4 + idx as f64 * 1e-4;
        let width = 500e-9 + (idx % 5) as f64 * 100e-9;
        let length = 180e-9 + (idx % 4) as f64 * 20e-9;
        let threshold = 0.3 + (idx % 6) as f64 * 0.05;
        let gate = threshold + 0.4;
        let expected = 0.5 * kp * (width / length) * (gate - threshold) * (gate - threshold);
        let resistance = 0.6 / expected;
        let source = format!(
            "nmos level1 {idx}\nVdd vdd 0 1.8\nRd vdd d {resistance}\nVg g 0 {gate}\nM1 d g 0 0 NMOS W={width} L={length}\n.model NMOS NMOS(KP={kp} VTO={threshold})\n.op\n.end"
        );
        cases.add(format!("ams_nmos_level1_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing NMOS drain".to_string())?;
            if (drain - 1.2).abs() > 1e-7 {
                Err(format!("V(d)={drain}, expected=1.2"))
            } else {
                Ok(())
            }
        });
    }

    // PMOS Level-1 saturation with symmetric body-referenced drive.
    for idx in 0..25 {
        let kp = 4e-4 + idx as f64 * 8e-5;
        let width = 600e-9 + (idx % 5) as f64 * 100e-9;
        let length = 180e-9 + (idx % 3) as f64 * 30e-9;
        let threshold = 0.3 + (idx % 5) as f64 * 0.05;
        let gate = -(threshold + 0.4);
        let expected = 0.5 * kp * (width / length) * 0.4 * 0.4;
        let resistance = 0.6 / expected;
        let source = format!(
            "pmos level1 {idx}\nVdd vdd 0 -1.8\nRd vdd d {resistance}\nVg g 0 {gate}\nM1 d g 0 0 PM W={width} L={length}\n.model PM PMOS(KP={kp} VTO=-{threshold})\n.op\n.end"
        );
        cases.add(format!("ams_pmos_level1_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing PMOS drain".to_string())?;
            if (drain + 1.2).abs() > 1e-7 {
                Err(format!("V(d)={drain}, expected=-1.2"))
            } else {
                Ok(())
            }
        });
    }

    // Forward-active NPN current gain. The load is selected from the analytic
    // collector current so every device remains at V(CE)=0.8 V.
    for idx in 0..35 {
        let beta = 60.0 + idx as f64 * 3.0;
        let base_current = 2e-6 + (idx % 5) as f64 * 1e-6;
        let expected_collector_current = beta * base_current;
        let resistance = 1.0 / expected_collector_current;
        let saturation = 10f64.powf(-(13.0 + (idx % 4) as f64));
        let source = format!(
            "npn forward active {idx}\nVdd vdd 0 1.8\nRc vdd c {resistance}\nIb 0 b {base_current}\nQ1 c b 0 NPN\n.model NPN NPN(IS={saturation} BF={beta} BR=1)\n.op\n.end"
        );
        cases.add(format!("ams_bjt_forward_active_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let collector = op
                .node_voltages
                .get("c")
                .copied()
                .ok_or_else(|| "missing BJT collector".to_string())?;
            let base = op
                .node_voltages
                .get("b")
                .copied()
                .ok_or_else(|| "missing BJT base".to_string())?;
            if base <= 0.4 || base >= 0.9 {
                Err(format!("unexpected V(BE)={base}"))
            } else if (collector - 0.8).abs() > 1e-5 {
                Err(format!("V(c)={collector}, expected=0.8"))
            } else {
                Ok(())
            }
        });
    }
}

fn advanced_mos_mobility_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nmos = idx % 2 == 0;
        let kp = 4e-4 + idx as f64 * 2e-5;
        let width = 500e-9 + (idx % 5) as f64 * 75e-9;
        let length = 180e-9 + (idx % 4) as f64 * 20e-9;
        let threshold = 0.3 + (idx % 6) as f64 * 0.04;
        let overdrive = 0.25 + (idx % 7) as f64 * 0.05;
        let theta = [0.0, 0.05, 0.12, 0.25, 0.4][idx % 5];
        let vsat = [0.0, 0.9, 0.6, 0.45, 0.3][idx % 5];
        let gate = threshold + overdrive;
        let effective_kp = if theta > 0.0 {
            kp / (1.0 + theta * overdrive)
        } else {
            kp
        };
        let saturation_vds = if vsat > 0.0 {
            overdrive / (1.0 + overdrive / vsat)
        } else {
            overdrive
        };
        let expected = effective_kp
            * (width / length)
            * (overdrive * saturation_vds - 0.5 * saturation_vds * saturation_vds);
        let source = if nmos {
            format!(
                "nmos mobility velocity {idx}\nVdd d 0 1.8\nVg g 0 {gate}\nM1 d g 0 0 NM W={width} L={length}\n.model NM NMOS(KP={kp} VTO={threshold} THETA={theta} VSAT={vsat})\n.op\n.end"
            )
        } else {
            format!(
                "pmos mobility velocity {idx}\nVdd d 0 -1.8\nVg g 0 -{gate}\nM1 d g 0 0 PM W={width} L={length}\n.model PM PMOS(KP={kp} VTO=-{threshold} THETA={theta} VSAT={vsat})\n.op\n.end"
            )
        };
        cases.add(
            format!("ams_mos_mobility_velocity_saturation_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let op = dc_operating_point(&circuit)?;
                let current = op
                    .branch_currents
                    .get("Vdd")
                    .copied()
                    .ok_or("missing drain supply current")?;
                let signed_expected = if nmos { -expected } else { expected };
                if (current - signed_expected).abs() > 1e-9 + expected * 1e-9 {
                    Err(format!(
                        "theta={theta}, vsat={vsat}, current={current}, expected={signed_expected}"
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }

    for idx in 0..20 {
        let current = 0.0008 + idx as f64 * 0.00021;
        let resistance = 600.0 + idx as f64 * 47.0;
        let saturation_current = 5e-13 + (idx % 5) as f64 * 2.5e-13;
        let emission_coefficient = 0.9 + (idx % 7) as f64 * 0.1;
        let source = format!(
            "analytic derivative diode {idx}\nIin 0 out {current}\nR1 out 0 {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N={emission_coefficient})\n.temp 26.85\n.deriv V(out) params=R1.R,Iin.DC,DN.IS\n.end"
        );
        cases.add(format!("ams_analytic_derivative_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
            let voltage = result.nominal_output;
            let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19 * emission_coefficient;
            let exponent = (voltage / thermal_voltage).exp();
            let normalized_current = exponent - 1.0;
            let jacobian = 1.0 / resistance + saturation_current * exponent / thermal_voltage;
            let d_r = voltage / (resistance * resistance) / jacobian;
            let d_i = 1.0 / jacobian;
            let d_is = -normalized_current / jacobian;
            let expected_gradient = [d_r, d_i, d_is];
            let nonlinear_second =
                saturation_current * exponent / (thermal_voltage * thermal_voltage);
            let d2_r = -(2.0 * voltage / resistance.powi(3)
                + 2.0 * (-1.0 / (resistance * resistance)) * d_r
                + nonlinear_second * d_r * d_r)
                / jacobian;
            let d2_ri = -((-1.0 / (resistance * resistance)) * d_i + nonlinear_second * d_r * d_i)
                / jacobian;
            let d2_ris = -((-1.0 / (resistance * resistance)) * d_is
                + (exponent / thermal_voltage) * d_r
                + nonlinear_second * d_r * d_is)
                / jacobian;
            let d2_ii = -(nonlinear_second * d_i * d_i) / jacobian;
            let d2_iis =
                -((exponent / thermal_voltage) * d_i + nonlinear_second * d_i * d_is) / jacobian;
            let d2_is = -(2.0 * (exponent / thermal_voltage) * d_is
                + nonlinear_second * d_is * d_is)
                / jacobian;
            let expected_hessian = [
                [d2_r, d2_ri, d2_ris],
                [d2_ri, d2_ii, d2_iis],
                [d2_ris, d2_iis, d2_is],
            ];
            for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                if (got - expected).abs() > 1e-8 * (1.0 + expected.abs()) {
                    return Err(format!("gradient {got}/{expected}"));
                }
            }
            for (got, expected) in result
                .hessian()
                .into_iter()
                .flatten()
                .zip(expected_hessian.iter().flatten())
            {
                if (got - expected).abs() > 1e-7 * (1.0 + expected.abs()) {
                    return Err(format!("hessian {got}/{expected}"));
                }
            }
            Ok(())
        });
    }
}

fn bsim3_long_channel_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let threshold = 0.32 + (idx % 6) as f64 * 0.035;
        let oxide_thickness = 1.6e-9 + (idx % 5) as f64 * 0.15e-9;
        let low_field_mobility = 0.035 + (idx % 4) as f64 * 0.004;
        let first_order_mobility = 0.7e-15 + (idx % 5) as f64 * 0.12e-15;
        let second_order_mobility = 0.6e-24 + (idx % 4) as f64 * 0.18e-24;
        let saturation_velocity = 70_000.0 + (idx % 5) as f64 * 3_500.0;
        let width = 750e-9 + (idx % 6) as f64 * 85e-9;
        let length = 165e-9 + (idx % 5) as f64 * 21e-9;
        let supply = 1.55 + (idx % 4) as f64 * 0.08;
        let resistance = 480.0 + idx as f64 * 61.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold;
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = threshold;
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
        let source = format!(
            "BSIM3 long channel {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={threshold} K1=0 TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_long_channel_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_short_channel_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.32 + (idx % 5) as f64 * 0.025;
        let oxide_thickness = 1.7e-9 + (idx % 4) as f64 * 0.12e-9;
        let low_field_mobility = 0.036 + (idx % 5) as f64 * 0.003;
        let first_order_mobility = 0.8e-15 + (idx % 4) as f64 * 0.1e-15;
        let second_order_mobility = 0.7e-24 + (idx % 3) as f64 * 0.15e-24;
        let saturation_velocity = 72_000.0 + (idx % 6) as f64 * 2_000.0;
        let short_channel_amplitude = 0.025 + (idx % 5) as f64 * 0.012;
        let short_channel_scale = 16e-9 + (idx % 4) as f64 * 4e-9;
        let drain_bias_coefficient = 0.02 + (idx % 5) as f64 * 0.015;
        let width = 800e-9 + (idx % 5) as f64 * 70e-9;
        let length = 40e-9 + (idx % 6) as f64 * 25e-9;
        let supply = 1.45 + (idx % 4) as f64 * 0.07;
        let resistance = 520.0 + idx as f64 * 53.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = |voltage: f64| {
            nominal_threshold
                - short_channel_amplitude
                    * (-(length / short_channel_scale)).exp()
                    * (1.0 + drain_bias_coefficient * voltage)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold(voltage);
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.2).max(0.0);
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
        let source = format!(
            "BSIM3 short channel {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0 DVT0={short_channel_amplitude} DVT1={short_channel_scale} DVT2={drain_bias_coefficient} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_short_channel_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_narrow_width_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.3 + (idx % 5) as f64 * 0.03;
        let oxide_thickness = 1.75e-9 + (idx % 4) as f64 * 0.11e-9;
        let low_field_mobility = 0.037 + (idx % 4) as f64 * 0.004;
        let first_order_mobility = 0.85e-15 + (idx % 5) as f64 * 0.09e-15;
        let second_order_mobility = 0.75e-24 + (idx % 3) as f64 * 0.12e-24;
        let saturation_velocity = 73_000.0 + (idx % 5) as f64 * 1_800.0;
        let narrow_amplitude = 0.035 + (idx % 6) as f64 * 0.011;
        let narrow_exponent = 0.2 + (idx % 5) as f64 * 0.09;
        let reference_width = 220e-9 + (idx % 6) as f64 * 18e-9;
        let width = 120e-9 + (idx % 8) as f64 * 32e-9;
        let length = 175e-9 + (idx % 5) as f64 * 19e-9;
        let supply = 1.5 + (idx % 4) as f64 * 0.06;
        let resistance = 560.0 + idx as f64 * 49.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = || {
            nominal_threshold
                + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold();
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.15).max(0.0);
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
        let source = format!(
            "BSIM3 narrow width {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0 DVT0W={narrow_amplitude} DVT1W={narrow_exponent} W0={reference_width} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_narrow_width_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_temperature_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.31 + (idx % 5) as f64 * 0.028;
        let oxide_thickness = 1.8e-9 + (idx % 4) as f64 * 0.1e-9;
        let low_field_mobility = 0.038 + (idx % 5) as f64 * 0.0035;
        let first_order_mobility = 0.82e-15 + (idx % 4) as f64 * 0.11e-15;
        let second_order_mobility = 0.72e-24 + (idx % 3) as f64 * 0.14e-24;
        let saturation_velocity = 74_000.0 + (idx % 4) as f64 * 1_700.0;
        let short_channel_amplitude = 0.028 + (idx % 5) as f64 * 0.01;
        let short_channel_scale = 17e-9 + (idx % 4) as f64 * 4e-9;
        let drain_bias_coefficient = 0.025 + (idx % 5) as f64 * 0.013;
        let narrow_amplitude = 0.04 + (idx % 4) as f64 * 0.012;
        let narrow_exponent = 0.22 + (idx % 5) as f64 * 0.08;
        let reference_width = 225e-9 + (idx % 5) as f64 * 17e-9;
        let threshold_temperature_coefficient = -0.19 + (idx % 5) as f64 * 0.018;
        let body_temperature_coefficient = -0.045 + (idx % 4) as f64 * 0.012;
        let length_temperature_coefficient = (1.0 + (idx % 4) as f64) * 1e-11;
        let mobility_temperature_exponent = 1.25 + (idx % 5) as f64 * 0.08;
        let width = 180e-9 + (idx % 7) as f64 * 35e-9;
        let length = 145e-9 + (idx % 5) as f64 * 23e-9;
        let body_bias = -0.3 + (idx % 6) as f64 * 0.08;
        let temperature_c = [-25.0, -10.0, 15.0, 45.0, 85.0, 125.0][idx % 6];
        let temperature_k = temperature_c + 273.15;
        let supply = 1.5 + (idx % 4) as f64 * 0.06;
        let resistance = 540.0 + idx as f64 * 47.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = |voltage: f64| {
            let surface = 0.7_f64;
            let body_term = (surface - body_bias).max(0.0).sqrt() - surface.sqrt();
            nominal_threshold + 0.5 * body_term
                - short_channel_amplitude
                    * (-(length / short_channel_scale)).exp()
                    * (1.0 + drain_bias_coefficient * voltage)
                + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0)
                + (threshold_temperature_coefficient + body_temperature_coefficient * body_bias
                    - length_temperature_coefficient / length)
                    * (temperature_k / 300.0 - 1.0)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold(voltage);
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let nominal_mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let mobility =
                nominal_mobility * (300.0 / temperature_k).powf(mobility_temperature_exponent);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.2).max(0.0);
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
        let source = format!(
            "BSIM3 temperature {idx}\n.temp {temperature_c}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nVb b 0 {body_bias}\nM1 d d 0 b NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0.5 DVT0={short_channel_amplitude} DVT1={short_channel_scale} DVT2={drain_bias_coefficient} DVT0W={narrow_amplitude} DVT1W={narrow_exponent} W0={reference_width} KT1={threshold_temperature_coefficient} KT2={body_temperature_coefficient} KT1L={length_temperature_coefficient} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} UTE={mobility_temperature_exponent})\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_temperature_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_geometry_binning_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let first_bin = idx % 2 == 0;
        let (threshold, low_field_mobility) = if first_bin {
            (0.3, 0.038)
        } else {
            (0.43, 0.048)
        };
        let (width, length) = if first_bin {
            (
                120e-9 + (idx % 5) as f64 * 32e-9,
                105e-9 + (idx % 5) as f64 * 28e-9,
            )
        } else {
            (
                360e-9 + (idx % 5) as f64 * 85e-9,
                300e-9 + (idx % 5) as f64 * 72e-9,
            )
        };
        let oxide_thickness = if first_bin { 1.8e-9 } else { 2.0e-9 };
        let first_order_mobility = 0.8e-15;
        let second_order_mobility = 0.7e-24;
        let saturation_velocity = 72_000.0;
        let short_channel_amplitude = 0.03;
        let short_channel_scale = 18e-9;
        let narrow_amplitude = 0.04;
        let narrow_exponent = 0.22;
        let reference_width = if first_bin { 260e-9 } else { 520e-9 };
        let gate = threshold + 0.4;
        let drain = 1.2;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let overdrive = gate - threshold;
        let normalized = overdrive / oxide_thickness;
        let mobility = low_field_mobility
            / (1.0
                + first_order_mobility * normalized
                + second_order_mobility * normalized * normalized);
        let kp = mobility * oxide_cap;
        let effective_threshold = threshold
            - short_channel_amplitude * (-(length / short_channel_scale)).exp() * 1.0
            + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0);
        let effective_overdrive = gate - effective_threshold;
        let effective_saturation_voltage = effective_overdrive
            / (1.0 + effective_overdrive * mobility / (2.0 * saturation_velocity * length));
        let expected = kp * width / length
            * (effective_overdrive * effective_saturation_voltage
                - effective_saturation_voltage * effective_saturation_voltage / 2.0);
        let source = format!(
            "BSIM3 geometry bin {idx}\nVdd d 0 {drain}\nVg g 0 {gate}\nM1 d g 0 0 NM W={width} L={length}\n.model NM.1 NMOS(LEVEL=8 VTH0=0.3 WMIN=80n WMAX=300n LMIN=80n LMAX=270n DVT0=0.03 DVT1=18n DVT0W=0.04 DVT1W=0.22 W0=260n TOX=1.8n U0=0.038 UA=0.8e-15 UB=0.7e-24 VSAT=72k)\n.model NM.2 NMOS(LEVEL=8 VTH0=0.43 WMIN=320n WMAX=900n LMIN=290n LMAX=800n DVT0=0.03 DVT1=18n DVT0W=0.04 DVT1W=0.22 W0=520n TOX=2n U0=0.048 UA=0.8e-15 UB=0.7e-24 VSAT=72k)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_geometry_bin_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let selected = circuit.mosfet_model_for("NM", width, length)?;
            let expected_threshold = if first_bin { 0.3 } else { 0.43 };
            if (selected.threshold_voltage_v - expected_threshold).abs() > 1e-12 {
                return Err(format!(
                    "selected VTH0={} for W={width} L={length}",
                    selected.threshold_voltage_v
                ));
            }
            let op = dc_operating_point(&circuit)?;
            let current = op
                .branch_currents
                .get("Vdd")
                .copied()
                .ok_or_else(|| "missing binned MOS drain current".to_string())?;
            if (current + expected).abs() > 1e-9 * (1.0 + expected) {
                return Err(format!(
                    "current={current}, expected={}, selected={selected:?}",
                    -expected
                ));
            }
            Ok(())
        });
    }
}

fn noise_sweeps(cases: &mut CaseList) {
    for idx in 0..60 {
        let resistance = 100.0 + idx as f64 * 137.0;
        let expected = (4.0 * 1.380_649e-23 * 300.0 * resistance).sqrt();
        let source = format!(
            "resistor noise {idx}\nIin 0 out DC 0 AC 1\nR1 out 0 {resistance}\n.noise V(out) Iin lin 1 1 1\n.end"
        );
        cases.add(format!("ams_resistor_thermal_noise_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = noise_sweep(&circuit, &circuit.analyses[0])?;
            let got = result.output_noise()[0];
            if (got - expected).abs() > 1e-14 {
                Err(format!("noise={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..40 {
        let supply = 1.2 + (idx % 7) as f64 * 0.1;
        let resistance = 750.0 + idx as f64 * 113.0;
        let saturation = 10f64.powf(-(13.0 + (idx % 4) as f64));
        let source = format!(
            "diode noise {idx}\nV1 in 0 DC {supply} AC 1\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation} N=1)\n.noise V(out) V1 lin 1 1 1\n.end"
        );
        cases.add(format!("ams_diode_shot_noise_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let diode_voltage = op
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing diode output".to_string())?;
            let current = (supply - diode_voltage) / resistance;
            let dynamic_resistance = 0.025_852 / (current + saturation);
            let response = resistance * dynamic_resistance / (resistance + dynamic_resistance);
            let expected = (2.0 * 1.602_176_634e-19 * current).sqrt() * response;
            let result = noise_sweep(&circuit, &circuit.analyses[0])?;
            let got = result.output_noise()[0];
            if (got - expected).abs() > 1e-10 {
                Err(format!("noise={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn transistor_charge_cases(cases: &mut CaseList) {
    for idx in 0..60 {
        let resistance = 5_000.0 + idx as f64 * 500.0;
        let width = 700e-9 + (idx % 5) as f64 * 100e-9;
        let cgso = 600e-6 + (idx % 6) as f64 * 100e-6;
        let capacitance = cgso * width;
        let tau = resistance * capacitance;
        let step = tau / 10.0;
        let stop = tau * 5.0;
        let expected = 1.0 - (-5.0_f64).exp();
        let source = format!(
            "mos charge {idx}\nVin in 0 PULSE(0 1 0 1p 1p 1 1)\nRg in g {resistance}\nVdd d 0 1.8\nM1 d g 0 0 NM W={width} L=180n\n.model NM NMOS(KP=1m VTO=0.5 CGSO={cgso})\n.tran {step} {stop} method=be\n.end"
        );
        cases.add(format!("ams_mos_gate_charge_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = transient(&circuit, &circuit.analyses[0])?;
            let got = result
                .final_sample()
                .and_then(|sample| sample.node_voltages.get("g"))
                .copied()
                .ok_or_else(|| "missing MOS gate voltage".to_string())?;
            if (got - expected).abs() > 2e-3 {
                Err(format!("V(g)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..40 {
        let resistance = 4_000.0 + idx as f64 * 350.0;
        let cje = 50e-12 + (idx % 7) as f64 * 10e-12;
        let cjc = 5e-12 + (idx % 5) as f64 * 2e-12;
        let expected =
            1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * resistance * (cje + cjc)).powi(2)).sqrt();
        let source = format!(
            "bjt charge {idx}\nVin in 0 DC 0 AC 1\nRb in b {resistance}\nVdd vdd 0 1.8\nRc vdd c 1k\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 CJE={cje} CJC={cjc})\n.ac lin 2 1 1g\n.end"
        );
        cases.add(format!("ams_bjt_junction_charge_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = sweep_ac(&circuit, &circuit.analyses[0])?;
            let gains = result.node_magnitude("b")?;
            if gains.len() != 2 || gains[0] < 0.99 || (gains[1] - expected).abs() > 1e-4 {
                Err(format!(
                    "base gains={gains:?}, expected high-frequency gain={expected}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 500.0 + idx as f64 * 37.0;
        let r2 = 1_000.0 + idx as f64 * 53.0;
        let nominal = 1.8 * r2 / (r1 + r2);
        let tolerance = 0.04 + (idx % 6) as f64 * 0.01;
        let source = format!(
            "mc divider {idx}\nVin in 0 1.8\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 32 op V(out) seed={} rtol={tolerance} ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end",
            100 + idx
        );
        cases.add(format!("ams_monte_carlo_divider_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = monte_carlo(&circuit, &circuit.analyses[0])?;
            let stats = result.statistics;
            if result.samples.len() != 32
                || stats.minimum >= stats.mean
                || stats.mean >= stats.maximum
                || stats.standard_deviation <= 0.0
                || (stats.mean - nominal).abs() > tolerance
            {
                Err(format!(
                    "nominal={nominal}, statistics={stats:?}, samples={}",
                    result.samples.len()
                ))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let resistance = 750.0 + idx as f64 * 61.0;
        let capacitance = 400e-12 + (idx % 7) as f64 * 50e-12;
        let tau = resistance * capacitance;
        let step = tau / 10.0;
        let stop = tau * 5.0;
        let source = format!(
            "mc rc {idx}\nVin in 0 PULSE(0 1 0 1p 1p 1 1)\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.tran {step} {stop} method=be\n.mc 16 tran V(out) seed={} rtol=0 ctol=0.25 ltol=0 vtotol=0 kptol=0 bftol=0\n.end",
            500 + idx
        );
        cases.add(format!("ams_monte_carlo_rc_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = monte_carlo(&circuit, &circuit.analyses[1])?;
            if result.samples.len() != 16
                || result.statistics.minimum >= result.statistics.mean
                || result.statistics.mean >= result.statistics.maximum
                || result.statistics.standard_deviation <= 0.0
            {
                Err(format!("statistics={:?}", result.statistics))
            } else {
                Ok(())
            }
        });
    }
}

fn correlated_monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 600.0 + idx as f64 * 43.0;
        let r2 = 1_200.0 + idx as f64 * 71.0;
        let nominal = 1.8 * r2 / (r1 + r2);
        let source = format!(
            "correlated mc divider {idx}\nVin in 0 1.8\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 24 op V(out) seed={} rtol=0.25 rcorr=1 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end",
            900 + idx
        );
        cases.add(
            format!("ams_correlated_monte_carlo_divider_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = monte_carlo(&circuit, &circuit.analyses[0])?;
                if result.samples.len() != 24
                    || result
                        .values()
                        .into_iter()
                        .any(|value| (value - nominal).abs() > 1e-12)
                    || result.statistics.standard_deviation > 1e-12
                {
                    Err(format!(
                        "nominal={nominal}, statistics={:?}, samples={}",
                        result.statistics,
                        result.samples.len()
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }

    for idx in 0..20 {
        let resistance = 500.0 + idx as f64 * 37.0;
        let capacitance = 0.2e-6 + (idx % 6) as f64 * 0.07e-6;
        let source_value = 0.9 + (idx % 5) as f64 * 0.12;
        let initial_voltage = (idx % 4) as f64 * 0.1;
        let target_tau = 0.3 + idx as f64 * 0.21;
        let stop_s = resistance * capacitance * target_tau;
        let step_s = stop_s / 64.0;
        let decay = (-target_tau).exp();
        let initial_difference = initial_voltage - source_value;
        let nominal_output = source_value + initial_difference * decay;
        let expected_gradient = [
            -initial_difference * decay * target_tau / resistance,
            -initial_difference * decay * target_tau / capacitance,
            1.0 - decay,
        ];
        let second_common = initial_difference * decay * target_tau * (target_tau - 2.0);
        let second_rc =
            initial_difference * decay * target_tau * target_tau * (target_tau - 1.0) / stop_s;
        let expected_hessian = [
            [
                second_common / (resistance * resistance),
                second_rc,
                -decay * target_tau / resistance,
            ],
            [
                second_rc,
                second_common / (capacitance * capacitance),
                -decay * target_tau / capacitance,
            ],
            [
                -decay * target_tau / resistance,
                -decay * target_tau / capacitance,
                0.0,
            ],
        ];
        let source = format!(
            "analytic transient derivative rc {idx}\nVin in 0 DC {source_value}\nR1 in out {resistance}\nC1 out 0 {capacitance} IC={initial_voltage}\n.tran {step_s:.15} {stop_s:.15} method=be\n.deriv tran V(out) params=R1.R,C1.C,Vin.DC\n.end"
        );
        cases.add(
            format!("ams_analytic_derivative_transient_rc_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[1])?;
                if result.target != DerivativeTarget::TransientFinal
                    || (result.nominal_output - nominal_output).abs() > 1e-11
                {
                    return Err(format!(
                        "transient derivative metadata: {:?}, expected {nominal_output}",
                        result
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-10 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-9 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..20 {
        let resistance = 600.0 + idx as f64 * 41.0;
        let capacitance_1 = 0.15e-6 + (idx % 5) as f64 * 0.04e-6;
        let capacitance_2 = 0.22e-6 + (idx % 4) as f64 * 0.06e-6;
        let equivalent_capacitance = capacitance_1 + capacitance_2;
        let source_value = 0.8 + (idx % 6) as f64 * 0.11;
        let steps = 8 + idx * 2;
        let target_tau = 0.25 + (idx % 7) as f64 * 0.13;
        let stop_s = resistance * equivalent_capacitance * target_tau;
        let step_s = stop_s / steps as f64;
        let source = format!(
            "general transient derivative rc {idx}\nVin in 0 DC {source_value}\nR1 in out {resistance}\nC1 out 0 {capacitance_1} IC=0\nC2 out 0 {capacitance_2} IC=0\n.tran {step_s:.15} {stop_s:.15} method=be\n.deriv tran V(out) params=R1.R,C1.C,C2.C,Vin.DC\n.end"
        );
        cases.add(
            format!("ams_general_transient_derivative_rc_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let actual_step = match circuit.analyses[0] {
                    Analysis::Transient { step_s, .. } => step_s,
                    _ => return Err("general transient derivative lacks .tran".into()),
                };
                let actual_steps = (transient(&circuit, &circuit.analyses[0])?
                    .samples
                    .len()
                    - 1) as f64;
                let k = actual_step / (resistance * equivalent_capacitance);
                let base = 1.0 + k;
                let decay = base.powf(-actual_steps);
                let nominal_output = source_value * (1.0 - decay);
                let k_r = -k / resistance;
                let k_c = -k / equivalent_capacitance;
                let k_rr = 2.0 * k / (resistance * resistance);
                let k_cc = 2.0 * k / (equivalent_capacitance * equivalent_capacitance);
                let k_rc = k / (resistance * equivalent_capacitance);
                let n = actual_steps;
                let d_r = source_value * n * base.powf(-n - 1.0) * k_r;
                let d_c = source_value * n * base.powf(-n - 1.0) * k_c;
                let expected_gradient = [d_r, d_c, d_c, 1.0 - decay];
                let second_rr = source_value
                    * (-n * (n + 1.0) * base.powf(-n - 2.0) * k_r * k_r
                        + n * base.powf(-n - 1.0) * k_rr);
                let second_cc = source_value
                    * (-n * (n + 1.0) * base.powf(-n - 2.0) * k_c * k_c
                        + n * base.powf(-n - 1.0) * k_cc);
                let second_rc = source_value
                    * (-n * (n + 1.0) * base.powf(-n - 2.0) * k_r * k_c
                        + n * base.powf(-n - 1.0) * k_rc);
                let expected_hessian = [
                    [
                        second_rr,
                        second_rc,
                        second_rc,
                        d_r / source_value,
                    ],
                    [
                        second_rc,
                        second_cc,
                        second_cc,
                        d_c / source_value,
                    ],
                    [
                        second_rc,
                        second_cc,
                        second_cc,
                        d_c / source_value,
                    ],
                    [d_r / source_value, d_c / source_value, d_c / source_value, 0.0],
                ];
                let result =
                    derivative_analysis(&circuit, &circuit.analyses[1])?;
                if result.target != DerivativeTarget::TransientFinal
                    || (result.nominal_output - nominal_output).abs() > 1e-10
                {
                    return Err(format!(
                        "general transient derivative metadata: {:?}, expected {nominal_output}, steps={actual_steps}, k={k}",
                        result
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-9 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-6 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..20 {
        let resistance = 650.0 + idx as f64 * 43.0;
        let capacitance_1 = 0.18e-6 + (idx % 5) as f64 * 0.03e-6;
        let capacitance_2 = 0.24e-6 + (idx % 4) as f64 * 0.05e-6;
        let equivalent_capacitance = capacitance_1 + capacitance_2;
        let source_value = 0.85 + (idx % 6) as f64 * 0.09;
        let steps = 9 + idx * 2;
        let target_tau = 0.3 + (idx % 7) as f64 * 0.11;
        let stop_s = resistance * equivalent_capacitance * target_tau;
        let step_s = stop_s / steps as f64;
        let source = format!(
            "general trapezoidal derivative rc {idx}\nVin in 0 DC {source_value}\nR1 in out {resistance}\nC1 out 0 {capacitance_1} IC=0\nC2 out 0 {capacitance_2} IC=0\n.tran {step_s:.15} {stop_s:.15} method=trap\n.deriv tran V(out) params=R1.R,C1.C,C2.C,Vin.DC\n.end"
        );
        cases.add(
            format!("ams_general_trapezoidal_derivative_rc_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let actual_step = match circuit.analyses[0] {
                    Analysis::Transient { step_s, .. } => step_s,
                    _ => return Err("trapezoidal derivative lacks .tran".into()),
                };
                let actual_steps = transient(&circuit, &circuit.analyses[0])?
                    .samples
                    .len()
                    - 1;
                let (nominal_output, expected_gradient, expected_hessian) =
                    trapezoidal_rc_derivatives(
                        resistance,
                        equivalent_capacitance,
                        actual_step,
                        actual_steps,
                        source_value,
                    );
                let result =
                    derivative_analysis(&circuit, &circuit.analyses[1])?;
                if result.target != DerivativeTarget::TransientFinal
                    || (result.nominal_output - nominal_output).abs() > 1e-10
                {
                    return Err(format!(
                        "trapezoidal derivative metadata: {:?}, expected {nominal_output}, steps={actual_steps}",
                        result
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-9 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-6 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..20 {
        let resistance = 0.5 + idx as f64 * 0.23;
        let saturation_current = 5e-10 * (1.1_f64).powi(idx);
        let dc_value = 0.72 + (idx % 5) as f64 * 0.015;
        let ac_magnitude = 0.55 + (idx % 4) as f64 * 0.08;
        let source = format!(
            "nonlinear AC diode derivative {idx}\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1)\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC\n.end"
        );
        cases.add(
            format!("ams_nonlinear_ac_derivative_diode_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[1])?;
                let (gradient, hessian) = nonlinear_diode_ac_reference(
                    resistance,
                    saturation_current,
                    dc_value,
                    ac_magnitude,
                    0.0,
                    0.0,
                );
                let gradient = gradient[..4].to_vec();
                let hessian = hessian
                    .iter()
                    .map(|row| row[..4].to_vec())
                    .collect::<Vec<_>>();
                if result.target != DerivativeTarget::AcMagnitude {
                    return Err(format!("wrong derivative target: {result:?}"));
                }
                for (got, expected) in result.gradient().into_iter().zip(gradient) {
                    if (got - expected).abs() > 2e-3 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (flat, (got, expected)) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(hessian.iter().flatten())
                    .enumerate()
                {
                    if (got - expected).abs() > 2e-2 * (1.0 + expected.abs()) {
                        return Err(format!("hessian[{flat}/4]={got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..10 {
        let resistance = 0.65 + idx as f64 * 0.17;
        let saturation_current = 7e-10 * (1.08_f64).powi(idx);
        let dc_value = 0.74 + (idx % 4) as f64 * 0.012;
        let ac_magnitude = 0.6 + (idx % 3) as f64 * 0.07;
        let transit_time = 0.002 + idx as f64 * 0.0007;
        let source = format!(
            "nonlinear AC diode TT derivative {idx}\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1 TT={transit_time})\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT\n.end"
        );
        cases.add(
            format!("ams_nonlinear_ac_derivative_diode_tt_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[1])?;
                let (gradient, hessian) = nonlinear_diode_ac_reference(
                    resistance,
                    saturation_current,
                    dc_value,
                    ac_magnitude,
                    transit_time,
                    0.0,
                );
                let gradient = gradient[..5].to_vec();
                let hessian = hessian
                    .iter()
                    .map(|row| row[..5].to_vec())
                    .collect::<Vec<_>>();
                for (got, expected) in result.gradient().into_iter().zip(gradient) {
                    if (got - expected).abs() > 2e-3 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (flat, (got, expected)) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(hessian.iter().flatten())
                    .enumerate()
                {
                    if (got - expected).abs() > 2e-2 * (1.0 + expected.abs()) {
                        return Err(format!("hessian[{flat}/5]={got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..10 {
        let resistance = 0.7 + idx as f64 * 0.13;
        let saturation_current = 8e-10 * (1.07_f64).powi(idx);
        let dc_value = 0.75 + (idx % 4) as f64 * 0.011;
        let ac_magnitude = 0.62 + (idx % 3) as f64 * 0.06;
        let transit_time = 0.003 + (idx % 5) as f64 * 0.0009;
        let junction_capacitance = 5e-12 * (1.15_f64).powi(idx);
        let source = format!(
            "nonlinear AC diode CJO derivative {idx}\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1 TT={transit_time} CJO={junction_capacitance} VJ=700m M=0.5 FC=0.5)\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT,DN.CJO\n.end"
        );
        cases.add(
            format!("ams_nonlinear_ac_derivative_diode_cjo_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[1])?;
                let (gradient, hessian) = nonlinear_diode_ac_reference(
                    resistance,
                    saturation_current,
                    dc_value,
                    ac_magnitude,
                    transit_time,
                    junction_capacitance,
                );
                for (got, expected) in result.gradient().into_iter().zip(gradient) {
                    if (got - expected).abs() > 2e-3 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (flat, (got, expected)) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(hessian.iter().flatten())
                    .enumerate()
                {
                    if flat % 6 == 5 || flat / 6 == 5 {
                        continue;
                    }
                    if (got - expected).abs() > 2e-2 * (1.0 + expected.abs()) {
                        return Err(format!("hessian[{flat}/6]={got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }
    for idx in 0..10 {
        let resistance = 0.75 + idx as f64 * 0.11;
        let saturation_current = 9e-10 * (1.06_f64).powi(idx);
        let dc_value = 0.76 + (idx % 4) as f64 * 0.009;
        let ac_magnitude = 0.64 + (idx % 3) as f64 * 0.05;
        let transit_time = 0.004 + (idx % 5) as f64 * 0.0007;
        let junction_capacitance = 6e-12 * (1.1_f64).powi(idx);
        let junction_potential = 0.65 + (idx % 4) as f64 * 0.035;
        let grading_coefficient = 0.38 + (idx % 5) as f64 * 0.045;
        let forward_knee = 0.42 + (idx % 3) as f64 * 0.035;
        let source = format!(
            "nonlinear AC diode VJ/M/FC derivative {idx}\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1 TT={transit_time} CJO={junction_capacitance} VJ={junction_potential} M={grading_coefficient} FC={forward_knee})\n.temp 26.85\n.ac lin 1 20 20\n.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT,DN.CJO,DN.VJ,DN.M,DN.FC\n.end"
        );
        cases.add(
            format!("ams_nonlinear_ac_derivative_diode_junction_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[1])?;
                let (gradient, hessian) = nonlinear_diode_junction_reference(
                    resistance,
                    saturation_current,
                    dc_value,
                    ac_magnitude,
                    transit_time,
                    junction_capacitance,
                    junction_potential,
                    grading_coefficient,
                    forward_knee,
                );
                for (got, expected) in result.gradient().into_iter().zip(gradient) {
                    if (got - expected).abs() > 2e-3 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (flat, (got, expected)) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(hessian.iter().flatten())
                    .enumerate()
                {
                    // The pure CJO/CJO finite difference is below the scalar
                    // reference noise floor; that entry has a focused analytic
                    // and mixed-Hessian unit regression.
                    if flat == 50 {
                        continue;
                    }
                    if (got - expected).abs() > 2e-2 * (1.0 + expected.abs()) {
                        return Err(format!("hessian[{flat}/9]={got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }
}

fn correlated_matrix_monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 500.0 + idx as f64 * 47.0;
        let r2 = 1_000.0 + idx as f64 * 83.0;
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let tolerance = 0.02 + (idx % 4) as f64 * 0.01;
        let correlations = [-0.95, -0.75, -0.5, 0.0, 0.35, 0.65, 0.85, 0.95];
        let correlation = correlations[idx % correlations.len()];
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let sqrt_3 = 3.0_f64.sqrt();
        let sigma_r1 = r1 * tolerance / sqrt_3;
        let sigma_r2 = r2 * tolerance / sqrt_3;
        let expected_variance = derivative_r1.powi(2) * sigma_r1.powi(2)
            + derivative_r2.powi(2) * sigma_r2.powi(2)
            + 2.0 * correlation * derivative_r1 * derivative_r2 * sigma_r1 * sigma_r2;
        let expected_sigma = expected_variance.sqrt();
        let nominal = supply * r2 / total;
        let source = format!(
            "matrix mc divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 3072 op V(out) seed={} rtol={tolerance} ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 matrix=R1.R|R2.R|{correlation}\n.end",
            1700 + idx
        );
        cases.add(
            format!("ams_correlated_matrix_monte_carlo_divider_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = monte_carlo(&circuit, &circuit.analyses[0])?;
                if result.samples.len() != 3072
                    || result.samples[0].randomized_objects != 2
                    || (result.statistics.standard_deviation - expected_sigma).abs()
                        > expected_sigma * 0.12
                    || (result.statistics.mean - nominal).abs() > expected_sigma * 0.25
                {
                    Err(format!(
                        "rho={correlation}, tol={tolerance}, nominal={nominal}, expected_sigma={expected_sigma}, result={:?}",
                        result.statistics
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }
}

fn spatial_monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let r1 = 500.0 + idx as f64 * 53.0;
        let r2 = 1_000.0 + idx as f64 * 97.0;
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let tolerance = 0.03 + (idx % 3) as f64 * 0.01;
        let distance = 0.5 + (idx % 7) as f64 * 0.5;
        let scale = 1.0 + (idx % 4) as f64 * 0.5;
        let process_correlation = 0.5 + (idx % 5) as f64 * 0.1;
        let correlation =
            process_correlation * (-(distance * distance) / (2.0 * scale * scale)).exp();
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let sqrt_3 = 3.0_f64.sqrt();
        let sigma_r1 = r1 * tolerance / sqrt_3;
        let sigma_r2 = r2 * tolerance / sqrt_3;
        let expected_variance = derivative_r1.powi(2) * sigma_r1.powi(2)
            + derivative_r2.powi(2) * sigma_r2.powi(2)
            + 2.0 * correlation * derivative_r1 * derivative_r2 * sigma_r1 * sigma_r2;
        let expected_sigma = expected_variance.sqrt();
        let nominal = supply * r2 / total;
        let source = format!(
            "spatial mc {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.location R1 0 0\n.location R2 {distance} 0\n.mc 4096 op V(out) seed={} rtol={tolerance} ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr={process_correlation} spatialscale={scale}\n.end",
            2900 + idx
        );
        cases.add(format!("ams_spatial_monte_carlo_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = monte_carlo(&circuit, &circuit.analyses[0])?;
            if result.samples.len() != 4096
                || result.samples[0].randomized_objects != 2
                || (result.statistics.standard_deviation - expected_sigma).abs()
                    > expected_sigma * 0.12
                || (result.statistics.mean - nominal).abs() > expected_sigma * 0.25
            {
                Err(format!(
                    "rho={correlation}, distance={distance}, scale={scale}, expected={expected_sigma}, result={:?}",
                    result.statistics
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn distribution_monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let distribution = if idx % 2 == 0 {
            MonteCarloDistribution::Normal
        } else {
            MonteCarloDistribution::Lognormal
        };
        let r1 = 600.0 + idx as f64 * 71.0;
        let r2 = 1_200.0 + idx as f64 * 113.0;
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let tolerance = 0.03 + (idx % 4) as f64 * 0.01;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let expected_sigma = (derivative_r1.powi(2) * (r1 * tolerance).powi(2)
            + derivative_r2.powi(2) * (r2 * tolerance).powi(2))
        .sqrt();
        let nominal = supply * r2 / total;
        let source = format!(
            "distribution mc {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 4096 op V(out) seed={} rtol={tolerance} ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist={}\n.end",
            3000 + idx,
            distribution.as_str()
        );
        cases.add(format!("ams_distribution_monte_carlo_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = monte_carlo(&circuit, &circuit.analyses[0])?;
            if result.samples.len() != 4096
                || result.samples[0].randomized_objects != 2
                || (result.statistics.standard_deviation - expected_sigma).abs()
                    > expected_sigma * 0.08
                || (result.statistics.mean - nominal).abs() > expected_sigma * 0.3
            {
                Err(format!(
                    "distribution={:?}, expected_sigma={expected_sigma}, nominal={nominal}, result={:?}",
                    distribution,
                    result.statistics
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn covariance_monte_carlo_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let r1 = 600.0 + idx as f64 * 71.0;
        let r2 = 1_200.0 + idx as f64 * 113.0;
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let sigma_r1 = r1 * (0.02 + (idx % 4) as f64 * 0.01);
        let sigma_r2 = r2 * (0.03 + (idx % 3) as f64 * 0.01);
        let correlation = [-0.7, -0.4, 0.0, 0.35, 0.65][idx % 5];
        let covariance = correlation * sigma_r1 * sigma_r2;
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let expected_variance = derivative_r1.powi(2) * sigma_r1.powi(2)
            + derivative_r2.powi(2) * sigma_r2.powi(2)
            + 2.0 * derivative_r1 * derivative_r2 * covariance;
        let expected_sigma = expected_variance.sqrt();
        let nominal = supply * r2 / total;
        let source = format!(
            "covariance mc {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 4096 op V(out) seed={} rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=normal cov=R1.R:{},R2.R:{},R1.R|R2.R:{covariance}\n.end",
            3100 + idx,
            sigma_r1 * sigma_r1,
            sigma_r2 * sigma_r2
        );
        cases.add(format!("ams_covariance_monte_carlo_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = monte_carlo(&circuit, &circuit.analyses[0])?;
            if result.samples.len() != 4096
                || result.samples[0].randomized_objects != 2
                || (result.statistics.standard_deviation - expected_sigma).abs()
                    > expected_sigma * 0.08
                || (result.statistics.mean - nominal).abs() > expected_sigma * 0.25
            {
                Err(format!(
                    "rho={correlation}, expected_sigma={expected_sigma}, nominal={nominal}, result={:?}",
                    result.statistics
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn vendor_statistical_model_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let r1 = 600.0 + idx as f64 * 71.0;
        let r2 = 1_200.0 + idx as f64 * 113.0;
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let distribution = if idx % 2 == 0 {
            MonteCarloDistribution::Normal
        } else {
            MonteCarloDistribution::Lognormal
        };
        let relative_sigma_r1 = 0.02 + (idx % 4) as f64 * 0.005;
        let relative_sigma_r2 = 0.025 + (idx % 3) as f64 * 0.005;
        let correlation = [-0.7, -0.4, 0.0, 0.35, 0.65][idx % 5];
        let total = r1 + r2;
        let derivative_r1 = -supply * r2 / (total * total);
        let derivative_r2 = supply * r1 / (total * total);
        let delta_sigma_r1 = match distribution {
            MonteCarloDistribution::Normal => r1 * relative_sigma_r1,
            MonteCarloDistribution::Lognormal => r1 * (relative_sigma_r1.powi(2).exp_m1()).sqrt(),
            MonteCarloDistribution::Uniform => unreachable!(),
        };
        let delta_sigma_r2 = match distribution {
            MonteCarloDistribution::Normal => r2 * relative_sigma_r2,
            MonteCarloDistribution::Lognormal => r2 * (relative_sigma_r2.powi(2).exp_m1()).sqrt(),
            MonteCarloDistribution::Uniform => unreachable!(),
        };
        let delta_covariance = match distribution {
            MonteCarloDistribution::Normal => {
                correlation * r1 * relative_sigma_r1 * r2 * relative_sigma_r2
            }
            MonteCarloDistribution::Lognormal => {
                r1 * r2 * (correlation * relative_sigma_r1 * relative_sigma_r2).exp_m1()
            }
            MonteCarloDistribution::Uniform => unreachable!(),
        };
        let expected_variance = derivative_r1.powi(2) * delta_sigma_r1.powi(2)
            + derivative_r2.powi(2) * delta_sigma_r2.powi(2)
            + 2.0 * derivative_r1 * derivative_r2 * delta_covariance;
        let expected_sigma = expected_variance.sqrt();
        let model_source = format!(
            "model vendor_{idx}\nparam R1.R dist={} relative=true sigma={relative_sigma_r1}\nparam R2.R dist={} relative=true sigma={relative_sigma_r2}\ncorr R1.R R2.R {correlation}\n",
            distribution.as_str(),
            distribution.as_str()
        );
        let circuit_source = format!(
            "vendor model mc {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.mc 3072 op V(out) seed={} rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0\n.end",
            3300 + idx
        );
        cases.add(
            format!("ams_vendor_statistical_model_monte_carlo_{idx}"),
            move || {
                let model = parse_statistical_model(&model_source)?;
                let mut circuit = parse_spice(&circuit_source)?;
                circuit
                    .statistical_models
                    .insert(model.name.clone(), model);
                for analysis in &mut circuit.analyses {
                    if let Analysis::MonteCarlo(options) = analysis {
                        options.statistical_model = Some(format!("vendor_{idx}"));
                    }
                }
                let result = monte_carlo(&circuit, &circuit.analyses[0])?;
                if result.samples.len() != 3072
                    || result.samples[0].randomized_objects != 2
                    || (result.statistics.standard_deviation - expected_sigma).abs()
                        > expected_sigma * 0.12
                {
                    Err(format!(
                        "distribution={:?}, rho={correlation}, expected_sigma={expected_sigma}, result={:?}",
                        distribution,
                        result.statistics
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }
}

fn aging_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        if idx % 2 == 0 {
            let current = 0.5e-3 + (idx % 7) as f64 * 0.1e-3;
            let resistance = 1_000.0 + idx as f64 * 137.0;
            let years = 1.0 + (idx % 9) as f64;
            let exponent = 0.25 + (idx % 5) as f64 * 0.25;
            let increase = 0.01 + (idx % 6) as f64 * 0.02;
            let stress = years.powf(exponent);
            let nominal = -current * resistance;
            let aged = -current * resistance * (1.0 + increase * stress);
            let source = format!(
                "aging resistor {idx}\nIin out 0 {current}\nR1 out 0 {resistance}\n.aging V(out) years={years} exp={exponent} rinc={increase}\n.end"
            );
            cases.add(format!("ams_aging_resistor_{idx}"), move || {
                let circuit = parse_spice(&source)?;
                let result = aging_analysis(&circuit, &circuit.analyses[0])?;
                if (result.stress - stress).abs() > 1e-9
                    || (result.nominal_output - nominal).abs() > 1e-7
                    || (result.aged_output - aged).abs() > 1e-7
                    || (result.output_delta() - (aged - nominal)).abs() > 1e-7
                {
                    Err(format!(
                        "years={years}, exponent={exponent}, expected nominal={nominal}, aged={aged}, result={result:?}"
                    ))
                } else {
                    Ok(())
                }
            });
        } else {
            let years = 1.0 + (idx % 8) as f64;
            let exponent = 0.25 + (idx % 4) as f64 * 0.25;
            let threshold_shift = 0.001 + (idx % 5) as f64 * 0.002;
            let kp_loss = 0.01 + (idx % 4) as f64 * 0.02;
            let stress = years.powf(exponent);
            let expected_vto = 0.45 + threshold_shift * stress;
            let expected_kp = 0.001 * (1.0 - kp_loss * stress);
            let source = format!(
                "aging mos {idx}\nVd d 0 1.8\nVg g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.45)\n.aging V(d) years={years} exp={exponent} vtshift={threshold_shift} kploss={kp_loss}\n.end"
            );
            cases.add(format!("ams_aging_mos_model_{idx}"), move || {
                let circuit = parse_spice(&source)?;
                let result = aging_analysis(&circuit, &circuit.analyses[0])?;
                let model = result
                    .circuit
                    .mosfet_models
                    .get("NM")
                    .ok_or("missing aged MOS model")?;
                if (result.stress - stress).abs() > 1e-9
                    || (model.threshold_voltage_v - expected_vto).abs() > 1e-9
                    || (model.transconductance_a_per_v2 - expected_kp).abs() > 1e-12
                    || !result.aged_output.is_finite()
                {
                    Err(format!(
                        "years={years}, exponent={exponent}, expected VTO={expected_vto}, KP={expected_kp}, result={result:?}"
                    ))
                } else {
                    Ok(())
                }
            });
        }
    }
}

fn junction_capacitance_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let reverse_voltage = 1.0 + idx as f64 * 0.25;
        let cjo = 60e-12 + (idx % 7) as f64 * 10e-12;
        let vj = 0.6 + (idx % 4) as f64 * 0.05;
        let grading = 0.33 + (idx % 5) as f64 * 0.05;
        let capacitance = cjo / (1.0 + reverse_voltage / vj).powf(grading);
        let expected =
            1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * 10_000.0 * capacitance).powi(2)).sqrt();
        let source = format!(
            "diode cv {idx}\nVin in 0 DC -{reverse_voltage} AC 1\nR1 in out 10k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 CJO={cjo} VJ={vj} M={grading} FC=0.5)\n.ac lin 2 1 1g\n.end"
        );
        cases.add(format!("ams_diode_junction_cv_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = sweep_ac(&circuit, &circuit.analyses[0])?;
            let gains = result.node_magnitude("out")?;
            if gains.len() != 2 || gains[0] < 0.999999 || (gains[1] - expected).abs() > 1e-4 {
                Err(format!(
                    "gains={gains:?}, expected high-frequency gain={expected}"
                ))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let collector_voltage = 1.0 + idx as f64 * 0.1;
        let cje = 30e-12 + (idx % 6) as f64 * 5e-12;
        let cjc = 10e-12 + (idx % 5) as f64 * 3e-12;
        let vjc = 0.55 + (idx % 4) as f64 * 0.05;
        let mjc = 0.4 + (idx % 3) as f64 * 0.05;
        let source = format!(
            "bjt cv {idx}\nVin in 0 DC 0 AC 1\nRb in b 10k\nVdd vdd 0 {collector_voltage}\nRc vdd c 1u\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 CJE={cje} CJC={cjc} VJE=0.7 VJC={vjc} MJE=0.33 MJC={mjc} FC=0.5)\n.ac lin 2 1 1g\n.end"
        );
        cases.add(format!("ams_bjt_junction_cv_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let op = dc_operating_point(&circuit)?;
            let vbc = op.node_voltages["b"] - op.node_voltages["c"];
            let cbc = cjc / (1.0 - vbc / vjc).powf(mjc);
            let expected =
                1.0 / (1.0 + (std::f64::consts::TAU * 1e9 * 10_000.0 * (cje + cbc)).powi(2)).sqrt();
            let result = sweep_ac(&circuit, &circuit.analyses[0])?;
            let gains = result.node_magnitude("b")?;
            if gains.len() != 2 || gains[0] < 0.999999 || (gains[1] - expected).abs() > 1e-4 {
                Err(format!(
                    "gains={gains:?}, expected high-frequency gain={expected}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn mos_charge_partition_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let width = 600e-9 + (idx % 6) as f64 * 100e-9;
        let length = 160e-9 + (idx % 4) as f64 * 30e-9;
        let cox = 0.5 + (idx % 7) as f64 * 0.15;
        let threshold = 0.25 + (idx % 5) as f64 * 0.05;
        let gate = threshold + 0.45;
        let make = |partition: f64| {
            format!(
                "mos partition {idx} {partition}\nVin in 0 DC {gate} AC 1\nRg in g 10k\nVdd vdd 0 1.8\nRd vdd d 10k\nM1 d g 0 0 NM W={width} L={length}\n.model NM NMOS(VTO={threshold} COX={cox} XPART={partition})\n.ac lin 2 1 1g\n.end"
            )
        };
        let source_side_source = make(0.0);
        let drain_side_source = make(1.0);
        cases.add(format!("ams_mos_charge_partition_{idx}"), move || {
            let source_side = parse_spice(&source_side_source)?;
            let drain_side = parse_spice(&drain_side_source)?;
            let source_gain =
                sweep_ac(&source_side, &source_side.analyses[0])?.node_magnitude("d")?;
            let drain_gain = sweep_ac(&drain_side, &drain_side.analyses[0])?.node_magnitude("d")?;
            if source_gain.len() != 2
                || drain_gain.len() != 2
                || (drain_gain[1] - source_gain[1]).abs() <= 1e-6
            {
                Err(format!(
                    "source-side={source_gain:?}, drain-side={drain_gain:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn flicker_noise_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let kp = 0.8e-3 + (idx % 6) as f64 * 0.1e-3;
        let width = 700e-9 + (idx % 5) as f64 * 100e-9;
        let flicker = 1e-7 + (idx % 7) as f64 * 1e-7;
        let current_exponent = 0.9 + (idx % 4) as f64 * 0.1;
        let source = format!(
            "mos flicker {idx}\nVdd vdd 0 1.8\nRd vdd d 1k\nVin g 0 DC 1 AC 1\nM1 d g 0 0 NM W={width} L=180n\n.model NM NMOS(KP={kp} VTO=0.5 KF={flicker} AF={current_exponent} EF=1)\n.noise V(d) Vin lin 2 1k 1meg\n.end"
        );
        cases.add(format!("ams_mos_flicker_noise_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = noise_sweep(&circuit, &circuit.analyses[0])?;
            let low = result.points[0]
                .contributions
                .get("M1")
                .copied()
                .ok_or_else(|| "missing MOS flicker contribution".to_string())?;
            let high = result.points[1]
                .contributions
                .get("M1")
                .copied()
                .ok_or_else(|| "missing MOS flicker contribution".to_string())?;
            let ratio = low / high;
            let expected = 1000f64.sqrt();
            if !ratio.is_finite() || (ratio - expected).abs() > 2.0 {
                Err(format!("ratio={ratio}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let resistance = 750.0 + idx as f64 * 37.0;
        let flicker = 1e-13 + (idx % 8) as f64 * 1e-13;
        let current_exponent = 0.9 + (idx % 5) as f64 * 0.1;
        let source = format!(
            "diode flicker {idx}\nV1 in 0 DC 1.8 AC 1\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 KF={flicker} AF={current_exponent} EF=1)\n.noise V(out) V1 lin 2 1 1k\n.end"
        );
        cases.add(format!("ams_diode_flicker_noise_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = noise_sweep(&circuit, &circuit.analyses[0])?;
            let low = result.points[0]
                .contributions
                .get("D1")
                .copied()
                .ok_or_else(|| "missing diode flicker contribution".to_string())?;
            let high = result.points[1]
                .contributions
                .get("D1")
                .copied()
                .ok_or_else(|| "missing diode flicker contribution".to_string())?;
            let ratio = low / high;
            let expected = 1000f64.sqrt();
            if !ratio.is_finite() || (ratio - expected).abs() > 2.0 {
                Err(format!("ratio={ratio}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn temperature_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let celsius = -40.0 + idx as f64 * 4.0;
        let temperature_k = celsius + 273.15;
        let resistance = 500.0 + idx as f64 * 91.0;
        let expected = (4.0 * 1.380_649e-23 * temperature_k / resistance).sqrt() * resistance;
        let source = format!(
            "resistor temperature {idx}\n.temp {celsius}\nIin 0 out DC 0 AC 1\nR1 out 0 {resistance}\n.noise V(out) Iin lin 1 1 1\n.end"
        );
        cases.add(format!("ams_resistor_temperature_noise_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let got = noise_sweep(&circuit, &circuit.analyses[0])?.output_noise()[0];
            if (got - expected).abs() > 1e-14 {
                Err(format!("noise={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let celsius = -25.0 + idx as f64 * 3.0;
        let temperature_k = celsius + 273.15;
        let kp = 0.7e-3 + (idx % 6) as f64 * 0.1e-3;
        let width = 650e-9 + (idx % 5) as f64 * 100e-9;
        let length = 170e-9 + (idx % 4) as f64 * 20e-9;
        let mobility_exponent = 1.2 + (idx % 4) as f64 * 0.2;
        let effective_kp = kp * (300.0_f64 / temperature_k).powf(mobility_exponent);
        let current = 0.5 * effective_kp * (width / length) * 0.16;
        let expected = 1.8 - current * 1_000.0;
        let source = format!(
            "mos temperature {idx}\n.temp {celsius}\nVdd vdd 0 1.8\nRd vdd d 1k\nVg g 0 0.9\nM1 d g 0 0 NM W={width} L={length}\n.model NM NMOS(KP={kp} VTO=0.5 MEXP={mobility_exponent})\n.op\n.end"
        );
        cases.add(format!("ams_mos_temperature_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing MOS drain".to_string())?;
            if (got - expected).abs() > 1e-8 {
                Err(format!("V(d)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn transit_charge_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let transit_time = 200e-12 + (idx % 8) as f64 * 100e-12;
        let make = |time: f64| {
            format!(
                "diode transit {idx} {time}\nV1 in 0 DC 0.7 AC 1\nR1 in out 10k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1 TT={time})\n.ac lin 2 1 1g\n.end"
            )
        };
        let without_source = make(0.0);
        let with_source = make(transit_time);
        cases.add(format!("ams_diode_transit_charge_{idx}"), move || {
            let without = parse_spice(&without_source)?;
            let with = parse_spice(&with_source)?;
            let without_gain = sweep_ac(&without, &without.analyses[0])?.node_magnitude("out")?;
            let with_gain = sweep_ac(&with, &with.analyses[0])?.node_magnitude("out")?;
            if with_gain.len() != 2 || with_gain[1] >= without_gain[1] {
                Err(format!(
                    "without={without_gain:?}, with={with_gain:?}, TT={transit_time}"
                ))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let transit_time = 1e-9 + (idx % 9) as f64 * 500e-12;
        let make = |time: f64| {
            format!(
                "bjt transit {idx} {time}\nVin in 0 DC 0 AC 1\nRb in b 10k\nVdd vdd 0 3\nRc vdd c 1u\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-14 BF=100 TF={time})\n.ac lin 2 1 1g\n.end"
            )
        };
        let without_source = make(0.0);
        let with_source = make(transit_time);
        cases.add(format!("ams_bjt_transit_charge_{idx}"), move || {
            let without = parse_spice(&without_source)?;
            let with = parse_spice(&with_source)?;
            let without_gain = sweep_ac(&without, &without.analyses[0])?.node_magnitude("b")?;
            let with_gain = sweep_ac(&with, &with.analyses[0])?.node_magnitude("b")?;
            if with_gain.len() != 2 || with_gain[1] >= without_gain[1] {
                Err(format!(
                    "without={without_gain:?}, with={with_gain:?}, TF={transit_time}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn sensitivity_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 400.0 + idx as f64 * 37.0;
        let r2 = 800.0 + idx as f64 * 53.0;
        let supply = 1.2 + (idx % 7) as f64 * 0.1;
        let expected_r1 = -supply * r2 / (r1 + r2).powi(2);
        let expected_r2 = supply * r1 / (r1 + r2).powi(2);
        let source = format!(
            "sensitivity divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.sens V(out)\n.end"
        );
        cases.add(format!("ams_resistor_sensitivity_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = sensitivity(&circuit, &circuit.analyses[0])?;
            let got_r1 = result
                .find("R1.R")
                .ok_or_else(|| "missing R1 sensitivity".to_string())?
                .absolute_sensitivity;
            let got_r2 = result
                .find("R2.R")
                .ok_or_else(|| "missing R2 sensitivity".to_string())?
                .absolute_sensitivity;
            if (got_r1 - expected_r1).abs() > 1e-8 || (got_r2 - expected_r2).abs() > 1e-8 {
                Err(format!(
                    "dV/dR1={got_r1}/{expected_r1}, dV/dR2={got_r2}/{expected_r2}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn analytic_derivative_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        if idx % 2 == 0 {
            let r1 = 500.0 + idx as f64 * 73.0;
            let r2 = 1_000.0 + idx as f64 * 97.0;
            let supply = 1.0 + (idx % 6) as f64 * 0.1;
            let sum = r1 + r2;
            let expected_gradient = [
                -supply * r2 / sum.powi(2),
                supply * r1 / sum.powi(2),
                r2 / sum,
            ];
            let expected_hessian = [
                [
                    2.0 * supply * r2 / sum.powi(3),
                    supply * (r2 - r1) / sum.powi(3),
                    -r2 / sum.powi(2),
                ],
                [
                    supply * (r2 - r1) / sum.powi(3),
                    -2.0 * supply * r1 / sum.powi(3),
                    r1 / sum.powi(2),
                ],
                [-r2 / sum.powi(2), r1 / sum.powi(2), 0.0],
            ];
            let source = format!(
                "analytic derivative divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.deriv V(out) params=R1.R,R2.R,Vin.DC\n.end"
            );
            cases.add(
                format!("ams_analytic_derivative_divider_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                    let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
                    for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                        if (got - expected).abs() > 1e-10 {
                            return Err(format!("gradient {got}/{expected}"));
                        }
                    }
                    for (got, expected) in result
                        .hessian()
                        .into_iter()
                        .flatten()
                        .zip(expected_hessian.iter().flatten())
                    {
                        if (got - expected).abs() > 1e-10 {
                            return Err(format!("hessian {got}/{expected}"));
                        }
                    }
                    Ok(())
                },
            );
        } else {
            let current = 0.5e-3 + (idx % 7) as f64 * 0.1e-3;
            let resistance = 1_000.0 + idx as f64 * 113.0;
            let expected_gradient = [-current, -resistance];
            let expected_hessian = [[0.0, -1.0], [-1.0, 0.0]];
            let source = format!(
                "analytic derivative current {idx}\nIin out 0 {current}\nR1 out 0 {resistance}\n.deriv V(out) params=R1.R,Iin.DC\n.end"
            );
            cases.add(
                format!("ams_analytic_derivative_current_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
                    for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                        if (got - expected).abs() > 1e-8 {
                            return Err(format!("gradient {got}/{expected}"));
                        }
                    }
                    for (got, expected) in result
                        .hessian()
                        .into_iter()
                        .flatten()
                        .zip(expected_hessian.iter().flatten())
                    {
                        if (got - expected).abs() > 1e-8 {
                            return Err(format!("hessian {got}/{expected}"));
                        }
                    }
                    Ok(())
                },
            );
        }
    }

    for idx in 0..20 {
        let resistance = 500.0 + idx as f64 * 61.0;
        let capacitance = 0.5e-6 + idx as f64 * 0.07e-6;
        let frequency = 500.0 + idx as f64 * 317.0;
        let source_magnitude = 0.8 + (idx % 5) as f64 * 0.15;
        let omega = std::f64::consts::TAU * frequency;
        let reactance_scale = omega * resistance * capacitance;
        let root3 = (1.0 + reactance_scale * reactance_scale).powf(1.5);
        let root5 = (1.0 + reactance_scale * reactance_scale).powf(2.5);
        let magnitude = source_magnitude / (1.0 + reactance_scale * reactance_scale).sqrt();
        let r_scale = omega * capacitance;
        let c_scale = omega * resistance;
        let d_r = -source_magnitude * r_scale * reactance_scale / root3;
        let d_c = -source_magnitude * c_scale * reactance_scale / root3;
        let d2_r =
            source_magnitude * r_scale * r_scale * (-1.0 + 2.0 * reactance_scale * reactance_scale)
                / root5;
        let d2_c =
            source_magnitude * c_scale * c_scale * (-1.0 + 2.0 * reactance_scale * reactance_scale)
                / root5;
        let d2_rc =
            source_magnitude * r_scale * c_scale * (reactance_scale * reactance_scale - 2.0)
                / root5;
        let unit_source_gradient = magnitude / source_magnitude;
        let expected_gradient = [d_r, d_c, unit_source_gradient];
        let expected_hessian = [
            [d2_r, d2_rc, d_r / source_magnitude],
            [d2_rc, d2_c, d_c / source_magnitude],
            [d_r / source_magnitude, d_c / source_magnitude, 0.0],
        ];
        let source = format!(
            "analytic AC derivative lowpass {idx}\nVin in 0 DC 0 AC {source_magnitude}\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.deriv ac V(out) freq={frequency} params=R1.R,C1.C,Vin.AC\n.ac lin 2 {frequency} {frequency}\n.end"
        );
        cases.add(
            format!("ams_analytic_derivative_ac_lowpass_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
                if result.target != DerivativeTarget::AcMagnitude
                    || result.frequency_hz != frequency
                    || (result.nominal_output - magnitude).abs() > 1e-10
                {
                    return Err(format!(
                        "AC derivative metadata: {:?}, expected magnitude {magnitude}",
                        result
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-9 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-8 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }
    for idx in 0..20 {
        let resistance = 3_000.0 + idx as f64 * 137.0;
        let threshold = 0.32 + (idx % 6) as f64 * 0.035;
        let model_kp = 90e-6 + (idx % 5) as f64 * 15e-6;
        let supply = 1.6 + (idx % 4) as f64 * 0.08;
        let width = 1.6e-6 + (idx % 4) as f64 * 0.2e-6;
        let length = 160e-9 + (idx % 3) as f64 * 20e-9;
        let geometry = width / length;
        let transconductance = model_kp * geometry;
        let quadratic = transconductance / 2.0;
        let linear = 1.0 / resistance - 2.0 * quadratic * threshold;
        let constant = quadratic * threshold * threshold - supply / resistance;
        let discriminant = linear * linear - 4.0 * quadratic * constant;
        let voltage = (-linear + discriminant.sqrt()) / (2.0 * quadratic);
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
        let source = format!(
            "MOS analytic derivative {idx}\nVin vdd 0 DC {supply}\nR1 vdd out {resistance}\nM1 out out 0 0 NM W={width} L={length}\n.model NM NMOS(KP={model_kp} VTO={threshold})\n.deriv V(out) params=R1.R,NM.VTO,NM.KP,Vin.DC\n.end"
        );
        cases.add(
            format!("ams_analytic_derivative_mos_diode_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
                if (result.nominal_output - voltage).abs() > 1e-8 {
                    return Err(format!(
                        "nominal={:?}, expected {voltage}, result={result:?}",
                        result.nominal_output
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-8 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-6 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }

    for idx in 0..20 {
        let resistance = 2_200.0 + idx as f64 * 263.0;
        let supply = 1.55 + (idx % 5) as f64 * 0.09;
        let saturation_current = 0.65e-12 + (idx % 4) as f64 * 0.12e-12;
        let forward_beta = 65.0 + (idx % 6) as f64 * 19.0;
        let reverse_beta = 1.0 + (idx % 3) as f64;
        let area = 0.85 + (idx % 5) as f64 * 0.09;
        let thermal_voltage = 1.380_649e-23 * 300.0 / 1.602_176_634e-19;
        let admittance = 1.0 + 1.0 / forward_beta;
        let equation = |voltage: f64| {
            (supply - voltage) / resistance
                - saturation_current * area * admittance * ((voltage / thermal_voltage).exp() - 1.0)
        };
        let mut low = 0.0_f64;
        let mut high = supply;
        for _ in 0..200 {
            let middle = (low + high) / 2.0;
            if equation(middle) >= 0.0 {
                low = middle;
            } else {
                high = middle;
            }
        }
        let voltage = (low + high) / 2.0;
        let exponent = (voltage / thermal_voltage).exp();
        let normalized_current = exponent - 1.0;
        let current_first = normalized_current / thermal_voltage;
        let current_second = exponent / (thermal_voltage * thermal_voltage);
        let f_voltage = -1.0 / resistance - saturation_current * area * admittance * current_first;
        let beta_derivative = -1.0 / (forward_beta * forward_beta);
        let first_rhs = [
            -(supply - voltage) / (resistance * resistance),
            -area * admittance * normalized_current,
            -saturation_current * area * beta_derivative * normalized_current,
            1.0 / resistance,
            -saturation_current * admittance * normalized_current,
        ];
        let expected_gradient = first_rhs.map(|value| -value / f_voltage);
        let voltage_voltage_derivative = [
            1.0 / (resistance * resistance),
            -area * admittance * current_first,
            -saturation_current * area * beta_derivative * current_first,
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
        let source = format!(
            "BJT analytic derivative {idx}\nVin vdd 0 DC {supply}\nR1 vdd out {resistance}\nQ1 out out 0 NPN AREA={area}\n.model NPN NPN(IS={saturation_current} BF={forward_beta} BR={reverse_beta})\n.deriv V(out) params=R1.R,NPN.IS,NPN.BF,Vin.DC,Q1.AREA\n.op\n.end"
        );
        cases.add(
            format!("ams_analytic_derivative_bjt_diode_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                if idx == 0 {
                    assert_eq!(
                        circuit,
                        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                    );
                }
                let result = derivative_analysis(&circuit, &circuit.analyses[0])?;
                if (result.nominal_output - voltage).abs() > 1e-8 {
                    return Err(format!(
                        "nominal={:?}, expected {voltage}, result={result:?}",
                        result.nominal_output
                    ));
                }
                for (got, expected) in result.gradient().into_iter().zip(expected_gradient) {
                    if (got - expected).abs() > 1e-8 * (1.0 + expected.abs()) {
                        return Err(format!("gradient {got}/{expected}"));
                    }
                }
                for (got, expected) in result
                    .hessian()
                    .into_iter()
                    .flatten()
                    .zip(expected_hessian.iter().flatten())
                {
                    if (got - expected).abs() > 1e-6 * (1.0 + expected.abs()) {
                        return Err(format!("hessian {got}/{expected}"));
                    }
                }
                Ok(())
            },
        );
    }
}

fn global_sensitivity_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 500.0 + idx as f64 * 43.0;
        let r2 = 1_000.0 + idx as f64 * 71.0;
        let supply = 1.2 + (idx % 6) as f64 * 0.1;
        let source = format!(
            "sobol divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.sobol V(out) samples=256 seed={} tol=0.2\n.end",
            100 + idx
        );
        cases.add(format!("ams_sobol_divider_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = global_sensitivity(&circuit, &circuit.analyses[0])?;
            let first_sum: f64 = result
                .entries
                .iter()
                .map(|entry| entry.first_order_index)
                .sum();
            let total_sum: f64 = result
                .entries
                .iter()
                .map(|entry| entry.total_order_index)
                .sum();
            if result.output_variance <= 0.0
                || first_sum <= 0.8
                || first_sum >= 1.5
                || result.find("Vin.DC").unwrap().total_order_index <= 0.5
                || result.find("Vin.DC").unwrap().first_order_index <= 0.5
                || result.find("R1.R").unwrap().total_order_index <= 0.0
                || result.find("R2.R").unwrap().total_order_index <= 0.0
            {
                Err(format!(
                    "variance={:?}, first_sum={first_sum}, total_sum={total_sum}, entries={:?}",
                    result.output_variance, result.entries
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn correlated_global_sensitivity_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let coefficient = if idx % 2 == 0 { 1.0 } else { -1.0 };
        let r1 = 500.0 + idx as f64 * 37.0;
        let r2 = 1_000.0 + idx as f64 * 53.0;
        let supply = 1.2 + (idx % 6) as f64 * 0.1;
        let source = format!(
            "correlated sobol {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {r2}\n.sobol V(out) samples=64 seed={} tol=0.2 corr=R1.R|R2.R|{coefficient}\n.end",
            700 + idx
        );
        cases.add(format!("ams_correlated_sobol_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = global_sensitivity(&circuit, &circuit.analyses[0])?;
            let r1 = result.find("R1.R").unwrap();
            let r2 = result.find("R2.R").unwrap();
            let source = result.find("Vin.DC").unwrap();
            let valid_perfect = coefficient == 1.0
                && r1.total_order_index.abs() < 1e-7
                && r2.total_order_index.abs() < 1e-7
                && (source.total_order_index - 1.0).abs() < 1e-7;
            let valid_anti = coefficient == -1.0
                && r1.total_order_index > r1.first_order_index
                && r2.total_order_index > r2.first_order_index
                && source.total_order_index > 0.5;
            if !(valid_perfect || valid_anti) {
                Err(format!(
                    "coefficient={coefficient}, entries={:?}",
                    result.entries
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn resistor_temperature_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let celsius = -40.0 + idx as f64 * 3.0;
        let temperature = celsius + 273.15;
        let delta = temperature - 300.0;
        let r1_nominal = 700.0 + idx as f64 * 41.0;
        let r2_nominal = 1_400.0 + idx as f64 * 67.0;
        let r1 = r1_nominal * (1.0 + 0.001_2 * delta);
        let r2 = r2_nominal * (1.0 - 0.001_8 * delta);
        let expected = 1.8 * r2 / (r1 + r2);
        let source = format!(
            "resistor tc1 {idx}\n.temp {celsius}\nVin in 0 1.8\nR1 in out {r1_nominal} TC1=0.0012\nR2 out 0 {r2_nominal} TC1=-0.0018\n.op\n.end"
        );
        cases.add(format!("ams_resistor_tc1_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing resistor divider output".to_string())?;
            if (got - expected).abs() > 1e-10 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }

    for idx in 0..50 {
        let celsius = -50.0 + idx as f64 * 3.5;
        let temperature = celsius + 273.15;
        let delta = temperature - 300.0;
        let r1_nominal = 800.0 + idx as f64 * 37.0;
        let r2_nominal = 1_600.0 + idx as f64 * 53.0;
        let r1 = r1_nominal * (1.0 + 0.000_015 * delta * delta);
        let r2 = r2_nominal * (1.0 + 0.000_022 * delta * delta);
        let expected = 1.8 * r2 / (r1 + r2);
        let source = format!(
            "resistor tc2 {idx}\n.temp {celsius}\nVin in 0 1.8\nR1 in out {r1_nominal} TC2=0.000015\nR2 out 0 {r2_nominal} TC2=0.000022\n.op\n.end"
        );
        cases.add(format!("ams_resistor_tc2_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let got = dc_operating_point(&circuit)?
                .node_voltages
                .get("out")
                .copied()
                .ok_or_else(|| "missing resistor divider output".to_string())?;
            if (got - expected).abs() > 1e-10 {
                Err(format!("V(out)={got}, expected={expected}"))
            } else {
                Ok(())
            }
        });
    }
}

fn optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 500.0 + idx as f64 * 47.0;
        let target = 0.6 + (idx % 6) as f64 * 0.1;
        let supply = 1.6 + (idx % 5) as f64 * 0.1;
        let expected_r2 = r1 * target / (supply - target);
        let lower = (expected_r2 * 0.2).max(10.0);
        let upper = expected_r2 * 5.0;
        let source = format!(
            "optim divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {lower}\n.optim match V(out) goal={target} params=R2.R:{lower}:{upper} population=8 iterations=128 seed={} tol=1e-7\n.end",
            300 + idx
        );
        cases.add(format!("ams_optimization_divider_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = optimize(&circuit, &circuit.analyses[0])?;
            let final_r2 = result
                .parameters
                .iter()
                .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
                .ok_or_else(|| "missing optimized R2".to_string())?
                .final_value;
            if !result.converged
                || (result.final_output - target).abs() > 1e-6
                || (final_r2 - expected_r2).abs() > expected_r2 * 0.01
            {
                Err(format!(
                    "target={target}, expected_r2={expected_r2}, result={result:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn quantized_optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        if idx % 2 == 0 {
            let quanta = [10.0, 22.0, 47.0, 100.0, 220.0];
            let quantum = quanta[idx / 2 % quanta.len()];
            let r1 = 500.0 + idx as f64 * 47.0;
            let supply = 1.6 + (idx % 5) as f64 * 0.1;
            let desired = quantum * (40.0 + idx as f64 * 5.0);
            let lower = quantum * 8.0;
            let upper = desired + quantum * (12.0 + (idx % 9) as f64);
            let target = supply * desired / (r1 + desired);
            let source = format!(
                "quant divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {lower}\n.optim match V(out) goal={target} params=R2.R:{lower}:{upper}:{lower}:{quantum} population=8 iterations=32 seed={} tol=1e-9\n.end",
                700 + idx
            );
            cases.add(
                format!("ams_quantized_optimization_divider_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    let result = optimize(&circuit, &circuit.analyses[0])?;
                    let parameter = result
                        .parameters
                        .iter()
                        .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
                        .ok_or_else(|| "missing optimized R2".to_string())?;
                    let residual = (parameter.final_value - lower)
                        - ((parameter.final_value - lower) / quantum).round() * quantum;
                    if !result.converged
                        || (result.final_output - target).abs() > 1e-8
                        || (parameter.final_value - desired).abs() > 1e-7
                        || residual.abs() > 1e-7
                        || parameter.final_value < lower
                        || parameter.final_value > upper
                    {
                        Err(format!(
                        "quantum={quantum}, desired={desired}, target={target}, result={result:?}"
                    ))
                    } else {
                        Ok(())
                    }
                },
            );
        } else {
            let quantum = 1e-12 * (10.0 + (idx % 5) as f64 * 5.0);
            let resistance = 1_000.0 + (idx % 7) as f64 * 500.0;
            let frequency = 100_000.0 + (idx % 11) as f64 * 50_000.0;
            let desired = quantum * (80.0 + idx as f64 * 6.0);
            let lower = quantum * 10.0;
            let upper = desired + quantum * (12.0 + (idx % 8) as f64);
            let omega = std::f64::consts::TAU * frequency;
            let target = 1.0 / (1.0 + (omega * resistance * desired).powi(2)).sqrt();
            let source = format!(
                "quant cap {idx}\nVin in 0 DC 0 AC 1\nR1 in out {resistance}\nC1 out 0 {lower}\n.ac lin 2 {frequency} {frequency}\n.optim match ac V(out) freq={frequency} goal={target} params=C1.C:{lower}:{upper}:{lower}:{quantum} population=8 iterations=32 seed={} tol=1e-9\n.end",
                800 + idx
            );
            cases.add(
                format!("ams_quantized_optimization_capacitor_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    let result = optimize(&circuit, &circuit.analyses[1])?;
                    let parameter = result
                        .parameters
                        .iter()
                        .find(|parameter| parameter.parameter.eq_ignore_ascii_case("C1.C"))
                        .ok_or_else(|| "missing optimized capacitor".to_string())?;
                    let residual = (parameter.final_value - lower)
                        - ((parameter.final_value - lower) / quantum).round() * quantum;
                    if !result.converged
                        || (result.final_output - target).abs() > 1e-8
                        || (parameter.final_value - desired).abs() > quantum * 1e-7
                        || residual.abs() > quantum * 1e-7
                        || parameter.final_value < lower
                        || parameter.final_value > upper
                    {
                        Err(format!(
                            "quantum={quantum}, desired={desired}, target={target}, result={result:?}"
                        ))
                    } else {
                        Ok(())
                    }
                },
            );
        }
    }
}

fn pareto_optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let middle_resistance = 700.0 + (idx % 9) as f64 * 110.0;
        let lower = 450.0 + (idx % 7) as f64 * 40.0;
        let upper = 4_200.0 + (idx % 11) as f64 * 180.0;
        let source = format!(
            "pareto ladder {idx}\nVin in 0 {supply}\nR1 in a {middle_resistance}\nR2 a b {middle_resistance}\nR3 b 0 {middle_resistance}\n.pareto V(a) V(b) sense=min,max params=R1.R:{lower}:{upper},R3.R:{lower}:{upper} population=12 iterations=24 seed={} points=10\n.end",
            2100 + idx
        );
        cases.add(format!("ams_pareto_optimization_ladder_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 10
                || !result.is_nondominated()
                || result.evaluations != 12 + 12 * 24
            {
                return Err(format!("{result:?}"));
            }
            for pair in result.points.windows(2) {
                if pair[0].outputs[0] > pair[1].outputs[0]
                    || pair[0].outputs[1] > pair[1].outputs[1]
                {
                    return Err(format!("front is not sorted: {result:?}"));
                }
            }
            if result.points.last().unwrap().outputs[0] <= result.points.first().unwrap().outputs[0]
                || result.points.last().unwrap().outputs[1]
                    <= result.points.first().unwrap().outputs[1]
            {
                return Err(format!("front has no measurable tradeoff: {result:?}"));
            }
            for point in &result.points {
                for parameter in &point.parameters {
                    if parameter.final_value < lower || parameter.final_value > upper {
                        return Err(format!("parameter outside bounds: {point:?}"));
                    }
                }
            }
            Ok(())
        });
    }
}

fn constrained_pareto_cases(cases: &mut CaseList) {
    for idx in 0..25 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let middle = 700.0 + (idx % 9) as f64 * 110.0;
        let lower = 450.0 + (idx % 7) as f64 * 40.0;
        let upper = 4_200.0 + (idx % 11) as f64 * 180.0;
        let minimum_b = supply * lower / (middle + lower);
        let maximum_b = supply * upper / (middle + upper);
        let fraction = 0.25 + (idx % 5) as f64 * 0.12;
        let threshold = minimum_b + fraction * (maximum_b - minimum_b);
        let source = format!(
            "constrained pareto {idx}\nVin in 0 {supply}\nR1 in a {middle}\nR2 a b {middle}\nR3 b 0 {middle}\n.pareto V(a) V(b) sense=min,max constraints=V(b)>={threshold} params=R1.R:{lower}:{upper},R3.R:{lower}:{upper} population=12 iterations=24 seed={} points=8\n.end",
            2400 + idx
        );
        cases.add(format!("ams_constrained_pareto_ladder_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 8
                || !result.is_nondominated()
                || result.constraints.len() != 1
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                if !point.constraints_satisfied
                    || point.constraint_violation > 1e-10
                    || point.outputs[1] + 1e-9 < threshold
                {
                    return Err(format!(
                        "threshold={threshold}, constraint_violation={}, outputs={:?}",
                        point.constraint_violation, point.outputs
                    ));
                }
            }
            Ok(())
        });
    }
}

fn algebraic_constraint_pareto_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 8) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 190.0;
        let expression_at = |r1: f64, r4: f64| {
            let total = r1 + series + series + series + r4;
            let va = supply * (series + series + r4) / total;
            let vb = supply * (series + r4) / total;
            let vc = supply * r4 / total;
            2.0 * va + vb - 0.5 * vc
        };
        let minimum = expression_at(upper, lower);
        let maximum = expression_at(lower, upper);
        let fraction = 0.55 + (idx % 5) as f64 * 0.08;
        let threshold = minimum + fraction * (maximum - minimum);
        let source = format!(
            "algebraic pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(2*V(a)+V(b)-0.5*V(c))<={threshold} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2700 + idx
        );
        cases.add(
            format!("ams_algebraic_constraint_pareto_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
                if result.points.len() < 2
                    || result.points.len() > 6
                    || !result.is_nondominated()
                    || result.constraints.len() != 1
                {
                    return Err(format!("{result:?}"));
                }
                for point in &result.points {
                    let value = 2.0 * point.outputs[0] + point.outputs[1] - 0.5 * point.outputs[2];
                    if !point.constraints_satisfied
                        || point.constraint_violation > 1e-10
                        || value > threshold + 1e-9
                    {
                        return Err(format!(
                            "threshold={threshold}, value={value}, violation={}, point={point:?}",
                            point.constraint_violation
                        ));
                    }
                }
                Ok(())
            },
        );
    }
}

fn function_constraint_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 8) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 190.0;
        let expression_at = |r1: f64, r4: f64| {
            let total = r1 + 3.0 * series + r4;
            let vb = supply * (series + r4) / total;
            let vc = supply * r4 / total;
            vb + 0.1 * (1.0 + vc).ln()
        };
        let minimum = expression_at(upper, lower);
        let maximum = expression_at(lower, upper);
        let fraction = 0.35 + (idx % 5) as f64 * 0.1;
        let threshold = minimum + fraction * (maximum - minimum);
        let source = format!(
            "function constraint pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(min(V(a),V(b))+0.1*ln(1+V(c)))>={threshold} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2800 + idx
        );
        cases.add(format!("ams_function_constraint_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 6
                || !result.is_nondominated()
                || result.constraints.len() != 1
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let value = point.outputs[1] + 0.1 * (1.0 + point.outputs[2]).ln();
                if !point.constraints_satisfied
                    || point.constraint_violation > 1e-10
                    || value + 1e-9 < threshold
                {
                    return Err(format!(
                        "threshold={threshold}, value={value}, violation={}, point={point:?}",
                        point.constraint_violation
                    ));
                }
            }
            Ok(())
        });
    }
}

fn multi_objective_pareto_cases(cases: &mut CaseList) {
    for idx in 0..15 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 700.0 + (idx % 8) as f64 * 120.0;
        let lower = 500.0 + (idx % 6) as f64 * 50.0;
        let upper = 4_000.0 + (idx % 9) as f64 * 200.0;
        let source = format!(
            "three-objective pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2500 + idx
        );
        cases.add(
            format!("ams_three_objective_pareto_ladder_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
                if result.output_nodes.len() != 3
                    || result.senses.len() != 3
                    || result.points.len() < 2
                    || result.points.len() > 6
                    || !result.is_nondominated()
                {
                    return Err(format!("{result:?}"));
                }
                for point in &result.points {
                    if point.outputs.len() != 3 {
                        return Err(format!("{point:?}"));
                    }
                    if !point.crowding_distance.is_finite()
                        && !point.crowding_distance.is_infinite()
                    {
                        return Err(format!("non-finite crowding distance: {point:?}"));
                    }
                    for parameter in &point.parameters {
                        if parameter.final_value < lower || parameter.final_value > upper {
                            return Err(format!("parameter outside bounds: {point:?}"));
                        }
                    }
                }
                Ok(())
            },
        );
    }
}

fn reference_direction_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 7) as f64 * 130.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 210.0;
        let divisions = 3 + idx % 3;
        let source = format!(
            "reference-direction pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions={divisions} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2600 + idx
        );
        cases.add(format!("ams_reference_direction_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.front_method != ParetoFrontMethod::ReferenceDirections
                || result.points.len() != 6
                || !result.is_nondominated()
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let Some(direction) = &point.reference_direction else {
                    return Err(format!("missing reference direction: {point:?}"));
                };
                let norm = direction
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                if direction.len() != 3
                    || direction.iter().any(|value| *value < 0.0)
                    || (norm - 1.0).abs() > 1e-9
                {
                    return Err(format!("invalid reference direction: {point:?}"));
                }
            }
            Ok(())
        });
    }
}

fn adaptive_reference_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 800.0 + (idx % 7) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 40.0;
        let upper = 3_900.0 + (idx % 8) as f64 * 175.0;
        let divisions = 4 + idx % 3;
        let source = format!(
            "adaptive reference pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions={divisions} adaptive=true params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=7\n.end",
            3200 + idx
        );
        cases.add(format!("ams_adaptive_reference_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if !result.adaptive_references
                || result.front_method != ParetoFrontMethod::ReferenceDirections
                || result.points.len() != 7
                || !result.is_nondominated()
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let Some(direction) = &point.reference_direction else {
                    return Err(format!("missing adaptive reference: {point:?}"));
                };
                let norm = direction
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                if direction.len() != 3
                    || direction.iter().any(|value| *value < 0.0)
                    || (norm - 1.0).abs() > 1e-9
                {
                    return Err(format!("invalid adaptive reference: {point:?}"));
                }
            }
            Ok(())
        });
    }
}

fn periodic_steady_state_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let resistance = 500.0 + (idx % 7) as f64 * 250.0;
        let capacitance = 100e-9 + (idx % 6) as f64 * 100e-9;
        let frequency = 1_000.0 + (idx % 9) as f64 * 500.0;
        let omega = std::f64::consts::TAU * frequency;
        let expected = 1.0 / (1.0 + (omega * resistance * capacitance).powi(2)).sqrt();
        let source = format!(
            "pss lowpass {idx}\nVin in 0 SIN(0 1 {frequency})\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.tran 1u 20m method=be\n.pss V(out) fund={frequency} min=8 max=64 samples=64 harmonics=4 tol=1e-4\n.end"
        );
        cases.add(format!("ams_pss_rc_lowpass_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            let fundamental = result
                .harmonic(1)
                .ok_or_else(|| "missing PSS fundamental".to_string())?;
            let second = result
                .harmonic(2)
                .ok_or_else(|| "missing PSS second harmonic".to_string())?;
            if !result.converged
                || (fundamental.phasor.magnitude() - expected).abs() > 0.02
                || second.phasor.magnitude() > 1e-3
            {
                Err(format!(
                    "expected={expected}, fundamental={fundamental:?}, second={second:?}, result_cycles={}",
                    result.cycles
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn automatic_pss_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let frequency = 500.0 + idx as f64 * 250.0;
        let period = 1.0 / frequency;
        let transient_step = period / 128.0;
        let transient_stop = period * 24.0;
        let source = format!(
            "automatic pss {idx}\nVin out 0 SIN(0 1 {frequency})\n.tran {transient_step} {transient_stop} method=be\n.pss V(out) auto min=6 max=32 samples=64 harmonics=3 tol=1e-4\n.end"
        );
        cases.add(format!("ams_automatic_pss_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            if !result.automatic_fundamental
                || !result.converged
                || (result.fundamental_hz - frequency).abs() > frequency * 0.01
                || result.harmonic(1).unwrap().phasor.magnitude() < 0.99
                || result.harmonic(2).unwrap().phasor.magnitude() > 1e-3
            {
                Err(format!(
                    "expected={frequency}, fundamental={:?}, harmonic1={:?}, harmonic2={:?}",
                    result.harmonic(1).map(|item| item.phasor),
                    result.harmonic(2).map(|item| item.phasor),
                    result.fundamental_hz
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn autonomous_pss_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let nmos_width = 0.8e-6 + (idx % 6) as f64 * 0.1e-6;
        let pmos_width = 2.0 * nmos_width;
        let capacitance = 0.6e-12 + (idx % 7) as f64 * 0.2e-12;
        let source = format!(
            "ring oscillator {idx}\nVdd vdd 0 {supply}\n\
             Mn1 n1 n3 0 0 NM W={nmos_width} L=180n\nMp1 n1 n3 vdd vdd PM W={pmos_width} L=180n\n\
             Mn2 n2 n1 0 0 NM W={nmos_width} L=180n\nMp2 n2 n1 vdd vdd PM W={pmos_width} L=180n\n\
             Mn3 n3 n2 0 0 NM W={nmos_width} L=180n\nMp3 n3 n2 vdd vdd PM W={pmos_width} L=180n\n\
             C1 n1 0 {capacitance} IC={supply}\nC2 n2 0 {capacitance}\nC3 n3 0 {capacitance}\n\
             .model NM NMOS(KP=200u VTO=0.5)\n.model PM PMOS(KP=100u VTO=-0.5)\n\
             .tran 10p 120n method=be\n.pss V(n1) auto min=6 max=24 samples=192 harmonics=3 tol=0.06\n.end"
        );
        cases.add(format!("ams_autonomous_pss_ring_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            let first = result
                .samples
                .iter()
                .map(|sample| sample.value)
                .fold(f64::INFINITY, f64::min);
            let last = result
                .samples
                .iter()
                .map(|sample| sample.value)
                .fold(f64::NEG_INFINITY, f64::max);
            let fundamental = result.harmonic(1).ok_or("missing fundamental")?;
            if !result.automatic_fundamental
                || !result.converged
                || result.residual > 0.06
                || result.fundamental_hz <= 20_000_000.0
                || result.fundamental_hz >= 250_000_000.0
                || (fundamental.frequency_hz - result.fundamental_hz).abs() > 1e-3
                || fundamental.phasor.magnitude() < 0.5 * supply
                || first > 0.25 * supply
                || last < 0.75 * supply
            {
                Err(format!(
                    "supply={supply}, C={capacitance}, result={result:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn envelope_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let carrier = 50_000.0 + idx as f64 * 2_000.0;
        let modulation = 100.0 + idx as f64 * 20.0;
        let depth = 0.2 + (idx % 7) as f64 * 0.1;
        let transient_step = 1.0 / (carrier * 16.0);
        let transient_stop = 4.0 / modulation;
        let expected_min = 1.0 - depth;
        let expected_max = 1.0 + depth;
        let source = format!(
            "envelope am {idx}\nVin out 0 AM(0 1 {carrier} {modulation} {depth})\n.tran {transient_step} {transient_stop} method=be\n.env V(out) carrier={carrier} cycles=8 points=128\n.end"
        );
        cases.add(format!("ams_envelope_am_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = envelope_follow(&circuit, &circuit.analyses[1])?;
            if result.samples.len() != 128
                || (result.minimum - expected_min).abs() > 0.12
                || (result.maximum - expected_max).abs() > 0.12
                || (result.average - 1.0).abs() > 0.12
            {
                Err(format!(
                    "depth={depth}, expected=[{expected_min},{expected_max}], result min={}, max={}, average={}",
                    result.minimum, result.maximum, result.average
                ))
            } else {
                Ok(())
            }
        });
    }
}
