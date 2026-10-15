//! Generated as part of an AMS integration-case split; domains are grouped below.

use crate::CaseList;
use eda_api::*;

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

pub(super) fn register(cases: &mut CaseList) {
    noise_sweeps(cases);
    transistor_charge_cases(cases);
    aging_cases(cases);
    junction_capacitance_cases(cases);
    mos_charge_partition_cases(cases);
    flicker_noise_cases(cases);
    temperature_cases(cases);
    transit_charge_cases(cases);
}
