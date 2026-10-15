//! Functional coverage, waveform, assertion, and AMS RPC handlers.

use crate::{required_string, Json};
use eda_api::{
    assertion_report, check_assertions, coverage_report, functional_coverage, parse_assertions,
    parse_covergroups, parse_spice, parse_waveform, report_ams_result,
    run_all_ams_analyses_cancellable, run_ams_analysis_cancellable, AmsResult, FlowCancellation,
};
use std::collections::BTreeMap;

pub(crate) fn functional_coverage_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let waveform_source = required_string(params, "vcd")?;
    let plan_source = required_string(params, "coverage")?;
    let waveform = parse_waveform(&waveform_source)?;
    let plan = parse_covergroups(&plan_source)?;
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let result = functional_coverage(&waveform, &plan)?;
    if cancellation.is_cancelled() {
        return Err("functional coverage analysis cancelled".into());
    }
    let bins = result
        .bins
        .iter()
        .map(|bin| {
            Ok(Json::object([
                ("group", Json::string(bin.group.clone())),
                ("point", Json::string(bin.point.clone())),
                ("bin", Json::string(bin.bin.clone())),
                ("hits", Json::from(bin.hits)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let crosses = result
        .crosses
        .iter()
        .map(|cross| {
            Ok(Json::object([
                ("group", Json::string(cross.group.clone())),
                ("cross", Json::string(cross.cross.clone())),
                (
                    "combination",
                    Json::Array(
                        cross
                            .combination
                            .iter()
                            .map(|(point, bin)| {
                                Json::object([
                                    ("point", Json::string(point.clone())),
                                    ("bin", Json::string(bin.clone())),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("hits", Json::from(cross.hits)),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let toggles = result
        .toggles
        .iter()
        .map(|toggle| {
            Ok(Json::object([
                ("name", Json::string(toggle.name.clone())),
                ("transitions", Json::from(toggle.transitions)),
                ("samples", Json::number(toggle.samples as f64)?),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("samples", Json::number(result.samples as f64)?),
        ("bins", Json::Array(bins)),
        ("crosses", Json::Array(crosses)),
        ("toggles", Json::Array(toggles)),
        ("percentage", Json::number(result.percentage)?),
        ("report", Json::string(coverage_report(&result))),
    ]))
}

pub(crate) fn assertions_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let waveform_source = required_string(params, "vcd")?;
    let assertion_source = required_string(params, "sva")?;
    let waveform = parse_waveform(&waveform_source)?;
    let assertions = parse_assertions(&assertion_source)?;
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let results = check_assertions(&waveform, &assertions)?;
    if cancellation.is_cancelled() {
        return Err("assertion analysis cancelled".into());
    }
    let values = results
        .iter()
        .map(|result| {
            Ok(Json::object([
                ("name", Json::string(result.name.clone())),
                ("kind", Json::string(assertion_kind(&result.kind))),
                ("status", Json::string(assertion_status(&result.status))),
                (
                    "sample",
                    match result.status {
                        eda_api::AssertionStatus::Fail { sample }
                        | eda_api::AssertionStatus::CoverHit { sample } => {
                            Json::from(sample as u64)
                        }
                        _ => Json::Null,
                    },
                ),
                ("samples", Json::from(result.samples as u64)),
                (
                    "clock",
                    result.clock.clone().map_or(Json::Null, Json::string),
                ),
                ("passed", Json::Bool(result.passed())),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("count", Json::from(results.len() as u64)),
        (
            "passed",
            Json::from(results.iter().filter(|result| result.passed()).count() as u64),
        ),
        ("results", Json::Array(values)),
        ("report", Json::string(assertion_report(&results))),
    ]))
}

fn assertion_kind(kind: &eda_api::AssertionKind) -> &'static str {
    match kind {
        eda_api::AssertionKind::Assert => "assert",
        eda_api::AssertionKind::Assume => "assume",
        eda_api::AssertionKind::Cover => "cover",
    }
}

fn assertion_status(status: &eda_api::AssertionStatus) -> &'static str {
    match status {
        eda_api::AssertionStatus::Pass => "pass",
        eda_api::AssertionStatus::Fail { .. } => "fail",
        eda_api::AssertionStatus::CoverHit { .. } => "coverHit",
        eda_api::AssertionStatus::CoverMiss => "coverMiss",
        eda_api::AssertionStatus::Unknown => "unknown",
    }
}

pub(crate) fn waveform_measure_rpc(
    params: &Json,
    cancellation: &FlowCancellation,
) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let source = required_string(params, "vcd")?;
    let signal_name = required_string(params, "signal")?;
    let waveform = parse_waveform(&source)?;
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let clock = waveform.clock_metrics(&signal_name)?;
    let pulses = waveform.pulse_metrics(&signal_name)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let comparison = match fields.get("compareWith") {
        None | Some(Json::Null) => None,
        Some(Json::String(other)) => Some(waveform.compare_signals(&signal_name, other)?),
        Some(_) => return Err("'compareWith' must be a signal name string".into()),
    };
    if cancellation.is_cancelled() {
        return Err("waveform measurement cancelled".into());
    }
    let mut result = vec![
        ("signal".to_string(), Json::string(signal_name.clone())),
        ("clock".to_string(), clock_metrics_json(&clock)?),
        ("pulses".to_string(), pulse_metrics_json(&pulses)?),
    ];
    if let Some(comparison) = comparison {
        result.push(("comparison".to_string(), comparison_json(&comparison)?));
    }
    Ok(Json::Object(result.into_iter().collect()))
}

fn clock_metrics_json(metrics: &eda_api::ClockMetrics) -> Result<Json, String> {
    Ok(Json::object([
        ("riseCount", Json::from(metrics.rise_count as u64)),
        ("fallCount", Json::from(metrics.fall_count as u64)),
        ("periodCount", Json::from(metrics.period_count as u64)),
        ("minPeriodNs", Json::number(metrics.min_period_ns)?),
        ("maxPeriodNs", Json::number(metrics.max_period_ns)?),
        ("averagePeriodNs", Json::number(metrics.average_period_ns)?),
        ("minHighNs", Json::number(metrics.min_high_ns)?),
        ("maxHighNs", Json::number(metrics.max_high_ns)?),
        ("averageHighNs", Json::number(metrics.average_high_ns)?),
        ("minLowNs", Json::number(metrics.min_low_ns)?),
        ("maxLowNs", Json::number(metrics.max_low_ns)?),
        ("averageLowNs", Json::number(metrics.average_low_ns)?),
        ("frequencyMhz", Json::number(metrics.frequency_mhz)?),
        (
            "dutyCyclePercent",
            Json::number(metrics.duty_cycle_percent)?,
        ),
    ]))
}

fn pulse_metrics_json(metrics: &eda_api::PulseMetrics) -> Result<Json, String> {
    Ok(Json::object([
        ("highCount", Json::from(metrics.high_count as u64)),
        ("lowCount", Json::from(metrics.low_count as u64)),
        ("minHighNs", Json::number(metrics.min_high_ns)?),
        ("maxHighNs", Json::number(metrics.max_high_ns)?),
        ("averageHighNs", Json::number(metrics.average_high_ns)?),
        ("minLowNs", Json::number(metrics.min_low_ns)?),
        ("maxLowNs", Json::number(metrics.max_low_ns)?),
        ("averageLowNs", Json::number(metrics.average_low_ns)?),
        (
            "dutyCyclePercent",
            Json::number(metrics.duty_cycle_percent)?,
        ),
    ]))
}

fn comparison_json(comparison: &eda_api::SignalComparison) -> Result<Json, String> {
    Ok(Json::object([
        ("sampleCount", Json::from(comparison.sample_count as u64)),
        (
            "matchingSamples",
            Json::from(comparison.matching_samples as u64),
        ),
        (
            "differingSamples",
            Json::from(comparison.differing_samples as u64),
        ),
        (
            "unknownSamples",
            Json::from(comparison.unknown_samples as u64),
        ),
        (
            "hammingDistance",
            Json::from(comparison.hamming_distance as u64),
        ),
    ]))
}

pub(crate) fn spice_dc(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    if cancellation.is_cancelled() {
        return Err("DC operating point RPC cancelled".into());
    }
    let source = required_string(params, "source")?;
    let circuit = parse_spice(&source)?;
    let op = eda_api::dc_operating_point_cancellable(&circuit, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("DC operating point RPC cancelled".into());
    }
    let nodes = op
        .node_voltages
        .iter()
        .map(|(node, value)| Ok((node.clone(), Json::number(*value)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let branches = op
        .branch_currents
        .iter()
        .map(|(branch, value)| Ok((branch.clone(), Json::number(*value)?)))
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    Ok(Json::object([
        ("title", Json::string(circuit.title.clone())),
        ("converged", Json::Bool(op.converged)),
        ("iterations", Json::number(op.iterations as f64)?),
        ("nodeVoltages", Json::Object(nodes)),
        ("branchCurrents", Json::Object(branches)),
    ]))
}

pub(crate) fn spice_run(params: &Json, cancellation: &FlowCancellation) -> Result<Json, String> {
    let source = required_string(params, "source")?;
    let circuit = parse_spice(&source)?;
    let fields = params
        .as_object()
        .ok_or_else(|| "params must be an object".to_string())?;
    let all = matches!(fields.get("all"), Some(Json::Bool(true)));
    if all {
        let results = run_all_ams_analyses_cancellable(&circuit, cancellation)?;
        let reports = results
            .iter()
            .map(report_ams_result)
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Json::object([
            ("kinds", Json::Array(results.iter().map(ams_kind).collect())),
            ("count", Json::number(results.len() as f64)?),
            ("reports", Json::string(reports)),
        ]));
    }
    let index = match fields.get("analysisIndex") {
        None => 0_usize,
        Some(Json::Number(value)) => {
            let index = *value;
            if index < 0.0 || index.fract() != 0.0 || index > usize::MAX as f64 {
                return Err("'analysisIndex' must be a non-negative integer".into());
            }
            index as usize
        }
        Some(Json::Null) => 0_usize,
        Some(_) => return Err("'analysisIndex' must be a non-negative integer".into()),
    };
    let analysis = circuit
        .analyses
        .get(index)
        .ok_or_else(|| format!("circuit has no analysis at index {index}"))?;
    let result = run_ams_analysis_cancellable(&circuit, analysis, cancellation)?;
    let mut fields = vec![
        ("kind".to_string(), ams_kind(&result)),
        ("analysisIndex".to_string(), Json::number(index as f64)?),
        (
            "report".to_string(),
            Json::string(report_ams_result(&result)),
        ),
    ];
    if let AmsResult::Transient(transient) = &result {
        fields.push(("result".to_string(), transient_result_json(transient)?));
    }
    Ok(Json::Object(fields.into_iter().collect()))
}

fn transient_result_json(result: &eda_api::TransientResult) -> Result<Json, String> {
    fn numeric_map(values: &BTreeMap<String, f64>) -> Result<Json, String> {
        let mut out = BTreeMap::new();
        for (name, value) in values {
            out.insert(name.clone(), Json::number(*value)?);
        }
        Ok(Json::Object(out))
    }
    let samples = result
        .samples
        .iter()
        .map(|sample| {
            let nodes = numeric_map(&sample.node_voltages)?;
            let branches = numeric_map(&sample.branch_currents)?;
            let capacitors = numeric_map(&sample.capacitor_currents)?;
            let temperatures = numeric_map(&sample.device_temperatures)?;
            Ok(Json::object([
                ("timeS", Json::number(sample.time_s)?),
                ("nodeVoltages", nodes),
                ("branchCurrents", branches),
                ("capacitorCurrents", capacitors),
                ("deviceTemperatures", temperatures),
            ]))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Json::object([
        ("method", Json::string(result.method.as_str())),
        ("stepS", Json::number(result.step_s)?),
        ("stopS", Json::number(result.stop_s)?),
        (
            "convergenceFailures",
            Json::from(result.convergence_failures as u64),
        ),
        ("sampleCount", Json::from(samples.len() as u64)),
        ("samples", Json::Array(samples)),
    ]))
}

fn ams_kind(result: &AmsResult) -> Json {
    let kind = match result {
        AmsResult::OperatingPoint(_) => "operatingPoint",
        AmsResult::DcSweep(_) => "dcSweep",
        AmsResult::Transient(_) => "transient",
        AmsResult::AcSweep(_) => "acSweep",
        AmsResult::NoiseSweep(_) => "noiseSweep",
        AmsResult::MonteCarlo(_) => "monteCarlo",
        AmsResult::Sensitivity(_) => "sensitivity",
        AmsResult::GlobalSensitivity(_) => "globalSensitivity",
        AmsResult::Derivative(_) => "derivative",
        AmsResult::Optimization(_) => "optimization",
        AmsResult::Pareto(_) => "pareto",
        AmsResult::PeriodicSteadyState(_) => "periodicSteadyState",
        AmsResult::EnvelopeFollow(_) => "envelopeFollow",
        AmsResult::Aging(_) => "aging",
    };
    Json::string(kind)
}
