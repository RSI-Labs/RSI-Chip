//! Generated as part of an AMS integration-case split; domains are grouped below.

use crate::CaseList;
use eda_api::*;

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

pub(super) fn register(cases: &mut CaseList) {
    sensitivity_cases(cases);
    analytic_derivative_cases(cases);
    global_sensitivity_cases(cases);
    correlated_global_sensitivity_cases(cases);
    resistor_temperature_cases(cases);
}
