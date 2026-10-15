use super::placement::{check_legal, net_pins};
use super::routing::TrackUsageMap;
use super::types::*;
use eda_core::netlist::*;
use std::collections::HashMap;

pub(super) fn shape_via(segments: &[Segment], net: usize) -> Option<Via> {
    let (a, b) = (
        segments.last()?,
        segments.get(segments.len().checked_sub(2)?)?,
    );
    let horizontal = match (a.orientation, b.orientation) {
        (Orientation::Horizontal, Orientation::Vertical) => a,
        (Orientation::Vertical, Orientation::Horizontal) => b,
        _ => return None,
    };
    let vertical = if std::ptr::eq(a, horizontal) { b } else { a };
    (horizontal.net == net && vertical.net == net).then_some(Via {
        net,
        x: vertical.channel,
        y: horizontal.channel,
    })
}

/// Validate channel ranges, track bounds, and cross-net track exclusivity.
pub fn check_routing(r: &Routing) -> Result<(), String> {
    if r.tracks_per_channel == 0 {
        return Err("routing has zero tracks per channel".into());
    }
    let mut used: TrackUsageMap = HashMap::new();
    let mut blocked = std::collections::BTreeSet::new();
    for blockage in &r.blocked_channels {
        blockage.validate()?;
        if !blocked.insert((blockage.orientation, blockage.channel)) {
            return Err(format!(
                "duplicate routing blockage for {:?} channel {}",
                blockage.orientation, blockage.channel
            ));
        }
    }
    for seg in &r.segments {
        if seg.start > seg.end {
            return Err(format!("reversed segment {:?}", seg));
        }
        if seg.track >= r.tracks_per_channel {
            return Err(format!("track {} out of range {:?}", seg.track, seg));
        }
        if blocked.contains(&(seg.orientation, seg.channel)) {
            return Err(format!(
                "segment {:?} crosses hard routing blockage {:?} channel {}",
                seg, seg.orientation, seg.channel
            ));
        }
        let ranges = used
            .entry((seg.orientation, seg.channel, seg.track))
            .or_default();
        if ranges
            .iter()
            .any(|&(s, e, net)| net != seg.net && seg.start <= e && seg.end >= s)
        {
            return Err(format!("track collision {:?}", seg));
        }
        ranges.push((seg.start, seg.end, seg.net));
    }
    for via in &r.via_locations {
        let horizontal = r.segments.iter().any(|seg| {
            seg.net == via.net
                && seg.orientation == Orientation::Horizontal
                && point_on_segment((via.x, via.y), seg)
        });
        let vertical = r.segments.iter().any(|seg| {
            seg.net == via.net
                && seg.orientation == Orientation::Vertical
                && point_on_segment((via.x, via.y), seg)
        });
        if !horizontal || !vertical {
            return Err(format!("via {:?} does not connect its net segments", via));
        }
    }
    Ok(())
}

/// Text summary of global routing.
pub fn routing_report(r: &Routing) -> String {
    let mut out = format!(
        "Routing report: {} segments, {} wirelength units, {} vias, {} tracks/channel\n",
        r.segments.len(),
        r.wirelength(),
        r.vias,
        r.tracks_per_channel
    );
    if !r.blocked_channels.is_empty() {
        out.push_str(&format!("  hard blockages: {}\n", r.blocked_channels.len()));
    }
    out
}

/// Run placement/routing design-rule checks.
///
/// Errors cover illegal placement, invalid track allocation, out-of-bounds
/// channels, cross-net shorts, and disconnected multi-pin nets. Warnings
/// cover conservative antenna-ratio violations.
pub fn drc(nl: &Netlist, p: &Placement, r: &Routing) -> Result<DrcResult, String> {
    drc_with_options(nl, p, r, &DrcOptions::default())
}

/// Run placement/routing DRC with an explicit foundry antenna-ratio rule.
pub fn drc_with_options(
    nl: &Netlist,
    p: &Placement,
    r: &Routing,
    options: &DrcOptions,
) -> Result<DrcResult, String> {
    options.validate()?;
    let mut out = DrcResult::default();

    if p.cell_width.len() != nl.cell_count() || p.cell_pos.len() != nl.cell_count() {
        out.violations.push(DrcViolation {
            code: "PLACE-001".into(),
            severity: DrcSeverity::Error,
            message: format!(
                "placement width/vector length does not match {} cells",
                nl.cell_count()
            ),
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }
    if let Err(e) = check_legal(nl, p) {
        out.violations.push(DrcViolation {
            code: "PLACE-002".into(),
            severity: DrcSeverity::Error,
            message: e,
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }
    if let Err(e) = check_routing(r) {
        out.violations.push(DrcViolation {
            code: "ROUTE-001".into(),
            severity: DrcSeverity::Error,
            message: e,
            cell: None,
            net: None,
            row: None,
            site: None,
        });
    }

    for seg in &r.segments {
        if seg.net >= nl.net_count() {
            out.violations.push(DrcViolation {
                code: "ROUTE-002".into(),
                severity: DrcSeverity::Error,
                message: format!("segment references unknown net {}", seg.net),
                cell: None,
                net: Some(seg.net),
                row: if seg.orientation == Orientation::Horizontal {
                    Some(seg.channel)
                } else {
                    None
                },
                site: Some(seg.channel),
            });
            continue;
        }

        let max_channel = match seg.orientation {
            Orientation::Horizontal => p.rows,
            Orientation::Vertical => SITES_PER_ROW,
        };
        let max_range = match seg.orientation {
            Orientation::Horizontal => SITES_PER_ROW,
            Orientation::Vertical => p.rows,
        };
        if seg.channel > max_channel || seg.end > max_range {
            out.violations.push(DrcViolation {
                code: "ROUTE-003".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "segment for net '{}' outside die bounds",
                    nl.net(seg.net).name
                ),
                cell: None,
                net: Some(seg.net),
                row: if seg.orientation == Orientation::Horizontal {
                    Some(seg.channel)
                } else {
                    None
                },
                site: Some(seg.channel),
            });
        }
    }

    for via in &r.via_locations {
        if via.net >= nl.net_count() {
            out.violations.push(DrcViolation {
                code: "VIA-001".into(),
                severity: DrcSeverity::Error,
                message: format!("via references unknown net {}", via.net),
                cell: None,
                net: Some(via.net),
                row: Some(via.y),
                site: Some(via.x),
            });
        }
        if via.x > SITES_PER_ROW || via.y > p.rows {
            out.violations.push(DrcViolation {
                code: "VIA-002".into(),
                severity: DrcSeverity::Error,
                message: format!("via {:?} outside die bounds", via),
                cell: None,
                net: Some(via.net),
                row: Some(via.y),
                site: Some(via.x),
            });
        }
    }

    let mut segments_by_net = vec![Vec::<Segment>::new(); nl.net_count()];
    let mut vias_by_net = vec![Vec::<Via>::new(); nl.net_count()];
    for seg in &r.segments {
        if seg.net < segments_by_net.len() {
            segments_by_net[seg.net].push(*seg);
        }
    }
    for via in &r.via_locations {
        if via.net < vias_by_net.len() {
            vias_by_net[via.net].push(*via);
        }
    }

    for ni in 0..nl.net_count() {
        let pins = net_pins(nl, p, ni);
        let components = connected_pin_components(&pins, &segments_by_net[ni], &vias_by_net[ni]);

        if pins.len() >= 2 && components > 1 {
            out.violations.push(DrcViolation {
                code: "OPEN-001".into(),
                severity: DrcSeverity::Error,
                message: format!(
                    "net '{}' has {} physically disconnected pin group(s)",
                    nl.net(ni).name,
                    components
                ),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }

        let pin_count = pins.len().max(1);
        let ratio = segments_by_net[ni]
            .iter()
            .map(Segment::length)
            .sum::<usize>() as f64
            / pin_count as f64;
        if ratio > options.max_antenna_ratio {
            out.violations.push(DrcViolation {
                code: "ANT-001".into(),
                severity: DrcSeverity::Warning,
                message: format!(
                    "net '{}' antenna ratio {:.2} exceeds {:.2}",
                    nl.net(ni).name,
                    ratio,
                    options.max_antenna_ratio
                ),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }

        if !segments_by_net[ni].is_empty() && pins.is_empty() {
            out.violations.push(DrcViolation {
                code: "DANGLE-001".into(),
                severity: DrcSeverity::Error,
                message: format!("net '{}' has routing but no physical pins", nl.net(ni).name),
                cell: None,
                net: Some(ni),
                row: None,
                site: None,
            });
        }
    }

    out.violations.sort_by(|a, b| {
        (a.severity, a.code.clone(), a.message.clone()).cmp(&(
            b.severity,
            b.code.clone(),
            b.message.clone(),
        ))
    });
    Ok(out)
}

pub(super) fn point_on_segment(point: (usize, usize), seg: &Segment) -> bool {
    match seg.orientation {
        Orientation::Horizontal => {
            point.1 == seg.channel && point.0 >= seg.start && point.0 <= seg.end
        }
        Orientation::Vertical => {
            point.0 == seg.channel && point.1 >= seg.start && point.1 <= seg.end
        }
    }
}

pub(super) fn segments_touch(a: &Segment, b: &Segment) -> bool {
    match (a.orientation, b.orientation) {
        (Orientation::Horizontal, Orientation::Vertical) => {
            b.channel >= a.start && b.channel <= a.end && a.channel >= b.start && a.channel <= b.end
        }
        (Orientation::Vertical, Orientation::Horizontal) => segments_touch(b, a),
        (Orientation::Horizontal, Orientation::Horizontal) => {
            a.channel == b.channel && a.track == b.track && a.start <= b.end && b.start <= a.end
        }
        (Orientation::Vertical, Orientation::Vertical) => {
            a.channel == b.channel && a.track == b.track && a.start <= b.end && b.start <= a.end
        }
    }
}

pub(super) fn connected_pin_components(
    pins: &[(usize, usize)],
    segments: &[Segment],
    vias: &[Via],
) -> usize {
    if pins.is_empty() {
        return 0;
    }
    let mut parent: Vec<usize> = (0..pins.len() + segments.len()).collect();

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (a, b) = (find(parent, a), find(parent, b));
        if a != b {
            parent[a] = b;
        }
    }

    for i in 0..pins.len() {
        for j in i + 1..pins.len() {
            if pins[i] == pins[j] {
                union(&mut parent, i, j);
            }
        }
        for (si, seg) in segments.iter().enumerate() {
            if point_on_segment(pins[i], seg) {
                union(&mut parent, i, pins.len() + si);
            }
        }
    }
    for i in 0..segments.len() {
        for j in i + 1..segments.len() {
            let a = &segments[i];
            let b = &segments[j];
            let connected = if a.orientation == b.orientation {
                a.net == b.net && segments_touch(a, b)
            } else {
                a.net == b.net
                    && segments_touch(a, b)
                    && vias.iter().any(|via| {
                        via.net == a.net
                            && point_on_segment((via.x, via.y), a)
                            && point_on_segment((via.x, via.y), b)
                    })
            };
            if connected {
                union(&mut parent, pins.len() + i, pins.len() + j);
            }
        }
    }

    let mut roots = std::collections::HashSet::new();
    for i in 0..pins.len() {
        roots.insert(find(&mut parent, i));
    }
    roots.len()
}

/// Human-readable DRC summary.
pub fn drc_report(r: &DrcResult) -> String {
    let mut s = format!(
        "DRC report: {} error(s), {} warning(s)\n",
        r.error_count(),
        r.warning_count()
    );
    for v in &r.violations {
        s.push_str(&format!(
            "  {}: {} [{}]{}\n",
            v.code,
            v.message,
            if v.severity == DrcSeverity::Error {
                "error"
            } else {
                "warning"
            },
            v.net.map(|n| format!(" net={}", n)).unwrap_or_default()
        ));
    }
    s
}

/// Text summary of the placement.
pub fn report(nl: &Netlist, p: &Placement) -> String {
    let used: usize = p.cell_width.iter().sum();
    let mut out = format!(
        "PnR report for '{}': {} cells, {} rows x {} sites, utilization {:.1}%\n",
        nl.name,
        nl.cell_count(),
        p.rows,
        SITES_PER_ROW,
        100.0 * used as f64 / (p.rows * SITES_PER_ROW) as f64
    );
    if !p.blockages.is_empty() {
        out.push_str(&format!(
            "  hard placement blockages: {}\n",
            p.blockages.len()
        ));
    }
    out
}
