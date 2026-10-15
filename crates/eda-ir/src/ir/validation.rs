//! Shared numerical and domain option validation.

use super::types::DomainIrOptions;

pub(super) fn validate_domain_options(
    domain: &str,
    options: &DomainIrOptions,
) -> Result<(), String> {
    validate_f64(
        &format!("domain '{}' voltage", domain),
        options.voltage_v,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' VDD resistance", domain),
        options.vdd_ohm_per_site,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' GND resistance", domain),
        options.gnd_ohm_per_site,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' maximum drop", domain),
        options.max_total_drop_v,
        true,
    )?;
    validate_f64(
        &format!("domain '{}' maximum rail current", domain),
        options.max_rail_current_a,
        true,
    )?;
    Ok(())
}

pub(super) fn validate_f64(name: &str, value: f64, positive: bool) -> Result<(), String> {
    if !value.is_finite() || (positive && value <= 0.0) {
        return Err(format!("invalid {} {}", name, value));
    }
    Ok(())
}
