//! SPICE netlist serialization.

use super::models::{
    format_optimization_parameters, format_parameter_correlations, format_parameter_covariances,
    format_pareto_constraints, format_resistor_layer_model, format_statistical_parameters,
};
use crate::types::*;

pub fn write_spice(circuit: &SpiceCircuit) -> Result<String, String> {
    circuit.validate()?;
    let mut out = String::new();
    out.push_str(if circuit.title.trim().is_empty() {
        "untitled circuit"
    } else {
        &circuit.title
    });
    out.push('\n');
    if (circuit.temperature_k - 300.15).abs() > 1e-9 {
        out.push_str(&format!(
            ".temp {}\n",
            format_number(circuit.temperature_k - 273.15)
        ));
    }
    for (name, model) in &circuit.resistor_layer_models {
        out.push_str(&format_resistor_layer_model(name, model));
    }
    for (name, location) in &circuit.locations {
        out.push_str(&format!(
            ".location {} {} {}\n",
            name,
            format_number(location.x_m),
            format_number(location.y_m)
        ));
    }
    for model in circuit.statistical_models.values() {
        out.push_str(&format!(
            ".stats name={} params={} corr={}\n",
            model.name,
            format_statistical_parameters(&model.parameters),
            format_parameter_correlations(&model.correlations)
        ));
    }
    for (name, model) in &circuit.diode_models {
        out.push_str(&format!(
            ".model {} D(IS={} N={} RS={} CJO={} VJ={} M={} FC={} KF={} AF={} EF={} EG={} XTI={} TT={})\n",
            name,
            format_number(model.saturation_current_a),
            format_number(model.emission_coefficient),
            format_number(model.series_resistance_ohm),
            format_number(model.zero_bias_junction_cap_f),
            format_number(model.junction_potential_v),
            format_number(model.grading_coefficient),
            format_number(model.forward_knee),
            format_number(model.flicker_coefficient_a2),
            format_number(model.flicker_current_exponent),
            format_number(model.flicker_frequency_exponent),
            format_number(model.energy_gap_ev),
            format_number(model.saturation_current_temperature_exponent),
            format_number(model.transit_time_s)
        ));
    }
    for (name, model) in &circuit.mosfet_models {
        let polarity = match model.polarity {
            MosfetPolarity::Nmos => "NMOS",
            MosfetPolarity::Pmos => "PMOS",
        };
        let body = match model.kind {
            MosfetModelKind::Level1 => format!(
                "KP={} VTO={} LAMBDA={} GAMMA={} PHI={} THETA={} VSAT={} CGSO={} CGDO={} CGB={} COX={} CJSWS={} CJSWD={} PB={} MJ={} FC={} XPART={} KF={} AF={} EF={} MEXP={} RTH={} CTH={}",
                format_number(model.transconductance_a_per_v2),
                format_number(model.threshold_voltage_v),
                format_number(model.channel_length_modulation_per_v),
                format_number(model.body_effect_v_per_sqrt_v),
                format_number(model.surface_potential_v),
                format_number(model.mobility_degradation_inverse_v),
                format_number(model.velocity_saturation_voltage_v),
                format_number(model.gate_source_overlap_cap_per_m),
                format_number(model.gate_drain_overlap_cap_per_m),
                format_number(model.gate_bulk_cap_per_m2),
                format_number(model.gate_oxide_cap_per_m2),
                format_number(model.source_junction_sidewall_cap_per_m),
                format_number(model.drain_junction_sidewall_cap_per_m),
                format_number(model.junction_potential_v),
                format_number(model.junction_grading_coefficient),
                format_number(model.junction_forward_knee),
                format_number(model.saturation_charge_partition),
                format_number(model.flicker_coefficient_a2),
                format_number(model.flicker_current_exponent),
                format_number(model.flicker_frequency_exponent),
                format_number(model.mobility_temperature_exponent),
                format_number(model.self_heating_thermal_resistance_k_per_w),
                format_number(model.self_heating_thermal_capacitance_j_per_k)
            ),
            MosfetModelKind::Bsim3LongChannel => format!(
                "LEVEL=8 VTH0={} K1={} K2={} DVT0={} DVT1={} DVT2={} DVT0W={} DVT1W={} W0={} KT1={} KT2={} KT1L={} LAMBDA={} PHI={} TOX={} U0={} UA={} UB={} UC={} VSAT={} CGSO={} CGDO={} CGB={} COX={} CJSWS={} CJSWD={} PB={} MJ={} FC={} XPART={} KF={} AF={} EF={} UTE={} RTH={} CTH={}",
                format_number(model.threshold_voltage_v),
                format_number(model.body_effect_v_per_sqrt_v),
                format_number(model.bsim3_body_bias_threshold_coefficient),
                format_number(model.bsim3_short_channel_threshold_amplitude),
                format_number(model.bsim3_short_channel_length_scale_m),
                format_number(model.bsim3_short_channel_drain_bias_coefficient_per_v),
                format_number(model.bsim3_narrow_width_threshold_amplitude_v),
                format_number(model.bsim3_narrow_width_exponent),
                format_number(model.bsim3_reference_width_m),
                format_number(model.bsim3_threshold_temperature_coefficient_v),
                format_number(model.bsim3_threshold_body_temperature_coefficient_v_per_v),
                format_number(model.bsim3_threshold_length_temperature_coefficient_v_m),
                format_number(model.channel_length_modulation_per_v),
                format_number(model.surface_potential_v),
                format_number(model.bsim3_oxide_thickness_m),
                format_number(model.bsim3_low_field_mobility_m2_per_v_s),
                format_number(model.bsim3_mobility_degradation_first_order_m_per_v),
                format_number(model.bsim3_mobility_degradation_second_order_m2_per_v2),
                format_number(model.bsim3_mobility_body_bias_coefficient_m2_per_v2),
                format_number(model.bsim3_saturation_velocity_m_per_s),
                format_number(model.gate_source_overlap_cap_per_m),
                format_number(model.gate_drain_overlap_cap_per_m),
                format_number(model.gate_bulk_cap_per_m2),
                format_number(model.gate_oxide_cap_per_m2),
                format_number(model.source_junction_sidewall_cap_per_m),
                format_number(model.drain_junction_sidewall_cap_per_m),
                format_number(model.junction_potential_v),
                format_number(model.junction_grading_coefficient),
                format_number(model.junction_forward_knee),
                format_number(model.saturation_charge_partition),
                format_number(model.flicker_coefficient_a2),
                format_number(model.flicker_current_exponent),
                format_number(model.flicker_frequency_exponent),
                format_number(model.mobility_temperature_exponent),
                format_number(model.self_heating_thermal_resistance_k_per_w),
                format_number(model.self_heating_thermal_capacitance_j_per_k)
            ),
        };
        let body = if let Some(bin) = model.geometry_bin {
            format!(
                "{body} WMIN={} WMAX={} LMIN={} LMAX={}",
                format_number(bin.min_width_m),
                format_number(bin.max_width_m),
                format_number(bin.min_length_m),
                format_number(bin.max_length_m)
            )
        } else {
            body
        };
        out.push_str(&format!(".model {} {}({body})\n", name, polarity));
    }
    for (name, model) in &circuit.bjt_models {
        out.push_str(&format!(
            ".model {} {}(IS={} BF={} BR={} VAF={} VAR={} CJE={} CJC={} VJE={} VJC={} MJE={} MJC={} FC={} KF={} AF={} EF={} EG={} XTI={} RTH={} CTH={} TF={} TR={})\n",
            name,
            match model.polarity {
                BjtPolarity::Npn => "NPN",
                BjtPolarity::Pnp => "PNP",
            },
            format_number(model.saturation_current_a),
            format_number(model.forward_beta),
            format_number(model.reverse_beta),
            format_number(model.forward_early_voltage_v),
            format_number(model.reverse_early_voltage_v),
            format_number(model.base_emitter_zero_bias_cap_f),
            format_number(model.base_collector_zero_bias_cap_f),
            format_number(model.base_emitter_junction_potential_v),
            format_number(model.base_collector_junction_potential_v),
            format_number(model.base_emitter_grading_coefficient),
            format_number(model.base_collector_grading_coefficient),
            format_number(model.junction_forward_knee),
            format_number(model.flicker_coefficient_a2),
            format_number(model.flicker_current_exponent),
            format_number(model.flicker_frequency_exponent),
            format_number(model.energy_gap_ev),
            format_number(model.saturation_current_temperature_exponent),
            format_number(model.self_heating_thermal_resistance_k_per_w),
            format_number(model.self_heating_thermal_capacitance_j_per_k),
            format_number(model.forward_transit_time_s),
            format_number(model.reverse_transit_time_s)
        ));
    }
    for element in &circuit.elements {
        write_element(&mut out, element);
    }
    for analysis in &circuit.analyses {
        match analysis {
            Analysis::OperatingPoint => out.push_str(".op\n"),
            Analysis::DcSweep {
                source,
                start,
                stop,
                step,
            } => out.push_str(&format!(
                ".dc {} {} {} {}\n",
                source,
                format_number(*start),
                format_number(*stop),
                format_number(*step)
            )),
            Analysis::Transient {
                step_s,
                stop_s,
                method,
            } => out.push_str(&format!(
                ".tran {} {} method={}\n",
                format_number(*step_s),
                format_number(*stop_s),
                method.as_str()
            )),
            Analysis::AcSweep {
                kind,
                points,
                start_hz,
                stop_hz,
            } => out.push_str(&format!(
                ".ac {} {} {} {}\n",
                match kind {
                    AcSweepKind::Linear => "lin",
                    AcSweepKind::Decade => "dec",
                    AcSweepKind::Octave => "oct",
                },
                points,
                format_number(*start_hz),
                format_number(*stop_hz)
            )),
        Analysis::NoiseSweep {
                output_node,
                input_source,
                kind,
                points,
                start_hz,
                stop_hz,
            } => out.push_str(&format!(
                ".noise V({}) {} {} {} {} {}\n",
                output_node,
                input_source,
                match kind {
                    AcSweepKind::Linear => "lin",
                    AcSweepKind::Decade => "dec",
                    AcSweepKind::Octave => "oct",
                },
                points,
                format_number(*start_hz),
            format_number(*stop_hz)
        )),
        Analysis::MonteCarlo(options) => out.push_str(&format!(
            ".mc {} {} V({}) seed={} freq={} rtol={} ctol={} ltol={} disdec={} vtotol={} kptol={} bftol={} bisdec={} rcorr={} ccorr={} lcorr={} dscorr={} vtocorr={} kpcorr={} bfcorr={} biscorr={} spatialcorr={} spatialscale={} dist={} stats={} matrix={} cov={}\n",
            options.runs,
            options.target.as_str(),
            options.output_node,
            options.seed,
            format_number(options.frequency_hz),
            format_number(options.resistor_tolerance),
            format_number(options.capacitor_tolerance),
            format_number(options.inductor_tolerance),
            format_number(options.diode_is_decades),
            format_number(options.mos_vto_tolerance),
            format_number(options.mos_kp_tolerance),
            format_number(options.bjt_beta_tolerance),
            format_number(options.bjt_is_decades),
            format_number(options.resistor_correlation),
            format_number(options.capacitor_correlation),
            format_number(options.inductor_correlation),
            format_number(options.diode_is_correlation),
            format_number(options.mos_vto_correlation),
            format_number(options.mos_kp_correlation),
            format_number(options.bjt_beta_correlation),
            format_number(options.bjt_is_correlation),
            format_number(options.spatial_correlation),
            format_number(options.spatial_correlation_length_m),
            options.distribution.as_str(),
            options.statistical_model.as_deref().unwrap_or_default(),
            format_parameter_correlations(&options.correlations),
            format_parameter_covariances(&options.covariances)
        )),
        Analysis::Sensitivity(options) => out.push_str(&format!(
            ".sens {} V({}) freq={} step={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            format_number(options.relative_step)
        )),
        Analysis::GlobalSensitivity(options) => out.push_str(&format!(
            ".sobol {} V({}) freq={} samples={} seed={} tol={} corr={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            options.samples,
            options.seed,
            format_number(options.tolerance),
            format_parameter_correlations(&options.correlations)
        )),
        Analysis::Derivative(options) => {
            let prefix = match options.target {
                DerivativeTarget::AcMagnitude => format!(
                    ".deriv ac V({}) freq={}",
                    options.output_node,
                    format_number(options.frequency_hz)
                ),
                DerivativeTarget::TransientFinal => {
                    format!(".deriv tran V({})", options.output_node)
                }
                DerivativeTarget::Dc => format!(".deriv V({})", options.output_node),
            };
            out.push_str(&format!(
                "{prefix} params={}\n",
                options.parameters.join(",")
            ))
        }
        Analysis::Optimization(options) => out.push_str(&format!(
            ".optim {} {} V({}) goal={} freq={} params={} iterations={} population={} seed={} tol={} f={} cr={}\n",
            options.objective.as_str(),
            options.target.as_str(),
            options.output_node,
            format_number(options.goal_value),
            format_number(options.frequency_hz),
            format_optimization_parameters(&options.parameters),
            options.iterations,
            options.population,
            options.seed,
            format_number(options.tolerance),
            format_number(options.differential_weight),
            format_number(options.crossover_probability)
        )),
        Analysis::Pareto(options) => {
            let outputs = options
                .output_nodes
                .iter()
                .map(|node| format!("V({node})"))
                .collect::<Vec<_>>()
                .join(" ");
            let senses = options
                .senses
                .iter()
                .map(|sense| sense.as_str())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(
            ".pareto {} {} sense={} freq={} params={} iterations={} population={} seed={} points={} front={} divisions={} adaptive={} f={} cr={} quantum={} constraints={}\n",
            options.target.as_str(),
            outputs,
            senses,
            format_number(options.frequency_hz),
            format_optimization_parameters(&options.parameters),
            options.iterations,
            options.population,
            options.seed,
            options.maximum_points,
            options.front_method.as_str(),
            options.reference_divisions,
            options.adaptive_references,
            format_number(options.differential_weight),
            format_number(options.crossover_probability),
            options
                .parameters
                .iter()
                .filter_map(|parameter| parameter.quantum)
                .next()
                .map(format_number)
                .unwrap_or_default(),
            format_pareto_constraints(&options.constraints)
            ))
        }
        Analysis::PeriodicSteadyState(options) => {
            let mut line = format!(
                ".pss V({}) fund={} auto={}",
                options.output_node,
                format_number(options.fundamental_hz),
                options.automatic_fundamental
            );
            if let Some(threshold) = options.threshold {
                line.push_str(&format!(" threshold={}", format_number(threshold)));
            }
            line.push_str(&format!(
                " min={} max={} samples={} harmonics={} tol={}\n",
                options.minimum_cycles,
                options.maximum_cycles,
                options.samples_per_cycle,
                options.harmonics,
                format_number(options.tolerance)
            ));
            out.push_str(&line);
        }
        Analysis::EnvelopeFollow(options) => out.push_str(&format!(
            ".env V({}) carrier={} cycles={} points={}\n",
            options.output_node,
            format_number(options.carrier_hz),
            format_number(options.carrier_cycles),
            options.points
        )),
        Analysis::Aging(options) => out.push_str(&format!(
            ".aging {} V({}) freq={} years={} exp={} rinc={} vtshift={} kploss={}\n",
            options.target.as_str(),
            options.output_node,
            format_number(options.frequency_hz),
            format_number(options.age_years),
            format_number(options.time_exponent),
            format_number(options.resistance_increase_per_year),
            format_number(options.mos_threshold_shift_v_per_year),
            format_number(options.mos_kp_degradation_per_year)
        )),
    }
    }
    out.push_str(".end\n");
    Ok(out)
}

fn write_element(out: &mut String, element: &SpiceElement) {
    let mut line = element.name.clone();
    for node in &element.nodes {
        line.push(' ');
        line.push_str(node);
    }
    match &element.kind {
        SpiceElementKind::Resistor {
            resistance,
            first_temperature_coefficient_per_k,
            second_temperature_coefficient_per_k2,
            sheet_resistance_ohm_per_square,
            length_m,
            width_m,
            multiplier,
            layer,
        } => {
            line.push_str(&format!(
                " {} TC1={} TC2={}",
                format_number(
                    if layer.is_some() || *sheet_resistance_ohm_per_square > 0.0 {
                        *multiplier
                    } else {
                        *resistance
                    }
                ),
                format_number(*first_temperature_coefficient_per_k),
                format_number(*second_temperature_coefficient_per_k2),
            ));
            if let Some(layer) = layer {
                line.push_str(&format!(
                    " LAYER={} L={} W={}",
                    layer,
                    format_number(*length_m),
                    format_number(*width_m)
                ));
            } else if *sheet_resistance_ohm_per_square > 0.0 {
                line.push_str(&format!(
                    " SHEET={} L={} W={}",
                    format_number(*sheet_resistance_ohm_per_square),
                    format_number(*length_m),
                    format_number(*width_m)
                ));
            }
        }
        SpiceElementKind::Capacitor {
            capacitance,
            initial_voltage,
        } => {
            line.push_str(&format!(
                " {} IC={}",
                format_number(*capacitance),
                format_number(*initial_voltage)
            ));
        }
        SpiceElementKind::Inductor {
            inductance,
            initial_current,
        } => {
            line.push_str(&format!(
                " {} IC={}",
                format_number(*inductance),
                format_number(*initial_current)
            ));
        }
        SpiceElementKind::VoltageSource {
            waveform,
            ac_magnitude,
            ac_phase_deg,
        } => {
            line.push_str(&format_source(waveform, *ac_magnitude, *ac_phase_deg));
        }
        SpiceElementKind::CurrentSource {
            waveform,
            ac_magnitude,
            ac_phase_deg,
        } => {
            line.push_str(&format_source(waveform, *ac_magnitude, *ac_phase_deg));
        }
        SpiceElementKind::VoltageControlledVoltageSource { gain }
        | SpiceElementKind::VoltageControlledCurrentSource {
            transconductance: gain,
        } => {
            line.push_str(&format!(" {}", format_number(*gain)));
        }
        SpiceElementKind::CurrentControlledVoltageSource {
            controlling_source,
            transresistance,
        } => {
            line.push_str(&format!(
                " {} {}",
                controlling_source,
                format_number(*transresistance)
            ));
        }
        SpiceElementKind::CurrentControlledCurrentSource {
            controlling_source,
            gain,
        } => {
            line.push_str(&format!(" {} {}", controlling_source, format_number(*gain)));
        }
        SpiceElementKind::Diode { model } => line.push_str(&format!(" {}", model)),
        SpiceElementKind::Mosfet {
            model,
            width_m,
            length_m,
            multiplier,
        } => line.push_str(&format!(
            " {} W={} L={} M={}",
            model,
            format_number(*width_m),
            format_number(*length_m),
            format_number(*multiplier)
        )),
        SpiceElementKind::BipolarJunctionTransistor { model, area } => {
            line.push_str(&format!(" {} AREA={}", model, format_number(*area)));
        }
    }
    out.push_str(&line);
    out.push('\n');
}

fn format_source(waveform: &SourceWaveform, ac_magnitude: Option<f64>, ac_phase: f64) -> String {
    let mut out = String::new();
    if let Some(magnitude) = ac_magnitude {
        out.push_str(&format!(
            " AC {} {}",
            format_number(magnitude),
            format_number(ac_phase)
        ));
    }
    match waveform {
        SourceWaveform::Dc(value) => {
            if ac_magnitude.is_some() {
                out.push_str(&format!(" DC {}", format_number(*value)));
            } else {
                out.push_str(&format!(" {}", format_number(*value)));
            }
        }
        SourceWaveform::Sin {
            offset,
            amplitude,
            frequency_hz,
            delay_s,
            damping_per_s,
        } => out.push_str(&format!(
            " SIN({} {} {} {} {})",
            format_number(*offset),
            format_number(*amplitude),
            format_number(*frequency_hz),
            format_number(*delay_s),
            format_number(*damping_per_s)
        )),
        SourceWaveform::Am {
            offset,
            amplitude,
            carrier_hz,
            modulation_hz,
            depth,
        } => out.push_str(&format!(
            " AM({} {} {} {} {})",
            format_number(*offset),
            format_number(*amplitude),
            format_number(*carrier_hz),
            format_number(*modulation_hz),
            format_number(*depth)
        )),
        SourceWaveform::Pulse {
            initial,
            pulsed,
            delay_s,
            rise_s,
            fall_s,
            width_s,
            period_s,
        } => out.push_str(&format!(
            " PULSE({} {} {} {} {} {} {})",
            format_number(*initial),
            format_number(*pulsed),
            format_number(*delay_s),
            format_number(*rise_s),
            format_number(*fall_s),
            format_number(*width_s),
            format_number(*period_s)
        )),
        SourceWaveform::Pwl(points) => {
            out.push_str(" PWL(");
            for (idx, (time, value)) in points.iter().enumerate() {
                if idx > 0 {
                    out.push(' ');
                }
                out.push_str(&format!(
                    "{} {}",
                    format_number(*time),
                    format_number(*value)
                ));
            }
            out.push(')');
        }
    }
    out
}

pub fn format_number(value: f64) -> String {
    if !value.is_finite() {
        if value.is_infinite() && value.is_sign_positive() {
            return "inf".into();
        }
        if value.is_infinite() {
            return "-inf".into();
        }
        return "nan".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    let abs = value.abs();
    if (1e-15..1e12).contains(&abs) {
        format!("{}", value)
    } else {
        format!("{:e}", value)
    }
}
