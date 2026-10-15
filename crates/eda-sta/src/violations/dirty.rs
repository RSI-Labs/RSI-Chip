//! Stable-name dirty-object derivation.

use crate::constraints::TimingConstraints;
use crate::TimingTopologyDiff;

use super::types::*;

pub fn timing_violation_dirty_objects_from_constraints(
    old: &TimingConstraints,
    new: &TimingConstraints,
) -> TimingViolationDirtyObjects {
    let diff = crate::diff_timing_constraints(old, new);
    let mut dirty = TimingViolationDirtyObjects::default();
    dirty.clocks.extend(diff.changed_clocks);
    dirty.clocks.extend(diff.changed_clock_uncertainty);
    dirty.clocks.extend(diff.changed_hold_uncertainty);
    dirty.nets.extend(diff.changed_input_ports);
    dirty.nets.extend(diff.changed_output_ports);
    dirty.nets.extend(diff.changed_port_derates);
    dirty.cells.extend(diff.changed_cell_derates);
    dirty.cells.extend(diff.changed_cell_check_derates);
    dirty.add_pins(&diff.changed_pin_derates);
    dirty.add_pins(&diff.changed_pin_check_derates);

    let broad_change = diff.timing_exceptions_changed
        || diff.delay_limits_changed
        || diff.disabled_timing_changed
        || diff.case_analysis_changed
        || diff.global_timing_derate_changed
        || diff.global_check_derate_changed;
    if broad_change {
        dirty.add_all_clocks(old.clocks.keys());
        dirty.add_all_clocks(new.clocks.keys());
    }
    dirty
}

/// Derive stable-name dirty checks from a flat netlist topology change.
pub fn timing_violation_dirty_objects_from_topology(
    diff: &TimingTopologyDiff,
) -> TimingViolationDirtyObjects {
    let mut dirty = TimingViolationDirtyObjects::default();
    dirty.cells.extend(diff.added_cells.iter().cloned());
    dirty.cells.extend(diff.removed_cells.iter().cloned());
    dirty.cells.extend(diff.changed_cells.iter().cloned());
    dirty.nets.extend(diff.added_nets.iter().cloned());
    dirty.nets.extend(diff.removed_nets.iter().cloned());
    dirty.nets.extend(diff.changed_nets.iter().cloned());
    dirty.nets.extend(diff.changed_ports.iter().cloned());
    dirty
}
