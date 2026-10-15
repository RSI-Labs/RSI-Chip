//! Generated as part of an AMS integration-case split; domains are grouped below.

use eda_api::*;

pub(super) fn trapezoidal_rc_derivatives(
    resistance: f64,
    equivalent_capacitance: f64,
    step_s: f64,
    steps: usize,
    source_value: f64,
) -> (f64, Vec<f64>, Vec<Vec<f64>>) {
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
            let rhs_first = match left {
                0 => -source_value / (resistance * resistance),
                1 | 2 => conductance_first[left] * voltage,
                3 => 1.0 / resistance,
                _ => unreachable!(),
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
    (voltage, voltage_first, voltage_second)
}

fn nonlinear_diode_ac_magnitude(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
) -> f64 {
    let text = format!(
        "nonlinear diode AC reference\nVin in 0 DC {dc_value} AC {ac_magnitude}\nR1 in out {resistance}\nD1 out 0 DN\n.model DN D(IS={saturation_current} N=1 TT={transit_time} CJO={junction_capacitance} VJ=700m M=0.5 FC=0.5)\n.temp 26.85\n.ac lin 1 20 20\n.end"
    );
    let circuit = parse_spice(&text).unwrap();
    sweep_ac(&circuit, &circuit.analyses[0])
        .unwrap()
        .node_magnitude("out")
        .unwrap()[0]
}

pub(super) fn nonlinear_diode_ac_reference(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
) -> (Vec<f64>, Vec<Vec<f64>>) {
    let deltas = [
        resistance * 1e-2,
        saturation_current * 1e-3,
        dc_value * 1e-3,
        ac_magnitude * 1e-3,
        transit_time * 1e-3,
        junction_capacitance * 1e-2,
    ];
    let point = |values: [f64; 6]| {
        nonlinear_diode_ac_magnitude(
            values[0], values[1], values[2], values[3], values[4], values[5],
        )
    };
    let nominal = [
        resistance,
        saturation_current,
        dc_value,
        ac_magnitude,
        transit_time,
        junction_capacitance,
    ];
    let mut gradient = Vec::with_capacity(6);
    for index in 0..6 {
        let mut high = nominal;
        let mut low = nominal;
        high[index] += deltas[index];
        low[index] -= deltas[index];
        gradient.push((point(high) - point(low)) / (2.0 * deltas[index]));
    }
    let mut hessian = vec![vec![0.0; 6]; 6];
    for left in 0..6 {
        let mut high = nominal;
        let mut low = nominal;
        high[left] += deltas[left];
        low[left] -= deltas[left];
        hessian[left][left] =
            (point(high) - 2.0 * point(nominal) + point(low)) / (deltas[left] * deltas[left]);
    }
    for left in 0..6 {
        for right in (left + 1)..6 {
            let mut high_low = nominal;
            let mut low_high = nominal;
            let mut high_high = nominal;
            let mut low_low = nominal;
            high_high[left] += deltas[left];
            high_high[right] += deltas[right];
            high_low[left] += deltas[left];
            high_low[right] -= deltas[right];
            low_high[left] -= deltas[left];
            low_high[right] += deltas[right];
            low_low[left] -= deltas[left];
            low_low[right] -= deltas[right];
            hessian[left][right] = (point(high_high) - point(high_low) - point(low_high)
                + point(low_low))
                / (4.0 * deltas[left] * deltas[right]);
            hessian[right][left] = hessian[left][right];
        }
    }
    (gradient, hessian)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn nonlinear_diode_junction_reference(
    resistance: f64,
    saturation_current: f64,
    dc_value: f64,
    ac_magnitude: f64,
    transit_time: f64,
    junction_capacitance: f64,
    junction_potential: f64,
    grading_coefficient: f64,
    forward_knee: f64,
) -> (Vec<f64>, Vec<Vec<f64>>) {
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
    let nominal = [
        resistance,
        saturation_current,
        dc_value,
        ac_magnitude,
        transit_time,
        junction_capacitance,
        junction_potential,
        grading_coefficient,
        forward_knee,
    ];
    let deltas = [
        resistance * 1e-2,
        saturation_current * 1e-3,
        dc_value * 1e-3,
        ac_magnitude * 1e-3,
        transit_time * 1e-3,
        1e-10,
        junction_potential * 1e-3,
        grading_coefficient * 1e-3,
        forward_knee * 1e-3,
    ];
    let mut gradient = vec![0.0; 9];
    for index in 0..9 {
        let mut high = nominal;
        let mut low = nominal;
        high[index] += deltas[index];
        low[index] -= deltas[index];
        gradient[index] = (evaluate(high) - evaluate(low)) / (2.0 * deltas[index]);
    }
    let mut hessian = vec![vec![0.0; 9]; 9];
    for left in 0..9 {
        let mut high = nominal;
        let mut low = nominal;
        high[left] += deltas[left];
        low[left] -= deltas[left];
        hessian[left][left] = (evaluate(high) - 2.0 * evaluate(nominal) + evaluate(low))
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
            hessian[left][right] = (evaluate(high_high) - evaluate(high_low) - evaluate(low_high)
                + evaluate(low_low))
                / (4.0 * deltas[left] * deltas[right]);
            hessian[right][left] = hessian[left][right];
        }
    }
    (gradient, hessian)
}
