//! Sobol, optimization, quantization, and Pareto tests.

use super::*;

#[test]
fn sobol_indices_are_deterministic_and_variance_decomposed() {
    let source = "sobol divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=256 seed=17 tol=0.2\n.end";
    let circuit = parse_spice(source).unwrap();
    let first = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
    let second = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
    assert_eq!(first, second);
    assert!(first.output_variance > 0.0, "{first:?}");

    let first_sum: f64 = first
        .entries
        .iter()
        .map(|entry| entry.first_order_index)
        .sum();
    let total_sum: f64 = first
        .entries
        .iter()
        .map(|entry| entry.total_order_index)
        .sum();
    assert!(
        first_sum > 0.8 && first_sum < 1.2,
        "{first:?} first_sum={first_sum}"
    );
    assert!(
        total_sum >= first_sum - 0.05,
        "{first:?} first_sum={first_sum} total_sum={total_sum}"
    );
    for entry in &first.entries {
        assert!(
            entry.total_order_index >= entry.first_order_index - 0.05,
            "{entry:?}"
        );
    }
    assert!(first.find("R1.R").unwrap().first_order_index > 0.05);
    assert!(first.find("R2.R").unwrap().first_order_index > 0.05);
}

#[test]
fn sobol_directive_round_trips() {
    let source = "sobol round trip\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=9 tol=0.15\n.end";
    let circuit = parse_spice(source).unwrap();
    assert_eq!(
        circuit,
        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
    );
}

#[test]
fn perfectly_correlated_sobol_parameters_cancel_ratio_sensitivity() {
    let source = "correlated sobol\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=23 tol=0.2 corr=R1.R|R2.R|1\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = global_sensitivity(&circuit, &circuit.analyses[0]).unwrap();
    assert!(
        result.find("R1.R").unwrap().total_order_index.abs() < 1e-9,
        "{result:?}"
    );
    assert!(
        result.find("R2.R").unwrap().total_order_index.abs() < 1e-9,
        "{result:?}"
    );
    assert!(
        (result.find("Vin.DC").unwrap().total_order_index - 1.0).abs() < 1e-9,
        "{result:?}"
    );
    assert!(result.find("R1.R").unwrap().first_order_index > 0.0);
}

#[test]
fn sobol_correlation_round_trips_and_rejects_bad_matrices() {
    let source = "sobol correlation\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=64 seed=9 corr=R1.R|R2.R|0.8\n.end";
    let circuit = parse_spice(source).unwrap();
    assert_eq!(
        circuit,
        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
    );

    let invalid_circuit =
        parse_spice("bad\nR1 out 0 1k\n.sobol V(out) samples=16 corr=R1.R|Missing.R|0.5\n.end")
            .unwrap();
    let invalid = global_sensitivity(&invalid_circuit, &invalid_circuit.analyses[0]).unwrap_err();
    assert!(invalid.contains("unknown Sobol parameter"), "{invalid}");

    let non_pd_circuit = parse_spice(
        "bad\nVin in 0 1\nR1 in out 1k\nR2 out 0 1k\n.sobol V(out) samples=16 corr=R1.R|R2.R|1,R1.R|Vin.DC|1,R2.R|Vin.DC|-1\n.end",
    )
    .unwrap();
    let non_positive_definite =
        global_sensitivity(&non_pd_circuit, &non_pd_circuit.analyses[0]).unwrap_err();
    assert!(
        non_positive_definite.contains("not positive definite"),
        "{non_positive_definite}"
    );
}

#[test]
fn differential_evolution_matches_divider_target() {
    let source = "optim divider\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:10000 population=12 iterations=64 seed=17 tol=1e-8\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
    assert!(result.converged, "{result:?}");
    assert!((result.final_output - 1.2).abs() < 1e-7);
    let r2 = result
        .parameters
        .iter()
        .find(|parameter| parameter.parameter.eq_ignore_ascii_case("R2.R"))
        .unwrap()
        .final_value;
    let expected = 1_000.0 * 1.2 / (1.8 - 1.2);
    assert!((r2 - expected).abs() < 1.0, "{r2} {expected}");
}

#[test]
fn differential_evolution_matches_ac_target_with_analytic_hessian() {
    let source = "optim ac lowpass\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1k 1k\n.optim match ac V(out) goal=0.5 freq=1k params=R1.R:100:10000 population=5 iterations=1 seed=17 tol=1e-9\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[1]).unwrap();
    assert!(result.converged, "{result:?}");
    assert!((result.final_output - 0.5).abs() < 1e-8);
    let resistance = result.parameters[0].final_value;
    let expected = 3.0_f64.sqrt() / (std::f64::consts::TAU * 1_000.0 * 1e-6);
    assert!(
        (resistance - expected).abs() < 1.0,
        "{resistance} {expected}"
    );
}

#[test]
fn differential_evolution_matches_ac_source_magnitude() {
    let source = "optim ac source\nVin in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u\n.ac lin 2 1k 1k\n.optim match ac V(out) goal=0.1 freq=1k params=Vin.AC:0.1:1 population=5 iterations=1 seed=23 tol=1e-9\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[1]).unwrap();
    assert!(result.converged, "{result:?}");
    let magnitude = result.parameters[0].final_value;
    assert!((result.final_output - 0.1).abs() < 1e-8, "{result:?}");
    assert!(
        (magnitude - 0.1 / result.initial_output).abs() < 1e-6,
        "{result:?}"
    );
}

#[test]
fn differential_evolution_matches_diode_target_with_implicit_hessian() {
    let source = "optim diode\nIin 0 out 1m\nR1 out 0 1k\nD1 out 0 DN\n.model DN D(IS=1e-12 N=1)\n.temp 26.85\n.optim match V(out) goal=0.55 params=DN.IS:1e-14:1e-10 population=8 iterations=1 seed=31 tol=1e-9\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
    assert!(result.converged, "{result:?}");
    assert!((result.final_output - 0.55).abs() < 1e-8);
    let saturation_current = result.parameters[0].final_value;
    assert!(
        saturation_current.is_finite() && saturation_current > 1e-14 && saturation_current < 1e-10,
        "{result:?}"
    );
}

#[test]
fn optimization_minimize_reaches_parameter_bound() {
    let source = "optim minimize\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim min V(out) params=R2.R:100:5000 population=8 iterations=32 seed=7\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
    let final_r2 = result.parameters[0].final_value;
    assert!((final_r2 - 100.0).abs() < 1.0, "{result:?}");
    assert!(result.final_score < result.initial_score, "{result:?}");
}

#[test]
fn quantized_optimization_returns_feasible_grid_value() {
    let source = "optim quantized\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) goal=1.2 params=R2.R:100:4777:1000:100 population=8 iterations=16 seed=17 tol=1e-8\n.end";
    let circuit = parse_spice(source).unwrap();
    let result = optimize(&circuit, &circuit.analyses[0]).unwrap();
    assert!(result.converged, "{result:?}");
    let Analysis::Optimization(options) = &circuit.analyses[0] else {
        panic!("expected optimization analysis");
    };
    let specification = options.parameters.first().unwrap();
    let quantum = specification.quantum.unwrap();
    let r2 = result.parameters[0].final_value;
    let residual =
        (r2 - specification.lower) - ((r2 - specification.lower) / quantum).round() * quantum;
    assert!(
        r2 <= specification.upper && residual.abs() < 1e-7,
        "{result:?}"
    );
    assert_eq!(result.parameters[0].quantum, Some(quantum));
    assert!((r2 - 2000.0).abs() < 1e-7, "{result:?}");
}

#[test]
fn pareto_optimization_returns_nondominated_tradeoff() {
    let source = "pareto ladder\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max params=R1.R:500:5000,R3.R:500:5000 population=20 iterations=32 seed=11 points=12\n.end";
    let circuit = parse_spice(source).unwrap();
    assert_eq!(
        circuit,
        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
    );
    let result = pareto_optimize(&circuit, &circuit.analyses[0]).unwrap();
    assert_eq!(result.points.len(), 12, "{result:?}");
    assert!(result.is_nondominated(), "{result:?}");
    for pair in result.points.windows(2) {
        assert!(pair[0].outputs[0] <= pair[1].outputs[0], "{result:?}");
        assert!(pair[0].outputs[1] <= pair[1].outputs[1], "{result:?}");
    }
    for point in &result.points {
        for (parameter, specification) in point.parameters.iter().zip(match &circuit.analyses[0] {
            Analysis::Pareto(options) => &options.parameters,
            _ => unreachable!(),
        }) {
            assert!(
                parameter.final_value >= specification.lower
                    && parameter.final_value <= specification.upper,
                "{parameter:?}"
            );
        }
    }

    let quantized = parse_spice(
        "pareto grid\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max params=R1.R:500:4777:1000:100,R3.R:500:4777:1000:100 population=12 iterations=16 seed=19 points=4\n.end",
    )
    .unwrap();
    let quantized_result = pareto_optimize(&quantized, &quantized.analyses[0]).unwrap();
    assert!(quantized_result.is_nondominated(), "{quantized_result:?}");
    for point in &quantized_result.points {
        for parameter in &point.parameters {
            let residual = (parameter.final_value - 500.0)
                - ((parameter.final_value - 500.0) / 100.0).round() * 100.0;
            assert!(
                parameter.final_value <= 4777.0 && residual.abs() < 1e-7,
                "{parameter:?}"
            );
        }
    }

    let constrained = parse_spice(
        "constrained pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) sense=min,max constraints=V(b)>=0.4 params=R1.R:500:5000,R3.R:500:5000 population=12 iterations=24 seed=23 points=6\n.end",
    )
    .unwrap();
    assert_eq!(
        constrained,
        parse_spice(&write_spice(&constrained).unwrap()).unwrap()
    );
    let constrained_result = pareto_optimize(&constrained, &constrained.analyses[0]).unwrap();
    assert_eq!(constrained_result.constraints.len(), 1);
    assert!(
        constrained_result.is_nondominated(),
        "{constrained_result:?}"
    );
    for point in &constrained_result.points {
        assert!(point.constraints_satisfied, "{point:?}");
        assert!(point.outputs[1] >= 0.4, "{point:?}");
    }

    let algebraic = parse_spice(
        "algebraic pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max constraints=(2*V(a)+V(b)-0.5*V(c))<=2.8 params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=37 points=5\n.end",
    )
    .unwrap();
    assert_eq!(
        algebraic,
        parse_spice(&write_spice(&algebraic).unwrap()).unwrap()
    );
    let algebraic_result = pareto_optimize(&algebraic, &algebraic.analyses[0]).unwrap();
    assert!(algebraic_result.is_nondominated(), "{algebraic_result:?}");
    for point in &algebraic_result.points {
        let value = 2.0 * point.outputs[0] + point.outputs[1] - 0.5 * point.outputs[2];
        assert!(
            point.constraints_satisfied && value <= 2.8 + 1e-9,
            "{point:?}"
        );
    }

    let precedence = parse_spice(
        "constraint precedence\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) constraints=(2*V(a)+V(b)^2-V(c)/2)==9 params=R1.R:500:5000,R4.R:500:5000\n.end",
    )
    .unwrap();
    let expression = match &precedence.analyses[0] {
        Analysis::Pareto(options) => options.constraints[0]
            .expression
            .clone()
            .expect("algebraic expression"),
        _ => unreachable!(),
    };
    let mut values = std::collections::BTreeMap::new();
    values.insert("a".to_string(), 1.0);
    values.insert("b".to_string(), 3.0);
    values.insert("c".to_string(), 4.0);
    assert!((expression.evaluate(&values).unwrap() - 9.0).abs() < 1e-12);
    assert_eq!(
        crate::types::constraint_violation(2.25, ConstraintSense::Equal, 2.0),
        0.25
    );

    let functions = parse_spice(
        "constraint functions\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) constraints=(min(V(a),V(b))+abs(V(a)-V(b))-ln(exp(2)))==2 params=R1.R:500:5000,R4.R:500:5000\n.end",
    )
    .unwrap();
    assert_eq!(
        functions,
        parse_spice(&write_spice(&functions).unwrap()).unwrap()
    );
    let function_expression = match &functions.analyses[0] {
        Analysis::Pareto(options) => options.constraints[0]
            .expression
            .clone()
            .expect("function expression"),
        _ => unreachable!(),
    };
    assert!(
        (function_expression.evaluate(&values).unwrap() - 1.0).abs() < 1e-12,
        "{function_expression:?}"
    );

    let three_objective = parse_spice(
        "three-objective pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=29 points=5\n.end",
    )
    .unwrap();
    assert_eq!(
        three_objective,
        parse_spice(&write_spice(&three_objective).unwrap()).unwrap()
    );
    let three_result = pareto_optimize(&three_objective, &three_objective.analyses[0]).unwrap();
    assert_eq!(three_result.output_nodes.len(), 3);
    assert!(three_result.is_nondominated(), "{three_result:?}");
    assert!(three_result.points.len() >= 2, "{three_result:?}");
    assert!(
        result
            .points
            .first()
            .unwrap()
            .crowding_distance
            .is_infinite()
            && result
                .points
                .last()
                .unwrap()
                .crowding_distance
                .is_infinite(),
        "{result:?}"
    );

    let reference = parse_spice(
        "reference pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions=4 params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=31 points=5\n.end",
    )
    .unwrap();
    assert_eq!(
        reference,
        parse_spice(&write_spice(&reference).unwrap()).unwrap()
    );
    let reference_result = pareto_optimize(&reference, &reference.analyses[0]).unwrap();
    assert_eq!(
        reference_result.front_method,
        ParetoFrontMethod::ReferenceDirections
    );
    assert_eq!(reference_result.points.len(), 5, "{reference_result:?}");
    assert!(reference_result.is_nondominated(), "{reference_result:?}");
    for point in &reference_result.points {
        let direction = point.reference_direction.as_ref().unwrap();
        let norm = direction
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        assert!(
            direction.len() == 3
                && direction.iter().all(|value| *value >= 0.0)
                && (norm - 1.0).abs() < 1e-9,
            "{point:?}"
        );
    }

    let adaptive = parse_spice(
        "adaptive reference pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b c 1k\nR4 c 0 1k\n.pareto V(a) V(b) V(c) sense=min,max,max front=nsga3 divisions=4 adaptive=true params=R1.R:500:5000,R4.R:500:5000 population=12 iterations=24 seed=41 points=6\n.end",
    )
    .unwrap();
    assert_eq!(
        adaptive,
        parse_spice(&write_spice(&adaptive).unwrap()).unwrap()
    );
    let adaptive_result = pareto_optimize(&adaptive, &adaptive.analyses[0]).unwrap();
    assert!(adaptive_result.adaptive_references, "{adaptive_result:?}");
    assert_eq!(adaptive_result.points.len(), 6, "{adaptive_result:?}");
    assert!(adaptive_result.is_nondominated(), "{adaptive_result:?}");
    assert!(
        adaptive_result.points.iter().any(|point| {
            point.reference_direction.as_ref().is_some_and(|direction| {
                direction
                    .iter()
                    .any(|value| (value * 4.0 - (value * 4.0).round()).abs() > 1e-6)
            })
        }),
        "{adaptive_result:?}"
    );

    let invalid = parse_spice(
        "bad pareto\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(a) params=R1.R:1:2\n.end",
    )
    .unwrap_err();
    assert!(
        invalid.contains("Pareto optimization output nodes must be distinct"),
        "{invalid}"
    );
    let missing_parameter = parse_spice(
        "bad pareto parameter\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) params=Missing.R:1:2\n.end",
    )
    .unwrap_err();
    assert!(
        missing_parameter.contains("unknown Pareto optimization parameter"),
        "{missing_parameter}"
    );
    let bad_constraint = parse_spice(
        "bad pareto constraint\nVin in 0 1.8\nR1 in a 1k\nR2 a b 1k\nR3 b 0 1k\n.pareto V(a) V(b) constraints=V(missing)>=0.1 params=R1.R:1:2\n.end",
    )
    .unwrap_err();
    assert!(
        bad_constraint.contains("Pareto optimization constraint node"),
        "{bad_constraint}"
    );
}

#[test]
fn optimization_directive_round_trips_and_validates_parameters() {
    let source = "optim round trip\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match op V(out) goal=1.2 params=R2.R:100:5000:1000 population=8 iterations=16\n.end";
    let circuit = parse_spice(source).unwrap();
    assert_eq!(
        circuit,
        parse_spice(&write_spice(&circuit).unwrap()).unwrap()
    );
    let Analysis::Optimization(options) = &circuit.analyses[0] else {
        panic!("expected optimization analysis");
    };
    assert_eq!(options.parameters[0].initial, Some(1000.0));
    let quantized = parse_spice(
        "quantum round trip\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=R2.R:100:5000::100 quantum=100\n.end",
    )
    .unwrap();
    assert_eq!(
        quantized,
        parse_spice(&write_spice(&quantized).unwrap()).unwrap()
    );
    assert_eq!(
        match &quantized.analyses[0] {
            Analysis::Optimization(options) => options.parameters[0].quantum,
            _ => panic!("expected optimization analysis"),
        },
        Some(100.0)
    );
    let invalid = parse_spice(
        "bad\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=Missing.R:1:2\n.end",
    )
    .unwrap_err();
    assert!(
        invalid.contains("unknown optimization parameter"),
        "{invalid}"
    );
    let bad_quantum = parse_spice(
        "bad quantum\nVin in 0 1.8\nR1 in out 1k\nR2 out 0 1k\n.optim match V(out) params=R2.R:100:5000:1001:100\n.end",
    )
    .unwrap_err();
    assert!(
        bad_quantum.contains("outside its quantum grid"),
        "{bad_quantum}"
    );
}
