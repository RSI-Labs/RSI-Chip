//! Compact MOS/BJT/diode, noise, charge, and electrothermal tests.

use super::*;

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
            / (1.0 + (1.0e-15 + 1.0e-21 * 0.0) * normalized + 1.0e-24 * normalized * normalized);
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
fn bjt_self_heating_changes_current_and_round_trips() {
    let make = |thermal_resistance: &str| {
        format!(
            "bjt self heating\nV1 c 0 1.8\nV2 b 0 0.65\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-12 BF=100 XTI=3 RTH={thermal_resistance})\n.op\n.end"
        )
    };
    let isothermal = parse_spice(&make("0")).unwrap();
    let self_heated = parse_spice(&make("50k")).unwrap();
    assert_eq!(
        self_heated,
        parse_spice(&write_spice(&self_heated).unwrap()).unwrap()
    );
    assert_eq!(
        self_heated.bjt_models["NPN"].self_heating_thermal_resistance_k_per_w,
        50_000.0
    );

    let terminal_voltages = [1.8, 0.65, 0.0, 0.0];
    let isothermal_current = crate::analysis::nonlinear_terminal_currents(
        &isothermal,
        &isothermal.elements[2],
        &terminal_voltages,
    )
    .unwrap()[0];
    let self_heated_current = crate::analysis::nonlinear_terminal_currents(
        &self_heated,
        &self_heated.elements[2],
        &terminal_voltages,
    )
    .unwrap()[0];
    assert!(
        self_heated_current > isothermal_current,
        "{self_heated_current} {isothermal_current}"
    );
    assert!(parse_spice(&make("-1")).is_err());
}

#[test]
fn bjt_dynamic_charge_uses_self_heated_temperature() {
    let make = |thermal_resistance: &str| {
        format!(
            "bjt thermal charge\nV1 c 0 1.8\nV2 b 0 0.65\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-12 BF=100 TF=1n RTH={thermal_resistance})\n.op\n.end"
        )
    };
    let isothermal = parse_spice(&make("0")).unwrap();
    let self_heated = parse_spice(&make("1meg")).unwrap();
    let voltages = [("c".to_string(), 1.8), ("b".to_string(), 0.65)]
        .into_iter()
        .collect();
    let cold = crate::analysis::dynamic_capacitors(&isothermal, &isothermal.elements[2], &voltages)
        .unwrap()
        .into_iter()
        .find(|(name, _, _, _)| name == "Q1:cbe")
        .unwrap()
        .3;
    let hot =
        crate::analysis::dynamic_capacitors(&self_heated, &self_heated.elements[2], &voltages)
            .unwrap()
            .into_iter()
            .find(|(name, _, _, _)| name == "Q1:cbe")
            .unwrap()
            .3;
    assert!(hot > cold, "{cold} {hot}");
}

#[test]
fn bjt_thermal_capacitance_creates_transient_temperature_state() {
    let source = "bjt thermal transient\nVdd vdd 0 1.8\nRc vdd c 1k\nIb 0 b 10u\nQ1 c b 0 NPN\n.model NPN NPN(IS=1e-12 BF=100 XTI=3 RTH=1k CTH=10n)\n.tran 1m 10m method=trapezoidal\n.end";
    let circuit = parse_spice(source).unwrap();
    assert_eq!(
        circuit,
        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
    );
    let result = transient(&circuit, &circuit.analyses[0]).unwrap();
    let temperatures: Vec<_> = result
        .samples
        .iter()
        .map(|sample| sample.device_temperatures["Q1"])
        .collect();
    assert!(
        (temperatures[0] - circuit.temperature_k).abs() < 1e-9,
        "{temperatures:?}"
    );
    assert!(
        temperatures.windows(2).all(|pair| pair[1] >= pair[0]),
        "{temperatures:?}"
    );
    assert!(
        temperatures
            .last()
            .copied()
            .unwrap_or(circuit.temperature_k)
            > circuit.temperature_k,
        "{temperatures:?}"
    );
    assert!(parse_spice("bad\nQ1 c b 0 NPN\n.model NPN NPN(CTH=1u)\n.tran 1 1\n.end").is_err());
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

    let near =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[0.7, 0.7, 0.0, 0.0])
            .unwrap()[0];
    let reverse_biased =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[1.7, 0.7, 0.0, 0.0])
            .unwrap()[0];
    assert!(reverse_biased > near);
    assert!((reverse_biased / near - 1.1).abs() < 1e-9);

    let reverse_near =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[0.0, 0.7, 0.7, 0.0])
            .unwrap()[0];
    let reverse_early =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[0.0, 0.7, 1.7, 0.0])
            .unwrap()[0];
    assert!(reverse_early.abs() > reverse_near.abs());

    model.forward_early_voltage_v = 0.0;
    model.reverse_early_voltage_v = 0.0;
    circuit.bjt_models.insert("NPN".into(), model);
    let disabled_near =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[0.7, 0.7, 0.0, 0.0])
            .unwrap()[0];
    let disabled_reverse =
        crate::analysis::nonlinear_terminal_currents(&circuit, transistor, &[1.7, 0.7, 0.0, 0.0])
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
    let source =
        "noise round trip\nIin 0 out DC 0 AC 1\nR1 out 0 1k\n.noise V(out) Iin dec 10 1 1g\n.end";
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
