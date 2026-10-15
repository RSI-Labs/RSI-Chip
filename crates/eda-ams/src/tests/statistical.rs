//! Monte Carlo distributions, correlations, and statistical imports.

use super::*;

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

    let invalid = parse_spice("bad location\nVin in 0 1\nR1 out 0 1k\n.location Missing 0 0\n.end")
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
        (derivative_r1.powi(2) * (r1 * 0.05).powi(2) + derivative_r2.powi(2) * (r2 * 0.05).powi(2))
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

    let missing =
        parse_spice("missing stats\nVin in 0 1\nR1 out 0 1k\n.mc 8 op V(out) stats=absent\n.end")
            .unwrap_err();
    assert!(
        missing.contains("Monte Carlo statistical model 'absent' does not exist"),
        "{missing}"
    );
    let unknown_parameter =
        parse_statistical_model("model bad\nparam Missing.R dist=normal relative=true sigma=0.1\n")
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
