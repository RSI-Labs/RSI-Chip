use std::collections::HashMap;

/// Sites per placement row.
/// Keep the default grid large enough for the complete RV32I regression
/// vehicle; explicit PDK layer data can still override routing capacity.
pub const SITES_PER_ROW: usize = 256;
/// Global-routing tracks available in each horizontal/vertical channel.
/// RV32I's default-grid route peaks at 254 occupied vertical tracks, so 256
/// is the smallest power-of-two default observed to close that release gate.
pub const TRACKS_PER_CHANNEL: usize = 256;
/// Conservative antenna threshold in routing-length units per connected pin.
pub const MAX_ANTENNA_RATIO: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub row: usize,
    pub site: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub rows: usize,
    /// Indexed by cell id; every cell has one slot.
    pub cell_pos: Vec<Pos>,
    /// Cell widths in sites (mirrors library widths).
    pub cell_width: Vec<usize>,
    /// Hard placement obstructions retained for legalization and DRC.
    pub blockages: Vec<PlacementBlockage>,
}

/// A rectangular hard placement obstruction in one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementBlockage {
    pub name: String,
    pub row: usize,
    pub start_site: usize,
    pub end_site: usize,
}

impl PlacementBlockage {
    pub fn new(
        name: impl Into<String>,
        row: usize,
        start_site: usize,
        end_site: usize,
    ) -> Result<Self, String> {
        let out = Self {
            name: name.into(),
            row,
            start_site,
            end_site,
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("placement blockage name cannot be empty".into());
        }
        if self.start_site > self.end_site || self.end_site >= SITES_PER_ROW {
            return Err(format!(
                "placement blockage '{}' spans sites {}..{} outside a {}-site row",
                self.name, self.start_site, self.end_site, SITES_PER_ROW
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub net: usize,
    pub orientation: Orientation,
    /// Horizontal channel row for horizontal segments; x column for vertical
    /// segments.
    pub channel: usize,
    pub start: usize,
    pub end: usize,
    pub track: usize,
}

impl Segment {
    pub fn length(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Routing {
    pub segments: Vec<Segment>,
    pub vias: usize,
    /// Explicit layer-change locations. Each via connects a horizontal and a
    /// vertical segment of the same net at its grid coordinate.
    pub via_locations: Vec<Via>,
    pub tracks_per_channel: usize,
    /// Channels that exhausted their track capacity.
    pub congested_channels: Vec<(Orientation, usize)>,
    /// Hard routing blockages retained for DRC and physical audit.
    pub blocked_channels: Vec<RouteChannelBlockage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Via {
    pub net: usize,
    pub x: usize,
    pub y: usize,
}

/// A hard blockage covering one complete global-routing channel.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RouteChannelBlockage {
    pub name: String,
    pub orientation: Orientation,
    pub channel: usize,
}

impl RouteChannelBlockage {
    pub fn new(
        name: impl Into<String>,
        orientation: Orientation,
        channel: usize,
    ) -> Result<Self, String> {
        let out = Self {
            name: name.into(),
            orientation,
            channel,
        };
        out.validate()?;
        Ok(out)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("routing blockage name cannot be empty".into());
        }
        Ok(())
    }
}

/// Global-routing capacity and congestion-policy options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteOptions {
    pub tracks_per_channel: usize,
    /// Keep partial routing and report congested channels instead of failing
    /// immediately. DRC subsequently reports the resulting opens.
    pub allow_congestion: bool,
    pub blocked_channels: Vec<RouteChannelBlockage>,
}

/// Connectivity weights for timing-driven placement.
///
/// Nets absent from `net_weights` receive `default_net_weight`. Higher
/// weights give a net more influence over the weighted-HPWL objective.
#[derive(Clone, Debug)]
pub struct TimingDrivenPlacementOptions {
    pub net_weights: HashMap<usize, f64>,
    pub default_net_weight: f64,
    pub max_passes: usize,
}

impl Default for TimingDrivenPlacementOptions {
    fn default() -> Self {
        Self {
            net_weights: HashMap::new(),
            default_net_weight: 1.0,
            max_passes: 4,
        }
    }
}

impl TimingDrivenPlacementOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_passes == 0 {
            return Err("timing-driven placement requires at least one pass".into());
        }
        if !self.default_net_weight.is_finite() || self.default_net_weight < 0.0 {
            return Err("default timing-driven net weight must be finite and non-negative".into());
        }
        for (net, weight) in &self.net_weights {
            if !weight.is_finite() || *weight < 0.0 {
                return Err(format!("invalid timing-driven weight for net {}", net));
            }
        }
        Ok(())
    }

    pub(super) fn weight(&self, net: usize) -> f64 {
        self.net_weights
            .get(&net)
            .copied()
            .unwrap_or(self.default_net_weight)
    }
}

#[derive(Clone, Debug)]
pub struct TimingDrivenPlacementResult {
    pub placement: Placement,
    pub initial_hpwl: f64,
    pub final_hpwl: f64,
    pub initial_weighted_hpwl: f64,
    pub final_weighted_hpwl: f64,
    pub passes: usize,
    pub improving_moves: usize,
}

/// Timing/criticality weights for negotiated-congestion routing.
///
/// Connections are rerouted in descending weight order, so critical nets get
/// first use of scarce direct channels while low-priority nets detour.
#[derive(Clone, Debug)]
pub struct TimingDrivenRoutingOptions {
    pub net_weights: HashMap<usize, f64>,
    pub default_net_weight: f64,
}

impl Default for TimingDrivenRoutingOptions {
    fn default() -> Self {
        Self {
            net_weights: HashMap::new(),
            default_net_weight: 1.0,
        }
    }
}

impl TimingDrivenRoutingOptions {
    pub fn validate(&self, net_count: usize) -> Result<(), String> {
        if !self.default_net_weight.is_finite() || self.default_net_weight < 0.0 {
            return Err("default routing weight must be finite and non-negative".into());
        }
        for (net, weight) in &self.net_weights {
            if *net >= net_count {
                return Err(format!(
                    "routing weight references net {} but netlist has {} nets",
                    net, net_count
                ));
            }
            if !weight.is_finite() || *weight < 0.0 {
                return Err(format!("invalid routing weight for net {}", net));
            }
        }
        Ok(())
    }

    pub(super) fn weight(&self, net: usize) -> f64 {
        self.net_weights
            .get(&net)
            .copied()
            .unwrap_or(self.default_net_weight)
    }
}

impl Default for RouteOptions {
    fn default() -> Self {
        Self {
            tracks_per_channel: TRACKS_PER_CHANNEL,
            allow_congestion: false,
            blocked_channels: Vec::new(),
        }
    }
}

impl RouteOptions {
    pub fn new(tracks_per_channel: usize) -> Result<Self, String> {
        if tracks_per_channel == 0 {
            return Err("routing requires at least one track per channel".into());
        }
        Ok(Self {
            tracks_per_channel,
            allow_congestion: false,
            blocked_channels: Vec::new(),
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.tracks_per_channel == 0 {
            return Err("routing requires at least one track per channel".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for blockage in &self.blocked_channels {
            blockage.validate()?;
            if !seen.insert((blockage.orientation, blockage.channel)) {
                return Err(format!(
                    "duplicate routing blockage for {:?} channel {}",
                    blockage.orientation, blockage.channel
                ));
            }
        }
        Ok(())
    }

    pub fn with_blocked_channel(
        mut self,
        name: impl Into<String>,
        orientation: Orientation,
        channel: usize,
    ) -> Result<Self, String> {
        let blockage = RouteChannelBlockage::new(name, orientation, channel)?;
        self.blocked_channels.push(blockage);
        self.validate()?;
        Ok(self)
    }

    pub(super) fn blocked_set(&self) -> std::collections::BTreeSet<(Orientation, usize)> {
        self.blocked_channels
            .iter()
            .map(|blockage| (blockage.orientation, blockage.channel))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DrcSeverity {
    Warning,
    Error,
}

/// Physical verification options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrcOptions {
    /// Maximum routed wirelength per physical pin before an antenna warning.
    pub max_antenna_ratio: f64,
}

impl Default for DrcOptions {
    fn default() -> Self {
        Self {
            max_antenna_ratio: MAX_ANTENNA_RATIO,
        }
    }
}

impl DrcOptions {
    pub fn new_with_antenna_ratio(max_antenna_ratio: f64) -> Result<Self, String> {
        let options = Self { max_antenna_ratio };
        options.validate()?;
        Ok(options)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.max_antenna_ratio.is_finite() || self.max_antenna_ratio <= 0.0 {
            return Err("maximum antenna ratio must be finite and positive".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrcViolation {
    pub code: String,
    pub severity: DrcSeverity,
    pub message: String,
    pub cell: Option<usize>,
    pub net: Option<usize>,
    pub row: Option<usize>,
    pub site: Option<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct DrcResult {
    pub violations: Vec<DrcViolation>,
}

impl DrcResult {
    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == DrcSeverity::Error)
    }

    pub fn has_warnings(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == DrcSeverity::Warning)
    }

    pub fn error_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == DrcSeverity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == DrcSeverity::Warning)
            .count()
    }
}

impl Routing {
    pub fn wirelength(&self) -> usize {
        self.segments.iter().map(Segment::length).sum()
    }

    pub fn utilization(&self) -> f64 {
        let mut occupancy: HashMap<(Orientation, usize), Vec<(usize, i32)>> = HashMap::new();
        for seg in &self.segments {
            occupancy
                .entry((seg.orientation, seg.channel))
                .or_default()
                .extend([(seg.start, 1), (seg.end + 1, -1)]);
        }
        occupancy
            .values()
            .map(|events| {
                let mut sorted = events.clone();
                sorted.sort_unstable();
                let mut active = 0_i32;
                let mut peak = 0_i32;
                for (_, delta) in sorted {
                    active += delta;
                    peak = peak.max(active);
                }
                peak as f64
            })
            .fold(0.0_f64, f64::max)
            / self.tracks_per_channel as f64
    }
}
