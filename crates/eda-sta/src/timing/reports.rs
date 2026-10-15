//! Core timing implementation module.

use super::library::*;
use super::types::*;
use eda_core::netlist::*;
use std::collections::*;

pub fn worst_slack(r: &StaResult, period: f64) -> f64 {
    let limiting = r
        .reg2reg_critical
        .max(r.in2reg_critical)
        .max(r.critical_path_delay);
    period - limiting
}

/// Worst unconstrained hold slack in nanoseconds. Hold uncertainty tightens
/// the required minimum data-stable time.
pub fn worst_hold_slack(nl: &Netlist, r: &StaResult, hold_uncertainty: f64) -> f64 {
    r.hold_endpoint_list()
        .iter()
        .map(|endpoint| {
            let data_net = nl.cell(endpoint.cell).inputs.get(1).copied();
            let clock_net = nl.cell(endpoint.cell).inputs.first().copied();
            let hold = seq_hold_at(
                nl,
                endpoint.cell,
                data_net.and_then(|net| r.net_min_transition(net)),
                clock_net.and_then(|net| r.net_min_transition(net)),
            )
            .unwrap_or_else(|_| seq_hold(nl, &nl.cell(endpoint.cell).kind).unwrap_or(0.0));
            endpoint.data_arrival - hold - hold_uncertainty
        })
        .fold(f64::INFINITY, f64::min)
}

pub(super) fn generic_input_pin_label(index: usize) -> String {
    match index {
        0 => "a".into(),
        1 => "b".into(),
        2 => "c".into(),
        3 => "d".into(),
        index => index.to_string(),
    }
}

pub(super) fn timing_arc_input_pin(nl: &Netlist, arc: &TimingArc) -> String {
    let Some(cell_id) = arc.cell else {
        return arc
            .from_net
            .map(|net| nl.net(net).name.clone())
            .unwrap_or_else(|| "<none>".into());
    };
    let cell = nl.cell(cell_id);
    match (&cell.kind, arc.from_net) {
        (CellKind::Lib(id), Some(from_net)) => {
            let Some(library) = nl.library() else {
                return "<none>".into();
            };
            let definition = library.get(*id);
            cell.inputs
                .iter()
                .position(|&net| net == from_net)
                .and_then(|pin| definition.inputs.get(pin).cloned())
                .unwrap_or_else(|| "<none>".into())
        }
        (_, Some(from_net)) => cell
            .inputs
            .iter()
            .position(|&net| net == from_net)
            .map(generic_input_pin_label)
            .unwrap_or_else(|| "<none>".into()),
        (_, None) => "<none>".into(),
    }
}

pub(super) fn timing_arc_output_pin(nl: &Netlist, arc: &TimingArc) -> String {
    let Some(cell_id) = arc.cell else {
        return nl.net(arc.to_net).name.clone();
    };
    output_pin_name(nl, cell_id).unwrap_or_else(|| nl.net(arc.to_net).name.clone())
}

pub(super) fn push_timing_arc_report(nl: &Netlist, arc: &TimingArc, out: &mut String) {
    let from_net = arc
        .from_net
        .map(|net| nl.net(net).name.clone())
        .unwrap_or_else(|| "<none>".into());
    let to_net = nl.net(arc.to_net).name.clone();
    let description = match arc.kind {
        TimingArcKind::PrimaryInputLaunch => format!(
            "input {} -> {}: launch delay {:.6} ns, arrival {:.6} ns",
            from_net, to_net, arc.delay_ns, arc.to_arrival_ns
        ),
        TimingArcKind::ClockToOutput => {
            let cell_name = arc
                .cell
                .map(|cell| nl.cell(cell).name.clone())
                .unwrap_or_default();
            let input_pin = timing_arc_input_pin(nl, arc);
            let output_pin = timing_arc_output_pin(nl, arc);
            format!(
                "{}/{} -> {}/{}: clock-to-output {:.6} ns, arrival {:.6} ns",
                cell_name, input_pin, cell_name, output_pin, arc.delay_ns, arc.to_arrival_ns
            )
        }
        TimingArcKind::Combinational => {
            let cell_name = arc
                .cell
                .map(|cell| nl.cell(cell).name.clone())
                .unwrap_or_default();
            let input_pin = timing_arc_input_pin(nl, arc);
            let output_pin = timing_arc_output_pin(nl, arc);
            format!(
                "{}/{} -> {}/{}: combinational {:.6} ns, arrival {:.6} ns",
                cell_name, input_pin, cell_name, output_pin, arc.delay_ns, arc.to_arrival_ns
            )
        }
    };
    out.push_str(&format!("    {}\n", description));
}

pub(crate) fn push_timing_arcs_report(nl: &Netlist, arcs: &[TimingArc], out: &mut String) {
    if arcs.is_empty() {
        out.push_str("    arcs: <none>\n");
        return;
    }
    out.push_str("    arcs:\n");
    for arc in arcs {
        push_timing_arc_report(nl, arc, out);
    }
}

/// Human-readable timing-path report.
pub fn timing_path_report(nl: &Netlist, paths: &[TimingPath]) -> String {
    let mut out = format!("Timing path report: {} path(s)\n", paths.len());
    for path in paths {
        let kind = match path.kind {
            TimingPathKind::InputToSequential => "input-to-sequential",
            TimingPathKind::SequentialToSequential => "sequential-to-sequential",
            TimingPathKind::InputToOutput => "input-to-output",
            TimingPathKind::SequentialToOutput => "sequential-to-output",
        };
        let launch = match &path.launch {
            TimingLaunch::PrimaryInput(name) => format!("input {}", name),
            TimingLaunch::SequentialCell(name) => format!("flop {}", name),
        };
        let endpoint = match &path.endpoint {
            TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
            TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
        };
        out.push_str(&format!(
            "  {} -> {}: arrival {:.6} ns\n    launch: {}\n    endpoint: {}\n",
            kind, endpoint, path.arrival_ns, launch, endpoint
        ));
        if path.cells.is_empty() {
            out.push_str("    cells: <none>\n");
        } else {
            let cells = path
                .cells
                .iter()
                .map(|&cell| nl.cell(cell).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            out.push_str(&format!("    cells: {}\n", cells));
        }
        let nets = path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    nets: {}\n", nets));
        push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

/// Group one timing path by the hierarchy instance that owns each cell.
pub fn hierarchical_timing_path(
    nl: &Netlist,
    path: &TimingPath,
) -> Result<HierarchicalTimingPath, String> {
    #[derive(Clone)]
    struct OpenSegment {
        instance: Option<String>,
        module: Option<String>,
        first_arc: usize,
        last_arc: usize,
        cells: Vec<usize>,
        delay_ns: f64,
    }

    let mut segments: Vec<OpenSegment> = Vec::new();
    let mut crossings = Vec::new();
    let mut previous_instance: Option<String> = None;

    for (arc_index, arc) in path.arcs.iter().enumerate() {
        if !arc.delay_ns.is_finite() || arc.delay_ns < 0.0 {
            return Err(format!(
                "timing arc {} has invalid delay {}",
                arc_index, arc.delay_ns
            ));
        }
        let owner = arc
            .cell
            .map(|cell| {
                if cell >= nl.cell_count() {
                    return Err(format!("timing arc {} references cell {}", arc_index, cell));
                }
                Ok(nl
                    .cell_hierarchy_instance(cell)
                    .map(|instance| (instance.path.clone(), instance.module.clone())))
            })
            .transpose()?
            .flatten();
        let (instance, module) = owner
            .clone()
            .map_or((None, None), |(path, module)| (Some(path), Some(module)));

        if segments.last().map(|segment| &segment.instance) != Some(&instance) {
            let previous = previous_instance.clone();
            if previous != instance {
                let cell = arc
                    .cell
                    .ok_or_else(|| format!("crossing arc {} has no cell", arc_index))?;
                crossings.push(HierarchicalTimingCrossing {
                    arc: arc_index,
                    cell,
                    cell_name: nl.cell(cell).name.clone(),
                    from_instance: previous,
                    to_instance: instance.clone(),
                });
            }
            segments.push(OpenSegment {
                instance: instance.clone(),
                module,
                first_arc: arc_index,
                last_arc: arc_index,
                cells: Vec::new(),
                delay_ns: 0.0,
            });
        }
        let segment = segments
            .last_mut()
            .ok_or_else(|| format!("timing arc {} produced no hierarchy segment", arc_index))?;
        segment.last_arc = arc_index;
        segment.delay_ns += arc.delay_ns;
        if let Some(cell) = arc.cell {
            if !segment.cells.contains(&cell) {
                segment.cells.push(cell);
            }
        }
        previous_instance = instance;
    }

    Ok(HierarchicalTimingPath {
        path: path.clone(),
        segments: segments
            .into_iter()
            .map(|segment| HierarchicalTimingSegment {
                instance: segment.instance,
                module: segment.module,
                first_arc: segment.first_arc,
                last_arc: segment.last_arc,
                cells: segment.cells,
                delay_ns: segment.delay_ns,
            })
            .collect(),
        crossings,
    })
}

/// Group timing paths by retained hierarchy instances.
pub fn hierarchical_timing_paths(
    nl: &Netlist,
    paths: &[TimingPath],
) -> Result<Vec<HierarchicalTimingPath>, String> {
    paths
        .iter()
        .map(|path| hierarchical_timing_path(nl, path))
        .collect()
}

/// Render a hierarchy-grouped timing path report.
pub fn hierarchical_timing_path_report(paths: &[HierarchicalTimingPath]) -> String {
    let mut out = format!("Hierarchical timing path report: {} path(s)\n", paths.len());
    for hierarchical in paths {
        let path = &hierarchical.path;
        out.push_str(&format!(
            "  arrival {:.6} ns, {} segment(s), {} crossing(s)\n",
            path.arrival_ns,
            hierarchical.segments.len(),
            hierarchical.crossings.len()
        ));
        for segment in &hierarchical.segments {
            let instance = segment.instance.as_deref().unwrap_or("<top-level>");
            let module = segment.module.as_deref().unwrap_or("<flat>");
            out.push_str(&format!(
                "    instance '{}' module '{}': arcs {}..{}, cells {}, delay {:.6} ns\n",
                instance,
                module,
                segment.first_arc,
                segment.last_arc,
                segment.cells.len(),
                segment.delay_ns
            ));
        }
        for crossing in &hierarchical.crossings {
            out.push_str(&format!(
                "    crossing arc {}: {} -> {} at cell '{}'\n",
                crossing.arc,
                crossing.from_instance.as_deref().unwrap_or("<top-level>"),
                crossing.to_instance.as_deref().unwrap_or("<top-level>"),
                crossing.cell_name
            ));
        }
    }
    out
}

/// Human-readable hold-path report.
pub fn hold_timing_path_report(nl: &Netlist, paths: &[HoldTimingPath]) -> String {
    let mut out = format!("Hold timing path report: {} path(s)\n", paths.len());
    for hold in paths {
        let path = &hold.path;
        out.push_str(&format!(
            "  hold slack {:.6} ns, arrival {:.6} ns, required {:.6} ns, hold {:.6} ns\n",
            hold.slack_ns, path.arrival_ns, hold.required_ns, hold.hold_ns
        ));
        let cells = path
            .cells
            .iter()
            .map(|&cell| nl.cell(cell).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        let nets = path
            .nets
            .iter()
            .map(|&net| nl.net(net).name.clone())
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!("    cells: {}\n    nets: {}\n", cells, nets));
        push_timing_arcs_report(nl, &path.arcs, &mut out);
    }
    out
}

/// Human-readable asynchronous-reset path report.
pub fn reset_timing_path_report(nl: &Netlist, paths: &[ResetTimingPaths]) -> String {
    let mut out = format!("Reset timing path report: {} endpoint(s)\n", paths.len());
    for reset in paths {
        for (kind, path) in [("recovery", &reset.recovery), ("removal", &reset.removal)] {
            let cells = path
                .cells
                .iter()
                .map(|&cell| nl.cell(cell).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            let nets = path
                .nets
                .iter()
                .map(|&net| nl.net(net).name.clone())
                .collect::<Vec<_>>()
                .join(" -> ");
            out.push_str(&format!(
                "  {} arrival {:.6} ns\n    endpoint: {}\n    cells: {}\n    nets: {}\n",
                kind,
                path.arrival_ns,
                match &path.endpoint {
                    TimingPathEndpoint::SequentialCell(name) => format!("flop {}", name),
                    TimingPathEndpoint::OutputPort(name) => format!("output {}", name),
                },
                cells,
                nets
            ));
            push_timing_arcs_report(nl, &path.arcs, &mut out);
        }
    }
    out
}

/// Analyze `nl`. Requires library-backed `Lib` cells (`Netlist::set_library`)
/// for arc delays; generic primitives get a nominal 0.5 ns per level.
pub fn path_delay(nl: &Netlist, cells: &[usize]) -> f64 {
    cells.iter().map(|&c| arc_delay(nl, c, 1.0)).sum()
}

/// Human-readable timing report.
pub fn report(nl: &Netlist, r: &StaResult) -> String {
    format!(
        "STA report for '{}':\n  critical path : {:.3} ns ({} cells)\n  setup reg2reg : {:.3} ns\n  setup in2reg  : {:.3} ns\n  hold reg2reg  : {:.3} ns\n  hold in2reg   : {:.3} ns\n",
        nl.name,
        r.critical_path_delay,
        r.critical_cells.len(),
        r.reg2reg_critical,
        r.in2reg_critical,
        r.reg2reg_hold_critical,
        r.in2reg_hold_critical
    )
}

/// Map of net name → arrival time (debug helper).
pub fn arrival_by_name(nl: &Netlist, r: &StaResult) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (i, n) in nl.nets().iter().enumerate() {
        if let Some(&t) = r.arrivals.get(i) {
            if t.is_finite() {
                m.insert(n.name.clone(), t);
            }
        }
    }
    m
}

/// Map of net name → earliest arrival time (hold/min-delay debug helper).
pub fn min_arrival_by_name(nl: &Netlist, r: &StaResult) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for (i, n) in nl.nets().iter().enumerate() {
        if let Some(&t) = r.min_arrivals.get(i) {
            if t.is_finite() {
                m.insert(n.name.clone(), t);
            }
        }
    }
    m
}
