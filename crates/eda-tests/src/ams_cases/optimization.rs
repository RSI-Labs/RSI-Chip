//! Generated as part of an AMS integration-case split; domains are grouped below.

use crate::CaseList;
use eda_api::*;

fn optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let r1 = 500.0 + idx as f64 * 47.0;
        let target = 0.6 + (idx % 6) as f64 * 0.1;
        let supply = 1.6 + (idx % 5) as f64 * 0.1;
        let expected_r2 = r1 * target / (supply - target);
        let lower = (expected_r2 * 0.2).max(10.0);
        let upper = expected_r2 * 5.0;
        let source = format!(
            "optim divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {lower}\n.optim match V(out) goal={target} params=R2.R:{lower}:{upper} population=8 iterations=128 seed={} tol=1e-7\n.end",
            300 + idx
        );
        cases.add(format!("ams_optimization_divider_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = optimize(&circuit, &circuit.analyses[0])?;
            let final_r2 = result
                .parameters
                .iter()
                .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
                .ok_or_else(|| "missing optimized R2".to_string())?
                .final_value;
            if !result.converged
                || (result.final_output - target).abs() > 1e-6
                || (final_r2 - expected_r2).abs() > expected_r2 * 0.01
            {
                Err(format!(
                    "target={target}, expected_r2={expected_r2}, result={result:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn quantized_optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        if idx % 2 == 0 {
            let quanta = [10.0, 22.0, 47.0, 100.0, 220.0];
            let quantum = quanta[idx / 2 % quanta.len()];
            let r1 = 500.0 + idx as f64 * 47.0;
            let supply = 1.6 + (idx % 5) as f64 * 0.1;
            let desired = quantum * (40.0 + idx as f64 * 5.0);
            let lower = quantum * 8.0;
            let upper = desired + quantum * (12.0 + (idx % 9) as f64);
            let target = supply * desired / (r1 + desired);
            let source = format!(
                "quant divider {idx}\nVin in 0 {supply}\nR1 in out {r1}\nR2 out 0 {lower}\n.optim match V(out) goal={target} params=R2.R:{lower}:{upper}:{lower}:{quantum} population=8 iterations=32 seed={} tol=1e-9\n.end",
                700 + idx
            );
            cases.add(
                format!("ams_quantized_optimization_divider_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    let result = optimize(&circuit, &circuit.analyses[0])?;
                    let parameter = result
                        .parameters
                        .iter()
                        .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
                        .ok_or_else(|| "missing optimized R2".to_string())?;
                    let residual = (parameter.final_value - lower)
                        - ((parameter.final_value - lower) / quantum).round() * quantum;
                    if !result.converged
                        || (result.final_output - target).abs() > 1e-8
                        || (parameter.final_value - desired).abs() > 1e-7
                        || residual.abs() > 1e-7
                        || parameter.final_value < lower
                        || parameter.final_value > upper
                    {
                        Err(format!(
                        "quantum={quantum}, desired={desired}, target={target}, result={result:?}"
                    ))
                    } else {
                        Ok(())
                    }
                },
            );
        } else {
            let quantum = 1e-12 * (10.0 + (idx % 5) as f64 * 5.0);
            let resistance = 1_000.0 + (idx % 7) as f64 * 500.0;
            let frequency = 100_000.0 + (idx % 11) as f64 * 50_000.0;
            let desired = quantum * (80.0 + idx as f64 * 6.0);
            let lower = quantum * 10.0;
            let upper = desired + quantum * (12.0 + (idx % 8) as f64);
            let omega = std::f64::consts::TAU * frequency;
            let target = 1.0 / (1.0 + (omega * resistance * desired).powi(2)).sqrt();
            let source = format!(
                "quant cap {idx}\nVin in 0 DC 0 AC 1\nR1 in out {resistance}\nC1 out 0 {lower}\n.ac lin 2 {frequency} {frequency}\n.optim match ac V(out) freq={frequency} goal={target} params=C1.C:{lower}:{upper}:{lower}:{quantum} population=8 iterations=32 seed={} tol=1e-9\n.end",
                800 + idx
            );
            cases.add(
                format!("ams_quantized_optimization_capacitor_{idx}"),
                move || {
                    let circuit = parse_spice(&source)?;
                    let result = optimize(&circuit, &circuit.analyses[1])?;
                    let parameter = result
                        .parameters
                        .iter()
                        .find(|parameter| parameter.parameter.eq_ignore_ascii_case("C1.C"))
                        .ok_or_else(|| "missing optimized capacitor".to_string())?;
                    let residual = (parameter.final_value - lower)
                        - ((parameter.final_value - lower) / quantum).round() * quantum;
                    if !result.converged
                        || (result.final_output - target).abs() > 1e-8
                        || (parameter.final_value - desired).abs() > quantum * 1e-7
                        || residual.abs() > quantum * 1e-7
                        || parameter.final_value < lower
                        || parameter.final_value > upper
                    {
                        Err(format!(
                            "quantum={quantum}, desired={desired}, target={target}, result={result:?}"
                        ))
                    } else {
                        Ok(())
                    }
                },
            );
        }
    }
}

fn pareto_optimization_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let middle_resistance = 700.0 + (idx % 9) as f64 * 110.0;
        let lower = 450.0 + (idx % 7) as f64 * 40.0;
        let upper = 4_200.0 + (idx % 11) as f64 * 180.0;
        let source = format!(
            "pareto ladder {idx}\nVin in 0 {supply}\nR1 in a {middle_resistance}\nR2 a b {middle_resistance}\nR3 b 0 {middle_resistance}\n.pareto V(a) V(b) sense=min,max params=R1.R:{lower}:{upper},R3.R:{lower}:{upper} population=12 iterations=24 seed={} points=10\n.end",
            2100 + idx
        );
        cases.add(format!("ams_pareto_optimization_ladder_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 10
                || !result.is_nondominated()
                || result.evaluations != 12 + 12 * 24
            {
                return Err(format!("{result:?}"));
            }
            for pair in result.points.windows(2) {
                if pair[0].outputs[0] > pair[1].outputs[0]
                    || pair[0].outputs[1] > pair[1].outputs[1]
                {
                    return Err(format!("front is not sorted: {result:?}"));
                }
            }
            if result.points.last().unwrap().outputs[0] <= result.points.first().unwrap().outputs[0]
                || result.points.last().unwrap().outputs[1]
                    <= result.points.first().unwrap().outputs[1]
            {
                return Err(format!("front has no measurable tradeoff: {result:?}"));
            }
            for point in &result.points {
                for parameter in &point.parameters {
                    if parameter.final_value < lower || parameter.final_value > upper {
                        return Err(format!("parameter outside bounds: {point:?}"));
                    }
                }
            }
            Ok(())
        });
    }
}

fn constrained_pareto_cases(cases: &mut CaseList) {
    for idx in 0..25 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let middle = 700.0 + (idx % 9) as f64 * 110.0;
        let lower = 450.0 + (idx % 7) as f64 * 40.0;
        let upper = 4_200.0 + (idx % 11) as f64 * 180.0;
        let minimum_b = supply * lower / (middle + lower);
        let maximum_b = supply * upper / (middle + upper);
        let fraction = 0.25 + (idx % 5) as f64 * 0.12;
        let threshold = minimum_b + fraction * (maximum_b - minimum_b);
        let source = format!(
            "constrained pareto {idx}\nVin in 0 {supply}\nR1 in a {middle}\nR2 a b {middle}\nR3 b 0 {middle}\n.pareto V(a) V(b) sense=min,max constraints=V(b)>={threshold} params=R1.R:{lower}:{upper},R3.R:{lower}:{upper} population=12 iterations=24 seed={} points=8\n.end",
            2400 + idx
        );
        cases.add(format!("ams_constrained_pareto_ladder_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 8
                || !result.is_nondominated()
                || result.constraints.len() != 1
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                if !point.constraints_satisfied
                    || point.constraint_violation > 1e-10
                    || point.outputs[1] + 1e-9 < threshold
                {
                    return Err(format!(
                        "threshold={threshold}, constraint_violation={}, outputs={:?}",
                        point.constraint_violation, point.outputs
                    ));
                }
            }
            Ok(())
        });
    }
}

fn algebraic_constraint_pareto_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 8) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 190.0;
        let expression_at = |r1: f64, r4: f64| {
            let total = r1 + series + series + series + r4;
            let va = supply * (series + series + r4) / total;
            let vb = supply * (series + r4) / total;
            let vc = supply * r4 / total;
            2.0 * va + vb - 0.5 * vc
        };
        let minimum = expression_at(upper, lower);
        let maximum = expression_at(lower, upper);
        let fraction = 0.55 + (idx % 5) as f64 * 0.08;
        let threshold = minimum + fraction * (maximum - minimum);
        let source = format!(
            "algebraic pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(2*V(a)+V(b)-0.5*V(c))<={threshold} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2700 + idx
        );
        cases.add(
            format!("ams_algebraic_constraint_pareto_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
                if result.points.len() < 2
                    || result.points.len() > 6
                    || !result.is_nondominated()
                    || result.constraints.len() != 1
                {
                    return Err(format!("{result:?}"));
                }
                for point in &result.points {
                    let value = 2.0 * point.outputs[0] + point.outputs[1] - 0.5 * point.outputs[2];
                    if !point.constraints_satisfied
                        || point.constraint_violation > 1e-10
                        || value > threshold + 1e-9
                    {
                        return Err(format!(
                            "threshold={threshold}, value={value}, violation={}, point={point:?}",
                            point.constraint_violation
                        ));
                    }
                }
                Ok(())
            },
        );
    }
}

fn function_constraint_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 8) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 190.0;
        let expression_at = |r1: f64, r4: f64| {
            let total = r1 + 3.0 * series + r4;
            let vb = supply * (series + r4) / total;
            let vc = supply * r4 / total;
            vb + 0.1 * (1.0 + vc).ln()
        };
        let minimum = expression_at(upper, lower);
        let maximum = expression_at(lower, upper);
        let fraction = 0.35 + (idx % 5) as f64 * 0.1;
        let threshold = minimum + fraction * (maximum - minimum);
        let source = format!(
            "function constraint pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(min(V(a),V(b))+0.1*ln(1+V(c)))>={threshold} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2800 + idx
        );
        cases.add(format!("ams_function_constraint_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.points.len() < 2
                || result.points.len() > 6
                || !result.is_nondominated()
                || result.constraints.len() != 1
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let value = point.outputs[1] + 0.1 * (1.0 + point.outputs[2]).ln();
                if !point.constraints_satisfied
                    || point.constraint_violation > 1e-10
                    || value + 1e-9 < threshold
                {
                    return Err(format!(
                        "threshold={threshold}, value={value}, violation={}, point={point:?}",
                        point.constraint_violation
                    ));
                }
            }
            Ok(())
        });
    }
}

fn multi_objective_pareto_cases(cases: &mut CaseList) {
    for idx in 0..15 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 700.0 + (idx % 8) as f64 * 120.0;
        let lower = 500.0 + (idx % 6) as f64 * 50.0;
        let upper = 4_000.0 + (idx % 9) as f64 * 200.0;
        let source = format!(
            "three-objective pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2500 + idx
        );
        cases.add(
            format!("ams_three_objective_pareto_ladder_{idx}"),
            move || {
                let circuit = parse_spice(&source)?;
                let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
                if result.output_nodes.len() != 3
                    || result.senses.len() != 3
                    || result.points.len() < 2
                    || result.points.len() > 6
                    || !result.is_nondominated()
                {
                    return Err(format!("{result:?}"));
                }
                for point in &result.points {
                    if point.outputs.len() != 3 {
                        return Err(format!("{point:?}"));
                    }
                    if !point.crowding_distance.is_finite()
                        && !point.crowding_distance.is_infinite()
                    {
                        return Err(format!("non-finite crowding distance: {point:?}"));
                    }
                    for parameter in &point.parameters {
                        if parameter.final_value < lower || parameter.final_value > upper {
                            return Err(format!("parameter outside bounds: {point:?}"));
                        }
                    }
                }
                Ok(())
            },
        );
    }
}

fn reference_direction_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 750.0 + (idx % 7) as f64 * 130.0;
        let lower = 500.0 + (idx % 6) as f64 * 45.0;
        let upper = 3_800.0 + (idx % 9) as f64 * 210.0;
        let divisions = 3 + idx % 3;
        let source = format!(
            "reference-direction pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions={divisions} params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=6\n.end",
            2600 + idx
        );
        cases.add(format!("ams_reference_direction_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if result.front_method != ParetoFrontMethod::ReferenceDirections
                || result.points.len() != 6
                || !result.is_nondominated()
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let Some(direction) = &point.reference_direction else {
                    return Err(format!("missing reference direction: {point:?}"));
                };
                let norm = direction
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                if direction.len() != 3
                    || direction.iter().any(|value| *value < 0.0)
                    || (norm - 1.0).abs() > 1e-9
                {
                    return Err(format!("invalid reference direction: {point:?}"));
                }
            }
            Ok(())
        });
    }
}

fn adaptive_reference_pareto_cases(cases: &mut CaseList) {
    for idx in 0..10 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let series = 800.0 + (idx % 7) as f64 * 125.0;
        let lower = 500.0 + (idx % 6) as f64 * 40.0;
        let upper = 3_900.0 + (idx % 8) as f64 * 175.0;
        let divisions = 4 + idx % 3;
        let source = format!(
            "adaptive reference pareto {idx}\nVin in 0 {supply}\nR1 in a {series}\nR2 a b {series}\nR3 b c {series}\nR4 c 0 {series}\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions={divisions} adaptive=true params=R1.R:{lower}:{upper},R4.R:{lower}:{upper} population=12 iterations=24 seed={} points=7\n.end",
            3200 + idx
        );
        cases.add(format!("ams_adaptive_reference_pareto_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = pareto_optimize(&circuit, &circuit.analyses[0])?;
            if !result.adaptive_references
                || result.front_method != ParetoFrontMethod::ReferenceDirections
                || result.points.len() != 7
                || !result.is_nondominated()
            {
                return Err(format!("{result:?}"));
            }
            for point in &result.points {
                let Some(direction) = &point.reference_direction else {
                    return Err(format!("missing adaptive reference: {point:?}"));
                };
                let norm = direction
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt();
                if direction.len() != 3
                    || direction.iter().any(|value| *value < 0.0)
                    || (norm - 1.0).abs() > 1e-9
                {
                    return Err(format!("invalid adaptive reference: {point:?}"));
                }
            }
            Ok(())
        });
    }
}

fn periodic_steady_state_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let resistance = 500.0 + (idx % 7) as f64 * 250.0;
        let capacitance = 100e-9 + (idx % 6) as f64 * 100e-9;
        let frequency = 1_000.0 + (idx % 9) as f64 * 500.0;
        let omega = std::f64::consts::TAU * frequency;
        let expected = 1.0 / (1.0 + (omega * resistance * capacitance).powi(2)).sqrt();
        let source = format!(
            "pss lowpass {idx}\nVin in 0 SIN(0 1 {frequency})\nR1 in out {resistance}\nC1 out 0 {capacitance}\n.tran 1u 20m method=be\n.pss V(out) fund={frequency} min=8 max=64 samples=64 harmonics=4 tol=1e-4\n.end"
        );
        cases.add(format!("ams_pss_rc_lowpass_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            let fundamental = result
                .harmonic(1)
                .ok_or_else(|| "missing PSS fundamental".to_string())?;
            let second = result
                .harmonic(2)
                .ok_or_else(|| "missing PSS second harmonic".to_string())?;
            if !result.converged
                || (fundamental.phasor.magnitude() - expected).abs() > 0.02
                || second.phasor.magnitude() > 1e-3
            {
                Err(format!(
                    "expected={expected}, fundamental={fundamental:?}, second={second:?}, result_cycles={}",
                    result.cycles
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn automatic_pss_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let frequency = 500.0 + idx as f64 * 250.0;
        let period = 1.0 / frequency;
        let transient_step = period / 128.0;
        let transient_stop = period * 24.0;
        let source = format!(
            "automatic pss {idx}\nVin out 0 SIN(0 1 {frequency})\n.tran {transient_step} {transient_stop} method=be\n.pss V(out) auto min=6 max=32 samples=64 harmonics=3 tol=1e-4\n.end"
        );
        cases.add(format!("ams_automatic_pss_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            if !result.automatic_fundamental
                || !result.converged
                || (result.fundamental_hz - frequency).abs() > frequency * 0.01
                || result.harmonic(1).unwrap().phasor.magnitude() < 0.99
                || result.harmonic(2).unwrap().phasor.magnitude() > 1e-3
            {
                Err(format!(
                    "expected={frequency}, fundamental={:?}, harmonic1={:?}, harmonic2={:?}",
                    result.harmonic(1).map(|item| item.phasor),
                    result.harmonic(2).map(|item| item.phasor),
                    result.fundamental_hz
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn autonomous_pss_cases(cases: &mut CaseList) {
    for idx in 0..20 {
        let supply = 1.6 + (idx % 5) as f64 * 0.05;
        let nmos_width = 0.8e-6 + (idx % 6) as f64 * 0.1e-6;
        let pmos_width = 2.0 * nmos_width;
        let capacitance = 0.6e-12 + (idx % 7) as f64 * 0.2e-12;
        let source = format!(
            "ring oscillator {idx}\nVdd vdd 0 {supply}\n\
             Mn1 n1 n3 0 0 NM W={nmos_width} L=180n\nMp1 n1 n3 vdd vdd PM W={pmos_width} L=180n\n\
             Mn2 n2 n1 0 0 NM W={nmos_width} L=180n\nMp2 n2 n1 vdd vdd PM W={pmos_width} L=180n\n\
             Mn3 n3 n2 0 0 NM W={nmos_width} L=180n\nMp3 n3 n2 vdd vdd PM W={pmos_width} L=180n\n\
             C1 n1 0 {capacitance} IC={supply}\nC2 n2 0 {capacitance}\nC3 n3 0 {capacitance}\n\
             .model NM NMOS(KP=200u VTO=0.5)\n.model PM PMOS(KP=100u VTO=-0.5)\n\
             .tran 10p 120n method=be\n.pss V(n1) auto min=6 max=24 samples=192 harmonics=3 tol=0.06\n.end"
        );
        cases.add(format!("ams_autonomous_pss_ring_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = periodic_steady_state(&circuit, &circuit.analyses[1])?;
            let first = result
                .samples
                .iter()
                .map(|sample| sample.value)
                .fold(f64::INFINITY, f64::min);
            let last = result
                .samples
                .iter()
                .map(|sample| sample.value)
                .fold(f64::NEG_INFINITY, f64::max);
            let fundamental = result.harmonic(1).ok_or("missing fundamental")?;
            if !result.automatic_fundamental
                || !result.converged
                || result.residual > 0.06
                || result.fundamental_hz <= 20_000_000.0
                || result.fundamental_hz >= 250_000_000.0
                || (fundamental.frequency_hz - result.fundamental_hz).abs() > 1e-3
                || fundamental.phasor.magnitude() < 0.5 * supply
                || first > 0.25 * supply
                || last < 0.75 * supply
            {
                Err(format!(
                    "supply={supply}, C={capacitance}, result={result:?}"
                ))
            } else {
                Ok(())
            }
        });
    }
}

fn envelope_cases(cases: &mut CaseList) {
    for idx in 0..50 {
        let carrier = 50_000.0 + idx as f64 * 2_000.0;
        let modulation = 100.0 + idx as f64 * 20.0;
        let depth = 0.2 + (idx % 7) as f64 * 0.1;
        let transient_step = 1.0 / (carrier * 16.0);
        let transient_stop = 4.0 / modulation;
        let expected_min = 1.0 - depth;
        let expected_max = 1.0 + depth;
        let source = format!(
            "envelope am {idx}\nVin out 0 AM(0 1 {carrier} {modulation} {depth})\n.tran {transient_step} {transient_stop} method=be\n.env V(out) carrier={carrier} cycles=8 points=128\n.end"
        );
        cases.add(format!("ams_envelope_am_{idx}"), move || {
            let circuit = parse_spice(&source)?;
            let result = envelope_follow(&circuit, &circuit.analyses[1])?;
            if result.samples.len() != 128
                || (result.minimum - expected_min).abs() > 0.12
                || (result.maximum - expected_max).abs() > 0.12
                || (result.average - 1.0).abs() > 0.12
            {
                Err(format!(
                    "depth={depth}, expected=[{expected_min},{expected_max}], result min={}, max={}, average={}",
                    result.minimum, result.maximum, result.average
                ))
            } else {
                Ok(())
            }
        });
    }
}

pub(super) fn register(cases: &mut CaseList) {
    optimization_cases(cases);
    quantized_optimization_cases(cases);
    pareto_optimization_cases(cases);
    constrained_pareto_cases(cases);
    algebraic_constraint_pareto_cases(cases);
    function_constraint_pareto_cases(cases);
    multi_objective_pareto_cases(cases);
    reference_direction_pareto_cases(cases);
    adaptive_reference_pareto_cases(cases);
    periodic_steady_state_cases(cases);
    automatic_pss_cases(cases);
    autonomous_pss_cases(cases);
    envelope_cases(cases);
}
