//! Core parsing, linear DC/AC, transient, cancellation, and round-trip tests.

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
        parse_spice("cancelled op\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 2k\n.op\n.end").unwrap();
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
        monte_carlo_cancellable(&monte_carlo, &monte_carlo.analyses[0], &cancellation).unwrap_err(),
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
        optimize_cancellable(&optimization, &optimization.analyses[0], &cancellation).unwrap_err(),
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

    let vcvs =
        parse_spice("exact vcvs\nVin ctrl 0 DC 0 AC 1\nE1 out 0 ctrl 0 2.5\n.ac lin 1 1k 1k\n.end")
            .unwrap();
    let result = ac_sweep(&vcvs, &vcvs.analyses[0]).unwrap();
    let output = result.points[0].node_phasors["out"];
    assert!(
        (output.real - 2.5).abs() < 1e-12 && output.imaginary.abs() < 1e-12,
        "{result:?}"
    );
}
