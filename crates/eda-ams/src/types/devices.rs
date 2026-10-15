//! Compact device models and SPICE circuit storage.

use super::circuit::{ensure_finite, ensure_finite_nonnegative, ensure_finite_positive};
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiodeModel {
    pub saturation_current_a: f64,
    pub emission_coefficient: f64,
    pub series_resistance_ohm: f64,
    pub zero_bias_junction_cap_f: f64,
    pub junction_potential_v: f64,
    pub grading_coefficient: f64,
    pub forward_knee: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub energy_gap_ev: f64,
    pub saturation_current_temperature_exponent: f64,
    pub transit_time_s: f64,
}

impl DiodeModel {
    pub fn validate(&self) -> Result<(), String> {
        if !self.saturation_current_a.is_finite() || self.saturation_current_a <= 0.0 {
            return Err("diode IS must be finite and positive".into());
        }
        if !self.emission_coefficient.is_finite() || self.emission_coefficient <= 0.0 {
            return Err("diode N must be finite and positive".into());
        }
        if !self.series_resistance_ohm.is_finite() || self.series_resistance_ohm < 0.0 {
            return Err("diode RS must be finite and nonnegative".into());
        }
        ensure_finite_nonnegative(self.zero_bias_junction_cap_f, "diode CJO")?;
        ensure_finite_positive(self.junction_potential_v, "diode VJ")?;
        ensure_finite_nonnegative(self.grading_coefficient, "diode M")?;
        if self.grading_coefficient >= 1.0 {
            return Err("diode M must be below 1.0".into());
        }
        ensure_finite_nonnegative(self.forward_knee, "diode FC")?;
        if self.forward_knee >= 0.999 {
            return Err("diode FC must be below 0.999".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "diode KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "diode AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "diode EF")?;
        ensure_finite_positive(self.energy_gap_ev, "diode EG")?;
        ensure_finite_nonnegative(self.saturation_current_temperature_exponent, "diode XTI")?;
        ensure_finite_nonnegative(self.transit_time_s, "diode TT")?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MosfetPolarity {
    Nmos,
    Pmos,
}

impl MosfetPolarity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "nmos" | "n" => Some(Self::Nmos),
            "pmos" | "p" => Some(Self::Pmos),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MosfetModelKind {
    Level1,
    Bsim3LongChannel,
}

impl MosfetModelKind {
    pub fn from_level(value: f64) -> Option<Self> {
        match value {
            1.0 => Some(Self::Level1),
            8.0 => Some(Self::Bsim3LongChannel),
            _ => None,
        }
    }

    pub fn as_level(self) -> f64 {
        match self {
            Self::Level1 => 1.0,
            Self::Bsim3LongChannel => 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MosfetModel {
    pub polarity: MosfetPolarity,
    pub kind: MosfetModelKind,
    pub transconductance_a_per_v2: f64,
    pub threshold_voltage_v: f64,
    pub channel_length_modulation_per_v: f64,
    pub body_effect_v_per_sqrt_v: f64,
    pub surface_potential_v: f64,
    pub mobility_degradation_inverse_v: f64,
    pub velocity_saturation_voltage_v: f64,
    pub gate_source_overlap_cap_per_m: f64,
    pub gate_drain_overlap_cap_per_m: f64,
    pub gate_bulk_cap_per_m2: f64,
    pub gate_oxide_cap_per_m2: f64,
    pub source_junction_sidewall_cap_per_m: f64,
    pub drain_junction_sidewall_cap_per_m: f64,
    pub junction_potential_v: f64,
    pub junction_grading_coefficient: f64,
    pub junction_forward_knee: f64,
    pub saturation_charge_partition: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub mobility_temperature_exponent: f64,
    pub self_heating_thermal_resistance_k_per_w: f64,
    pub self_heating_thermal_capacitance_j_per_k: f64,
    pub bsim3_body_bias_threshold_coefficient: f64,
    pub bsim3_low_field_mobility_m2_per_v_s: f64,
    pub bsim3_mobility_degradation_first_order_m_per_v: f64,
    pub bsim3_mobility_degradation_second_order_m2_per_v2: f64,
    pub bsim3_mobility_body_bias_coefficient_m2_per_v2: f64,
    pub bsim3_oxide_thickness_m: f64,
    pub bsim3_saturation_velocity_m_per_s: f64,
    pub bsim3_short_channel_threshold_amplitude: f64,
    pub bsim3_short_channel_length_scale_m: f64,
    pub bsim3_short_channel_drain_bias_coefficient_per_v: f64,
    pub bsim3_narrow_width_threshold_amplitude_v: f64,
    pub bsim3_narrow_width_exponent: f64,
    pub bsim3_reference_width_m: f64,
    pub bsim3_threshold_temperature_coefficient_v: f64,
    pub bsim3_threshold_body_temperature_coefficient_v_per_v: f64,
    pub bsim3_threshold_length_temperature_coefficient_v_m: f64,
    pub geometry_bin: Option<MosfetGeometryBin>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MosfetGeometryBin {
    pub min_width_m: f64,
    pub max_width_m: f64,
    pub min_length_m: f64,
    pub max_length_m: f64,
}

impl MosfetGeometryBin {
    fn validate(&self) -> Result<(), String> {
        for (value, name) in [
            (self.min_width_m, "MOS WMIN"),
            (self.max_width_m, "MOS WMAX"),
            (self.min_length_m, "MOS LMIN"),
            (self.max_length_m, "MOS LMAX"),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{name} must be finite and positive"));
            }
        }
        if self.min_width_m > self.max_width_m {
            return Err("MOS WMIN cannot exceed WMAX".into());
        }
        if self.min_length_m > self.max_length_m {
            return Err("MOS LMIN cannot exceed LMAX".into());
        }
        Ok(())
    }

    pub(super) fn contains(&self, width_m: f64, length_m: f64) -> bool {
        (self.min_width_m..=self.max_width_m).contains(&width_m)
            && (self.min_length_m..=self.max_length_m).contains(&length_m)
    }
}

impl Default for MosfetModel {
    fn default() -> Self {
        Self {
            polarity: MosfetPolarity::Nmos,
            kind: MosfetModelKind::Level1,
            transconductance_a_per_v2: 1e-4,
            threshold_voltage_v: 0.45,
            channel_length_modulation_per_v: 0.0,
            body_effect_v_per_sqrt_v: 0.0,
            surface_potential_v: 0.7,
            mobility_degradation_inverse_v: 0.0,
            velocity_saturation_voltage_v: 0.0,
            gate_source_overlap_cap_per_m: 0.0,
            gate_drain_overlap_cap_per_m: 0.0,
            gate_bulk_cap_per_m2: 0.0,
            gate_oxide_cap_per_m2: 0.0,
            source_junction_sidewall_cap_per_m: 0.0,
            drain_junction_sidewall_cap_per_m: 0.0,
            junction_potential_v: 0.8,
            junction_grading_coefficient: 0.5,
            junction_forward_knee: 0.5,
            saturation_charge_partition: 0.0,
            flicker_coefficient_a2: 0.0,
            flicker_current_exponent: 1.0,
            flicker_frequency_exponent: 1.0,
            mobility_temperature_exponent: 1.5,
            self_heating_thermal_resistance_k_per_w: 0.0,
            self_heating_thermal_capacitance_j_per_k: 0.0,
            bsim3_body_bias_threshold_coefficient: 0.0,
            bsim3_low_field_mobility_m2_per_v_s: 0.045,
            bsim3_mobility_degradation_first_order_m_per_v: 1.0e-15,
            bsim3_mobility_degradation_second_order_m2_per_v2: 1.0e-24,
            bsim3_mobility_body_bias_coefficient_m2_per_v2: 0.0,
            bsim3_oxide_thickness_m: 2.0e-9,
            bsim3_saturation_velocity_m_per_s: 80_000.0,
            bsim3_short_channel_threshold_amplitude: 0.0,
            bsim3_short_channel_length_scale_m: 30.0e-9,
            bsim3_short_channel_drain_bias_coefficient_per_v: 0.0,
            bsim3_narrow_width_threshold_amplitude_v: 0.0,
            bsim3_narrow_width_exponent: 0.3,
            bsim3_reference_width_m: 250.0e-9,
            bsim3_threshold_temperature_coefficient_v: -0.16,
            bsim3_threshold_body_temperature_coefficient_v_per_v: -0.03,
            bsim3_threshold_length_temperature_coefficient_v_m: 0.0,
            geometry_bin: None,
        }
    }
}

impl MosfetModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.transconductance_a_per_v2, "MOS KP")?;
        ensure_finite(self.threshold_voltage_v, "MOS VTO")?;
        ensure_finite_nonnegative(self.channel_length_modulation_per_v, "MOS LAMBDA")?;
        ensure_finite_nonnegative(self.body_effect_v_per_sqrt_v, "MOS GAMMA")?;
        ensure_finite_positive(self.surface_potential_v, "MOS PHI")?;
        ensure_finite_nonnegative(self.mobility_degradation_inverse_v, "MOS THETA")?;
        ensure_finite_nonnegative(self.velocity_saturation_voltage_v, "MOS VSAT")?;
        ensure_finite_nonnegative(self.gate_source_overlap_cap_per_m, "MOS CGSO")?;
        ensure_finite_nonnegative(self.gate_drain_overlap_cap_per_m, "MOS CGDO")?;
        ensure_finite_nonnegative(self.gate_bulk_cap_per_m2, "MOS CGB")?;
        ensure_finite_nonnegative(self.gate_oxide_cap_per_m2, "MOS COX")?;
        ensure_finite_nonnegative(self.source_junction_sidewall_cap_per_m, "MOS CJSW/CJSWS")?;
        ensure_finite_nonnegative(self.drain_junction_sidewall_cap_per_m, "MOS CJSW/CJSWD")?;
        ensure_finite_positive(self.junction_potential_v, "MOS PB")?;
        if !self.junction_grading_coefficient.is_finite()
            || !(0.0..=1.0).contains(&self.junction_grading_coefficient)
        {
            return Err("MOS MJ must be finite and between zero and one".into());
        }
        if !self.junction_forward_knee.is_finite()
            || self.junction_forward_knee < 0.0
            || self.junction_forward_knee >= 1.0
        {
            return Err("MOS FC must be finite and between zero and one".into());
        }
        ensure_finite_nonnegative(
            self.saturation_charge_partition,
            "MOS saturation charge partition",
        )?;
        if self.saturation_charge_partition > 1.0 {
            return Err("MOS saturation charge partition cannot exceed 1.0".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "MOS KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "MOS AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "MOS EF")?;
        ensure_finite_nonnegative(self.mobility_temperature_exponent, "MOS MEXP")?;
        ensure_finite_nonnegative(self.self_heating_thermal_resistance_k_per_w, "MOS RTH")?;
        ensure_finite_nonnegative(self.self_heating_thermal_capacitance_j_per_k, "MOS CTH")?;
        if self.self_heating_thermal_capacitance_j_per_k > 0.0
            && self.self_heating_thermal_resistance_k_per_w == 0.0
        {
            return Err("MOS CTH requires a positive RTH".into());
        }
        ensure_finite_nonnegative(self.bsim3_body_bias_threshold_coefficient, "BSIM3 K2")?;
        ensure_finite_positive(self.bsim3_low_field_mobility_m2_per_v_s, "BSIM3 U0")?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_degradation_first_order_m_per_v,
            "BSIM3 UA",
        )?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_degradation_second_order_m2_per_v2,
            "BSIM3 UB",
        )?;
        ensure_finite_nonnegative(
            self.bsim3_mobility_body_bias_coefficient_m2_per_v2,
            "BSIM3 UC",
        )?;
        ensure_finite_positive(self.bsim3_oxide_thickness_m, "BSIM3 TOX")?;
        if self.kind == MosfetModelKind::Bsim3LongChannel {
            ensure_finite_positive(self.bsim3_saturation_velocity_m_per_s, "BSIM3 VSAT")?;
        } else if !self.bsim3_saturation_velocity_m_per_s.is_finite()
            || self.bsim3_saturation_velocity_m_per_s < 0.0
        {
            return Err("BSIM3 VSAT must be finite and non-negative".into());
        }
        ensure_finite_nonnegative(self.bsim3_short_channel_threshold_amplitude, "BSIM3 DVT0")?;
        ensure_finite_positive(self.bsim3_short_channel_length_scale_m, "BSIM3 DVT1")?;
        ensure_finite_nonnegative(
            self.bsim3_short_channel_drain_bias_coefficient_per_v,
            "BSIM3 DVT2",
        )?;
        ensure_finite_nonnegative(self.bsim3_narrow_width_threshold_amplitude_v, "BSIM3 DVT0W")?;
        ensure_finite_nonnegative(self.bsim3_narrow_width_exponent, "BSIM3 DVT1W")?;
        ensure_finite_positive(self.bsim3_reference_width_m, "BSIM3 W0")?;
        ensure_finite(self.bsim3_threshold_temperature_coefficient_v, "BSIM3 KT1")?;
        ensure_finite(
            self.bsim3_threshold_body_temperature_coefficient_v_per_v,
            "BSIM3 KT2",
        )?;
        ensure_finite(
            self.bsim3_threshold_length_temperature_coefficient_v_m,
            "BSIM3 KT1L",
        )?;
        if let Some(bin) = self.geometry_bin {
            bin.validate()?;
        }
        if self.kind == MosfetModelKind::Bsim3LongChannel
            && self.velocity_saturation_voltage_v != 0.0
        {
            return Err(
                "BSIM3 uses physical VSAT in m/s; do not set legacy voltage-domain VSAT".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BjtPolarity {
    Npn,
    Pnp,
}

impl BjtPolarity {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "npn" | "n" => Some(Self::Npn),
            "pnp" | "p" => Some(Self::Pnp),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BjtModel {
    pub polarity: BjtPolarity,
    pub saturation_current_a: f64,
    pub forward_beta: f64,
    pub reverse_beta: f64,
    pub forward_early_voltage_v: f64,
    pub reverse_early_voltage_v: f64,
    pub base_emitter_zero_bias_cap_f: f64,
    pub base_collector_zero_bias_cap_f: f64,
    pub base_emitter_junction_potential_v: f64,
    pub base_collector_junction_potential_v: f64,
    pub base_emitter_grading_coefficient: f64,
    pub base_collector_grading_coefficient: f64,
    pub junction_forward_knee: f64,
    pub flicker_coefficient_a2: f64,
    pub flicker_current_exponent: f64,
    pub flicker_frequency_exponent: f64,
    pub energy_gap_ev: f64,
    pub saturation_current_temperature_exponent: f64,
    pub self_heating_thermal_resistance_k_per_w: f64,
    pub self_heating_thermal_capacitance_j_per_k: f64,
    pub forward_transit_time_s: f64,
    pub reverse_transit_time_s: f64,
}

impl Default for BjtModel {
    fn default() -> Self {
        Self {
            polarity: BjtPolarity::Npn,
            saturation_current_a: 1e-14,
            forward_beta: 100.0,
            reverse_beta: 1.0,
            forward_early_voltage_v: 0.0,
            reverse_early_voltage_v: 0.0,
            base_emitter_zero_bias_cap_f: 0.0,
            base_collector_zero_bias_cap_f: 0.0,
            base_emitter_junction_potential_v: 0.7,
            base_collector_junction_potential_v: 0.7,
            base_emitter_grading_coefficient: 0.33,
            base_collector_grading_coefficient: 0.33,
            junction_forward_knee: 0.5,
            flicker_coefficient_a2: 0.0,
            flicker_current_exponent: 1.0,
            flicker_frequency_exponent: 1.0,
            energy_gap_ev: 1.12,
            saturation_current_temperature_exponent: 3.0,
            self_heating_thermal_resistance_k_per_w: 0.0,
            self_heating_thermal_capacitance_j_per_k: 0.0,
            forward_transit_time_s: 0.0,
            reverse_transit_time_s: 0.0,
        }
    }
}

impl BjtModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.saturation_current_a, "BJT IS")?;
        ensure_finite_positive(self.forward_beta, "BJT BF")?;
        ensure_finite_positive(self.reverse_beta, "BJT BR")?;
        ensure_finite_nonnegative(self.forward_early_voltage_v, "BJT VAF")?;
        ensure_finite_nonnegative(self.reverse_early_voltage_v, "BJT VAR")?;
        ensure_finite_nonnegative(self.base_emitter_zero_bias_cap_f, "BJT CJE")?;
        ensure_finite_nonnegative(self.base_collector_zero_bias_cap_f, "BJT CJC")?;
        ensure_finite_positive(self.base_emitter_junction_potential_v, "BJT VJE")?;
        ensure_finite_positive(self.base_collector_junction_potential_v, "BJT VJC")?;
        ensure_finite_nonnegative(self.base_emitter_grading_coefficient, "BJT MJE")?;
        ensure_finite_nonnegative(self.base_collector_grading_coefficient, "BJT MJC")?;
        if self.base_emitter_grading_coefficient >= 1.0
            || self.base_collector_grading_coefficient >= 1.0
        {
            return Err("BJT grading coefficients must be below 1.0".into());
        }
        ensure_finite_nonnegative(self.junction_forward_knee, "BJT FC")?;
        if self.junction_forward_knee >= 0.999 {
            return Err("BJT FC must be below 0.999".into());
        }
        ensure_finite_nonnegative(self.flicker_coefficient_a2, "BJT KF")?;
        ensure_finite_nonnegative(self.flicker_current_exponent, "BJT AF")?;
        ensure_finite_nonnegative(self.flicker_frequency_exponent, "BJT EF")?;
        ensure_finite_positive(self.energy_gap_ev, "BJT EG")?;
        ensure_finite_nonnegative(self.saturation_current_temperature_exponent, "BJT XTI")?;
        ensure_finite_nonnegative(self.self_heating_thermal_resistance_k_per_w, "BJT RTH")?;
        ensure_finite_nonnegative(self.self_heating_thermal_capacitance_j_per_k, "BJT CTH")?;
        if self.self_heating_thermal_capacitance_j_per_k > 0.0
            && self.self_heating_thermal_resistance_k_per_w == 0.0
        {
            return Err("BJT CTH requires a positive RTH".into());
        }
        ensure_finite_nonnegative(self.forward_transit_time_s, "BJT TF")?;
        ensure_finite_nonnegative(self.reverse_transit_time_s, "BJT TR")?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpiceCircuit {
    pub title: String,
    pub temperature_k: f64,
    pub elements: Vec<SpiceElement>,
    pub analyses: Vec<Analysis>,
    pub diode_models: BTreeMap<String, DiodeModel>,
    pub mosfet_models: BTreeMap<String, MosfetModel>,
    pub bjt_models: BTreeMap<String, BjtModel>,
    pub locations: BTreeMap<String, ElementLocation>,
    pub statistical_models: BTreeMap<String, StatisticalModel>,
    pub resistor_layer_models: BTreeMap<String, ResistorLayerModel>,
}

impl Default for SpiceCircuit {
    fn default() -> Self {
        Self {
            title: String::new(),
            temperature_k: 300.0,
            elements: Vec::new(),
            analyses: Vec::new(),
            diode_models: BTreeMap::new(),
            mosfet_models: BTreeMap::new(),
            bjt_models: BTreeMap::new(),
            locations: BTreeMap::new(),
            statistical_models: BTreeMap::new(),
            resistor_layer_models: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementLocation {
    pub x_m: f64,
    pub y_m: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResistorLayerModel {
    pub sheet_resistance_ohm_per_square: f64,
    pub first_temperature_coefficient_per_k: f64,
    pub second_temperature_coefficient_per_k2: f64,
    pub width_offset_m: f64,
    pub minimum_width_m: f64,
    pub maximum_width_m: f64,
    pub width_bins: Vec<ResistorWidthBin>,
}

impl ResistorLayerModel {
    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.sheet_resistance_ohm_per_square, "resistor layer SHEET")?;
        ensure_finite(
            self.first_temperature_coefficient_per_k,
            "resistor layer TC1",
        )?;
        ensure_finite(
            self.second_temperature_coefficient_per_k2,
            "resistor layer TC2",
        )?;
        ensure_finite(self.width_offset_m, "resistor layer WOFF")?;
        ensure_finite_nonnegative(self.minimum_width_m, "resistor layer MINW")?;
        ensure_finite_nonnegative(self.maximum_width_m, "resistor layer MAXW")?;
        if self.maximum_width_m > 0.0 && self.maximum_width_m < self.minimum_width_m {
            return Err("resistor layer MAXW cannot be below MINW".into());
        }
        for (index, bin) in self.width_bins.iter().enumerate() {
            ensure_finite_positive(bin.width_m, "resistor layer-bin width")?;
            ensure_finite_positive(
                bin.sheet_resistance_ohm_per_square,
                "resistor layer-bin SHEET",
            )?;
            ensure_finite(bin.width_offset_m, "resistor layer-bin WOFF")?;
            if index > 0 && bin.width_m <= self.width_bins[index - 1].width_m {
                return Err("resistor layer-bin widths must be strictly increasing".into());
            }
        }
        Ok(())
    }

    pub fn effective_sheet(&self, width_m: f64) -> Result<(f64, f64), String> {
        ensure_finite_positive(width_m, "resistor width")?;
        let Some(first) = self.width_bins.first() else {
            return Ok((self.sheet_resistance_ohm_per_square, self.width_offset_m));
        };
        let Some(last) = self.width_bins.last() else {
            return Ok((self.sheet_resistance_ohm_per_square, self.width_offset_m));
        };
        if width_m <= first.width_m {
            return Ok((first.sheet_resistance_ohm_per_square, first.width_offset_m));
        }
        if width_m >= last.width_m {
            return Ok((last.sheet_resistance_ohm_per_square, last.width_offset_m));
        }
        let upper = self.width_bins.partition_point(|bin| bin.width_m < width_m);
        let left = &self.width_bins[upper - 1];
        let right = &self.width_bins[upper];
        let span = right.width_m - left.width_m;
        if span <= 0.0 {
            return Err("resistor layer-bin widths must be strictly increasing".into());
        }
        let alpha = (width_m - left.width_m) / span;
        Ok((
            left.sheet_resistance_ohm_per_square
                + alpha
                    * (right.sheet_resistance_ohm_per_square
                        - left.sheet_resistance_ohm_per_square),
            left.width_offset_m + alpha * (right.width_offset_m - left.width_offset_m),
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResistorWidthBin {
    pub width_m: f64,
    pub sheet_resistance_ohm_per_square: f64,
    pub width_offset_m: f64,
}

impl SpiceCircuit {
    pub fn set_temperature_celsius(&mut self, celsius: f64) -> Result<(), String> {
        let kelvin = celsius + 273.15;
        if !kelvin.is_finite() || !(1.0..=1_000.0).contains(&kelvin) {
            return Err(format!("invalid circuit temperature {} C", celsius));
        }
        self.temperature_k = kelvin;
        Ok(())
    }
}
