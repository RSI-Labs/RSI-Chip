//! Generated as part of an AMS integration-case split; domains are grouped below.

use crate::CaseList;
use eda_api::*;

fn bsim3_long_channel_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let threshold = 0.32 + (idx % 6) as f64 * 0.035;
        let oxide_thickness = 1.6e-9 + (idx % 5) as f64 * 0.15e-9;
        let low_field_mobility = 0.035 + (idx % 4) as f64 * 0.004;
        let first_order_mobility = 0.7e-15 + (idx % 5) as f64 * 0.12e-15;
        let second_order_mobility = 0.6e-24 + (idx % 4) as f64 * 0.18e-24;
        let saturation_velocity = 70_000.0 + (idx % 5) as f64 * 3_500.0;
        let width = 750e-9 + (idx % 6) as f64 * 85e-9;
        let length = 165e-9 + (idx % 5) as f64 * 21e-9;
        let supply = 1.55 + (idx % 4) as f64 * 0.08;
        let resistance = 480.0 + idx as f64 * 61.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold;
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = threshold;
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
        let source = format!(
            "BSIM3 long channel {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={threshold} K1=0 TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_long_channel_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_short_channel_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.32 + (idx % 5) as f64 * 0.025;
        let oxide_thickness = 1.7e-9 + (idx % 4) as f64 * 0.12e-9;
        let low_field_mobility = 0.036 + (idx % 5) as f64 * 0.003;
        let first_order_mobility = 0.8e-15 + (idx % 4) as f64 * 0.1e-15;
        let second_order_mobility = 0.7e-24 + (idx % 3) as f64 * 0.15e-24;
        let saturation_velocity = 72_000.0 + (idx % 6) as f64 * 2_000.0;
        let short_channel_amplitude = 0.025 + (idx % 5) as f64 * 0.012;
        let short_channel_scale = 16e-9 + (idx % 4) as f64 * 4e-9;
        let drain_bias_coefficient = 0.02 + (idx % 5) as f64 * 0.015;
        let width = 800e-9 + (idx % 5) as f64 * 70e-9;
        let length = 40e-9 + (idx % 6) as f64 * 25e-9;
        let supply = 1.45 + (idx % 4) as f64 * 0.07;
        let resistance = 520.0 + idx as f64 * 53.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = |voltage: f64| {
            nominal_threshold
                - short_channel_amplitude
                    * (-(length / short_channel_scale)).exp()
                    * (1.0 + drain_bias_coefficient * voltage)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold(voltage);
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.2).max(0.0);
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
        let source = format!(
            "BSIM3 short channel {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0 DVT0={short_channel_amplitude} DVT1={short_channel_scale} DVT2={drain_bias_coefficient} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_short_channel_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_narrow_width_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.3 + (idx % 5) as f64 * 0.03;
        let oxide_thickness = 1.75e-9 + (idx % 4) as f64 * 0.11e-9;
        let low_field_mobility = 0.037 + (idx % 4) as f64 * 0.004;
        let first_order_mobility = 0.85e-15 + (idx % 5) as f64 * 0.09e-15;
        let second_order_mobility = 0.75e-24 + (idx % 3) as f64 * 0.12e-24;
        let saturation_velocity = 73_000.0 + (idx % 5) as f64 * 1_800.0;
        let narrow_amplitude = 0.035 + (idx % 6) as f64 * 0.011;
        let narrow_exponent = 0.2 + (idx % 5) as f64 * 0.09;
        let reference_width = 220e-9 + (idx % 6) as f64 * 18e-9;
        let width = 120e-9 + (idx % 8) as f64 * 32e-9;
        let length = 175e-9 + (idx % 5) as f64 * 19e-9;
        let supply = 1.5 + (idx % 4) as f64 * 0.06;
        let resistance = 560.0 + idx as f64 * 49.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = || {
            nominal_threshold
                + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold();
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.15).max(0.0);
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
        let source = format!(
            "BSIM3 narrow width {idx}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nM1 d d 0 0 NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0 DVT0W={narrow_amplitude} DVT1W={narrow_exponent} W0={reference_width} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} XPART=0.6)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_narrow_width_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_temperature_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let nominal_threshold = 0.31 + (idx % 5) as f64 * 0.028;
        let oxide_thickness = 1.8e-9 + (idx % 4) as f64 * 0.1e-9;
        let low_field_mobility = 0.038 + (idx % 5) as f64 * 0.0035;
        let first_order_mobility = 0.82e-15 + (idx % 4) as f64 * 0.11e-15;
        let second_order_mobility = 0.72e-24 + (idx % 3) as f64 * 0.14e-24;
        let saturation_velocity = 74_000.0 + (idx % 4) as f64 * 1_700.0;
        let short_channel_amplitude = 0.028 + (idx % 5) as f64 * 0.01;
        let short_channel_scale = 17e-9 + (idx % 4) as f64 * 4e-9;
        let drain_bias_coefficient = 0.025 + (idx % 5) as f64 * 0.013;
        let narrow_amplitude = 0.04 + (idx % 4) as f64 * 0.012;
        let narrow_exponent = 0.22 + (idx % 5) as f64 * 0.08;
        let reference_width = 225e-9 + (idx % 5) as f64 * 17e-9;
        let threshold_temperature_coefficient = -0.19 + (idx % 5) as f64 * 0.018;
        let body_temperature_coefficient = -0.045 + (idx % 4) as f64 * 0.012;
        let length_temperature_coefficient = (1.0 + (idx % 4) as f64) * 1e-11;
        let mobility_temperature_exponent = 1.25 + (idx % 5) as f64 * 0.08;
        let width = 180e-9 + (idx % 7) as f64 * 35e-9;
        let length = 145e-9 + (idx % 5) as f64 * 23e-9;
        let body_bias = -0.3 + (idx % 6) as f64 * 0.08;
        let temperature_c = [-25.0, -10.0, 15.0, 45.0, 85.0, 125.0][idx % 6];
        let temperature_k = temperature_c + 273.15;
        let supply = 1.5 + (idx % 4) as f64 * 0.06;
        let resistance = 540.0 + idx as f64 * 47.0;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let threshold = |voltage: f64| {
            let surface = 0.7_f64;
            let body_term = (surface - body_bias).max(0.0).sqrt() - surface.sqrt();
            nominal_threshold + 0.5 * body_term
                - short_channel_amplitude
                    * (-(length / short_channel_scale)).exp()
                    * (1.0 + drain_bias_coefficient * voltage)
                + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0)
                + (threshold_temperature_coefficient + body_temperature_coefficient * body_bias
                    - length_temperature_coefficient / length)
                    * (temperature_k / 300.0 - 1.0)
        };
        let current = |voltage: f64| -> f64 {
            let overdrive = voltage - threshold(voltage);
            if overdrive <= 0.0 {
                return 0.0;
            }
            let normalized = overdrive / oxide_thickness;
            let nominal_mobility = low_field_mobility
                / (1.0
                    + first_order_mobility * normalized
                    + second_order_mobility * normalized * normalized);
            let mobility =
                nominal_mobility * (300.0 / temperature_k).powf(mobility_temperature_exponent);
            let kp = mobility * oxide_cap;
            let saturation_voltage =
                overdrive / (1.0 + overdrive * mobility / (2.0 * saturation_velocity * length));
            kp * width / length
                * (overdrive * saturation_voltage - saturation_voltage * saturation_voltage / 2.0)
        };
        let equation = |voltage: f64| (supply - voltage) / resistance - current(voltage);
        let mut low = (nominal_threshold - 0.2).max(0.0);
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
        let source = format!(
            "BSIM3 temperature {idx}\n.temp {temperature_c}\nVdd vdd 0 {supply}\nRd vdd d {resistance}\nVb b 0 {body_bias}\nM1 d d 0 b NM W={width} L={length}\n.model NM NMOS(LEVEL=8 VTH0={nominal_threshold} K1=0.5 DVT0={short_channel_amplitude} DVT1={short_channel_scale} DVT2={drain_bias_coefficient} DVT0W={narrow_amplitude} DVT1W={narrow_exponent} W0={reference_width} KT1={threshold_temperature_coefficient} KT2={body_temperature_coefficient} KT1L={length_temperature_coefficient} TOX={oxide_thickness} U0={low_field_mobility} UA={first_order_mobility} UB={second_order_mobility} VSAT={saturation_velocity} UTE={mobility_temperature_exponent})\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_temperature_diode_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let op = dc_operating_point(&circuit)?;
            let drain = op
                .node_voltages
                .get("d")
                .copied()
                .ok_or_else(|| "missing BSIM3 drain".to_string())?;
            if (drain - expected).abs() > 1e-8 {
                return Err(format!("V(d)={drain}, expected={expected}, op={op:?}"));
            }
            Ok(())
        });
    }
}

fn bsim3_geometry_binning_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let first_bin = idx % 2 == 0;
        let (threshold, low_field_mobility) = if first_bin {
            (0.3, 0.038)
        } else {
            (0.43, 0.048)
        };
        let (width, length) = if first_bin {
            (
                120e-9 + (idx % 5) as f64 * 32e-9,
                105e-9 + (idx % 5) as f64 * 28e-9,
            )
        } else {
            (
                360e-9 + (idx % 5) as f64 * 85e-9,
                300e-9 + (idx % 5) as f64 * 72e-9,
            )
        };
        let oxide_thickness = if first_bin { 1.8e-9 } else { 2.0e-9 };
        let first_order_mobility = 0.8e-15;
        let second_order_mobility = 0.7e-24;
        let saturation_velocity = 72_000.0;
        let short_channel_amplitude = 0.03;
        let short_channel_scale = 18e-9;
        let narrow_amplitude = 0.04;
        let narrow_exponent = 0.22;
        let reference_width = if first_bin { 260e-9 } else { 520e-9 };
        let gate = threshold + 0.4;
        let drain = 1.2;
        let oxide_cap = 3.9 * 8.854_187_812_8e-12 / oxide_thickness;
        let overdrive = gate - threshold;
        let normalized = overdrive / oxide_thickness;
        let mobility = low_field_mobility
            / (1.0
                + first_order_mobility * normalized
                + second_order_mobility * normalized * normalized);
        let kp = mobility * oxide_cap;
        let effective_threshold = threshold
            - short_channel_amplitude * (-(length / short_channel_scale)).exp() * 1.0
            + narrow_amplitude * ((reference_width / width).powf(narrow_exponent) - 1.0);
        let effective_overdrive = gate - effective_threshold;
        let effective_saturation_voltage = effective_overdrive
            / (1.0 + effective_overdrive * mobility / (2.0 * saturation_velocity * length));
        let expected = kp * width / length
            * (effective_overdrive * effective_saturation_voltage
                - effective_saturation_voltage * effective_saturation_voltage / 2.0);
        let source = format!(
            "BSIM3 geometry bin {idx}\nVdd d 0 {drain}\nVg g 0 {gate}\nM1 d g 0 0 NM W={width} L={length}\n.model NM.1 NMOS(LEVEL=8 VTH0=0.3 WMIN=80n WMAX=300n LMIN=80n LMAX=270n DVT0=0.03 DVT1=18n DVT0W=0.04 DVT1W=0.22 W0=260n TOX=1.8n U0=0.038 UA=0.8e-15 UB=0.7e-24 VSAT=72k)\n.model NM.2 NMOS(LEVEL=8 VTH0=0.43 WMIN=320n WMAX=900n LMIN=290n LMAX=800n DVT0=0.03 DVT1=18n DVT0W=0.04 DVT1W=0.22 W0=520n TOX=2n U0=0.048 UA=0.8e-15 UB=0.7e-24 VSAT=72k)\n.op\n.end"
        );
        cases.add(format!("ams_bsim3_geometry_bin_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            if idx == 0 {
                assert_eq!(
                    circuit,
                    parse_spice(&write_spice(&circuit).unwrap()).unwrap()
                );
            }
            let selected = circuit.mosfet_model_for("NM", width, length)?;
            let expected_threshold = if first_bin { 0.3 } else { 0.43 };
            if (selected.threshold_voltage_v - expected_threshold).abs() > 1e-12 {
                return Err(format!(
                    "selected VTH0={} for W={width} L={length}",
                    selected.threshold_voltage_v
                ));
            }
            let op = dc_operating_point(&circuit)?;
            let current = op
                .branch_currents
                .get("Vdd")
                .copied()
                .ok_or_else(|| "missing binned MOS drain current".to_string())?;
            if (current + expected).abs() > 1e-9 * (1.0 + expected) {
                return Err(format!(
                    "current={current}, expected={}, selected={selected:?}",
                    -expected
                ));
            }
            Ok(())
        });
    }
}

pub(super) fn register(cases: &mut CaseList) {
    bsim3_long_channel_cases(cases);
    bsim3_short_channel_cases(cases);
    bsim3_narrow_width_cases(cases);
    bsim3_temperature_cases(cases);
    bsim3_geometry_binning_cases(cases);
}
