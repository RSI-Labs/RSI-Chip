//! PSS, envelope-follow, AM source, and aging tests.

use super::*;

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
    let source =
        "aging current\nIin out 0 1m\nR1 out 0 2k\n.aging V(out) years=9 exp=0.5 rinc=0.1\n.end";
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

    let invalid =
        parse_spice("bad aging\nIin out 0 1m\nR1 out 0 1k\n.aging V(out) years=1 kploss=1\n.end")
            .unwrap_err();
    assert!(
        invalid.contains("aging would remove all MOS transconductance"),
        "{invalid}"
    );
}
