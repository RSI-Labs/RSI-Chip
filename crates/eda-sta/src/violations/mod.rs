//! Persistent timing-violation tracking split by model, store, lifecycle, I/O, and reports.

mod dirty;
mod lifecycle;
mod paths;
mod registry;
mod reports;
mod serialization;
mod store;
#[cfg(test)]
mod tests;
mod types;

pub use dirty::{
    timing_violation_dirty_objects_from_constraints, timing_violation_dirty_objects_from_topology,
};
pub use reports::{
    active_timing_waivers_from_audit, timing_violation_registry_from_stores,
    timing_violation_registry_report, timing_violation_report,
    timing_waiver_audit_hierarchy_report, timing_waiver_audit_hierarchy_summaries,
    timing_waiver_audit_report,
};
pub use serialization::{
    parse_timing_violation_waivers, parse_timing_waiver_audit, write_timing_violation_waivers,
    write_timing_waiver_audit,
};
pub use types::{
    CrossCornerHierarchyViolationSummary, CrossCornerTimingViolation,
    CrossCornerTimingViolationChange, CrossCornerTimingViolationDiff, TimingViolation,
    TimingViolationChange, TimingViolationDiff, TimingViolationDirtyObjects, TimingViolationKind,
    TimingViolationPath, TimingViolationRegistry, TimingViolationRegistryUpdate,
    TimingViolationStore, TimingViolationWaiver, TimingWaiverAuditAction, TimingWaiverAuditEntry,
    TimingWaiverAuditHierarchySummary,
};
