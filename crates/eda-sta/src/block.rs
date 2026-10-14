//! Interface-level timing abstraction for hierarchical composition.

use crate::constraints::{
    constrained_hold_timing_paths, constrained_timing_paths, ConstrainedTiming,
};
use crate::{Netlist, TimingLaunch, TimingPathEndpoint};
use eda_core::netlist::Dir;
use std::collections::{BTreeMap, BTreeSet};

/// Direction of a port represented in a block timing model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockTimingDirection {
    Input,
    Output,
}

impl BlockTimingDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

/// Timing requirements/arrivals exposed by one block boundary port.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingPort {
    pub name: String,
    pub direction: BlockTimingDirection,
    /// Reference clock for the values below, when a path reaches this port.
    pub clock: Option<String>,
    /// Latest clock-relative arrival at an output port.
    pub late_arrival_ns: Option<f64>,
    /// Earliest clock-relative arrival at an output port.
    pub early_arrival_ns: Option<f64>,
    /// Latest clock-relative arrival accepted at a data input.
    pub late_required_ns: Option<f64>,
    /// Worst setup/output/max-delay slack contributing to this port.
    pub setup_slack_ns: Option<f64>,
    /// Earliest clock-relative arrival accepted at a data input.
    pub early_required_ns: Option<f64>,
    /// Worst hold/min-delay slack contributing to this port.
    pub hold_slack_ns: Option<f64>,
    /// Number of concrete interface paths represented by this port entry.
    pub paths: usize,
}

/// Stable-name timing interface extracted from one constrained block analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingModel {
    pub module: String,
    pub clock: String,
    pub period_ns: f64,
    pub ports: BTreeMap<String, BlockTimingPort>,
    /// Register-to-register paths intentionally hidden from the boundary model.
    pub internal_paths: usize,
}

/// One named instance of a serialized block timing model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockTimingCompositionInstance {
    pub name: String,
    pub model: String,
}

impl BlockTimingCompositionInstance {
    pub fn new(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            model: model.into(),
        }
    }
}

/// Maps one instance's local clock name to the shared composition clock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockTimingClockMap {
    pub instance: String,
    pub clocks: BTreeMap<String, String>,
}

impl BlockTimingClockMap {
    pub fn new(instance: impl Into<String>) -> Self {
        Self {
            instance: instance.into(),
            clocks: BTreeMap::new(),
        }
    }

    pub fn with_clock(mut self, local: impl Into<String>, composition: impl Into<String>) -> Self {
        self.clocks.insert(local.into(), composition.into());
        self
    }
}

/// One named clock in a block timing composition.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingClock {
    pub name: String,
    pub period_ns: f64,
    /// Phase relative to the composition origin, in nanoseconds.
    pub phase_ns: f64,
}

impl BlockTimingClock {
    pub fn new(name: impl Into<String>, period_ns: f64, phase_ns: f64) -> Self {
        Self {
            name: name.into(),
            period_ns,
            phase_ns,
        }
    }
}

/// User-selected launch-to-capture edge separation for two clocks.
///
/// The value is relative to the source launch edge. It can represent phase
/// difference, multicycle selection, or an explicitly timed asynchronous edge.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingClockRelationship {
    pub source_clock: String,
    pub destination_clock: String,
    pub edge_separation_ns: f64,
}

impl BlockTimingClockRelationship {
    pub fn new(
        source_clock: impl Into<String>,
        destination_clock: impl Into<String>,
        edge_separation_ns: f64,
    ) -> Self {
        Self {
            source_clock: source_clock.into(),
            destination_clock: destination_clock.into(),
            edge_separation_ns,
        }
    }
}

/// A stable-name output-to-input connection between block instances.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingCompositionConnection {
    pub from_instance: String,
    pub from_port: String,
    pub to_instance: String,
    pub to_port: String,
    pub interconnect_ns: f64,
}

impl BlockTimingCompositionConnection {
    pub fn new(
        from_instance: impl Into<String>,
        from_port: impl Into<String>,
        to_instance: impl Into<String>,
        to_port: impl Into<String>,
        interconnect_ns: f64,
    ) -> Self {
        Self {
            from_instance: from_instance.into(),
            from_port: from_port.into(),
            to_instance: to_instance.into(),
            to_port: to_port.into(),
            interconnect_ns,
        }
    }
}

/// One composed block-boundary setup check.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingInterfaceCheck {
    pub from_instance: String,
    pub from_port: String,
    pub to_instance: String,
    pub to_port: String,
    pub clock: String,
    pub source_clock: String,
    pub destination_clock: String,
    pub edge_separation_ns: f64,
    pub source_arrival_ns: f64,
    pub interconnect_ns: f64,
    pub arrival_ns: f64,
    pub required_ns: f64,
    pub slack_ns: f64,
}

/// One composed block-boundary hold/min-delay check.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingHoldInterfaceCheck {
    pub from_instance: String,
    pub from_port: String,
    pub to_instance: String,
    pub to_port: String,
    pub clock: String,
    pub source_clock: String,
    pub destination_clock: String,
    pub edge_separation_ns: f64,
    pub source_arrival_ns: f64,
    pub interconnect_ns: f64,
    pub arrival_ns: f64,
    pub required_ns: f64,
    pub slack_ns: f64,
}

/// Result of composing block timing interfaces by stable instance/port names.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTimingCompositionResult {
    pub clock: String,
    pub period_ns: f64,
    pub clocks: Vec<BlockTimingClock>,
    pub clock_relationships: Vec<BlockTimingClockRelationship>,
    pub instances: Vec<BlockTimingCompositionInstance>,
    pub clock_maps: Vec<BlockTimingClockMap>,
    pub connections: Vec<BlockTimingCompositionConnection>,
    pub checks: Vec<BlockTimingInterfaceCheck>,
    pub hold_checks: Vec<BlockTimingHoldInterfaceCheck>,
    pub unconnected_inputs: Vec<String>,
    pub unconnected_outputs: Vec<String>,
    pub internal_paths: usize,
    pub worst_slack_ns: Option<f64>,
    pub worst_hold_slack_ns: Option<f64>,
}

impl BlockTimingCompositionResult {
    pub fn is_clean(&self) -> bool {
        self.checks.iter().all(|check| check.slack_ns >= -1e-12)
            && self
                .hold_checks
                .iter()
                .all(|check| check.slack_ns >= -1e-12)
    }
}

fn optional_min(left: Option<f64>, right: f64) -> Option<f64> {
    Some(left.map_or(right, |old| old.min(right)))
}

fn optional_max(left: Option<f64>, right: f64) -> Option<f64> {
    Some(left.map_or(right, |old| old.max(right)))
}

/// Extract the block timing interface from constrained setup and hold paths.
///
/// Input-to-register paths become input setup/hold requirements. Input- and
/// register-to-output paths become clock-relative output arrivals. Internal
/// register-to-register paths remain private but are counted for auditability.
pub fn block_timing_model(
    nl: &Netlist,
    timing: &ConstrainedTiming,
) -> Result<BlockTimingModel, String> {
    if !timing.period.is_finite() || timing.period <= 0.0 {
        return Err("block timing model requires a positive clock period".into());
    }

    let mut ports = BTreeMap::new();
    for port in &nl.ports {
        let direction = match port.dir {
            Dir::In => BlockTimingDirection::Input,
            Dir::Out => BlockTimingDirection::Output,
        };
        ports.insert(
            port.name.clone(),
            BlockTimingPort {
                name: port.name.clone(),
                direction,
                clock: None,
                late_arrival_ns: None,
                early_arrival_ns: None,
                late_required_ns: None,
                setup_slack_ns: None,
                early_required_ns: None,
                hold_slack_ns: None,
                paths: 0,
            },
        );
    }

    let mut internal_paths = 0_usize;
    for path in constrained_timing_paths(nl, timing, 0) {
        match (&path.path.launch, &path.path.endpoint) {
            (TimingLaunch::PrimaryInput(name), TimingPathEndpoint::SequentialCell(_)) => {
                let Some(port) = ports.get_mut(name.as_str()) else {
                    return Err(format!(
                        "timing path launches through unknown block port '{}'",
                        name
                    ));
                };
                port.clock = Some(path.clocks.capture.clock.clone());
                port.late_required_ns = optional_min(port.late_required_ns, path.required_ns);
                port.setup_slack_ns = optional_min(port.setup_slack_ns, path.slack_ns);
                port.paths += 1;
            }
            (_, TimingPathEndpoint::OutputPort(name)) => {
                let Some(port) = ports.get_mut(name.as_str()) else {
                    return Err(format!("timing path ends at unknown block port '{}'", name));
                };
                port.clock = Some(path.clocks.launch.clock.clone());
                port.late_arrival_ns = optional_max(port.late_arrival_ns, path.path.arrival_ns);
                port.setup_slack_ns = optional_min(port.setup_slack_ns, path.slack_ns);
                port.paths += 1;
            }
            (TimingLaunch::SequentialCell(_), TimingPathEndpoint::SequentialCell(_)) => {
                internal_paths += 1;
            }
        }
    }

    for path in constrained_hold_timing_paths(nl, timing, 0) {
        if let TimingLaunch::PrimaryInput(name) = &path.path.path.launch {
            let Some(port) = ports.get_mut(name.as_str()) else {
                return Err(format!(
                    "hold path launches through unknown block port '{}'",
                    name
                ));
            };
            port.clock = Some(path.clocks.capture.clock.clone());
            port.early_required_ns = optional_max(port.early_required_ns, path.required_ns);
            port.hold_slack_ns = optional_min(port.hold_slack_ns, path.slack_ns);
        }
    }

    for path in timing.base.min_timing_paths(nl, 0) {
        if let TimingPathEndpoint::OutputPort(name) = &path.endpoint {
            let Some(port) = ports.get_mut(name.as_str()) else {
                return Err(format!(
                    "minimum timing path ends at unknown block port '{}'",
                    name
                ));
            };
            port.early_arrival_ns = optional_min(port.early_arrival_ns, path.arrival_ns);
        }
    }

    Ok(BlockTimingModel {
        module: nl.name.clone(),
        clock: timing.clock.clone(),
        period_ns: timing.period,
        ports,
        internal_paths,
    })
}

/// Compose block-boundary setup interfaces by stable instance and port names.
///
/// This is an interface-level composition check, not a replacement for full
/// hierarchical STA. It verifies that each producer's clock-relative output
/// arrival plus interconnect delay meets the consumer block's extracted input
/// setup requirement. Internal paths remain summarized by each block model.
pub fn compose_block_timing_models_with_clock_domains<'a, I, J, K, L, M, N>(
    models: I,
    instances: J,
    connections: K,
    clock_maps: L,
    clocks: M,
    clock_relationships: N,
) -> Result<BlockTimingCompositionResult, String>
where
    I: IntoIterator<Item = &'a BlockTimingModel>,
    J: IntoIterator<Item = &'a BlockTimingCompositionInstance>,
    K: IntoIterator<Item = &'a BlockTimingCompositionConnection>,
    L: IntoIterator<Item = &'a BlockTimingClockMap>,
    M: IntoIterator<Item = &'a BlockTimingClock>,
    N: IntoIterator<Item = &'a BlockTimingClockRelationship>,
{
    let models: Vec<&BlockTimingModel> = models.into_iter().collect();
    let instances: Vec<BlockTimingCompositionInstance> = instances.into_iter().cloned().collect();
    let connections: Vec<BlockTimingCompositionConnection> =
        connections.into_iter().cloned().collect();
    let clock_maps: Vec<BlockTimingClockMap> = clock_maps.into_iter().cloned().collect();
    let clocks: Vec<BlockTimingClock> = clocks.into_iter().cloned().collect();
    let clock_relationships: Vec<BlockTimingClockRelationship> =
        clock_relationships.into_iter().cloned().collect();
    if models.is_empty() {
        return Err("block timing composition requires at least one model".into());
    }
    if instances.is_empty() {
        return Err("block timing composition requires at least one instance".into());
    }

    let mut model_map = BTreeMap::new();
    for model in &models {
        if model.module.trim().is_empty() {
            return Err("block timing model name cannot be empty".into());
        }
        if model_map.insert(model.module.as_str(), *model).is_some() {
            return Err(format!("duplicate block timing model '{}'", model.module));
        }
    }

    for model in &models {
        if !model.period_ns.is_finite() || model.period_ns <= 0.0 {
            return Err(format!(
                "block timing model '{}' has invalid period {}",
                model.module, model.period_ns
            ));
        }
    }

    let mut clock_definitions = BTreeMap::new();
    for clock in &clocks {
        if clock.name.trim().is_empty() {
            return Err("block timing composition clock name cannot be empty".into());
        }
        if !clock.period_ns.is_finite() || clock.period_ns <= 0.0 {
            return Err(format!(
                "block timing clock '{}' has invalid period {}",
                clock.name, clock.period_ns
            ));
        }
        if !clock.phase_ns.is_finite() {
            return Err(format!(
                "block timing clock '{}' has non-finite phase",
                clock.name
            ));
        }
        if clock_definitions
            .insert(clock.name.clone(), clock)
            .is_some()
        {
            return Err(format!(
                "duplicate block timing composition clock '{}'",
                clock.name
            ));
        }
    }

    let mut instance_models = BTreeMap::new();
    let mut instance_names = BTreeSet::new();
    for instance in &instances {
        if instance.name.trim().is_empty() {
            return Err("block timing instance name cannot be empty".into());
        }
        if !instance_names.insert(instance.name.as_str()) {
            return Err(format!(
                "duplicate block timing instance '{}'",
                instance.name
            ));
        }
        let model = model_map.get(instance.model.as_str()).ok_or_else(|| {
            format!(
                "block timing instance '{}' references unknown model '{}'",
                instance.name, instance.model
            )
        })?;
        instance_models.insert(instance.name.as_str(), *model);
    }

    let mut clock_map_by_instance = BTreeMap::new();
    for map in &clock_maps {
        if map.instance.trim().is_empty() {
            return Err("block timing clock map instance name cannot be empty".into());
        }
        if !instance_models.contains_key(map.instance.as_str()) {
            return Err(format!(
                "block timing clock map references unknown instance '{}'",
                map.instance
            ));
        }
        if clock_map_by_instance
            .insert(map.instance.as_str(), map)
            .is_some()
        {
            return Err(format!(
                "duplicate block timing clock map for instance '{}'",
                map.instance
            ));
        }
    }
    let mut relationship_keys = BTreeSet::new();
    for relationship in &clock_relationships {
        if relationship.source_clock.trim().is_empty()
            || relationship.destination_clock.trim().is_empty()
        {
            return Err("block timing clock relationship names cannot be empty".into());
        }
        if !relationship.edge_separation_ns.is_finite() {
            return Err(format!(
                "clock relationship '{}:{}' has non-finite edge separation",
                relationship.source_clock, relationship.destination_clock
            ));
        }
        let key = format!(
            "{}:{}",
            relationship.source_clock, relationship.destination_clock
        );
        if !relationship_keys.insert(key) {
            return Err(format!(
                "duplicate block timing clock relationship '{}:{}'",
                relationship.source_clock, relationship.destination_clock
            ));
        }
    }
    let effective_clock = |instance: &str, local: &str| -> Result<String, String> {
        if local.trim().is_empty() {
            return Err(format!(
                "instance '{}' has an empty local block timing clock",
                instance
            ));
        }
        if let Some(map) = clock_map_by_instance.get(instance) {
            if let Some((mapped_local, composition)) = map.clocks.iter().next() {
                if mapped_local.trim().is_empty() {
                    return Err(format!(
                        "clock map for instance '{}' has an empty local clock",
                        instance
                    ));
                }
                if composition.trim().is_empty() {
                    return Err(format!(
                        "clock map for instance '{}' maps '{}' to an empty clock",
                        instance, mapped_local
                    ));
                }
                if let Some(mapped) = map.clocks.get(local) {
                    return Ok(mapped.clone());
                }
                return Ok(local.to_string());
            }
        }
        Ok(local.to_string())
    };
    let validate_model_clock_map =
        |instance: &str, model: &BlockTimingModel| -> Result<(), String> {
            if let Some(map) = clock_map_by_instance.get(instance) {
                for (local, composition) in &map.clocks {
                    if local.trim().is_empty() {
                        return Err(format!(
                            "clock map for instance '{}' has an empty local clock",
                            instance
                        ));
                    }
                    if composition.trim().is_empty() {
                        return Err(format!(
                            "clock map for instance '{}' maps '{}' to an empty clock",
                            instance, local
                        ));
                    }
                    if local != &model.clock {
                        return Err(format!(
                            "clock map for instance '{}' references unknown local clock '{}'",
                            instance, local
                        ));
                    }
                }
            }
            Ok(())
        };
    let Some((first_instance, first_model)) = instance_models.iter().next() else {
        return Err("block timing composition requires at least one instance".into());
    };
    validate_model_clock_map(first_instance, first_model)?;
    let clock = effective_clock(first_instance, &first_model.clock)?;
    let period_ns = first_model.period_ns;
    for (instance, model) in &instance_models {
        validate_model_clock_map(instance, model)?;
        let effective = effective_clock(instance, &model.clock)?;
        if let Some(definition) = clock_definitions.get(effective.as_str()) {
            if (model.period_ns - definition.period_ns).abs() > 1e-12 {
                return Err(format!(
                    "block timing instance '{}' model period {} does not match clock '{}' period {}",
                    instance, model.period_ns, effective, definition.period_ns
                ));
            }
        } else if !clock_definitions.is_empty() {
            return Err(format!(
                "block timing instance '{}' references undefined composition clock '{}'",
                instance, effective
            ));
        } else if effective != clock {
            return Err(format!(
                "block timing instance '{}' uses clock '{}', expected '{}'",
                instance, effective, clock
            ));
        }
    }
    for relationship in &clock_relationships {
        for name in [&relationship.source_clock, &relationship.destination_clock] {
            if !clock_definitions.contains_key(name) {
                return Err(format!(
                    "clock relationship references undefined composition clock '{}'",
                    name
                ));
            }
        }
    }

    let mut destination_ports = BTreeSet::new();
    let mut checks = Vec::new();
    let mut hold_checks = Vec::new();
    let mut connected_inputs = BTreeSet::new();
    let mut connected_outputs = BTreeSet::new();
    for connection in &connections {
        let source_model = instance_models
            .get(connection.from_instance.as_str())
            .ok_or_else(|| {
                format!(
                    "connection references unknown source instance '{}'",
                    connection.from_instance
                )
            })?;
        let destination_model = instance_models
            .get(connection.to_instance.as_str())
            .ok_or_else(|| {
                format!(
                    "connection references unknown destination instance '{}'",
                    connection.to_instance
                )
            })?;
        let source = source_model
            .ports
            .get(&connection.from_port)
            .ok_or_else(|| {
                format!(
                    "instance '{}' has no block timing port '{}'",
                    connection.from_instance, connection.from_port
                )
            })?;
        let destination = destination_model
            .ports
            .get(&connection.to_port)
            .ok_or_else(|| {
                format!(
                    "instance '{}' has no block timing port '{}'",
                    connection.to_instance, connection.to_port
                )
            })?;
        if source.direction != BlockTimingDirection::Output {
            return Err(format!(
                "connection source '{}:{}' is not an output",
                connection.from_instance, connection.from_port
            ));
        }
        if destination.direction != BlockTimingDirection::Input {
            return Err(format!(
                "connection destination '{}:{}' is not an input",
                connection.to_instance, connection.to_port
            ));
        }
        if !connection.interconnect_ns.is_finite() || connection.interconnect_ns < 0.0 {
            return Err(format!(
                "connection '{}:{}' -> '{}:{}' has invalid interconnect delay",
                connection.from_instance,
                connection.from_port,
                connection.to_instance,
                connection.to_port
            ));
        }
        let destination_key = format!("{}:{}", connection.to_instance, connection.to_port);
        if !destination_ports.insert(destination_key) {
            return Err(format!(
                "block timing input '{}:{}' has multiple drivers",
                connection.to_instance, connection.to_port
            ));
        }
        let Some(source_arrival) = source.late_arrival_ns else {
            return Err(format!(
                "connected output '{}:{}' has no late arrival model",
                connection.from_instance, connection.from_port
            ));
        };
        let Some(required) = destination.late_required_ns else {
            return Err(format!(
                "connected input '{}:{}' has no setup requirement",
                connection.to_instance, connection.to_port
            ));
        };
        if !source_arrival.is_finite() || !required.is_finite() {
            return Err(format!(
                "connection '{}:{}' -> '{}:{}' contains a non-finite timing value",
                connection.from_instance,
                connection.from_port,
                connection.to_instance,
                connection.to_port
            ));
        }
        let source_local = source.clock.as_deref().ok_or_else(|| {
            format!(
                "connected output '{}:{}' has no clock model",
                connection.from_instance, connection.from_port
            )
        })?;
        let source_effective = effective_clock(&connection.from_instance, source_local)?;
        if !clock_definitions.is_empty() {
            if !clock_definitions.contains_key(source_effective.as_str()) {
                return Err(format!(
                    "connected output '{}:{}' uses undefined clock '{}'",
                    connection.from_instance, connection.from_port, source_effective
                ));
            }
        } else if source_effective != clock {
            return Err(format!(
                "connected output '{}:{}' uses clock '{:?}', expected '{}'",
                connection.from_instance, connection.from_port, source_effective, clock
            ));
        }
        let destination_local = destination.clock.as_deref().ok_or_else(|| {
            format!(
                "connected input '{}:{}' has no clock model",
                connection.to_instance, connection.to_port
            )
        })?;
        let destination_effective = effective_clock(&connection.to_instance, destination_local)?;
        if !clock_definitions.is_empty() {
            if !clock_definitions.contains_key(destination_effective.as_str()) {
                return Err(format!(
                    "connected input '{}:{}' uses undefined clock '{}'",
                    connection.to_instance, connection.to_port, destination_effective
                ));
            }
        } else if destination_effective != clock {
            return Err(format!(
                "connected input '{}:{}' uses clock '{:?}', expected '{}'",
                connection.to_instance, connection.to_port, destination_effective, clock
            ));
        }

        let explicit_relationship = clock_relationships.iter().find(|relationship| {
            relationship.source_clock == source_effective
                && relationship.destination_clock == destination_effective
        });
        let edge_separation_ns = match explicit_relationship {
            Some(relationship) => relationship.edge_separation_ns,
            None if source_effective == destination_effective => 0.0,
            None => {
                return Err(format!(
                    "block timing connection '{}:{}' -> '{}:{}' has no timed '{}' -> '{}' clock relationship",
                    connection.from_instance,
                    connection.from_port,
                    connection.to_instance,
                    connection.to_port,
                    source_effective,
                    destination_effective
                ));
            }
        };
        let arrival = source_arrival + connection.interconnect_ns;
        let absolute_required = edge_separation_ns + required;
        let slack = absolute_required - arrival;
        checks.push(BlockTimingInterfaceCheck {
            from_instance: connection.from_instance.clone(),
            from_port: connection.from_port.clone(),
            to_instance: connection.to_instance.clone(),
            to_port: connection.to_port.clone(),
            clock: format!("{} -> {}", source_effective, destination_effective),
            source_clock: source_effective.clone(),
            destination_clock: destination_effective.clone(),
            edge_separation_ns,
            source_arrival_ns: source_arrival,
            interconnect_ns: connection.interconnect_ns,
            arrival_ns: arrival,
            required_ns: absolute_required,
            slack_ns: slack,
        });
        if let Some(required) = destination.early_required_ns {
            let Some(source_arrival) = source.early_arrival_ns else {
                return Err(format!(
                    "connected output '{}:{}' has no early arrival model for hold composition",
                    connection.from_instance, connection.from_port
                ));
            };
            if !source_arrival.is_finite() || !required.is_finite() {
                return Err(format!(
                    "connection '{}:{}' -> '{}:{}' contains a non-finite hold timing value",
                    connection.from_instance,
                    connection.from_port,
                    connection.to_instance,
                    connection.to_port
                ));
            }
            let arrival = source_arrival + connection.interconnect_ns;
            let absolute_required = edge_separation_ns + required;
            let slack = arrival - absolute_required;
            hold_checks.push(BlockTimingHoldInterfaceCheck {
                from_instance: connection.from_instance.clone(),
                from_port: connection.from_port.clone(),
                to_instance: connection.to_instance.clone(),
                to_port: connection.to_port.clone(),
                clock: format!("{} -> {}", source_effective, destination_effective),
                source_clock: source_effective.clone(),
                destination_clock: destination_effective.clone(),
                edge_separation_ns,
                source_arrival_ns: source_arrival,
                interconnect_ns: connection.interconnect_ns,
                arrival_ns: arrival,
                required_ns: absolute_required,
                slack_ns: slack,
            });
        }
        connected_inputs.insert(format!("{}:{}", connection.to_instance, connection.to_port));
        connected_outputs.insert(format!(
            "{}:{}",
            connection.from_instance, connection.from_port
        ));
    }

    checks.sort_by(|left, right| {
        (
            &left.from_instance,
            &left.from_port,
            &left.to_instance,
            &left.to_port,
        )
            .cmp(&(
                &right.from_instance,
                &right.from_port,
                &right.to_instance,
                &right.to_port,
            ))
    });
    hold_checks.sort_by(|left, right| {
        (
            &left.from_instance,
            &left.from_port,
            &left.to_instance,
            &left.to_port,
        )
            .cmp(&(
                &right.from_instance,
                &right.from_port,
                &right.to_instance,
                &right.to_port,
            ))
    });
    let mut unconnected_inputs = Vec::new();
    let mut unconnected_outputs = Vec::new();
    let mut internal_paths = 0_usize;
    for instance in &instances {
        let model = instance_models[instance.name.as_str()];
        internal_paths += model.internal_paths;
        for port in model.ports.values() {
            let key = format!("{}:{}", instance.name, port.name);
            let is_clock_port = port.name == model.clock;
            match port.direction {
                BlockTimingDirection::Input => {
                    if !is_clock_port && !connected_inputs.contains(&key) {
                        unconnected_inputs.push(key);
                    }
                }
                BlockTimingDirection::Output => {
                    if !connected_outputs.contains(&key) {
                        unconnected_outputs.push(key);
                    }
                }
            }
        }
    }
    unconnected_inputs.sort();
    unconnected_outputs.sort();
    let worst_slack_ns = checks.iter().map(|check| check.slack_ns).reduce(f64::min);
    let worst_hold_slack_ns = hold_checks
        .iter()
        .map(|check| check.slack_ns)
        .reduce(f64::min);
    let result_clocks = if clocks.is_empty() {
        vec![BlockTimingClock::new(clock.clone(), period_ns, 0.0)]
    } else {
        clocks.clone()
    };

    Ok(BlockTimingCompositionResult {
        clock,
        period_ns,
        clocks: result_clocks,
        clock_relationships: clock_relationships.clone(),
        instances,
        clock_maps,
        connections,
        checks,
        hold_checks,
        unconnected_inputs,
        unconnected_outputs,
        internal_paths,
        worst_slack_ns,
        worst_hold_slack_ns,
    })
}

/// Compose block timing interfaces with local-to-composition clock name maps.
pub fn compose_block_timing_models_with_clock_maps<'a, I, J, K, L>(
    models: I,
    instances: J,
    connections: K,
    clock_maps: L,
) -> Result<BlockTimingCompositionResult, String>
where
    I: IntoIterator<Item = &'a BlockTimingModel>,
    J: IntoIterator<Item = &'a BlockTimingCompositionInstance>,
    K: IntoIterator<Item = &'a BlockTimingCompositionConnection>,
    L: IntoIterator<Item = &'a BlockTimingClockMap>,
{
    compose_block_timing_models_with_clock_domains(
        models,
        instances,
        connections,
        clock_maps,
        &[],
        &[],
    )
}

/// Compose block timing interfaces using each model's local clock name.
pub fn compose_block_timing_models<'a, I, J, K>(
    models: I,
    instances: J,
    connections: K,
) -> Result<BlockTimingCompositionResult, String>
where
    I: IntoIterator<Item = &'a BlockTimingModel>,
    J: IntoIterator<Item = &'a BlockTimingCompositionInstance>,
    K: IntoIterator<Item = &'a BlockTimingCompositionConnection>,
{
    compose_block_timing_models_with_clock_maps(models, instances, connections, &[])
}

/// Render a dependency-free block timing model summary.
pub fn block_timing_model_report(model: &BlockTimingModel) -> String {
    let mut out = format!(
        "Block timing model for '{}': clock '{}', period {:.6} ns, {} port(s), {} internal path(s)\n",
        model.module,
        model.clock,
        model.period_ns,
        model.ports.len(),
        model.internal_paths
    );
    for port in model.ports.values() {
        let clock = port.clock.as_deref().unwrap_or("<unspecified>");
        out.push_str(&format!(
            "  {} '{}': clock '{}', paths={}, late_arrival={}, early_arrival={}, late_required={}, setup_slack={}, early_required={}, hold_slack={}\n",
            port.direction.label(),
            port.name,
            clock,
            port.paths,
            format_optional(port.late_arrival_ns),
            format_optional(port.early_arrival_ns),
            format_optional(port.late_required_ns),
            format_optional(port.setup_slack_ns),
            format_optional(port.early_required_ns),
            format_optional(port.hold_slack_ns)
        ));
    }
    out
}

/// Render composed block-boundary timing checks.
pub fn block_timing_composition_report(result: &BlockTimingCompositionResult) -> String {
    let mut out = format!(
        "Block timing composition: clock '{}', period {:.6} ns, {} instance(s), {} connection(s), {} setup check(s), {} hold check(s)\n",
        result.clock,
        result.period_ns,
        result.instances.len(),
        result.connections.len(),
        result.checks.len(),
        result.hold_checks.len()
    );
    for map in &result.clock_maps {
        for (local, composition) in &map.clocks {
            out.push_str(&format!(
                "  clock map {}: {} -> {}\n",
                map.instance, local, composition
            ));
        }
    }
    for clock in &result.clocks {
        out.push_str(&format!(
            "  clock '{}': period {:.6} ns, phase {:.6} ns\n",
            clock.name, clock.period_ns, clock.phase_ns
        ));
    }
    for relationship in &result.clock_relationships {
        out.push_str(&format!(
            "  clock relationship {} -> {}: edge separation {:.6} ns\n",
            relationship.source_clock,
            relationship.destination_clock,
            relationship.edge_separation_ns
        ));
    }
    for check in &result.checks {
        out.push_str(&format!(
            "  setup {}:{} -> {}:{} clock [{}] edge separation {:.6} ns, arrival {:.6} ns (interconnect {:.6} ns), required {:.6} ns, slack {:.6} ns\n",
            check.from_instance,
            check.from_port,
            check.to_instance,
            check.to_port,
            check.clock,
            check.edge_separation_ns,
            check.arrival_ns,
            check.interconnect_ns,
            check.required_ns,
            check.slack_ns
        ));
    }
    for check in &result.hold_checks {
        out.push_str(&format!(
            "  hold {}:{} -> {}:{} clock [{}] edge separation {:.6} ns, arrival {:.6} ns (interconnect {:.6} ns), required {:.6} ns, slack {:.6} ns\n",
            check.from_instance,
            check.from_port,
            check.to_instance,
            check.to_port,
            check.clock,
            check.edge_separation_ns,
            check.arrival_ns,
            check.interconnect_ns,
            check.required_ns,
            check.slack_ns
        ));
    }
    out.push_str(&format!(
        "  internal paths: {}, unconnected inputs: {}, unconnected outputs: {}\n",
        result.internal_paths,
        result.unconnected_inputs.len(),
        result.unconnected_outputs.len()
    ));
    for port in &result.unconnected_inputs {
        out.push_str(&format!("    unconnected input '{}'\n", port));
    }
    for port in &result.unconnected_outputs {
        out.push_str(&format!("    unconnected output '{}'\n", port));
    }
    match result.worst_slack_ns {
        Some(slack) => out.push_str(&format!("  worst setup interface slack: {:.6} ns\n", slack)),
        None => out.push_str("  worst setup interface slack: <unspecified>\n"),
    }
    match result.worst_hold_slack_ns {
        Some(slack) => out.push_str(&format!("  worst hold interface slack: {:.6} ns\n", slack)),
        None => out.push_str("  worst hold interface slack: <unspecified>\n"),
    }
    out
}

fn format_optional(value: Option<f64>) -> String {
    value.map_or_else(
        || "<unspecified>".to_string(),
        |value| format!("{:.6} ns", value),
    )
}

fn quote_block_name(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '\\') {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// Serialize a block timing model in a deterministic, dependency-free format.
pub fn write_block_timing_model(model: &BlockTimingModel) -> Result<String, String> {
    if model.module.trim().is_empty() {
        return Err("block timing model module name cannot be empty".into());
    }
    if model.clock.trim().is_empty() {
        return Err("block timing model clock name cannot be empty".into());
    }
    if !model.period_ns.is_finite() || model.period_ns <= 0.0 {
        return Err("block timing model requires a positive finite period".into());
    }
    for port in model.ports.values() {
        if port.name.trim().is_empty() {
            return Err("block timing port name cannot be empty".into());
        }
        for value in [
            port.late_arrival_ns,
            port.early_arrival_ns,
            port.late_required_ns,
            port.setup_slack_ns,
            port.early_required_ns,
            port.hold_slack_ns,
        ] {
            if value.is_some_and(|value| !value.is_finite()) {
                return Err(format!(
                    "block timing port '{}' contains a non-finite value",
                    port.name
                ));
            }
        }
    }

    let mut out = format!(
        "block {} {} {:?}\n",
        quote_block_name(&model.module),
        quote_block_name(&model.clock),
        model.period_ns
    );
    for port in model.ports.values() {
        let clock = port.clock.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "port {} {} {} {} {} {} {} {} {} {}\n",
            port.direction.label(),
            quote_block_name(&port.name),
            quote_block_name(clock),
            serialize_optional(port.late_arrival_ns),
            serialize_optional(port.early_arrival_ns),
            serialize_optional(port.late_required_ns),
            serialize_optional(port.setup_slack_ns),
            serialize_optional(port.early_required_ns),
            serialize_optional(port.hold_slack_ns),
            port.paths
        ));
    }
    out.push_str(&format!("internal_paths {}\n", model.internal_paths));
    Ok(out)
}

fn serialize_optional(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |value| format!("{value:?}"))
}

/// Parse the deterministic format emitted by [`write_block_timing_model`].
pub fn parse_block_timing_model(source: &str) -> Result<BlockTimingModel, String> {
    let lines = tokenize_block_timing_model(source)?;
    let Some((header, header_line)) = lines.first() else {
        return Err("block timing model requires a header".into());
    };
    if header[0] != "block" {
        return Err(format!(
            "line {}: block timing model must begin with a header",
            header_line
        ));
    }
    if header.len() != 4 {
        return Err(format!(
            "line {}: block header requires module, clock, and period",
            header_line
        ));
    }
    let period = parse_finite_f64(&header[3], *header_line, "period")?;
    if period <= 0.0 {
        return Err(format!(
            "line {}: block period must be positive",
            header_line
        ));
    }
    if header[1].trim().is_empty() {
        return Err(format!("line {}: module name cannot be empty", header_line));
    }
    if header[2].trim().is_empty() {
        return Err(format!("line {}: clock name cannot be empty", header_line));
    }

    let mut ports = BTreeMap::new();
    let mut internal_paths = None;
    for (tokens, line_number) in lines.iter().skip(1) {
        match tokens[0].as_str() {
            "port" => {
                if tokens.len() != 11 {
                    return Err(format!(
                        "line {}: port requires direction, name, clock, six timing values, and path count",
                        line_number
                    ));
                }
                let direction = match tokens[1].as_str() {
                    "input" => BlockTimingDirection::Input,
                    "output" => BlockTimingDirection::Output,
                    other => {
                        return Err(format!(
                            "line {}: unknown block timing direction '{}'",
                            line_number, other
                        ))
                    }
                };
                if tokens[2].trim().is_empty() {
                    return Err(format!("line {}: port name cannot be empty", line_number));
                }
                let clock = (tokens[3] != "-").then(|| tokens[3].clone());
                if clock.as_deref().is_some_and(str::is_empty) {
                    return Err(format!(
                        "line {}: port clock name cannot be empty",
                        line_number
                    ));
                }
                let paths = tokens[10].parse::<usize>().map_err(|_| {
                    format!("line {}: invalid block timing path count", line_number)
                })?;
                let port = BlockTimingPort {
                    name: tokens[2].clone(),
                    direction,
                    clock,
                    late_arrival_ns: parse_optional(&tokens[4], *line_number, "late arrival")?,
                    early_arrival_ns: parse_optional(&tokens[5], *line_number, "early arrival")?,
                    late_required_ns: parse_optional(&tokens[6], *line_number, "late required")?,
                    setup_slack_ns: parse_optional(&tokens[7], *line_number, "setup slack")?,
                    early_required_ns: parse_optional(&tokens[8], *line_number, "early required")?,
                    hold_slack_ns: parse_optional(&tokens[9], *line_number, "hold slack")?,
                    paths,
                };
                if ports.insert(port.name.clone(), port).is_some() {
                    return Err(format!(
                        "line {}: duplicate block timing port '{}'",
                        line_number, tokens[2]
                    ));
                }
            }
            "internal_paths" => {
                if tokens.len() != 2 {
                    return Err(format!(
                        "line {}: internal_paths requires one count",
                        line_number
                    ));
                }
                if internal_paths.is_some() {
                    return Err(format!("line {}: duplicate internal_paths", line_number));
                }
                internal_paths =
                    Some(tokens[1].parse::<usize>().map_err(|_| {
                        format!("line {}: invalid internal path count", line_number)
                    })?);
            }
            other => {
                return Err(format!(
                    "line {}: unknown block timing model token '{}'",
                    line_number, other
                ))
            }
        }
    }

    let Some(internal_paths) = internal_paths else {
        return Err("block timing model requires internal_paths".into());
    };
    Ok(BlockTimingModel {
        module: header[1].clone(),
        clock: header[2].clone(),
        period_ns: period,
        ports,
        internal_paths,
    })
}

fn tokenize_block_timing_model(source: &str) -> Result<Vec<(Vec<String>, usize)>, String> {
    let mut lines = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let mut tokens = Vec::new();
        let mut token = String::new();
        let mut in_token = false;
        let mut in_quote = false;
        let mut escaped = false;
        let characters = raw.chars().peekable();
        for character in characters {
            if in_quote {
                if escaped {
                    if !matches!(character, '"' | '\\') {
                        return Err(format!(
                            "line {}: invalid escape '\\{}' in block timing model",
                            line_number, character
                        ));
                    }
                    token.push(character);
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_quote = false;
                    tokens.push(std::mem::take(&mut token));
                    in_token = false;
                } else {
                    token.push(character);
                }
                continue;
            }

            match character {
                '"' => {
                    if in_token && !token.is_empty() {
                        return Err(format!(
                            "line {}: quoted value must be a separate token",
                            line_number
                        ));
                    }
                    in_quote = true;
                    in_token = true;
                }
                ' ' | '\t' => {
                    if in_token && !token.is_empty() {
                        tokens.push(std::mem::take(&mut token));
                    }
                    in_token = false;
                }
                other => {
                    token.push(other);
                    in_token = true;
                }
            }
        }
        if in_quote {
            return Err(format!(
                "line {}: unterminated quoted block timing name",
                line_number
            ));
        }
        if escaped {
            return Err(format!(
                "line {}: dangling escape in block timing model",
                line_number
            ));
        }
        if !token.is_empty() {
            tokens.push(token);
        }
        if !tokens.is_empty() {
            lines.push((tokens, line_number));
        }
    }
    Ok(lines)
}

fn parse_optional(token: &str, line_number: usize, label: &str) -> Result<Option<f64>, String> {
    if token == "-" {
        Ok(None)
    } else {
        parse_finite_f64(token, line_number, label).map(Some)
    }
}

fn parse_finite_f64(token: &str, line_number: usize, label: &str) -> Result<f64, String> {
    token
        .parse::<f64>()
        .map_err(|_| {
            format!(
                "line {}: invalid block timing {} '{}'",
                line_number, label, token
            )
        })
        .and_then(|value| {
            if value.is_finite() {
                Ok(value)
            } else {
                Err(format!(
                    "line {}: block timing {} must be finite",
                    line_number, label
                ))
            }
        })
}
