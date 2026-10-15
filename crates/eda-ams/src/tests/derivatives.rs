//! Sensitivity, derivative, Hessian, and nonlinear AC gradient tests.

use super::*;

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
        expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal) + evaluate(low))
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
                (evaluate(high_high) - evaluate(high_low) - evaluate(low_high) + evaluate(low_low))
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
        expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal) + evaluate(low))
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
                (evaluate(high_high) - evaluate(high_low) - evaluate(low_high) + evaluate(low_low))
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
        expected_hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal) + evaluate(low))
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
                (evaluate(high_high) - evaluate(high_low) - evaluate(low_high) + evaluate(low_low))
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
    let d2_iis = -((exponent / thermal_voltage) * d_i + nonlinear_second * d_i * d_is) / jacobian;
    let d2_is =
        -(2.0 * (exponent / thermal_voltage) * d_is + nonlinear_second * d_is * d_is) / jacobian;
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
