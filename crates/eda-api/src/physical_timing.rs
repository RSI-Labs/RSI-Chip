#[cfg(test)]
mod tests {
    use crate::{
        map_placement_by_names, map_routing_by_names, AntennaRepairOptions, HoldRepairOptions,
        Netlist, PhysicalTimingFlowOptions, Placement, Routing,
    };
    use eda_core::netlist::{Cell, CellKind};
    use eda_pnr::{Orientation, Pos, Segment};

    #[test]
    fn hold_repair_options_are_validated() {
        assert!(HoldRepairOptions::default().validate().is_ok());
        assert!(HoldRepairOptions {
            patch_name: " ".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(HoldRepairOptions {
            max_buffers_per_endpoint: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn antenna_repair_options_are_validated() {
        assert!(AntennaRepairOptions::default().validate().is_ok());
        assert!(AntennaRepairOptions {
            patch_name: " ".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(AntennaRepairOptions {
            max_diodes_per_net: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn antenna_aware_physical_flow_options_are_validated() {
        assert!(PhysicalTimingFlowOptions {
            repair_antennas: true,
            ..Default::default()
        }
        .validate()
        .is_ok());
        assert!(PhysicalTimingFlowOptions {
            repair_antennas: true,
            antenna_max_diodes_per_net: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn maps_common_geometry_by_stable_names() {
        let mut owner = Netlist::new("owner");
        let a = owner.add_net("a");
        let b = owner.add_net("b");
        let y = owner.add_net("y");
        owner.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: y,
        });
        let z = owner.add_net("z");
        owner.add_cell(Cell {
            name: "v".into(),
            kind: CellKind::Buf,
            inputs: vec![a],
            output: z,
        });

        let mut target = Netlist::new("target");
        let target_b = target.add_net("b");
        let target_y = target.add_net("y");
        let target_a = target.add_net("a");
        let target_z = target.add_net("z");
        target.add_cell(Cell {
            name: "v".into(),
            kind: CellKind::Buf,
            inputs: vec![target_a],
            output: target_z,
        });
        target.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::And,
            inputs: vec![target_a, target_b],
            output: target_y,
        });

        let placement = Placement {
            rows: 1,
            cell_pos: vec![Pos { row: 0, site: 3 }, Pos { row: 0, site: 5 }],
            cell_width: vec![2, 2],
            blockages: Vec::new(),
        };
        let routing = Routing {
            segments: vec![Segment {
                net: a,
                orientation: Orientation::Horizontal,
                channel: 0,
                start: 0,
                end: 2,
                track: 0,
            }],
            vias: 0,
            via_locations: Vec::new(),
            tracks_per_channel: 2,
            congested_channels: Vec::new(),
            blocked_channels: Vec::new(),
        };
        let mapped_placement =
            map_placement_by_names(&owner, &placement, &target, "target").unwrap();
        let mapped_routing = map_routing_by_names(&owner, &routing, &target, "target").unwrap();
        assert_eq!(mapped_placement.cell_pos[0], placement.cell_pos[1]);
        assert_eq!(mapped_placement.cell_pos[1], placement.cell_pos[0]);
        assert_eq!(mapped_placement.cell_width, placement.cell_width);
        assert_eq!(mapped_routing.segments[0].net, target_a);
    }

    #[test]
    fn verilog_export_requires_a_library() {
        let netlist = Netlist::new("generic");
        assert!(crate::export_verilog(&netlist).is_err());
    }
}
