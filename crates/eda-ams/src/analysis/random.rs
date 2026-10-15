//! Deterministic RNG, correlated sampling, and circuit mutation helpers.

use super::derivatives::{perturbed_for_sensitivity, SensitivityParameter};
use super::statistical::{monte_carlo_relative_multiplier, monte_carlo_standard_factor};
use crate::types::*;

pub(super) fn random_factor_matrix(columns: usize, rows: usize, rng: &mut McRng) -> Vec<Vec<f64>> {
    vec![vec![0.0; columns]; rows]
        .into_iter()
        .map(|row| row.into_iter().map(|_| rng.symmetric()).collect())
        .collect()
}

pub(super) fn global_correlation_matrix(
    parameters: &[SensitivityParameter],
    correlations: &[ParameterCorrelation],
) -> Result<Vec<Vec<f64>>, String> {
    let len = parameters.len();
    let mut matrix = vec![vec![0.0; len]; len];
    for (row, values) in matrix.iter_mut().enumerate() {
        values[row] = 1.0;
    }
    let index = |name: &str| {
        parameters
            .iter()
            .position(|parameter| parameter.name.eq_ignore_ascii_case(name))
    };
    for correlation in correlations {
        let left = index(&correlation.left)
            .ok_or_else(|| format!("unknown Sobol parameter '{}'", correlation.left))?;
        let right = index(&correlation.right)
            .ok_or_else(|| format!("unknown Sobol parameter '{}'", correlation.right))?;
        if left == right {
            return Err("a Sobol parameter cannot be correlated with itself".into());
        }
        if matrix[left][right] != 0.0
            && (matrix[left][right] - correlation.coefficient).abs() > f64::EPSILON
        {
            return Err(format!(
                "conflicting Sobol correlations for '{}' and '{}'",
                correlation.left, correlation.right
            ));
        }
        matrix[left][right] = correlation.coefficient;
        matrix[right][left] = correlation.coefficient;
    }
    cholesky_correlation(&matrix)?;
    Ok(matrix)
}

pub(super) fn cholesky_correlation(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let len = matrix.len();
    let mut lower = vec![vec![0.0; len]; len];
    for row in 0..len {
        for column in 0..=row {
            let mut sum = matrix[row][column];
            for (row_value, column_value) in
                lower[row].iter().zip(lower[column].iter()).take(column)
            {
                sum -= row_value * column_value;
            }
            if row == column {
                if sum < -1e-12 {
                    return Err("Sobol correlation matrix is not positive definite".into());
                }
                lower[row][column] = sum.max(0.0).sqrt();
            } else {
                let pivot = lower[column][column];
                if pivot <= 1e-12 {
                    if sum.abs() > 1e-12 {
                        return Err("Sobol correlation matrix is not positive definite".into());
                    }
                    lower[row][column] = 0.0;
                } else {
                    lower[row][column] = sum / pivot;
                }
            }
        }
    }
    Ok(lower)
}

pub(super) fn cholesky_covariance(matrix: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let len = matrix.len();
    let mut lower = vec![vec![0.0; len]; len];
    for row in 0..len {
        for column in 0..=row {
            let mut sum = matrix[row][column];
            for (row_value, column_value) in
                lower[row].iter().zip(lower[column].iter()).take(column)
            {
                sum -= row_value * column_value;
            }
            if row == column {
                if sum <= 1e-15 {
                    return Err("Monte Carlo covariance matrix is not positive definite".into());
                }
                lower[row][column] = sum.sqrt();
            } else {
                let pivot = lower[column][column];
                if pivot <= 1e-15 {
                    return Err("Monte Carlo covariance matrix is not positive definite".into());
                }
                lower[row][column] = sum / pivot;
            }
        }
    }
    Ok(lower)
}

pub(super) fn correlated_factor_matrix(
    rows: usize,
    correlation: &[Vec<f64>],
    rng: &mut McRng,
) -> Result<Vec<Vec<f64>>, String> {
    let lower = cholesky_correlation(correlation)?;
    let mut out = Vec::with_capacity(rows);
    for _ in 0..rows {
        out.push(correlated_factor_row(&lower, rng));
    }
    Ok(out)
}

pub(super) fn correlated_factor_row(lower: &[Vec<f64>], rng: &mut McRng) -> Vec<f64> {
    let columns = lower.len();
    let independent: Vec<f64> = (0..columns).map(|_| standard_normal(rng)).collect();
    (0..columns)
        .map(|column| {
            (0..=column)
                .map(|source| lower[column][source] * independent[source])
                .sum::<f64>()
        })
        .map(|value| 2.0 * normal_cdf(value) - 1.0)
        .collect()
}

pub(super) fn standard_normal(rng: &mut McRng) -> f64 {
    let u1 = rng.unit().max(f64::MIN_POSITIVE);
    let u2 = rng.unit();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

pub(super) fn normal_cdf(value: f64) -> f64 {
    0.5 * (1.0 + erf_approx(value / std::f64::consts::SQRT_2))
}

pub(super) fn normal_inverse_cdf(probability: f64) -> f64 {
    if !(0.0..=1.0).contains(&probability) || !probability.is_finite() {
        return f64::NAN;
    }
    if probability == 0.0 {
        return f64::NEG_INFINITY;
    }
    if probability == 1.0 {
        return f64::INFINITY;
    }
    let mut low = -8.0;
    let mut high = 8.0;
    for _ in 0..64 {
        let middle = (low + high) / 2.0;
        if normal_cdf(middle) < probability {
            low = middle;
        } else {
            high = middle;
        }
    }
    (low + high) / 2.0
}

pub(super) fn erf_approx(value: f64) -> f64 {
    // Abramowitz-Stegun 7.1.26 provides a compact, dependency-free erf
    // approximation with approximately 1.5e-7 absolute error.
    let absolute = value.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * absolute);
    let polynomial = 0.254_829_592 * t - 0.284_496_736 * t.powi(2) + 1.421_413_741 * t.powi(3)
        - 1.453_152_027 * t.powi(4)
        + 1.061_405_429 * t.powi(5);
    let approximation = 1.0 - polynomial * (-absolute * absolute).exp();
    if value >= 0.0 {
        approximation
    } else {
        -approximation
    }
}

pub(super) fn global_variant(
    circuit: &SpiceCircuit,
    parameters: &[SensitivityParameter],
    deltas: &[f64],
) -> Result<SpiceCircuit, String> {
    let mut out = circuit.clone();
    for (parameter, delta) in parameters.iter().zip(deltas) {
        if delta.abs() > f64::EPSILON {
            out = perturbed_for_sensitivity(&out, parameter, *delta)?;
        }
    }
    Ok(out)
}

pub(super) fn sample_variance(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    values
        .iter()
        .map(|value| {
            let delta = value - mean;
            delta * delta
        })
        .sum::<f64>()
        / values.len() as f64
}

pub(super) struct McRng {
    state: u64,
}

impl McRng {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(6364136223846793005).max(1),
        }
    }

    pub(super) fn unit(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let value = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / ((1u64 << 53) as f64)
    }

    fn symmetric(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

pub(super) fn randomized_circuit(
    circuit: &SpiceCircuit,
    options: &MonteCarloOptions,
    rng: &mut McRng,
) -> Result<(SpiceCircuit, usize), String> {
    let mut out = circuit.clone();
    let mut count = 0usize;
    let resistor_global = if options.resistor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let capacitor_global = if options.capacitor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let inductor_global = if options.inductor_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let diode_is_global = if options.diode_is_decades > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let mos_vto_global = if options.mos_vto_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let mos_kp_global = if options.mos_kp_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let bjt_beta_global = if options.bjt_beta_tolerance > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    let bjt_is_global = if options.bjt_is_decades > 0.0 {
        rng.symmetric()
    } else {
        0.0
    };
    for element in &mut out.elements {
        match &mut element.kind {
            SpiceElementKind::Resistor { resistance, .. } => {
                if options.resistor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(resistor_global, options.resistor_correlation, rng);
                    *resistance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.resistor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            SpiceElementKind::Capacitor { capacitance, .. } => {
                if options.capacitor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(capacitor_global, options.capacitor_correlation, rng);
                    *capacitance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.capacitor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            SpiceElementKind::Inductor { inductance, .. } => {
                if options.inductor_tolerance > 0.0 {
                    let sample =
                        correlated_sample(inductor_global, options.inductor_correlation, rng);
                    *inductance *= 1.0
                        + monte_carlo_relative_multiplier(
                            sample,
                            options.inductor_tolerance,
                            options.distribution,
                        );
                    count += 1;
                }
            }
            _ => {}
        }
    }
    for model in out.diode_models.values_mut() {
        if options.diode_is_decades > 0.0 {
            let sample = correlated_sample(diode_is_global, options.diode_is_correlation, rng);
            let exponent = model.saturation_current_a.log10()
                + options.diode_is_decades
                    * monte_carlo_standard_factor(sample, options.distribution);
            model.saturation_current_a = 10f64.powf(exponent);
            count += 1;
        }
    }
    for model in out.mosfet_models.values_mut() {
        if options.mos_vto_tolerance > 0.0 {
            let sample = correlated_sample(mos_vto_global, options.mos_vto_correlation, rng);
            model.threshold_voltage_v += options.mos_vto_tolerance
                * monte_carlo_standard_factor(sample, options.distribution);
            count += 1;
        }
        if options.mos_kp_tolerance > 0.0 {
            let sample = correlated_sample(mos_kp_global, options.mos_kp_correlation, rng);
            model.transconductance_a_per_v2 *= 1.0
                + monte_carlo_relative_multiplier(
                    sample,
                    options.mos_kp_tolerance,
                    options.distribution,
                );
            count += 1;
        }
    }
    for model in out.bjt_models.values_mut() {
        if options.bjt_beta_tolerance > 0.0 {
            let sample = correlated_sample(bjt_beta_global, options.bjt_beta_correlation, rng);
            model.forward_beta *= 1.0
                + monte_carlo_relative_multiplier(
                    sample,
                    options.bjt_beta_tolerance,
                    options.distribution,
                );
            count += 1;
        }
        if options.bjt_is_decades > 0.0 {
            let sample = correlated_sample(bjt_is_global, options.bjt_is_correlation, rng);
            let exponent = model.saturation_current_a.log10()
                + options.bjt_is_decades
                    * monte_carlo_standard_factor(sample, options.distribution);
            model.saturation_current_a = 10f64.powf(exponent);
            count += 1;
        }
    }
    out.validate()?;
    Ok((out, count))
}

pub(super) fn correlated_sample(global: f64, correlation: f64, rng: &mut McRng) -> f64 {
    let local_weight = (1.0 - correlation * correlation).max(0.0).sqrt();
    correlation * global + local_weight * rng.symmetric()
}

pub(super) fn monte_carlo_statistics(samples: &[MonteCarloSample]) -> MonteCarloStatistics {
    if samples.is_empty() {
        return MonteCarloStatistics::default();
    }
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for sample in samples {
        minimum = minimum.min(sample.value);
        maximum = maximum.max(sample.value);
        sum += sample.value;
    }
    let mean = sum / samples.len() as f64;
    let variance = samples
        .iter()
        .map(|sample| {
            let delta = sample.value - mean;
            delta * delta
        })
        .sum::<f64>()
        / samples.len() as f64;
    MonteCarloStatistics {
        minimum,
        maximum,
        mean,
        standard_deviation: variance.sqrt(),
    }
}
