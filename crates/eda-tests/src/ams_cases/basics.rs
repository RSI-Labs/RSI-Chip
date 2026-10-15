//! Generated as part of an AMS integration-case split; domains are grouped below.

use crate::CaseList;
use eda_api::*;

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

pub(super) fn register(cases: &mut CaseList) {
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
    advanced_mos_mobility_cases(cases);
}
