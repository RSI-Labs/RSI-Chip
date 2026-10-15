//! Generated as part of an AMS integration-case split; domains are grouped below.

use super::reference::{
    nonlinear_diode_ac_reference, nonlinear_diode_junction_reference, trapezoidal_rc_derivatives,
};
use crate::CaseList;
use eda_api::*;

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

pub(super) fn register(cases: &mut CaseList) {
    monte_carlo_cases(cases);
    correlated_monte_carlo_cases(cases);
    correlated_matrix_monte_carlo_cases(cases);
    spatial_monte_carlo_cases(cases);
    distribution_monte_carlo_cases(cases);
    covariance_monte_carlo_cases(cases);
    vendor_statistical_model_cases(cases);
}
