//! LEF/DEF-style physical interchange.
//!
//! The writer emits the compact physical subset consumed by placement and
//! routing tools: technology site, macro dimensions/pins, die/rows, placed
//! components, pins, and routed net segments. `parse_def` reads that subset
//! back for round-trip checks and external physical flow integration.

use crate::{Placement, Routing, SITES_PER_ROW};
use eda_core::netlist::{CellKind, Netlist};

const SITE_DBU: usize = 100;
const ROW_DBU: usize = 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefComponent {
    pub name: String,
    pub macro_name: String,
    pub x: usize,
    pub y: usize,
    pub orientation: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefRoute {
    pub layer: String,
    pub x1: usize,
    pub y1: usize,
    pub x2: usize,
    pub y2: usize,
    pub track: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefVia {
    pub name: String,
    pub x: usize,
    pub y: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefNet {
    pub name: String,
    pub routes: Vec<DefRoute>,
    pub vias: Vec<DefVia>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefPlacementBlockage {
    pub name: String,
    pub row: usize,
    pub start_site: usize,
    pub end_site: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefRouteBlockage {
    pub name: String,
    pub orientation: crate::Orientation,
    pub channel: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    pub design: String,
    pub rows: usize,
    pub tracks_per_channel: usize,
    pub components: Vec<DefComponent>,
    pub nets: Vec<DefNet>,
    pub placement_blockages: Vec<DefPlacementBlockage>,
    pub route_blockages: Vec<DefRouteBlockage>,
}

/// Generate a LEF subset describing the site and every macro used by a placed
/// netlist. Dimensions are in LEF database units.
pub fn write_lef(nl: &Netlist, placement: &Placement) -> Result<String, String> {
    if placement.cell_width.len() != nl.cell_count() {
        return Err("LEF export placement/netlist cell count mismatch".into());
    }
    let mut out = String::new();
    out.push_str("VERSION 5.8 ;\n");
    out.push_str("BUSBITCHARS \"[]\" ;\n");
    out.push_str("DIVIDERCHAR \"/\" ;\n");
    out.push_str("UNITS\n  DATABASE MICRONS 100 ;\nEND UNITS\n\n");
    out.push_str("SITE core\n  CLASS CORE ;\n  SYMMETRY Y ;\n  SIZE 1.00 BY 10.00 ;\nEND core\n\n");

    for (ci, cell) in nl.cells().iter().enumerate() {
        let width = placement.cell_width[ci];
        let name = macro_name(nl, ci);
        let output = nl.net(cell.output).name.clone();
        out.push_str(&format!("MACRO {name}\n"));
        out.push_str("  CLASS CORE ;\n");
        out.push_str("  ORIGIN 0 0 ;\n");
        out.push_str(&format!("  SIZE {:.2} BY {:.2} ;\n", width as f64, 10.0));
        for &input_net in &cell.inputs {
            let pin = &nl.net(input_net).name;
            out.push_str(&format!(
                "  PIN {pin}\n    DIRECTION INPUT ;\n    USE SIGNAL ;\n  END {pin}\n"
            ));
        }
        out.push_str(&format!(
            "  PIN {output}\n    DIRECTION OUTPUT ;\n    USE SIGNAL ;\n  END {output}\n"
        ));
        out.push_str(&format!("END {name}\n\n"));
    }
    Ok(out)
}

/// Generate a DEF 5.8 subset from placement and global routing.
pub fn write_def(nl: &Netlist, placement: &Placement, routing: &Routing) -> Result<String, String> {
    crate::check_legal(nl, placement)?;
    let mut out = String::new();
    let die_y = placement.rows * ROW_DBU;
    out.push_str("VERSION 5.8 ;\n");
    out.push_str(&format!("DESIGN {} ;\n", nl.name));
    out.push_str("UNITS DISTANCE MICRONS 100 ;\n");
    out.push_str(&format!("TRACKS {} ;\n", routing.tracks_per_channel));
    out.push_str(&format!(
        "DIEAREA ( 0 0 ) ( {} {} ) ;\n\n",
        SITES_PER_ROW * SITE_DBU,
        die_y
    ));
    for row in 0..placement.rows {
        out.push_str(&format!(
            "ROW ROW_{row} core 0 {y} N DO {SITES_PER_ROW} BY 1 STEP {SITE_DBU} 0 ;\n",
            y = row * ROW_DBU
        ));
    }
    out.push_str(&format!("\nCOMPONENTS {} ;\n", nl.cell_count()));
    for (ci, cell) in nl.cells().iter().enumerate() {
        let pos = placement.cell_pos[ci];
        out.push_str(&format!(
            "- {} {} + PLACED ( {} {} ) N ;\n",
            cell.name,
            macro_name(nl, ci),
            pos.site * SITE_DBU,
            pos.row * ROW_DBU
        ));
    }
    out.push_str("END COMPONENTS\n\n");
    let blockage_count = placement.blockages.len() + routing.blocked_channels.len();
    out.push_str(&format!("BLOCKAGES {blockage_count} ;\n"));
    for blockage in &placement.blockages {
        out.push_str(&format!(
            "- PLACEMENT {} + RECT ( {} {} ) ( {} {} ) ;\n",
            blockage.name,
            blockage.start_site * SITE_DBU,
            blockage.row * ROW_DBU,
            (blockage.end_site + 1) * SITE_DBU,
            (blockage.row + 1) * ROW_DBU
        ));
    }
    for blockage in &routing.blocked_channels {
        let orientation = match blockage.orientation {
            crate::Orientation::Horizontal => "HORIZONTAL",
            crate::Orientation::Vertical => "VERTICAL",
        };
        out.push_str(&format!(
            "- ROUTE {} {} {} ;\n",
            blockage.name, orientation, blockage.channel
        ));
    }
    out.push_str("END BLOCKAGES\n\n");
    out.push_str("PINS 0 ;\nEND PINS\n\n");
    let net_count = nl.net_count();
    out.push_str(&format!("NETS {net_count} ;\n"));
    for ni in 0..net_count {
        let name = &nl.net(ni).name;
        out.push_str(&format!("- {name}\n"));
        for segment in routing.segments.iter().filter(|segment| segment.net == ni) {
            let (layer, x1, y1, x2, y2) = match segment.orientation {
                crate::Orientation::Horizontal => (
                    "metal1",
                    segment.start * SITE_DBU,
                    segment.channel * ROW_DBU,
                    segment.end * SITE_DBU,
                    segment.channel * ROW_DBU,
                ),
                crate::Orientation::Vertical => (
                    "metal2",
                    segment.channel * SITE_DBU,
                    segment.start * ROW_DBU,
                    segment.channel * SITE_DBU,
                    segment.end * ROW_DBU,
                ),
            };
            out.push_str(&format!(
                "  + ROUTED {layer} ( {x1} {y1} ) ( {x2} {y2} ) TRACK {track}\n",
                track = segment.track
            ));
        }
        for via in routing.via_locations.iter().filter(|via| via.net == ni) {
            out.push_str(&format!(
                "  + VIA M1_M2 ( {} {} )\n",
                via.x * SITE_DBU,
                via.y * ROW_DBU
            ));
        }
        out.push_str("  ;\n");
    }
    out.push_str("END NETS\n\nEND DESIGN\n");
    Ok(out)
}

/// Parse the physical DEF subset emitted by [`write_def`].
pub fn parse_def(source: &str) -> Result<Def, String> {
    let mut design = String::new();
    let mut rows = 0usize;
    let mut tracks_per_channel = 0usize;
    let mut components = Vec::new();
    let mut nets = Vec::new();
    let mut placement_blockages = Vec::new();
    let mut route_blockages = Vec::new();
    let mut in_components = false;
    let mut in_blockages = false;
    let mut in_nets = false;
    let mut pending_net: Option<DefNet> = None;

    for (index, raw) in source.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("DESIGN ") {
            design = rest.trim_end_matches(';').trim().to_string();
            continue;
        }
        if line.starts_with("ROW ") {
            rows += 1;
        }
        if let Some(rest) = line.strip_prefix("TRACKS ") {
            tracks_per_channel = rest
                .trim_end_matches(';')
                .trim()
                .parse::<usize>()
                .map_err(|_| format!("line {}: invalid track count", line_no))?;
        }
        if line.starts_with("COMPONENTS ") {
            in_components = true;
            continue;
        }
        if line.starts_with("END COMPONENTS") {
            in_components = false;
            continue;
        }
        if line.starts_with("BLOCKAGES ") {
            in_blockages = true;
            continue;
        }
        if line.starts_with("END BLOCKAGES") {
            in_blockages = false;
            continue;
        }
        if in_components {
            if let Some(rest) = line.strip_prefix("- ") {
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.len() < 9 || parts[2..4] != ["+", "PLACED"] {
                    return Err(format!("line {}: malformed component", line_no));
                }
                let x = parts[5]
                    .trim_matches(|c| c == '(' || c == ')')
                    .parse::<usize>()
                    .map_err(|_| format!("line {}: invalid component x", line_no))?;
                let y = parts[6]
                    .trim_matches(|c| c == '(' || c == ')')
                    .parse::<usize>()
                    .map_err(|_| format!("line {}: invalid component y", line_no))?;
                components.push(DefComponent {
                    name: parts[0].to_string(),
                    macro_name: parts[1].to_string(),
                    x,
                    y,
                    orientation: parts[8].trim_end_matches(';').to_string(),
                });
            }
        }
        if in_blockages {
            if let Some(rest) = line.strip_prefix("- PLACEMENT ") {
                let rest = rest.trim_end_matches(';').trim();
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.len() != 11
                    || parts[1..3] != ["+", "RECT"]
                    || parts[3] != "("
                    || parts[6] != ")"
                    || parts[7] != "("
                    || parts[10] != ")"
                {
                    return Err(format!("line {}: malformed placement blockage", line_no));
                }
                let coordinate = |text: &str| -> Result<usize, String> {
                    text.trim_matches(|c| matches!(c, '(' | ')' | ','))
                        .parse::<usize>()
                        .map_err(|_| format!("line {}: invalid blockage coordinate", line_no))
                };
                let x1 = coordinate(parts[4])?;
                let y1 = coordinate(parts[5])?;
                let x2 = coordinate(parts[8])?;
                let y2 = coordinate(parts[9])?;
                if x1 % SITE_DBU != 0
                    || x2 % SITE_DBU != 0
                    || y1 % ROW_DBU != 0
                    || y2 % ROW_DBU != 0
                    || x2 <= x1
                    || y2 <= y1
                {
                    return Err(format!(
                        "line {}: invalid placement blockage rectangle",
                        line_no
                    ));
                }
                placement_blockages.push(DefPlacementBlockage {
                    name: parts[0].to_string(),
                    row: y1 / ROW_DBU,
                    start_site: x1 / SITE_DBU,
                    end_site: x2 / SITE_DBU - 1,
                });
            } else if let Some(rest) = line.strip_prefix("- ROUTE ") {
                let rest = rest.trim_end_matches(';').trim();
                let parts: Vec<_> = rest.split_whitespace().collect();
                if parts.len() != 3 {
                    return Err(format!("line {}: malformed route blockage", line_no));
                }
                let orientation = match parts[1] {
                    "HORIZONTAL" => crate::Orientation::Horizontal,
                    "VERTICAL" => crate::Orientation::Vertical,
                    _ => {
                        return Err(format!(
                            "line {}: invalid route blockage orientation",
                            line_no
                        ))
                    }
                };
                route_blockages.push(DefRouteBlockage {
                    name: parts[0].to_string(),
                    orientation,
                    channel: parts[2]
                        .trim_end_matches(';')
                        .parse::<usize>()
                        .map_err(|_| format!("line {}: invalid route blockage channel", line_no))?,
                });
            }
        }

        if line.starts_with("NETS ") {
            in_nets = true;
            continue;
        }
        if line.starts_with("END NETS") {
            if let Some(net) = pending_net.take() {
                nets.push(net);
            }
            in_nets = false;
            continue;
        }
        if !in_nets {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- ") {
            if let Some(net) = pending_net.take() {
                nets.push(net);
            }
            pending_net = Some(DefNet {
                name: rest.trim().to_string(),
                routes: Vec::new(),
                vias: Vec::new(),
            });
        } else if line == ";" {
            if let Some(net) = pending_net.take() {
                nets.push(net);
            }
        } else if let Some(rest) = line.strip_prefix("+ ROUTED") {
            let parts: Vec<_> = rest.split_whitespace().collect();
            if parts.len() != 9 && parts.len() != 11 {
                return Err(format!("line {}: malformed route", line_no));
            }
            let track = if parts.len() == 11 {
                if parts[9] != "TRACK" {
                    return Err(format!("line {}: malformed route track", line_no));
                }
                parts[10]
                    .trim_end_matches(';')
                    .parse::<usize>()
                    .map_err(|_| format!("line {}: invalid route track", line_no))?
            } else {
                0
            };
            let coordinate = |text: &str| -> Result<usize, String> {
                text.trim_matches(|c| matches!(c, '(' | ')' | ','))
                    .parse::<usize>()
                    .map_err(|_| format!("line {}: invalid route coordinate", line_no))
            };
            let route = DefRoute {
                layer: parts[0].to_string(),
                x1: coordinate(parts[2])?,
                y1: coordinate(parts[3])?,
                x2: coordinate(parts[6])?,
                y2: coordinate(parts[7])?,
                track,
            };
            pending_net
                .as_mut()
                .ok_or_else(|| format!("line {}: route outside net", line_no))?
                .routes
                .push(route);
        } else if let Some(rest) = line.strip_prefix("+ VIA") {
            let parts: Vec<_> = rest.split_whitespace().collect();
            if parts.len() != 5 {
                return Err(format!("line {}: malformed via", line_no));
            }
            let coordinate = |text: &str| -> Result<usize, String> {
                text.trim_matches(|c| matches!(c, '(' | ')' | ','))
                    .parse::<usize>()
                    .map_err(|_| format!("line {}: invalid via coordinate", line_no))
            };
            let via = DefVia {
                name: parts[0].to_string(),
                x: coordinate(parts[2])? / SITE_DBU,
                y: coordinate(parts[3])? / ROW_DBU,
            };
            pending_net
                .as_mut()
                .ok_or_else(|| format!("line {}: via outside net", line_no))?
                .vias
                .push(via);
        }
    }

    if design.is_empty() {
        return Err("DEF has no DESIGN statement".into());
    }
    if rows == 0 {
        return Err("DEF has no placement rows".into());
    }
    Ok(Def {
        design,
        rows,
        tracks_per_channel,
        components,
        nets,
        placement_blockages,
        route_blockages,
    })
}

/// Reconstruct placement and routing objects from the compact DEF subset.
///
/// Components and nets are matched to the netlist by stable names. Numeric
/// net/cell IDs are therefore not part of the interchange format.
pub fn physical_from_def(nl: &Netlist, def: &Def) -> Result<(Placement, Routing), String> {
    if def.components.len() != nl.cell_count() {
        return Err(format!(
            "DEF has {} components but netlist has {} cells",
            def.components.len(),
            nl.cell_count()
        ));
    }

    let widths = crate::place(nl)?.cell_width;
    let mut cell_pos = vec![crate::Pos { row: 0, site: 0 }; nl.cell_count()];
    let mut seen_cells = std::collections::BTreeSet::new();
    for (component, position) in def.components.iter().zip(&mut cell_pos) {
        if !seen_cells.insert(component.name.clone()) {
            return Err(format!("duplicate DEF component '{}'", component.name));
        }
        if (0..nl.cell_count()).all(|cell| nl.cell(cell).name != component.name) {
            return Err(format!(
                "DEF component '{}' is missing from netlist",
                component.name
            ));
        }
        if component.x % SITE_DBU != 0 || component.y % ROW_DBU != 0 {
            return Err(format!(
                "DEF component '{}' is not placed on the site/row grid",
                component.name
            ));
        }
        if component.orientation != "N" {
            return Err(format!(
                "DEF component '{}' has unsupported orientation '{}'",
                component.name, component.orientation
            ));
        }
        *position = crate::Pos {
            row: component.y / ROW_DBU,
            site: component.x / SITE_DBU,
        };
    }

    let mut blockages = Vec::new();
    for blockage in &def.placement_blockages {
        blockages.push(crate::PlacementBlockage::new(
            blockage.name.clone(),
            blockage.row,
            blockage.start_site,
            blockage.end_site,
        )?);
    }
    let placement = Placement {
        rows: def.rows,
        cell_pos,
        cell_width: widths,
        blockages,
    };
    crate::check_legal(nl, &placement)?;

    let mut segments = Vec::new();
    let mut via_locations = Vec::new();
    let mut max_track = 0usize;
    for def_net in &def.nets {
        let net_id = nl
            .net_by_name(&def_net.name)
            .ok_or_else(|| format!("DEF net '{}' is missing from netlist", def_net.name))?;
        for route in &def_net.routes {
            let orientation = match route.layer.to_ascii_lowercase().as_str() {
                "metal1" => crate::Orientation::Horizontal,
                "metal2" => crate::Orientation::Vertical,
                other => {
                    return Err(format!(
                        "DEF net '{}' route has unsupported layer '{}'",
                        def_net.name, other
                    ))
                }
            };
            let segment = match orientation {
                crate::Orientation::Horizontal => {
                    if route.x1 % SITE_DBU != 0
                        || route.x2 % SITE_DBU != 0
                        || route.y1 % ROW_DBU != 0
                        || route.y1 != route.y2
                    {
                        return Err(format!(
                            "DEF net '{}' has a malformed metal1 route",
                            def_net.name
                        ));
                    }
                    crate::Segment {
                        net: net_id,
                        orientation,
                        channel: route.y1 / ROW_DBU,
                        start: route.x1 / SITE_DBU,
                        end: route.x2 / SITE_DBU,
                        track: route.track,
                    }
                }
                crate::Orientation::Vertical => {
                    if route.x1 % SITE_DBU != 0
                        || route.x1 != route.x2
                        || route.y1 % ROW_DBU != 0
                        || route.y2 % ROW_DBU != 0
                    {
                        return Err(format!(
                            "DEF net '{}' has a malformed metal2 route",
                            def_net.name
                        ));
                    }
                    crate::Segment {
                        net: net_id,
                        orientation,
                        channel: route.x1 / SITE_DBU,
                        start: route.y1 / ROW_DBU,
                        end: route.y2 / ROW_DBU,
                        track: route.track,
                    }
                }
            };
            if segment.start > segment.end {
                return Err(format!("DEF net '{}' has a reversed route", def_net.name));
            }
            max_track = max_track.max(segment.track);
            segments.push(segment);
        }
        for via in &def_net.vias {
            via_locations.push(crate::Via {
                net: net_id,
                x: via.x,
                y: via.y,
            });
        }
    }

    let mut blocked_channels = Vec::new();
    for blockage in &def.route_blockages {
        blocked_channels.push(crate::RouteChannelBlockage::new(
            blockage.name.clone(),
            blockage.orientation,
            blockage.channel,
        )?);
    }
    let tracks_per_channel = if def.tracks_per_channel == 0 {
        max_track + 1
    } else {
        def.tracks_per_channel
    };
    if max_track >= tracks_per_channel {
        return Err(format!(
            "DEF route track {} exceeds {} tracks per channel",
            max_track, tracks_per_channel
        ));
    }
    let routing = Routing {
        segments,
        vias: via_locations.len(),
        via_locations,
        tracks_per_channel,
        congested_channels: Vec::new(),
        blocked_channels,
    };
    crate::check_routing(&routing)?;
    Ok((placement, routing))
}

fn macro_name(nl: &Netlist, ci: usize) -> String {
    match &nl.cell(ci).kind {
        CellKind::Lib(id) => nl
            .library()
            .map(|lib| lib.get(*id).name.clone())
            .unwrap_or_else(|| format!("GENERIC_{ci}")),
        CellKind::Const(value) => format!("CONST_{value:?}"),
        other => format!("GENERIC_{:?}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as eda_pnr;
    use eda_core::library::CellLibrary;
    use eda_core::netlist::{Cell, Dir};
    use std::sync::Arc;

    #[test]
    fn lef_def_round_trip() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("physical");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let placement_blockage = crate::PlacementBlockage::new("macro", 1, 2, 5).unwrap();
        let placement =
            eda_pnr::place_with_blockages(&nl, std::slice::from_ref(&placement_blockage)).unwrap();
        let baseline = eda_pnr::route(&nl, &placement).unwrap();
        let segment = *baseline
            .segments
            .first()
            .expect("baseline routing has a segment");
        let options = crate::RouteOptions::new(4)
            .unwrap()
            .with_blocked_channel("route_keepout", segment.orientation, segment.channel)
            .unwrap();
        let routing = eda_pnr::route_maze_with_options(&nl, &placement, &options).unwrap();
        let lef = write_lef(&nl, &placement).unwrap();
        assert!(lef.contains("MACRO AND2"));
        let def = write_def(&nl, &placement, &routing).unwrap();
        assert!(def.contains("BLOCKAGES 2 ;"));
        let parsed = parse_def(&def).unwrap();
        assert_eq!(parsed.design, "physical");
        assert_eq!(parsed.rows, placement.rows);
        assert_eq!(parsed.components.len(), 1);
        assert_eq!(parsed.components[0].macro_name, "AND2");
        assert_eq!(parsed.nets.len(), nl.net_count());
        assert_eq!(
            parsed
                .nets
                .iter()
                .map(|net| net.routes.len())
                .sum::<usize>(),
            routing.segments.len()
        );
        assert_eq!(
            parsed.nets.iter().map(|net| net.vias.len()).sum::<usize>(),
            routing.via_locations.len()
        );
        assert_eq!(
            parsed.placement_blockages,
            vec![DefPlacementBlockage {
                name: placement_blockage.name.clone(),
                row: placement_blockage.row,
                start_site: placement_blockage.start_site,
                end_site: placement_blockage.end_site,
            }]
        );
        assert_eq!(
            parsed.route_blockages,
            vec![DefRouteBlockage {
                name: "route_keepout".into(),
                orientation: segment.orientation,
                channel: segment.channel,
            }]
        );
        let (imported_placement, imported_routing) = physical_from_def(&nl, &parsed).unwrap();
        assert_eq!(imported_placement, placement);
        assert_eq!(imported_routing, routing);
    }
}
