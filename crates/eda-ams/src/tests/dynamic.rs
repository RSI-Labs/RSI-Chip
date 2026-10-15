//! Dynamic capacitance, transit charge, temperature, and self-heating tests.

use super::*;

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
fn mos_self_heating_reduces_drain_current_and_round_trips() {
    let make = |thermal_resistance: &str| {
        format!(
            "mos self heating\nVdd d 0 1.8\nVg g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 MEXP=1.5 RTH={thermal_resistance})\n.op\n.end"
        )
    };
    let isothermal = parse_spice(&make("0")).unwrap();
    let self_heated = parse_spice(&make("100k")).unwrap();
    assert_eq!(
        self_heated,
        parse_spice(&write_spice(&self_heated).unwrap()).unwrap()
    );
    assert_eq!(
        self_heated.mosfet_models["NM"].self_heating_thermal_resistance_k_per_w,
        100_000.0
    );

    let isothermal_current = -dc_operating_point(&isothermal).unwrap().branch_currents["Vdd"];
    let self_heated_current = -dc_operating_point(&self_heated).unwrap().branch_currents["Vdd"];
    assert!(
        self_heated_current < isothermal_current,
        "{self_heated_current} {isothermal_current}"
    );
    assert!(
        (isothermal_current - 0.5 * 1e-3 * (1e-6 / 180e-9) * 0.5_f64.powi(2)).abs() < 1e-10,
        "{isothermal_current}"
    );

    assert!(parse_spice(&make("-1")).is_err());
}

#[test]
fn mos_dynamic_charge_uses_self_heated_temperature() {
    let make = |thermal_resistance: &str| {
        format!(
            "mos thermal charge\nV1 d 0 1.8\nV2 g 0 0.52\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(LEVEL=8 VTH0=0.45 TOX=2n U0=0.04 VSAT=80k KT1=-0.16 COX=10m RTH={thermal_resistance})\n.op\n.end"
        )
    };
    let isothermal = parse_spice(&make("0")).unwrap();
    let self_heated = parse_spice(&make("1meg")).unwrap();
    let voltages = [
        ("d".to_string(), 1.8),
        ("g".to_string(), 0.52),
        ("b".to_string(), 0.0),
    ]
    .into_iter()
    .collect();
    let cold = crate::analysis::dynamic_capacitors(&isothermal, &isothermal.elements[2], &voltages)
        .unwrap()
        .into_iter()
        .find(|(name, _, _, _)| name == "M1:cgs")
        .unwrap()
        .3;
    let hot =
        crate::analysis::dynamic_capacitors(&self_heated, &self_heated.elements[2], &voltages)
            .unwrap()
            .into_iter()
            .find(|(name, _, _, _)| name == "M1:cgs")
            .unwrap()
            .3;
    assert!(hot > cold, "{cold} {hot}");
}

#[test]
fn mos_thermal_capacitance_creates_transient_temperature_state() {
    let make = |thermal_capacitance: &str| {
        format!(
            "mos thermal transient\nV1 d 0 1.8\nV2 g 0 1\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(KP=1m VTO=0.5 MEXP=1.5 RTH=100k CTH={thermal_capacitance})\n.tran 10m 100m method=trapezoidal\n.end"
        )
    };
    let thermal = parse_spice(&make("1u")).unwrap();
    assert_eq!(
        thermal,
        parse_spice(&write_spice(&thermal).unwrap()).unwrap()
    );
    let result = transient(&thermal, &thermal.analyses[0]).unwrap();
    assert_eq!(result.device_names(), vec!["M1".to_string()]);
    let temperatures: Vec<_> = result
        .samples
        .iter()
        .map(|sample| sample.device_temperatures["M1"])
        .collect();
    assert!(
        (temperatures[0] - thermal.temperature_k).abs() < 1e-9,
        "{temperatures:?}"
    );
    assert!(
        temperatures.windows(2).all(|pair| pair[1] >= pair[0]),
        "{temperatures:?}"
    );

    let static_model = parse_spice(&make("0")).unwrap();
    let static_result = transient(&static_model, &static_model.analyses[0]).unwrap();
    let static_temperature = static_result.samples.last().unwrap().device_temperatures["M1"];
    assert!(
        temperatures[0] < temperatures[temperatures.len() - 1]
            && temperatures[temperatures.len() - 1] < static_temperature,
        "{temperatures:?} static={static_temperature}"
    );
    assert_eq!(
        result.device_temperature_values("M1").unwrap(),
        temperatures
    );
    assert!(result.device_temperature_values("missing").is_err());
    let report = report_transient(&result);
    assert!(report.contains("T(M1)="), "{report}");
    let csv = write_transient_csv_with_temperatures(&result, &["d"], &["M1"]).unwrap();
    assert!(csv.contains("V(d),T(M1)"), "{csv}");
    assert!(write_transient_csv_with_temperatures(&result, &["d"], &["missing"]).is_err());

    assert!(parse_spice(&make("-1")).is_err());
    assert!(
        parse_spice("bad\nM1 d g 0 0 NM W=1u L=180n\n.model NM NMOS(CTH=1u)\n.tran 1 1\n.end")
            .is_err()
    );
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
