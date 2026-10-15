//! AC Jacobian/Hessian derivative analysis.

use super::super::dc_operating_point;
use super::super::devices::*;
use super::super::solver::*;
use crate::numeric::{solve_complex, Complex};
use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcDerivativeKind {
    Resistance,
    Capacitance,
    Inductance,
    AcSource,
}

#[derive(Clone, Debug)]
struct AcDerivativeParameter {
    name: String,
    object: String,
    value: f64,
    kind: AcDerivativeKind,
}

fn conjugate(value: Complex) -> Complex {
    Complex::new(value.re, -value.im)
}

fn ac_derivative_parameters(circuit: &SpiceCircuit) -> Vec<AcDerivativeParameter> {
    let mut out = Vec::new();
    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { resistance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.R", element.name),
                object: element.name.clone(),
                value: *resistance,
                kind: AcDerivativeKind::Resistance,
            }),
            SpiceElementKind::Capacitor { capacitance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.C", element.name),
                object: element.name.clone(),
                value: *capacitance,
                kind: AcDerivativeKind::Capacitance,
            }),
            SpiceElementKind::Inductor { inductance, .. } => out.push(AcDerivativeParameter {
                name: format!("{}.L", element.name),
                object: element.name.clone(),
                value: *inductance,
                kind: AcDerivativeKind::Inductance,
            }),
            SpiceElementKind::VoltageSource {
                ac_magnitude: Some(magnitude),
                ..
            }
            | SpiceElementKind::CurrentSource {
                ac_magnitude: Some(magnitude),
                ..
            } => out.push(AcDerivativeParameter {
                name: format!("{}.AC", element.name),
                object: element.name.clone(),
                value: *magnitude,
                kind: AcDerivativeKind::AcSource,
            }),
            _ => {}
        }
    }
    out
}

pub(super) fn ac_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    if circuit
        .elements
        .iter()
        .any(|element| matches!(element.kind, SpiceElementKind::Diode { .. }))
    {
        return nonlinear_diode_ac_derivative_analysis(circuit, options);
    }
    let system = SystemModel::new(circuit)?;
    let size = system.size;
    let mut matrix = vec![vec![Complex::zero(); size]; size];
    let mut rhs = vec![Complex::zero(); size];
    let omega = std::f64::consts::TAU * options.frequency_hz;
    let j = Complex::new(0.0, 1.0);

    for element in &circuit.elements {
        if matches!(
            element.kind,
            SpiceElementKind::Capacitor { .. } | SpiceElementKind::Inductor { .. }
        ) {
            let branch = system
                .branch_index(&element.name)
                .ok_or_else(|| format!("missing branch for element '{}'", element.name))?;
            matrix[branch][branch] = Complex::new(1.0, 0.0);
        }
    }

    for element in &circuit.elements {
        match &element.kind {
            SpiceElementKind::Resistor { .. } => {
                let effective = element_effective_resistance(circuit, element)?;
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(1.0 / effective, 0.0),
                );
            }
            SpiceElementKind::Capacitor { capacitance, .. } => {
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    j.scale(omega * capacitance),
                );
            }
            SpiceElementKind::Inductor { inductance, .. } => {
                let admittance = Complex::new(1.0, 0.0).div(j.scale(omega * inductance))?;
                stamp_complex_conductance(
                    &mut matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance,
                );
            }
            SpiceElementKind::VoltageSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                let branch = system
                    .branch_index(&element.name)
                    .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                stamp_complex_branch_current(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[0],
                    &element.nodes[1],
                );
                stamp_complex_node(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[0],
                    Complex::new(1.0, 0.0),
                );
                stamp_complex_node(
                    &mut matrix,
                    &system,
                    branch,
                    &element.nodes[1],
                    Complex::new(-1.0, 0.0),
                );
                if let Some(magnitude) = ac_magnitude {
                    rhs[branch] = Complex::polar(*magnitude, (*ac_phase_deg).to_radians());
                }
            }
            SpiceElementKind::CurrentSource {
                ac_magnitude,
                ac_phase_deg,
                ..
            } => {
                if let Some(magnitude) = ac_magnitude {
                    stamp_complex_current(
                        &mut rhs,
                        &system,
                        &element.nodes[0],
                        &element.nodes[1],
                        Complex::polar(*magnitude, (*ac_phase_deg).to_radians()),
                    );
                }
            }
            _ => return Err("AC analytic derivatives support linear RCL circuits only".into()),
        }
    }

    let solution = solve_complex(matrix.clone(), rhs)?;
    let output_index = system
        .node_index(&options.output_node)
        .ok_or_else(|| format!("unknown derivative node '{}'", options.output_node))?;
    let output = solution[output_index];
    let nominal_output = output.magnitude();
    if !nominal_output.is_finite() || nominal_output <= f64::EPSILON {
        return Err("AC derivative output magnitude is zero".into());
    }

    let available = ac_derivative_parameters(circuit);
    let mut first = Vec::with_capacity(options.parameters.len());
    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let parameter = available
            .iter()
            .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("unknown AC derivative parameter '{name}'"))?;
        let element = circuit
            .element(&parameter.object)
            .ok_or_else(|| format!("missing AC derivative element '{}'", parameter.object))?;
        let mut derivative_matrix = vec![vec![Complex::zero(); size]; size];
        let mut derivative_rhs = vec![Complex::zero(); size];
        let mut second_matrix = vec![vec![Complex::zero(); size]; size];

        match parameter.kind {
            AcDerivativeKind::Resistance => {
                let SpiceElementKind::Resistor { resistance, .. } = &element.kind else {
                    return Err(format!("'{}' is not a resistor", parameter.object));
                };
                let effective = element_effective_resistance(circuit, element)?;
                let scale = effective / resistance;
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(-scale / (effective * effective), 0.0),
                );
                stamp_complex_conductance(
                    &mut second_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::new(2.0 * scale * scale / effective.powi(3), 0.0),
                );
            }
            AcDerivativeKind::Capacitance => {
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    j.scale(omega),
                );
            }
            AcDerivativeKind::Inductance => {
                let SpiceElementKind::Inductor { inductance, .. } = &element.kind else {
                    return Err(format!("'{}' is not an inductor", parameter.object));
                };
                let admittance = Complex::new(1.0, 0.0).div(j.scale(omega * inductance))?;
                stamp_complex_conductance(
                    &mut derivative_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance.scale(-1.0 / inductance),
                );
                stamp_complex_conductance(
                    &mut second_matrix,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    admittance.scale(2.0 / (inductance * inductance)),
                );
            }
            AcDerivativeKind::AcSource => match &element.kind {
                SpiceElementKind::VoltageSource { ac_phase_deg, .. } => {
                    let branch = system
                        .branch_index(&element.name)
                        .ok_or_else(|| format!("missing branch for source '{}'", element.name))?;
                    derivative_rhs[branch] = Complex::polar(1.0, (*ac_phase_deg).to_radians());
                }
                SpiceElementKind::CurrentSource { ac_phase_deg, .. } => stamp_complex_current(
                    &mut derivative_rhs,
                    &system,
                    &element.nodes[0],
                    &element.nodes[1],
                    Complex::polar(1.0, (*ac_phase_deg).to_radians()),
                ),
                _ => return Err(format!("'{}' is not an AC source", parameter.object)),
            },
        }

        let mut dx_rhs = derivative_rhs.clone();
        for row in 0..size {
            let product = derivative_matrix[row]
                .iter()
                .zip(&solution)
                .map(|(coefficient, value)| coefficient.mul(*value))
                .fold(Complex::zero(), |sum, value| sum.add(value));
            dx_rhs[row] = dx_rhs[row].sub(product);
        }
        let solution_derivative = solve_complex(matrix.clone(), dx_rhs)?;
        let gradient = conjugate(output).mul(solution_derivative[output_index]).re / nominal_output;
        first.push((derivative_matrix, second_matrix, solution_derivative));
        entries.push(DerivativeEntry {
            parameter: parameter.name.clone(),
            nominal_parameter: parameter.value,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }

    for (left, left_derivative) in first.iter().enumerate() {
        for (right, right_derivative) in first.iter().enumerate() {
            let mut second_rhs = vec![Complex::zero(); size];
            for (row, second_value) in second_rhs.iter_mut().enumerate() {
                let left_term = left_derivative.0[row]
                    .iter()
                    .zip(&right_derivative.2)
                    .map(|(coefficient, value)| coefficient.mul(*value))
                    .fold(Complex::zero(), |sum, value| sum.add(value));
                let right_term = right_derivative.0[row]
                    .iter()
                    .zip(&left_derivative.2)
                    .map(|(coefficient, value)| coefficient.mul(*value))
                    .fold(Complex::zero(), |sum, value| sum.add(value));
                *second_value = left_term.add(right_term).scale(-1.0);
                if left == right {
                    let second_term = left_derivative.1[row]
                        .iter()
                        .zip(&solution)
                        .map(|(coefficient, value)| coefficient.mul(*value))
                        .fold(Complex::zero(), |sum, value| sum.add(value));
                    *second_value = second_value.sub(second_term);
                }
            }
            let second_solution = solve_complex(matrix.clone(), second_rhs)?;
            let cross = conjugate(left_derivative.2[output_index])
                .mul(right_derivative.2[output_index])
                .re
                / nominal_output;
            let direct = conjugate(output).mul(second_solution[output_index]).re / nominal_output;
            let value =
                direct + cross - entries[left].gradient * entries[right].gradient / nominal_output;
            entries[left].hessian_row.push(value);
        }
    }

    if entries.iter().any(|entry| {
        !entry.gradient.is_finite() || entry.hessian_row.iter().any(|value| !value.is_finite())
    }) {
        return Err("AC derivative analysis produced a non-finite derivative".into());
    }
    Ok(DerivativeResult {
        target: DerivativeTarget::AcMagnitude,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output,
        entries,
    })
}

/// A scalar value carrying first and second derivatives with respect to four
/// independent variables. This small internal jet keeps nonlinear diode
/// depletion-capacitance derivatives algorithmic without external crates.
#[derive(Clone, Copy)]
struct SecondJet {
    value: f64,
    gradient: [f64; 4],
    hessian: [[f64; 4]; 4],
}

impl SecondJet {
    fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: [0.0; 4],
            hessian: [[0.0; 4]; 4],
        }
    }

    fn variable(index: usize, value: f64) -> Self {
        let mut gradient = [0.0; 4];
        gradient[index] = 1.0;
        Self {
            value,
            gradient,
            hessian: [[0.0; 4]; 4],
        }
    }

    fn add(&self, other: &Self) -> Self {
        let mut gradient = [0.0; 4];
        for (destination, (left, right)) in gradient
            .iter_mut()
            .zip(self.gradient.iter().zip(&other.gradient))
        {
            *destination = left + right;
        }
        let mut out = Self {
            value: self.value + other.value,
            gradient,
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = self.hessian[left][right] + other.hessian[left][right];
            }
        }
        out
    }

    fn neg(&self) -> Self {
        Self {
            value: -self.value,
            gradient: self.gradient.map(|value| -value),
            hessian: self.hessian.map(|row| row.map(|value| -value)),
        }
    }

    fn sub(&self, other: &Self) -> Self {
        self.add(&other.neg())
    }

    fn mul(&self, other: &Self) -> Self {
        let mut out = Self {
            value: self.value * other.value,
            gradient: [0.0; 4],
            hessian: [[0.0; 4]; 4],
        };
        for index in 0..4 {
            out.gradient[index] =
                self.gradient[index] * other.value + other.gradient[index] * self.value;
            for other_index in 0..4 {
                out.hessian[index][other_index] = self.hessian[index][other_index] * other.value
                    + other.hessian[index][other_index] * self.value
                    + self.gradient[index] * other.gradient[other_index]
                    + other.gradient[index] * self.gradient[other_index];
            }
        }
        out
    }

    fn div(&self, other: &Self) -> Self {
        self.mul(&other.inverse())
    }

    fn inverse(&self) -> Self {
        let value = 1.0 / self.value;
        let mut out = Self {
            value,
            gradient: self.gradient.map(|derivative| -derivative * value * value),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] =
                    2.0 * self.gradient[left] * self.gradient[right] * value.powi(3)
                        - self.hessian[left][right] * value * value;
            }
        }
        out
    }

    fn ln(&self) -> Self {
        let inverse = 1.0 / self.value;
        let mut out = Self {
            value: self.value.ln(),
            gradient: self.gradient.map(|derivative| derivative * inverse),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = self.hessian[left][right] * inverse
                    - self.gradient[left] * self.gradient[right] * inverse * inverse;
            }
        }
        out
    }

    fn exp(&self) -> Self {
        let value = self.value.exp();
        let mut out = Self {
            value,
            gradient: self.gradient.map(|derivative| derivative * value),
            hessian: [[0.0; 4]; 4],
        };
        for left in 0..4 {
            for right in 0..4 {
                out.hessian[left][right] = value
                    * (self.gradient[left] * self.gradient[right] + self.hessian[left][right]);
            }
        }
        out
    }
}

fn diode_depletion_shape_jet(voltage: f64, potential: f64, grading: f64, knee: f64) -> SecondJet {
    let voltage_jet = SecondJet::variable(0, voltage);
    let potential_jet = SecondJet::variable(1, potential);
    let grading_jet = SecondJet::variable(2, grading);
    let knee_jet = SecondJet::variable(3, knee);
    if voltage <= knee * potential {
        let denominator = SecondJet::constant(1.0).sub(&voltage_jet.div(&potential_jet));
        let clamped = if denominator.value < 0.01 {
            SecondJet::constant(0.01)
        } else {
            denominator
        };
        let exponent = clamped.ln().mul(&grading_jet.neg());
        exponent.exp()
    } else {
        let knee_factor = SecondJet::constant(1.0).sub(&knee_jet);
        let squared_factor = knee_factor.mul(&knee_factor);
        let knee_cap = knee_factor.ln().mul(&grading_jet.neg()).exp();
        let numerator = voltage_jet.sub(&knee_jet.mul(&potential_jet));
        let normalized_difference = numerator.div(&potential_jet.mul(&squared_factor));
        let bracket = SecondJet::constant(1.0).add(&grading_jet.mul(&normalized_difference));
        knee_cap.mul(&bracket)
    }
}

fn nonlinear_diode_ac_derivative_analysis(
    circuit: &SpiceCircuit,
    options: &DerivativeOptions,
) -> Result<DerivativeResult, String> {
    let resistor = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Resistor { .. }))
        .ok_or_else(|| "nonlinear AC derivative needs a series resistor".to_string())?;
    let diode = circuit
        .elements
        .iter()
        .find(|element| matches!(element.kind, SpiceElementKind::Diode { .. }))
        .ok_or_else(|| "nonlinear AC derivative needs a diode".to_string())?;
    let source = circuit
        .elements
        .iter()
        .find(|element| {
            matches!(
                &element.kind,
                SpiceElementKind::VoltageSource {
                    waveform: SourceWaveform::Dc(_),
                    ac_magnitude: Some(_),
                    ..
                }
            )
        })
        .ok_or_else(|| "nonlinear AC derivative needs a DC/AC source".to_string())?;
    if circuit.elements.len() != 3 {
        return Err("nonlinear AC derivatives support exactly R, diode, and V".into());
    }
    let SpiceElementKind::Resistor {
        resistance,
        first_temperature_coefficient_per_k,
        second_temperature_coefficient_per_k2,
        sheet_resistance_ohm_per_square,
        layer,
        ..
    } = &resistor.kind
    else {
        unreachable!()
    };
    if layer.is_some()
        || *sheet_resistance_ohm_per_square > 0.0
        || *first_temperature_coefficient_per_k != 0.0
        || *second_temperature_coefficient_per_k2 != 0.0
    {
        return Err(
            "nonlinear AC R derivatives support an ideal temperature-neutral resistor".into(),
        );
    }
    let SpiceElementKind::Diode { model: model_name } = &diode.kind else {
        unreachable!()
    };
    let model = circuit
        .diode_models
        .get(model_name)
        .ok_or_else(|| format!("unknown diode model '{model_name}'"))?;
    if model.series_resistance_ohm > 0.0 {
        return Err("nonlinear AC diode derivatives require RS=0".into());
    }
    let SpiceElementKind::VoltageSource {
        waveform: SourceWaveform::Dc(dc_value),
        ac_magnitude: Some(ac_magnitude),
        ..
    } = &source.kind
    else {
        unreachable!()
    };
    let input = &source.nodes[0];
    let output = &diode.nodes[0];
    if !GROUND_ALIASES.contains(&source.nodes[1].as_str())
        || !GROUND_ALIASES.contains(&diode.nodes[1].as_str())
        || resistor.nodes != [input.as_str(), output.as_str()]
        || output != &options.output_node
    {
        return Err("nonlinear AC derivatives require V--R--V(out),D(out--ground) topology".into());
    }

    let operating_point = dc_operating_point(circuit)?;
    let diode_voltage = operating_point
        .node_voltages
        .get(output)
        .copied()
        .ok_or_else(|| format!("missing nonlinear AC node '{output}'"))?;
    let current = (*dc_value - diode_voltage) / resistance;
    let thermal_voltage = thermal_voltage_at(circuit.temperature_k, model.emission_coefficient);
    let temperature_scale = temperature_scaled_saturation_current(
        model.saturation_current_a,
        model.emission_coefficient,
        model.energy_gap_ev,
        model.saturation_current_temperature_exponent,
        circuit.temperature_k,
    ) / model.saturation_current_a;
    let saturation_current = temperature_scale * model.saturation_current_a;
    let dynamic_conductance = (current + saturation_current) / thermal_voltage + GMIN;
    if dynamic_conductance <= GMIN {
        return Err("nonlinear AC derivative requires a forward-biased diode".into());
    }

    // Scalar implicit derivatives of F(I,p) = R*I + VT*ln((I+IS)/IS) - VDC.
    let f_current = resistance + thermal_voltage / current;
    let current_first_base = [
        -current / f_current,
        -temperature_scale
            * thermal_voltage
            * (1.0 / (current + saturation_current) - 1.0 / saturation_current)
            / f_current,
        1.0 / f_current,
    ];
    let current_second_current = -thermal_voltage / (current * current);
    let mut current_second_base = [[0.0; 3]; 3];
    for left in 0..3 {
        for right in 0..3 {
            let mixed_current = match (left, right) {
                (0, 0) => 2.0 * current_first_base[0],
                (0, 1) | (1, 0) => current_first_base[1],
                (0, 2) | (2, 0) => current_first_base[2],
                _ => 0.0,
            };
            let direct = if (left, right) == (1, 1) {
                temperature_scale
                    * temperature_scale
                    * thermal_voltage
                    * (1.0 / (saturation_current * saturation_current)
                        - 1.0 / ((current + saturation_current) * (current + saturation_current)))
            } else {
                0.0
            };
            current_second_base[left][right] = -(mixed_current
                + current_second_current * current_first_base[left] * current_first_base[right]
                + direct)
                / f_current;
        }
    }

    // Physical parameter order: R, IS, VDC, TT, CJO, VJ, M, FC.
    // AC source magnitude is linear and handled as parameter index 8.
    let charge_first = [
        current_first_base[0],
        current_first_base[1] + temperature_scale,
        current_first_base[2],
    ];
    let mut conductance_first = [0.0; 8];
    conductance_first[..3].copy_from_slice(&charge_first.map(|value| value / thermal_voltage));
    let mut conductance_second = [[0.0; 8]; 8];
    for left in 0..3 {
        for right in 0..3 {
            conductance_second[left][right] = current_second_base[left][right] / thermal_voltage;
        }
    }
    let diffusion_capacitance =
        model.transit_time_s * (current + saturation_current) / thermal_voltage;
    let mut capacitance_first = [0.0; 8];
    let mut capacitance_second = [[0.0; 8]; 8];
    for left in 0..3 {
        capacitance_first[left] = model.transit_time_s * charge_first[left] / thermal_voltage;
        for right in 0..3 {
            capacitance_second[left][right] =
                model.transit_time_s * current_second_base[left][right] / thermal_voltage;
        }
        capacitance_second[3][left] = charge_first[left] / thermal_voltage;
        capacitance_second[left][3] = charge_first[left] / thermal_voltage;
    }
    capacitance_first[3] = (current + saturation_current) / thermal_voltage;

    // Depletion capacitance derivatives. The terminal voltage is
    // Vd=VDC-R*I. VJ/M/FC do not affect the DC solution; CJO is a direct
    // multiplier. A second-order jet over [Vd,VJ,M,FC] avoids long manual
    // derivatives of the depletion equation and its FC knee continuation.
    let mut voltage_first = [0.0; 8];
    voltage_first[0] = -current - resistance * current_first_base[0];
    voltage_first[1] = -resistance * current_first_base[1];
    voltage_first[2] = 1.0 - resistance * current_first_base[2];
    let mut voltage_second = [[0.0; 8]; 8];
    voltage_second[0][0] = -2.0 * current_first_base[0] - resistance * current_second_base[0][0];
    voltage_second[0][1] = -current_first_base[1] - resistance * current_second_base[0][1];
    voltage_second[0][2] = -current_first_base[2] - resistance * current_second_base[0][2];
    voltage_second[1][0] = voltage_second[0][1];
    voltage_second[1][1] = -resistance * current_second_base[1][1];
    voltage_second[1][2] = -resistance * current_second_base[1][2];
    voltage_second[2][0] = voltage_second[0][2];
    voltage_second[2][1] = voltage_second[1][2];
    voltage_second[2][2] = -resistance * current_second_base[2][2];

    let shape = diode_depletion_shape_jet(
        diode_voltage,
        model.junction_potential_v,
        model.grading_coefficient,
        model.forward_knee,
    );
    let shape_gradient = [
        shape.gradient[0],
        shape.gradient[1],
        shape.gradient[2],
        shape.gradient[3],
    ];
    // T maps a physical-parameter perturbation to [Vd,VJ,M,FC].
    let mut composition = [[0.0; 4]; 8];
    for parameter in 0..3 {
        composition[parameter][0] = voltage_first[parameter];
    }
    composition[4] = [0.0, 0.0, 0.0, 0.0];
    composition[5] = [0.0, 1.0, 0.0, 0.0];
    composition[6] = [0.0, 0.0, 1.0, 0.0];
    composition[7] = [0.0, 0.0, 0.0, 1.0];
    let depletion_gradient = |parameter: usize| {
        let mut value = 0.0;
        for (index, coefficient) in composition[parameter].iter().enumerate() {
            value += coefficient * shape_gradient[index];
        }
        value
    };
    let depletion_second = |left: usize, right: usize| {
        let mut value = 0.0;
        for row in 0..4 {
            for column in 0..4 {
                value += composition[left][row]
                    * shape.hessian[row][column]
                    * composition[right][column];
            }
        }
        if left < 3 && right < 3 {
            value += shape_gradient[0] * voltage_second[left][right];
        }
        value
    };
    let depletion_capacitance = model.zero_bias_junction_cap_f * shape.value;
    for parameter in 0..8 {
        capacitance_first[parameter] += if parameter == 4 {
            shape.value
        } else {
            model.zero_bias_junction_cap_f * depletion_gradient(parameter)
        };
        for (other, second_value) in capacitance_second[parameter].iter_mut().enumerate() {
            let mut value = model.zero_bias_junction_cap_f * depletion_second(parameter, other);
            if parameter == 4 || other == 4 {
                let multiplier_parameter = if parameter == 4 { other } else { parameter };
                value += depletion_gradient(multiplier_parameter);
            }
            *second_value += value;
        }
    }

    // Source-R/diode small-signal transfer magnitude.
    let real_part = 1.0 + resistance * dynamic_conductance;
    let omega = std::f64::consts::TAU * options.frequency_hz;
    let total_capacitance = diffusion_capacitance + depletion_capacitance;
    let imaginary_part = omega * resistance * total_capacitance;
    let mut real_first = [0.0; 8];
    let mut imaginary_first = [0.0; 8];
    let mut real_second = [[0.0; 8]; 8];
    let mut imaginary_second = [[0.0; 8]; 8];
    for left in 0..8 {
        real_first[left] = resistance * conductance_first[left];
        imaginary_first[left] = omega * resistance * capacitance_first[left];
        if left == 0 {
            real_first[left] += dynamic_conductance;
            imaginary_first[left] += omega * total_capacitance;
        }
        for right in 0..8 {
            real_second[left][right] = resistance * conductance_second[left][right];
            imaginary_second[left][right] = omega * resistance * capacitance_second[left][right];
            if left == 0 {
                real_second[left][right] += conductance_first[right];
                imaginary_second[left][right] += omega * capacitance_first[right];
            }
            if right == 0 {
                real_second[left][right] += conductance_first[left];
                imaginary_second[left][right] += omega * capacitance_first[left];
            }
        }
    }

    let denominator = real_part * real_part + imaginary_part * imaginary_part;
    let unit_magnitude = 1.0 / denominator.sqrt();
    let mut numerator_first = [0.0; 8];
    for index in 0..8 {
        numerator_first[index] =
            real_part * real_first[index] + imaginary_part * imaginary_first[index];
    }
    let mut unit_first = [0.0; 8];
    for index in 0..8 {
        unit_first[index] = -numerator_first[index] * unit_magnitude / denominator;
    }
    let mut unit_second = [[0.0; 8]; 8];
    for left in 0..8 {
        for right in 0..8 {
            let numerator_second = real_first[left] * real_first[right]
                + imaginary_first[left] * imaginary_first[right]
                + real_part * real_second[left][right]
                + imaginary_part * imaginary_second[left][right];
            unit_second[left][right] = -numerator_second * unit_magnitude / denominator
                + 3.0 * numerator_first[left] * numerator_first[right] * unit_magnitude
                    / (denominator * denominator);
        }
    }

    let parameter_info = |name: &str| -> Option<(usize, f64, String)> {
        if name.eq_ignore_ascii_case(&format!("{}.R", resistor.name)) {
            Some((0, *resistance, format!("{}.R", resistor.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.IS")) {
            Some((1, model.saturation_current_a, format!("{model_name}.IS")))
        } else if name.eq_ignore_ascii_case(&format!("{}.DC", source.name)) {
            Some((2, *dc_value, format!("{}.DC", source.name)))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.TT")) {
            Some((3, model.transit_time_s, format!("{model_name}.TT")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.CJO")) {
            Some((
                4,
                model.zero_bias_junction_cap_f,
                format!("{model_name}.CJO"),
            ))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.VJ")) {
            Some((5, model.junction_potential_v, format!("{model_name}.VJ")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.M")) {
            Some((6, model.grading_coefficient, format!("{model_name}.M")))
        } else if name.eq_ignore_ascii_case(&format!("{model_name}.FC")) {
            Some((7, model.forward_knee, format!("{model_name}.FC")))
        } else if name.eq_ignore_ascii_case(&format!("{}.AC", source.name)) {
            Some((8, *ac_magnitude, format!("{}.AC", source.name)))
        } else {
            None
        }
    };

    let mut entries = Vec::with_capacity(options.parameters.len());
    for name in &options.parameters {
        let Some((index, nominal, parameter)) = parameter_info(name) else {
            return Err(format!(
                "nonlinear AC derivative parameter '{name}' must be R, diode IS/TT/CJO/VJ/M/FC, DC, or AC magnitude"
            ));
        };
        let gradient = if index == 8 {
            unit_magnitude
        } else {
            ac_magnitude * unit_first[index]
        };
        entries.push(DerivativeEntry {
            parameter,
            nominal_parameter: nominal,
            gradient,
            hessian_row: Vec::with_capacity(options.parameters.len()),
        });
    }
    for entry in &mut entries {
        let Some((left, _, _)) = parameter_info(&entry.parameter) else {
            unreachable!("parameter was validated above");
        };
        for parameter in &options.parameters {
            let Some((right, _, _)) = parameter_info(parameter) else {
                unreachable!("parameter was validated above");
            };
            let value = if left == 8 && right == 8 {
                0.0
            } else if left == 8 {
                unit_first[right]
            } else if right == 8 {
                unit_first[left]
            } else {
                ac_magnitude * unit_second[left][right]
            };
            entry.hessian_row.push(value);
        }
    }

    Ok(DerivativeResult {
        target: DerivativeTarget::AcMagnitude,
        frequency_hz: options.frequency_hz,
        output_node: options.output_node.clone(),
        nominal_output: ac_magnitude * unit_magnitude,
        entries,
    })
}
