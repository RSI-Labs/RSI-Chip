//! Typed PDK manifests and deterministic implementation signoff.
//!
//! These APIs intentionally separate a portable PDK description from the RTL
//! implementation flow. Correlation entries are explicit; this crate does not
//! claim certification by a foundry.

use crate::{
    parse_liberty, run_flow_staged_cancellable, CellLibrary, FlowCancellation, LibConstraintTables,
    LibTimingTables, LibertyLut,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct PdkLayer {
    pub name: String,
    pub purpose: String,
    pub pitch_um: f64,
    pub min_width_um: f64,
    pub min_spacing_um: f64,
    /// Routing tracks available per channel on this layer. Drives the
    /// router capacity during PDK implementation.
    pub tracks_per_channel: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkCorner {
    pub name: String,
    pub process: String,
    pub voltage_v: f64,
    pub temperature_c: f64,
    pub delay_scale: f64,
    /// Optional inline Liberty view used instead of the scaled built-in library.
    pub liberty_source: Option<String>,
    /// Optional required SHA-256 of the canonical inline Liberty source.
    pub liberty_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkModelCorrelation {
    pub device: String,
    pub parameter: String,
    pub nominal: f64,
    pub golden: f64,
    pub tolerance: f64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PdkSignoffCheckKind {
    ModelCorrelation,
    Equivalence,
    Timing,
    Drc,
    PhysicalDatabase,
}

impl PdkSignoffCheckKind {
    pub const ALL: [Self; 5] = [
        Self::ModelCorrelation,
        Self::Equivalence,
        Self::Timing,
        Self::Drc,
        Self::PhysicalDatabase,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModelCorrelation => "model-correlation",
            Self::Equivalence => "equivalence",
            Self::Timing => "timing",
            Self::Drc => "drc",
            Self::PhysicalDatabase => "physical-database",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| value == kind.as_str())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkDefinition {
    pub name: String,
    pub version: String,
    pub layers: Vec<PdkLayer>,
    pub corners: Vec<PdkCorner>,
    pub model_correlations: Vec<PdkModelCorrelation>,
    /// Optional inline foundry layer/via rule deck.
    pub layout_rule_deck_source: Option<String>,
    /// Required SHA-256 of the inline layout rule deck.
    pub layout_rule_deck_sha256: Option<String>,
    pub required_checks: Vec<PdkSignoffCheckKind>,
}

impl PdkDefinition {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("PDK name cannot be empty".into());
        }
        if self.version.trim().is_empty() {
            return Err("PDK version cannot be empty".into());
        }
        let mut unique_layers = BTreeSet::new();
        for layer in &self.layers {
            if layer.name.trim().is_empty() || layer.purpose.trim().is_empty() {
                return Err("PDK layer names and purposes cannot be empty".into());
            }
            if !unique_layers.insert(layer.name.clone()) {
                return Err(format!("duplicate PDK layer '{}'", layer.name));
            }
            for (value, field) in [
                (layer.pitch_um, "pitch"),
                (layer.min_width_um, "minimum width"),
                (layer.min_spacing_um, "minimum spacing"),
            ] {
                if !value.is_finite() || value <= 0.0 {
                    return Err(format!(
                        "PDK layer '{}' {} must be finite and positive",
                        layer.name, field
                    ));
                }
            }
        }
        let mut unique_corners = BTreeSet::new();
        for corner in &self.corners {
            if corner.name.trim().is_empty() || corner.process.trim().is_empty() {
                return Err("PDK corner names and processes cannot be empty".into());
            }
            if !unique_corners.insert(corner.name.clone()) {
                return Err(format!("duplicate PDK corner '{}'", corner.name));
            }
            if !corner.voltage_v.is_finite()
                || corner.voltage_v <= 0.0
                || !corner.temperature_c.is_finite()
            {
                return Err(format!(
                    "PDK corner '{}' voltage and temperature must be finite",
                    corner.name
                ));
            }
            if !corner.delay_scale.is_finite() || corner.delay_scale <= 0.0 {
                return Err(format!(
                    "PDK corner '{}' delay scale must be finite and positive",
                    corner.name
                ));
            }
            if let Some(source) = &corner.liberty_source {
                if source.trim().is_empty() {
                    return Err(format!(
                        "PDK corner '{}' has an empty Liberty source",
                        corner.name
                    ));
                }
                if source.lines().any(|line| line.trim() == "END") {
                    return Err(format!(
                        "PDK corner '{}' Liberty source cannot contain a bare END line",
                        corner.name
                    ));
                }
            }
            if let Some(expected) = &corner.liberty_sha256 {
                if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(format!(
                        "PDK corner '{}' Liberty checksum must be 64 hexadecimal characters",
                        corner.name
                    ));
                }
                let Some(source) = &corner.liberty_source else {
                    return Err(format!(
                        "PDK corner '{}' has a Liberty checksum but no Liberty view",
                        corner.name
                    ));
                };
                let actual = pdk_liberty_sha256(source);
                if !expected.eq_ignore_ascii_case(&actual) {
                    return Err(format!(
                        "PDK corner '{}' Liberty checksum mismatch: expected {expected}, got {actual}",
                        corner.name
                    ));
                }
            }
        }
        let mut unique_correlations = BTreeSet::new();
        for correlation in &self.model_correlations {
            if correlation.device.trim().is_empty() || correlation.parameter.trim().is_empty() {
                return Err("PDK correlation device and parameter cannot be empty".into());
            }
            let identity = format!("{}.{}", correlation.device, correlation.parameter);
            if !unique_correlations.insert(identity.clone()) {
                return Err(format!("duplicate PDK correlation '{identity}'"));
            }
            if !correlation.nominal.is_finite()
                || !correlation.golden.is_finite()
                || !correlation.tolerance.is_finite()
                || correlation.tolerance <= 0.0
            {
                return Err(format!(
                    "PDK correlation '{identity}' values must be finite with positive tolerance"
                ));
            }
        }
        if let Some(source) = &self.layout_rule_deck_source {
            if source.trim().is_empty() {
                return Err("PDK layout rule deck source cannot be empty".into());
            }
            if source.lines().any(|line| line.trim() == "END") {
                return Err("PDK layout rule deck cannot contain a bare END line".into());
            }
            crate::read_layout_rule_deck(source)
                .map_err(|error| format!("invalid PDK layout rule deck: {error}"))?;
        }
        if let Some(expected) = &self.layout_rule_deck_sha256 {
            if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(
                    "PDK layout rule deck checksum must be 64 hexadecimal characters".into(),
                );
            }
            let Some(source) = &self.layout_rule_deck_source else {
                return Err("PDK layout rule deck checksum requires a rule deck".into());
            };
            let actual = pdk_liberty_sha256(source);
            if !expected.eq_ignore_ascii_case(&actual) {
                return Err(format!(
                    "PDK layout rule deck checksum mismatch: expected {expected}, got {actual}"
                ));
            }
        }
        if self.required_checks.is_empty() {
            return Err("PDK signoff requires at least one check".into());
        }
        let mut unique_checks = BTreeSet::new();
        for check in &self.required_checks {
            if !unique_checks.insert(*check) {
                return Err(format!("duplicate PDK signoff check '{}'", check.as_str()));
            }
        }
        if self
            .required_checks
            .contains(&PdkSignoffCheckKind::ModelCorrelation)
            && self.model_correlations.is_empty()
        {
            return Err("model-correlation signoff requires at least one correlation entry".into());
        }
        Ok(())
    }
}

pub fn parse_pdk(source: &str) -> Result<PdkDefinition, String> {
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut layers = Vec::new();
    let mut corners = Vec::new();
    let mut model_correlations = Vec::new();
    let mut required_checks = Vec::new();

    let mut liberties = BTreeMap::<String, String>::new();
    let mut liberty_checksums = BTreeMap::<String, String>::new();
    let mut layout_rule_deck: Option<String> = None;
    let mut layout_rule_deck_checksum: Option<String> = None;
    let mut logical_lines = Vec::new();
    let mut liberty_block: Option<(String, usize, Vec<String>)> = None;
    let mut rule_deck_block: Option<(usize, Vec<String>)> = None;
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if let Some((corner, start, body)) = liberty_block.as_mut() {
            if line == "END" {
                let source = body.join("\n").trim().to_string();
                if liberties.insert(corner.clone(), source).is_some() {
                    return Err(format!(
                        "line {}: duplicate Liberty view for PDK corner '{corner}'",
                        *start + 1
                    ));
                }
                liberty_block = None;
            } else {
                body.push(raw.to_string());
            }
            continue;
        }
        if let Some((start, body)) = rule_deck_block.as_mut() {
            if line == "END" {
                let source = body.join("\n").trim().to_string();
                if layout_rule_deck.replace(source).is_some() {
                    return Err(format!(
                        "line {}: duplicate PDK layout rule deck",
                        *start + 1
                    ));
                }
                rule_deck_block = None;
            } else {
                body.push(raw.to_string());
            }
            continue;
        }
        let tokens: Vec<_> = line.split_whitespace().collect();
        if tokens.len() == 3 && tokens[0] == "liberty" && tokens[2] == "<<END" {
            liberty_block = Some((tokens[1].to_string(), index, Vec::new()));
            continue;
        }
        if tokens.len() == 3 && tokens[0] == "liberty_sha256" {
            if liberty_checksums
                .insert(tokens[1].to_string(), tokens[2].to_ascii_lowercase())
                .is_some()
            {
                return Err(format!(
                    "line {}: duplicate Liberty checksum for PDK corner '{}'",
                    index + 1,
                    tokens[1]
                ));
            }
            continue;
        }
        if tokens.len() == 2 && tokens[0] == "rule_deck" && tokens[1] == "<<END" {
            rule_deck_block = Some((index, Vec::new()));
            continue;
        }
        if tokens.len() == 2 && tokens[0] == "rule_deck_sha256" {
            if layout_rule_deck_checksum
                .replace(tokens[1].to_ascii_lowercase())
                .is_some()
            {
                return Err(format!(
                    "line {}: duplicate PDK layout rule deck checksum",
                    index + 1
                ));
            }
            continue;
        }
        logical_lines.push((index, raw.to_string()));
    }
    if let Some((corner, start, _)) = liberty_block {
        return Err(format!(
            "line {}: unterminated Liberty view for PDK corner '{corner}'",
            start + 1
        ));
    }
    if let Some((start, _)) = rule_deck_block {
        return Err(format!(
            "line {}: unterminated PDK layout rule deck",
            start + 1
        ));
    }

    for (index, raw) in logical_lines {
        let line = raw.trim();
        if line.is_empty() || line == ".end" {
            continue;
        }
        let tokens: Vec<_> = line.split_whitespace().collect();
        let line_no = index + 1;
        match tokens.as_slice() {
            ["pdk", value] => {
                if name.replace((*value).to_string()).is_some() {
                    return Err(format!("line {line_no}: duplicate PDK header"));
                }
            }
            ["version", value] => {
                if version.replace((*value).to_string()).is_some() {
                    return Err(format!("line {line_no}: duplicate PDK version"));
                }
            }
            ["layer", layer, purpose, "pitch_um", pitch, "min_width_um", width, "min_spacing_um", spacing] =>
            {
                layers.push(PdkLayer {
                    name: (*layer).to_string(),
                    purpose: (*purpose).to_string(),
                    pitch_um: parse_pdk_number(pitch, line_no, "layer pitch")?,
                    min_width_um: parse_pdk_number(width, line_no, "layer width")?,
                    min_spacing_um: parse_pdk_number(spacing, line_no, "layer spacing")?,
                    tracks_per_channel: eda_pnr::TRACKS_PER_CHANNEL,
                });
            }
            ["layer", layer, purpose, "pitch_um", pitch, "min_width_um", width, "min_spacing_um", spacing, "tracks", tracks] =>
            {
                let tracks = tracks.parse::<usize>().map_err(|_| {
                    format!("line {line_no}: layer tracks must be a positive integer")
                })?;
                if tracks == 0 {
                    return Err(format!("line {line_no}: layer tracks must be positive"));
                }
                layers.push(PdkLayer {
                    name: (*layer).to_string(),
                    purpose: (*purpose).to_string(),
                    pitch_um: parse_pdk_number(pitch, line_no, "layer pitch")?,
                    min_width_um: parse_pdk_number(width, line_no, "layer width")?,
                    min_spacing_um: parse_pdk_number(spacing, line_no, "layer spacing")?,
                    tracks_per_channel: tracks,
                });
            }
            ["corner", corner, "process", process, "voltage_v", voltage, "temperature_c", temperature] =>
            {
                corners.push(PdkCorner {
                    name: (*corner).to_string(),
                    process: (*process).to_string(),
                    voltage_v: parse_pdk_number(voltage, line_no, "corner voltage")?,
                    temperature_c: parse_pdk_number(temperature, line_no, "corner temperature")?,
                    delay_scale: 1.0,
                    liberty_source: liberties.get(*corner).cloned(),
                    liberty_sha256: liberty_checksums.get(*corner).cloned(),
                });
            }
            ["corner", corner, "process", process, "voltage_v", voltage, "temperature_c", temperature, "delay_scale", scale] =>
            {
                corners.push(PdkCorner {
                    name: (*corner).to_string(),
                    process: (*process).to_string(),
                    voltage_v: parse_pdk_number(voltage, line_no, "corner voltage")?,
                    temperature_c: parse_pdk_number(temperature, line_no, "corner temperature")?,
                    delay_scale: parse_pdk_number(scale, line_no, "corner delay scale")?,
                    liberty_source: liberties.get(*corner).cloned(),
                    liberty_sha256: liberty_checksums.get(*corner).cloned(),
                });
            }
            ["correlate", device, parameter, "nominal", nominal, "golden", golden, "tolerance", tolerance] =>
            {
                model_correlations.push(PdkModelCorrelation {
                    device: (*device).to_string(),
                    parameter: (*parameter).to_string(),
                    nominal: parse_pdk_number(nominal, line_no, "nominal correlation value")?,
                    golden: parse_pdk_number(golden, line_no, "golden correlation value")?,
                    tolerance: parse_pdk_number(tolerance, line_no, "correlation tolerance")?,
                });
            }
            ["signoff", check] => {
                let Some(check) = PdkSignoffCheckKind::parse(check) else {
                    return Err(format!("line {line_no}: unknown signoff check '{check}'"));
                };
                required_checks.push(check);
            }
            _ => return Err(format!("line {line_no}: invalid PDK manifest syntax")),
        }
    }
    let definition = PdkDefinition {
        name: name.ok_or_else(|| "PDK manifest requires a 'pdk <name>' header".to_string())?,
        version: version.ok_or_else(|| "PDK manifest requires a version".to_string())?,
        layers,
        corners,
        model_correlations,
        layout_rule_deck_source: layout_rule_deck,
        layout_rule_deck_sha256: layout_rule_deck_checksum,
        required_checks,
    };
    definition.validate()?;
    let corner_names: BTreeSet<_> = definition
        .corners
        .iter()
        .map(|corner| &corner.name)
        .collect();
    for name in liberties.keys() {
        if !corner_names.contains(name) {
            return Err(format!(
                "Liberty view references unknown PDK corner '{name}'"
            ));
        }
    }
    for name in liberty_checksums.keys() {
        if !corner_names.contains(name) {
            return Err(format!(
                "Liberty checksum references unknown PDK corner '{name}'"
            ));
        }
    }
    Ok(definition)
}

pub fn write_pdk(definition: &PdkDefinition) -> Result<String, String> {
    definition.validate()?;
    let mut out = format!("pdk {}\nversion {}\n", definition.name, definition.version);
    for layer in &definition.layers {
        out.push_str(&format!(
            "layer {} {} pitch_um {} min_width_um {} min_spacing_um {} tracks {}\n",
            layer.name,
            layer.purpose,
            layer.pitch_um,
            layer.min_width_um,
            layer.min_spacing_um,
            layer.tracks_per_channel
        ));
    }
    for corner in &definition.corners {
        out.push_str(&format!(
            "corner {} process {} voltage_v {} temperature_c {} delay_scale {}\n",
            corner.name, corner.process, corner.voltage_v, corner.temperature_c, corner.delay_scale
        ));
        if let Some(source) = &corner.liberty_source {
            out.push_str(&format!(
                "liberty {} <<END\n{}\nEND\n",
                corner.name,
                source.trim_end()
            ));
        }
        if let Some(checksum) = &corner.liberty_sha256 {
            out.push_str(&format!(
                "liberty_sha256 {} {}\n",
                corner.name,
                checksum.to_ascii_lowercase()
            ));
        }
    }
    if let Some(source) = &definition.layout_rule_deck_source {
        out.push_str(&format!("rule_deck <<END\n{}\nEND\n", source.trim_end()));
    }
    if let Some(checksum) = &definition.layout_rule_deck_sha256 {
        out.push_str(&format!(
            "rule_deck_sha256 {}\n",
            checksum.to_ascii_lowercase()
        ));
    }
    for correlation in &definition.model_correlations {
        out.push_str(&format!(
            "correlate {} {} nominal {} golden {} tolerance {}\n",
            correlation.device,
            correlation.parameter,
            correlation.nominal,
            correlation.golden,
            correlation.tolerance
        ));
    }
    for check in &definition.required_checks {
        out.push_str(&format!("signoff {}\n", check.as_str()));
    }
    out.push_str(".end\n");
    Ok(out)
}

fn parse_pdk_number(value: &str, line_no: usize, field: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .map_err(|_| format!("line {line_no}: invalid {field} '{value}'"))
}

/// Return the lowercase SHA-256 digest used by PDK Liberty provenance checks.
pub fn pdk_liberty_sha256(source: &str) -> String {
    sha256_hex(source.as_bytes())
}

fn sha256_hex(message: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_length = (message.len() as u64).wrapping_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_length.to_be_bytes());

    for block in data.chunks_exact(64) {
        let mut schedule = [0_u32; 64];
        for (index, value) in block.chunks_exact(4).enumerate() {
            schedule[index] = u32::from_be_bytes([value[0], value[1], value[2], value[3]]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temporary1 = h
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary1);
            d = c;
            c = b;
            b = a;
            a = temporary1.wrapping_add(temporary2);
        }
        for (value, state_value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *value = value.wrapping_add(state_value);
        }
    }

    state
        .iter()
        .fold(String::with_capacity(64), |mut out, value| {
            use std::fmt::Write as _;
            let _ = write!(out, "{value:08x}");
            out
        })
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkModelCorrelationResult {
    pub device: String,
    pub parameter: String,
    pub nominal: f64,
    pub golden: f64,
    pub relative_error: f64,
    pub tolerance: f64,
    pub passed: bool,
}

pub fn correlate_pdk_models(
    definition: &PdkDefinition,
) -> Result<Vec<PdkModelCorrelationResult>, String> {
    definition.validate()?;
    Ok(definition
        .model_correlations
        .iter()
        .map(|entry| {
            let scale = entry.nominal.abs().max(1e-12);
            let relative_error = ((entry.nominal - entry.golden).abs() / scale).max(0.0);
            PdkModelCorrelationResult {
                device: entry.device.clone(),
                parameter: entry.parameter.clone(),
                nominal: entry.nominal,
                golden: entry.golden,
                relative_error,
                tolerance: entry.tolerance,
                passed: relative_error <= entry.tolerance,
            }
        })
        .collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkSignoffCheckResult {
    pub kind: PdkSignoffCheckKind,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkCornerResult {
    pub name: String,
    pub process: String,
    pub voltage_v: f64,
    pub temperature_c: f64,
    pub delay_scale: f64,
    pub library_name: String,
    pub imported_liberty: bool,
    pub liberty_sha256: Option<String>,
    pub equivalent: bool,
    pub critical_path_delay_ns: f64,
    pub drc_errors: usize,
    pub drc_warnings: usize,
    pub physical_database_complete: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdkSignoffResult {
    pub pdk_name: String,
    pub pdk_version: String,
    pub design_name: String,
    pub netlist_name: String,
    pub corners: Vec<PdkCorner>,
    pub corner_results: Vec<PdkCornerResult>,
    pub equivalent: bool,
    pub critical_path_delay_ns: f64,
    pub drc_errors: usize,
    pub drc_warnings: usize,
    pub layout_rule_deck_sha256: Option<String>,
    pub layout_rule_deck_validated: bool,
    pub model_correlations: Vec<PdkModelCorrelationResult>,
    pub checks: Vec<PdkSignoffCheckResult>,
    pub passed: bool,
}

impl PdkSignoffResult {
    pub fn check(&self, kind: PdkSignoffCheckKind) -> Option<&PdkSignoffCheckResult> {
        self.checks.iter().find(|result| result.kind == kind)
    }
}

fn scaled_lut(lut: &LibertyLut, scale: f64) -> LibertyLut {
    LibertyLut {
        index_1: lut.index_1.clone(),
        index_2: lut.index_2.clone(),
        values: lut.values.iter().map(|value| value * scale).collect(),
    }
}

fn scaled_constraint_tables(
    tables: &Option<LibConstraintTables>,
    scale: f64,
) -> Option<LibConstraintTables> {
    tables.as_ref().map(|tables| LibConstraintTables {
        rise: tables.rise.as_ref().map(|lut| scaled_lut(lut, scale)),
        fall: tables.fall.as_ref().map(|lut| scaled_lut(lut, scale)),
    })
}

fn scaled_timing_table(table: &LibTimingTables, scale: f64) -> LibTimingTables {
    LibTimingTables {
        input_pin: table.input_pin,
        rise: table.rise.as_ref().map(|lut| scaled_lut(lut, scale)),
        fall: table.fall.as_ref().map(|lut| scaled_lut(lut, scale)),
        rise_transition: table
            .rise_transition
            .as_ref()
            .map(|lut| scaled_lut(lut, scale)),
        fall_transition: table
            .fall_transition
            .as_ref()
            .map(|lut| scaled_lut(lut, scale)),
    }
}

fn scaled_cell_library(library: &CellLibrary, scale: f64) -> CellLibrary {
    let mut out = library.clone();
    out.name = format!("{}-x{scale:.6}", library.name);
    for cell in &mut out.cells {
        cell.arcs = cell
            .arcs
            .iter()
            .map(|(input, delay)| (*input, delay * scale))
            .collect();
        cell.timing_tables = cell
            .timing_tables
            .iter()
            .map(|table| scaled_timing_table(table, scale))
            .collect();
        if let Some(seq) = &mut cell.seq {
            seq.clk_to_q *= scale;
            seq.setup *= scale;
            seq.hold *= scale;
            seq.setup_tables = scaled_constraint_tables(&seq.setup_tables, scale);
            seq.hold_tables = scaled_constraint_tables(&seq.hold_tables, scale);
            seq.recovery_tables = scaled_constraint_tables(&seq.recovery_tables, scale);
            seq.removal_tables = scaled_constraint_tables(&seq.removal_tables, scale);
            seq.recovery *= scale;
            seq.removal *= scale;
            seq.min_pulse_high *= scale;
            seq.min_pulse_low *= scale;
        }
    }
    out
}

pub fn run_pdk_signoff(
    rtl: &str,
    top: Option<&str>,
    pdk: &PdkDefinition,
) -> Result<PdkSignoffResult, String> {
    run_pdk_signoff_cancellable(rtl, top, pdk, &FlowCancellation::new())
}

/// Run PDK signoff with cooperative cancellation between corner flows.
/// Routing capacity derived from the PDK routing layers. The densest
/// routing layer determines the per-channel track budget.
pub fn pdk_route_options(pdk: &PdkDefinition) -> Result<eda_pnr::RouteOptions, String> {
    pdk.validate()?;
    let tracks = pdk
        .layers
        .iter()
        .filter(|layer| layer.purpose == "routing")
        .map(|layer| layer.tracks_per_channel)
        .max()
        .unwrap_or(eda_pnr::TRACKS_PER_CHANNEL);
    eda_pnr::RouteOptions::new(tracks)
}

pub fn run_pdk_signoff_cancellable(
    rtl: &str,
    top: Option<&str>,
    pdk: &PdkDefinition,
    cancellation: &FlowCancellation,
) -> Result<PdkSignoffResult, String> {
    pdk.validate()?;
    if cancellation.is_cancelled() {
        return Err("PDK signoff cancelled".into());
    }
    let correlations = correlate_pdk_models(pdk)?;

    let mut corner_results = Vec::new();
    let mut corner_flows = Vec::new();
    for corner in &pdk.corners {
        if cancellation.is_cancelled() {
            return Err("PDK signoff cancelled".into());
        }
        let imported_liberty = corner.liberty_source.is_some();
        let library = if let Some(source) = &corner.liberty_source {
            let parsed = parse_liberty(source)
                .map_err(|error| format!("PDK corner '{}': {error}", corner.name))?;
            scaled_cell_library(&parsed, corner.delay_scale)
        } else {
            scaled_cell_library(&CellLibrary::std(), corner.delay_scale)
        };
        let library_name = library.name.clone();
        let include_lec = pdk
            .required_checks
            .contains(&PdkSignoffCheckKind::Equivalence);
        let flow = run_flow_staged_cancellable(
            rtl,
            top,
            &library,
            &pdk_route_options(pdk)?,
            include_lec,
            cancellation,
        )?;
        let drc_errors = flow
            .drc
            .violations
            .iter()
            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
            .count();
        let drc_warnings = flow
            .drc
            .violations
            .iter()
            .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
            .count();
        corner_results.push(PdkCornerResult {
            name: corner.name.clone(),
            process: corner.process.clone(),
            voltage_v: corner.voltage_v,
            temperature_c: corner.temperature_c,
            delay_scale: corner.delay_scale,
            library_name,
            imported_liberty,
            liberty_sha256: corner.liberty_sha256.clone(),
            equivalent: flow.equivalent,
            critical_path_delay_ns: flow.timing.critical_path_delay,
            drc_errors,
            drc_warnings,
            physical_database_complete: !flow.lef.trim().is_empty()
                && !flow.def.trim().is_empty()
                && !flow.sdf.trim().is_empty(),
        });
        corner_flows.push(flow);
    }
    let flow = if pdk.corners.is_empty() {
        run_flow_staged_cancellable(
            rtl,
            top,
            &CellLibrary::std(),
            &pdk_route_options(pdk)?,
            pdk.required_checks
                .contains(&PdkSignoffCheckKind::Equivalence),
            cancellation,
        )?
    } else {
        let worst = corner_results
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                left.critical_path_delay_ns
                    .partial_cmp(&right.critical_path_delay_ns)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        corner_flows.remove(worst)
    };
    if cancellation.is_cancelled() {
        return Err("PDK signoff cancelled".into());
    }

    let all_equivalent = corner_results.iter().all(|corner| corner.equivalent);
    let baseline_drc_errors = flow
        .drc
        .violations
        .iter()
        .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Error)
        .count();
    let baseline_drc_warnings = flow
        .drc
        .violations
        .iter()
        .filter(|violation| violation.severity == eda_pnr::DrcSeverity::Warning)
        .count();
    let total_drc_errors: usize = if corner_results.is_empty() {
        baseline_drc_errors
    } else {
        corner_results.iter().map(|corner| corner.drc_errors).sum()
    };
    let total_drc_warnings: usize = if corner_results.is_empty() {
        baseline_drc_warnings
    } else {
        corner_results
            .iter()
            .map(|corner| corner.drc_warnings)
            .sum()
    };
    let finite_corners = corner_results
        .iter()
        .filter(|corner| corner.critical_path_delay_ns.is_finite())
        .count();
    let mut checks = Vec::new();
    for kind in &pdk.required_checks {
        let (passed, detail) = match kind {
            PdkSignoffCheckKind::ModelCorrelation => {
                let failed = correlations.iter().filter(|entry| !entry.passed).count();
                (
                    failed == 0,
                    format!(
                        "{}/{} model correlations within tolerance",
                        correlations.len() - failed,
                        correlations.len()
                    ),
                )
            }
            PdkSignoffCheckKind::Equivalence => (
                if corner_results.is_empty() {
                    flow.equivalent
                } else {
                    all_equivalent
                },
                "generic and mapped netlists equivalent in every corner".to_string(),
            ),
            PdkSignoffCheckKind::Timing => {
                let passed = if corner_results.is_empty() {
                    flow.timing.critical_path_delay.is_finite()
                        && flow.timing.critical_path_delay >= 0.0
                } else {
                    finite_corners == corner_results.len()
                };
                (
                    passed,
                    if corner_results.is_empty() {
                        format!(
                            "critical path delay {:.6} ns",
                            flow.timing.critical_path_delay
                        )
                    } else {
                        format!(
                            "{}/{} corner timing results finite; worst {:.6} ns",
                            finite_corners,
                            corner_results.len(),
                            flow.timing.critical_path_delay
                        )
                    },
                )
            }
            PdkSignoffCheckKind::Drc => (
                total_drc_errors == 0,
                format!("{total_drc_errors} DRC errors, {total_drc_warnings} warnings"),
            ),
            PdkSignoffCheckKind::PhysicalDatabase => {
                let passed = if corner_results.is_empty() {
                    !flow.lef.trim().is_empty()
                        && !flow.def.trim().is_empty()
                        && !flow.sdf.trim().is_empty()
                } else {
                    corner_results
                        .iter()
                        .all(|corner| corner.physical_database_complete)
                };
                (
                    passed,
                    "LEF, DEF, and SDF databases generated in every corner".to_string(),
                )
            }
        };
        checks.push(PdkSignoffCheckResult {
            kind: *kind,
            passed,
            detail,
        });
    }
    let passed = checks.iter().all(|check| check.passed);
    Ok(PdkSignoffResult {
        pdk_name: pdk.name.clone(),
        pdk_version: pdk.version.clone(),
        design_name: flow.design.name.clone(),
        netlist_name: flow.netlist.name.clone(),
        corners: pdk.corners.clone(),
        equivalent: if corner_results.is_empty() {
            flow.equivalent
        } else {
            all_equivalent
        },
        critical_path_delay_ns: flow.timing.critical_path_delay,
        drc_errors: total_drc_errors,
        drc_warnings: total_drc_warnings,
        layout_rule_deck_sha256: pdk.layout_rule_deck_sha256.clone(),
        layout_rule_deck_validated: pdk.layout_rule_deck_source.is_some(),
        model_correlations: correlations,
        corner_results,
        checks,
        passed,
    })
}

pub fn report_pdk_signoff(result: &PdkSignoffResult) -> String {
    let mut out = format!(
        "PDK signoff: pdk={} version={} design={} netlist={}\n",
        result.pdk_name, result.pdk_version, result.design_name, result.netlist_name
    );
    out.push_str(&format!(
        "Layout rule deck: {}\n",
        result
            .layout_rule_deck_sha256
            .as_deref()
            .unwrap_or("not-supplied")
    ));
    for correlation in &result.model_correlations {
        out.push_str(&format!(
            "Model correlation {}.{}: nominal={:.9} golden={:.9} error={:.6}% tolerance={:.6}% {}\n",
            correlation.device,
            correlation.parameter,
            correlation.nominal,
            correlation.golden,
            correlation.relative_error,
            correlation.tolerance,
            if correlation.passed { "PASS" } else { "FAIL" }
        ));
    }
    for corner in &result.corner_results {
        out.push_str(&format!(
            "PDK corner result {}: process={} V={:.3} T={:.1} C scale={:.6} library={} imported-liberty={} liberty-checksum={} timing={:.6} ns equivalent={} DRC={} errors/{} warnings {}\n",
            corner.name,
            corner.process,
            corner.voltage_v,
            corner.temperature_c,
            corner.delay_scale,
            corner.library_name,
            corner.imported_liberty,
            corner.liberty_sha256.as_deref().unwrap_or("not-supplied"),
            corner.critical_path_delay_ns,
            corner.equivalent,
            corner.drc_errors,
            corner.drc_warnings,
            if corner.physical_database_complete {
                "physical-db=PASS"
            } else {
                "physical-db=FAIL"
            }
        ));
    }
    for check in &result.checks {
        out.push_str(&format!(
            "Check {}: {} ({})\n",
            check.kind.as_str(),
            if check.passed { "PASS" } else { "FAIL" },
            check.detail
        ));
    }
    out.push_str(if result.passed {
        "Overall signoff: PASS\n"
    } else {
        "Overall signoff: FAIL\n"
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(tolerance: f64) -> String {
        format!(
            "pdk test-pdk\nversion 1.2\nlayer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14\ncorner tt process TT voltage_v 1.8 temperature_c 25\ncorrelate nmos VTO nominal 0.45 golden 0.452 tolerance {tolerance}\nsignoff model-correlation\nsignoff equivalence\nsignoff timing\nsignoff drc\nsignoff physical-database\n.end"
        )
    }

    #[test]
    fn pdk_manifest_round_trips_and_validates() {
        let source = manifest(0.01);
        let definition = parse_pdk(&source).unwrap();
        assert_eq!(
            definition,
            parse_pdk(&write_pdk(&definition).unwrap()).unwrap()
        );
        assert_eq!(definition.required_checks.len(), 5);
        let bad = "pdk bad\nversion 1\nsignoff timing\nsignoff timing\n.end";
        assert!(parse_pdk(bad).is_err());
    }

    #[test]
    fn pdk_layout_rule_deck_is_checksummed_and_validated() {
        let rule_deck = r#"version 1
layer M1 min_width 10 min_spacing 20 min_area 100 min_notch 25
layer M2 min_width 10 min_spacing 20 min_area 100
spacing M1 M2 30
via VIA1 cut_layer V1 bottom_layer M1 top_layer M2 cut_width 20 cut_height 20 bottom_enclosure 1 top_enclosure 2 bottom_extension_x 10 bottom_extension_y 5 top_extension_x 6 top_extension_y 11 cut_spacing 20
"#;
        let checksum = pdk_liberty_sha256(rule_deck.trim());
        let source = format!(
            "pdk rule-pdk\nversion 1.0\nrule_deck <<END\n{rule_deck}\nEND\nrule_deck_sha256 {checksum}\nsignoff timing\nsignoff drc\n.end"
        );
        let definition = parse_pdk(&source).unwrap();
        assert_eq!(
            definition.layout_rule_deck_source.as_deref(),
            Some(rule_deck.trim())
        );
        assert_eq!(
            definition.layout_rule_deck_sha256.as_deref(),
            Some(checksum.as_str())
        );
        assert_eq!(
            definition,
            parse_pdk(&write_pdk(&definition).unwrap()).unwrap()
        );
        let rtl = "module top(input a, output y); assign y = ~a; endmodule";
        let signoff = run_pdk_signoff(rtl, Some("top"), &definition).unwrap();
        assert!(signoff.layout_rule_deck_validated);
        assert_eq!(
            signoff.layout_rule_deck_sha256.as_deref(),
            Some(checksum.as_str())
        );
        assert!(report_pdk_signoff(&signoff).contains(&format!("Layout rule deck: {checksum}")));

        let bad_checksum = "0".repeat(64);
        let bad = source.replace(&checksum, &bad_checksum);
        assert!(parse_pdk(&bad)
            .unwrap_err()
            .contains("layout rule deck checksum mismatch"));
        let invalid_deck = source.replace("min_width 10", "min_width broken");
        assert!(parse_pdk(&invalid_deck)
            .unwrap_err()
            .contains("invalid PDK layout rule deck"));
    }

    #[test]
    fn model_correlation_uses_relative_error() {
        let passing = correlate_pdk_models(&parse_pdk(&manifest(0.005)).unwrap()).unwrap();
        assert_eq!(passing.len(), 1);
        assert!(passing[0].passed);
        let failing = correlate_pdk_models(&parse_pdk(&manifest(0.001)).unwrap()).unwrap();
        assert!(!failing[0].passed);
    }

    #[test]
    fn signoff_runs_the_implementation_flow() {
        let rtl = "module top(input a, output y); assign y = ~a; endmodule";
        let pdk = parse_pdk(&manifest(0.01)).unwrap();
        let result = run_pdk_signoff(rtl, Some("top"), &pdk).unwrap();
        assert!(result.passed, "{result:?}");
        assert!(result.equivalent);
        let report = report_pdk_signoff(&result);
        assert!(report.contains("PDK corner result tt"));
        assert!(report.contains("Overall signoff: PASS"));
    }

    #[test]
    fn pdk_corners_drive_separate_mmmc_implementation_runs() {
        let source = r#"pdk mmmc-pdk
version 2.0
corner fast process FF voltage_v 1.9 temperature_c -40 delay_scale 0.8
corner slow process SS voltage_v 1.6 temperature_c 125 delay_scale 1.25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#;
        let pdk = parse_pdk(source).unwrap();
        assert_eq!(
            pdk,
            parse_pdk(&write_pdk(&pdk).unwrap()).unwrap(),
            "explicit delay-scale corners must round-trip"
        );
        let rtl = "module top(input a, b, output y); assign y = a & b; endmodule";
        let result = run_pdk_signoff(rtl, Some("top"), &pdk).unwrap();
        assert!(result.passed, "{result:?}");
        assert_eq!(result.corner_results.len(), 2);
        let fast = result
            .corner_results
            .iter()
            .find(|c| c.name == "fast")
            .unwrap();
        let slow = result
            .corner_results
            .iter()
            .find(|c| c.name == "slow")
            .unwrap();
        assert!((fast.delay_scale - 0.8).abs() < 1e-12);
        assert!((slow.delay_scale - 1.25).abs() < 1e-12);
        assert!(
            (slow.critical_path_delay_ns / fast.critical_path_delay_ns - 1.25 / 0.8).abs() < 1e-9,
            "{fast:?} {slow:?}"
        );
        assert_eq!(result.critical_path_delay_ns, slow.critical_path_delay_ns);
        let report = report_pdk_signoff(&result);
        assert!(
            report.contains("PDK corner result fast")
                && report.contains("PDK corner result slow")
                && report.contains("2/2 corner timing results finite"),
            "{report}"
        );
    }

    #[test]
    fn pdk_corner_can_import_inline_liberty_view() {
        let liberty = r#"
            library (vendor-view) {
                time_unit : "1ns";
                cell (INV) {
                    area : 1.2;
                    pin (A) { direction : input; capacitance : 0.01; }
                    pin (Y) {
                        direction : output;
                        function : "!A";
                        timing () { related_pin : "A"; cell_rise(values("0.35,0.42")); }
                    }
                }
            }
        "#;
        let checksum = pdk_liberty_sha256(liberty.trim());
        let source = format!(
            "pdk liberty-pdk\nversion 3.0\ncorner vendor process TT voltage_v 1.8 temperature_c 25 delay_scale 2.0\nliberty vendor <<END\n{liberty}\nEND\nliberty_sha256 vendor {checksum}\ncorrelate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01\nsignoff model-correlation\nsignoff equivalence\nsignoff timing\nsignoff drc\nsignoff physical-database\n.end"
        );
        let pdk = parse_pdk(&source).unwrap();
        assert_eq!(
            pdk,
            parse_pdk(&write_pdk(&pdk).unwrap()).unwrap(),
            "inline Liberty views must round-trip"
        );
        assert!(pdk.corners[0]
            .liberty_source
            .as_deref()
            .is_some_and(|source| source.contains("vendor-view")));
        assert_eq!(
            pdk.corners[0].liberty_sha256.as_deref(),
            Some(checksum.as_str())
        );

        let mut corrupted = source;
        let position = corrupted
            .find("capacitance : 0.01")
            .expect("test capacitance token");
        corrupted.replace_range(
            position..position + "capacitance : 0.01".len(),
            "capacitance : 0.99",
        );
        let error = parse_pdk(&corrupted).unwrap_err();
        assert!(error.contains("Liberty checksum mismatch"), "{error}");

        let rtl = "module top(input a, output y); assign y = ~a; endmodule";
        let result = run_pdk_signoff(rtl, Some("top"), &pdk).unwrap();
        assert!(result.passed, "{result:?}");
        let corner = &result.corner_results[0];
        assert!(corner.imported_liberty);
        assert!(corner.library_name.contains("vendor-view"));
        assert!(
            (corner.critical_path_delay_ns - 0.7).abs() < 1e-9,
            "{corner:?}"
        );
        let report = report_pdk_signoff(&result);
        assert!(
            report.contains("library=vendor-view-x2.000000")
                && report.contains("imported-liberty=true")
                && report.contains(&format!("liberty-checksum={checksum}")),
            "{report}"
        );
    }

    #[test]
    fn pdk_liberty_sha256_matches_standard_vectors() {
        assert_eq!(
            pdk_liberty_sha256(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            pdk_liberty_sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
