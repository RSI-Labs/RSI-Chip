//! Waiver/audit serialization and parsing.

use std::collections::{BTreeMap, BTreeSet};

use super::types::*;

pub(super) fn quote_waiver_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

pub(super) fn tokenize_waiver_line(line: &str, line_number: usize) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&current) = chars.peek() {
        if current.is_whitespace() {
            chars.next();
            continue;
        }
        if current == '#' {
            break;
        }
        if current == '"' {
            chars.next();
            let mut value = String::new();
            let mut closed = false;
            while let Some(character) = chars.next() {
                match character {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => {
                        let Some(escaped) = chars.next() else {
                            return Err(format!(
                                "line {}: unterminated waiver escape",
                                line_number
                            ));
                        };
                        match escaped {
                            'n' => value.push('\n'),
                            '\\' => value.push('\\'),
                            '"' => value.push('"'),
                            other => {
                                value.push('\\');
                                value.push(other);
                            }
                        }
                    }
                    other => value.push(other),
                }
            }
            if !closed {
                return Err(format!("line {}: unterminated waiver string", line_number));
            }
            tokens.push(value);
        } else {
            let mut value = String::new();
            while let Some(&character) = chars.peek() {
                if character.is_whitespace() || character == '#' {
                    break;
                }
                value.push(character);
                chars.next();
            }
            tokens.push(value);
        }
    }
    Ok(tokens)
}

pub(super) fn parse_waiver_kind(
    value: &str,
    line_number: usize,
) -> Result<TimingViolationKind, String> {
    match value {
        "setup" => Ok(TimingViolationKind::Setup),
        "hold" => Ok(TimingViolationKind::Hold),
        "recovery" => Ok(TimingViolationKind::Recovery),
        "removal" => Ok(TimingViolationKind::Removal),
        "output" => Ok(TimingViolationKind::Output),
        "max_delay" => Ok(TimingViolationKind::MaxDelay),
        "min_delay" => Ok(TimingViolationKind::MinDelay),
        "min_pulse_high" => Ok(TimingViolationKind::MinPulseHigh),
        "min_pulse_low" => Ok(TimingViolationKind::MinPulseLow),
        other => Err(format!(
            "line {}: unknown timing violation kind '{}'",
            line_number, other
        )),
    }
}

pub(super) fn validate_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<(), String> {
    let mut next_action = BTreeMap::<String, TimingWaiverAuditAction>::new();
    let mut last_generation = 0u64;
    for (index, entry) in entries.iter().enumerate() {
        let expected_sequence = index as u64 + 1;
        if entry.sequence != expected_sequence {
            return Err(format!(
                "waiver audit sequence {} at index {} should be {}",
                entry.sequence, index, expected_sequence
            ));
        }
        if entry.generation < last_generation {
            return Err(format!(
                "waiver audit generation {} is older than prior generation {}",
                entry.generation, last_generation
            ));
        }
        last_generation = entry.generation;
        if entry.waiver.id != entry.waiver_id {
            return Err(format!(
                "waiver audit entry #{} snapshot ID '{}' does not match event ID '{}'",
                entry.sequence, entry.waiver.id, entry.waiver_id
            ));
        }
        entry
            .waiver
            .validate()
            .map_err(|error| format!("waiver audit entry #{}: {}", entry.sequence, error))?;
        let expected = next_action.remove(&entry.waiver_id);
        let valid = matches!(
            (&expected, entry.action),
            (None, TimingWaiverAuditAction::Added)
                | (
                    Some(TimingWaiverAuditAction::Removed),
                    TimingWaiverAuditAction::Added
                )
                | (
                    Some(TimingWaiverAuditAction::Added),
                    TimingWaiverAuditAction::Removed
                )
        );
        if !valid {
            return Err(format!(
                "waiver audit event #{} has invalid action transition for '{}'",
                entry.sequence, entry.waiver_id
            ));
        }
        next_action.insert(entry.waiver_id.clone(), entry.action);
    }
    Ok(())
}

pub(super) fn active_waivers_from_audit(
    entries: &[TimingWaiverAuditEntry],
) -> BTreeMap<String, TimingViolationWaiver> {
    let mut active = BTreeMap::new();
    for entry in entries {
        match entry.action {
            TimingWaiverAuditAction::Added => {
                active.insert(entry.waiver_id.clone(), entry.waiver.clone());
            }
            TimingWaiverAuditAction::Removed => {
                active.remove(&entry.waiver_id);
            }
        }
    }
    active
}

/// Serialize waiver rules to a deterministic, dependency-free text format.
pub fn write_timing_violation_waivers(waivers: &[TimingViolationWaiver]) -> Result<String, String> {
    let mut ids = BTreeSet::new();
    let mut out = String::new();
    for waiver in waivers {
        waiver.validate()?;
        if !ids.insert(waiver.id.as_str()) {
            return Err(format!("duplicate timing violation waiver '{}'", waiver.id));
        }
        out.push_str(&format!(
            "waiver {} {} {} {}\n",
            quote_waiver_value(&waiver.id),
            quote_waiver_value(&waiver.scenario_pattern),
            quote_waiver_value(&waiver.endpoint_pattern),
            quote_waiver_value(&waiver.slack_floor_ns.to_string())
        ));
        if let Some(kind) = waiver.kind {
            out.push_str(&format!("kind {}\n", kind.label()));
        }
        if let Some(instance) = &waiver.instance_pattern {
            out.push_str(&format!("instance {}\n", quote_waiver_value(instance)));
        }
        if let Some(owner) = &waiver.owner {
            out.push_str(&format!("owner {}\n", quote_waiver_value(owner)));
        }
        if let Some(note) = &waiver.note {
            out.push_str(&format!("note {}\n", quote_waiver_value(note)));
        }
        if let Some(expiry) = waiver.expires_after_generation {
            out.push_str(&format!("expires_after {}\n", expiry));
        }
    }
    Ok(out)
}

/// Parse the text emitted by [`write_timing_violation_waivers`].
pub fn parse_timing_violation_waivers(source: &str) -> Result<Vec<TimingViolationWaiver>, String> {
    struct Partial {
        id: String,
        scenario: String,
        endpoint: String,
        instance: Option<String>,
        slack_floor: f64,
        kind: Option<TimingViolationKind>,
        owner: Option<String>,
        note: Option<String>,
        expiry: Option<u64>,
    }

    fn finish(
        partial: Partial,
        line_number: usize,
        ids: &mut BTreeSet<String>,
    ) -> Result<TimingViolationWaiver, String> {
        let mut waiver = TimingViolationWaiver::new(
            partial.id,
            partial.scenario,
            partial.endpoint,
            partial.slack_floor,
        )
        .map_err(|error| format!("line {}: {}", line_number, error))?;
        if let Some(kind) = partial.kind {
            waiver = waiver.with_kind(kind);
        }
        if let Some(instance) = partial.instance {
            waiver = waiver.with_instance_pattern(instance);
        }
        if let Some(owner) = partial.owner {
            waiver = waiver.with_owner(owner);
        }
        if let Some(note) = partial.note {
            waiver = waiver.with_note(note);
        }
        if let Some(expiry) = partial.expiry {
            waiver = waiver.expires_after(expiry);
        }
        waiver
            .validate()
            .map_err(|error| format!("line {}: {}", line_number, error))?;
        if !ids.insert(waiver.id.clone()) {
            return Err(format!(
                "line {}: duplicate timing violation waiver '{}'",
                line_number, waiver.id
            ));
        }
        Ok(waiver)
    }

    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut current: Option<Partial> = None;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let tokens = tokenize_waiver_line(raw, line_number)?;
        if tokens.is_empty() {
            continue;
        }
        match tokens[0].as_str() {
            "waiver" => {
                if let Some(partial) = current.take() {
                    out.push(finish(partial, line_number.saturating_sub(1), &mut ids)?);
                }
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: waiver requires ID, scenario, endpoint, and slack floor",
                        line_number
                    ));
                }
                let slack_floor = tokens[4]
                    .parse::<f64>()
                    .map_err(|_| format!("line {}: invalid waiver slack floor", line_number))?;
                current = Some(Partial {
                    id: tokens[1].clone(),
                    scenario: tokens[2].clone(),
                    endpoint: tokens[3].clone(),
                    instance: None,
                    slack_floor,
                    kind: None,
                    owner: None,
                    note: None,
                    expiry: None,
                });
            }
            "kind" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: kind appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.kind.is_some() {
                    return Err(format!("line {}: malformed or duplicate kind", line_number));
                }
                partial.kind = Some(parse_waiver_kind(&tokens[1], line_number)?);
            }
            "instance" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!(
                        "line {}: instance appears before waiver",
                        line_number
                    ));
                };
                if tokens.len() != 2 || partial.instance.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate instance",
                        line_number
                    ));
                }
                partial.instance = Some(tokens[1].clone());
            }
            "owner" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: owner appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.owner.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate owner",
                        line_number
                    ));
                }
                partial.owner = Some(tokens[1].clone());
            }
            "note" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!("line {}: note appears before waiver", line_number));
                };
                if tokens.len() != 2 || partial.note.is_some() {
                    return Err(format!("line {}: malformed or duplicate note", line_number));
                }
                partial.note = Some(tokens[1].clone());
            }
            "expires_after" => {
                let Some(partial) = current.as_mut() else {
                    return Err(format!(
                        "line {}: expires_after appears before waiver",
                        line_number
                    ));
                };
                if tokens.len() != 2 || partial.expiry.is_some() {
                    return Err(format!(
                        "line {}: malformed or duplicate expires_after",
                        line_number
                    ));
                }
                partial.expiry = Some(
                    tokens[1]
                        .parse::<u64>()
                        .map_err(|_| format!("line {}: invalid waiver expiry", line_number))?,
                );
            }
            other => {
                return Err(format!(
                    "line {}: unknown timing waiver token '{}'",
                    line_number, other
                ));
            }
        }
    }
    if let Some(partial) = current.take() {
        let line_number = source.lines().count();
        out.push(finish(partial, line_number, &mut ids)?);
    }
    Ok(out)
}

/// Serialize immutable waiver audit events and their complete rule snapshots.
pub fn write_timing_waiver_audit(entries: &[TimingWaiverAuditEntry]) -> Result<String, String> {
    validate_waiver_audit(entries)?;
    let mut out = String::new();
    for entry in entries {
        let actor = entry.changed_by.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "audit {} {} {} {}\n",
            entry.sequence,
            entry.action.label(),
            entry.generation,
            quote_waiver_value(actor)
        ));
        out.push_str(&write_timing_violation_waivers(std::slice::from_ref(
            &entry.waiver,
        ))?);
        out.push_str("end\n");
    }
    Ok(out)
}

/// Parse the text emitted by [`write_timing_waiver_audit`].
pub fn parse_timing_waiver_audit(source: &str) -> Result<Vec<TimingWaiverAuditEntry>, String> {
    struct Header {
        sequence: u64,
        action: TimingWaiverAuditAction,
        generation: u64,
        changed_by: Option<String>,
    }

    let mut entries = Vec::new();
    let mut current_header: Option<Header> = None;
    let mut block = String::new();
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let tokens = tokenize_waiver_line(raw, line_number)?;
        if tokens.is_empty() {
            continue;
        }
        match tokens[0].as_str() {
            "audit" => {
                if current_header.is_some() {
                    return Err(format!(
                        "line {}: previous timing waiver audit block is not closed",
                        line_number
                    ));
                }
                if tokens.len() != 5 {
                    return Err(format!(
                        "line {}: audit requires sequence, action, generation, and actor",
                        line_number
                    ));
                }
                let sequence = tokens[1]
                    .parse::<u64>()
                    .map_err(|_| format!("line {}: invalid audit sequence", line_number))?;
                let action = TimingWaiverAuditAction::parse(&tokens[2], line_number)?;
                let generation = tokens[3]
                    .parse::<u64>()
                    .map_err(|_| format!("line {}: invalid audit generation", line_number))?;
                current_header = Some(Header {
                    sequence,
                    action,
                    generation,
                    changed_by: (!tokens[4].is_empty()).then(|| tokens[4].clone()),
                });
                block.clear();
            }
            "end" => {
                let Some(header) = current_header.take() else {
                    return Err(format!(
                        "line {}: end appears outside a timing waiver audit block",
                        line_number
                    ));
                };
                if tokens.len() != 1 {
                    return Err(format!("line {}: malformed audit end", line_number));
                }
                let mut waivers = parse_timing_violation_waivers(&block)
                    .map_err(|error| format!("line {}: {}", line_number, error))?;
                if waivers.len() != 1 {
                    return Err(format!(
                        "line {}: audit block must contain exactly one waiver",
                        line_number
                    ));
                }
                let waiver = waivers.remove(0);
                entries.push(TimingWaiverAuditEntry {
                    sequence: header.sequence,
                    action: header.action,
                    waiver_id: waiver.id.clone(),
                    generation: header.generation,
                    changed_by: header.changed_by,
                    waiver,
                });
            }
            _ => {
                if current_header.is_some() {
                    block.push_str(raw);
                    block.push('\n');
                } else {
                    return Err(format!(
                        "line {}: token '{}' appears outside a timing waiver audit block",
                        line_number, tokens[0]
                    ));
                }
            }
        }
    }
    if current_header.is_some() {
        return Err("unterminated timing waiver audit block".into());
    }
    validate_waiver_audit(&entries)?;
    Ok(entries)
}
