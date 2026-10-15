//! SpiceCircuit construction, validation, lookup, and mutation methods.

use super::*;

impl SpiceCircuit {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    pub fn add_element(&mut self, element: SpiceElement) -> Result<(), String> {
        if element.name.is_empty() {
            return Err("SPICE element name cannot be empty".into());
        }
        if self
            .elements
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&element.name))
        {
            return Err(format!("duplicate SPICE element '{}'", element.name));
        }
        self.elements.push(element);
        Ok(())
    }

    pub fn node_names(&self) -> Vec<String> {
        let mut nodes = std::collections::BTreeSet::new();
        for element in &self.elements {
            for node in &element.nodes {
                if !GROUND_ALIASES.contains(&node.as_str()) {
                    nodes.insert(node.clone());
                }
            }
        }
        nodes.into_iter().collect()
    }

    pub fn element(&self, name: &str) -> Option<&SpiceElement> {
        self.elements
            .iter()
            .find(|element| element.name.eq_ignore_ascii_case(name))
    }

    pub fn mosfet_model_for(
        &self,
        name: &str,
        width_m: f64,
        length_m: f64,
    ) -> Result<&MosfetModel, String> {
        if let Some(model) = self.mosfet_models.get(name) {
            if model.geometry_bin.is_none() {
                return Ok(model);
            }
        }
        let prefix = format!("{name}.");
        let mut matches = Vec::new();
        for (candidate_name, model) in &self.mosfet_models {
            let Some(candidate) = model.geometry_bin else {
                continue;
            };
            if candidate_name.starts_with(&prefix) && candidate.contains(width_m, length_m) {
                matches.push(candidate_name.clone());
            }
        }
        match matches.as_slice() {
            [selected] => Ok(&self.mosfet_models[selected]),
            [] => Err(format!(
                "no MOSFET model bin '{name}' covers W={width_m} L={length_m}"
            )),
            many => Err(format!(
                "MOSFET model '{name}' has {} overlapping bins: {}",
                many.len(),
                many.join(", ")
            )),
        }
    }

    fn has_sensitivity_parameter(&self, name: &str) -> bool {
        let Some((object, field)) = name.rsplit_once('.') else {
            return false;
        };
        let field = field.to_ascii_lowercase();
        if let Some(element) = self.element(object) {
            return match (&element.kind, field.as_str()) {
                (SpiceElementKind::Resistor { .. }, "r")
                | (SpiceElementKind::Capacitor { .. }, "c")
                | (SpiceElementKind::Inductor { .. }, "l") => true,
                (
                    SpiceElementKind::VoltageSource { waveform, .. }
                    | SpiceElementKind::CurrentSource { waveform, .. },
                    "dc",
                ) => matches!(waveform, SourceWaveform::Dc(_)),
                (SpiceElementKind::BipolarJunctionTransistor { .. }, "area") => true,
                (
                    SpiceElementKind::VoltageSource {
                        ac_magnitude: Some(_),
                        ..
                    }
                    | SpiceElementKind::CurrentSource {
                        ac_magnitude: Some(_),
                        ..
                    },
                    "ac",
                ) => true,
                _ => false,
            };
        }
        if field == "is"
            && self
                .diode_models
                .keys()
                .any(|model| model.eq_ignore_ascii_case(object))
        {
            return true;
        }
        let Some(model) = object.strip_prefix("model:") else {
            return false;
        };
        match field.as_str() {
            "is" => self.diode_models.contains_key(model) || self.bjt_models.contains_key(model),
            "vto" | "kp" => self.mosfet_models.contains_key(model),
            "vth0" | "k1" | "u0" | "tox" | "vsat" | "dvt0" | "dvt1" | "dvt2" | "dvt0w"
            | "dvt1w" | "w0" | "kt1" | "kt2" | "kt1l" => self
                .mosfet_models
                .get(model)
                .is_some_and(|model| model.kind == MosfetModelKind::Bsim3LongChannel),
            "bf" => self.bjt_models.contains_key(model),
            _ => false,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        ensure_finite_positive(self.temperature_k, "circuit temperature")?;
        if self.temperature_k > 1_000.0 {
            return Err("circuit temperature cannot exceed 1000 K".into());
        }
        if self.elements.is_empty() {
            return Err("SPICE circuit has no elements".into());
        }
        let mut names = std::collections::BTreeSet::new();
        for element in &self.elements {
            if element.name.is_empty() || !names.insert(element.name.to_ascii_lowercase()) {
                return Err(format!("invalid or duplicate element '{}'", element.name));
            }
            let expected = match element.kind {
                SpiceElementKind::Resistor { .. }
                | SpiceElementKind::Capacitor { .. }
                | SpiceElementKind::Inductor { .. }
                | SpiceElementKind::VoltageSource { .. }
                | SpiceElementKind::CurrentSource { .. }
                | SpiceElementKind::Diode { .. } => 2,
                SpiceElementKind::Mosfet { .. } => 4,
                SpiceElementKind::BipolarJunctionTransistor { .. } => 3,
                SpiceElementKind::VoltageControlledVoltageSource { .. }
                | SpiceElementKind::VoltageControlledCurrentSource { .. } => 4,
                SpiceElementKind::CurrentControlledVoltageSource { .. }
                | SpiceElementKind::CurrentControlledCurrentSource { .. } => 2,
            };
            if element.nodes.len() != expected {
                if matches!(
                    element.kind,
                    SpiceElementKind::BipolarJunctionTransistor { .. }
                ) && element.nodes.len() == 4
                {
                    // Four-terminal SPICE BJT includes the substrate node.
                } else {
                    return Err(format!(
                        "'{}' expects {} nodes, found {}",
                        element.name,
                        expected,
                        element.nodes.len()
                    ));
                }
            }
            match element.kind {
                SpiceElementKind::Resistor {
                    resistance,
                    first_temperature_coefficient_per_k,
                    second_temperature_coefficient_per_k2,
                    sheet_resistance_ohm_per_square,
                    length_m,
                    width_m,
                    multiplier,
                    ref layer,
                } => {
                    ensure_finite_positive(resistance, "resistance")?;
                    ensure_finite_positive(multiplier, "physical resistor multiplier")?;
                    ensure_finite(first_temperature_coefficient_per_k, "resistor TC1")?;
                    ensure_finite(second_temperature_coefficient_per_k2, "resistor TC2")?;
                    ensure_finite_nonnegative(sheet_resistance_ohm_per_square, "resistor SHEET")?;
                    ensure_finite_nonnegative(length_m, "physical resistor L")?;
                    ensure_finite_nonnegative(width_m, "physical resistor W")?;
                    if sheet_resistance_ohm_per_square > 0.0 && (length_m <= 0.0 || width_m <= 0.0)
                    {
                        return Err(format!(
                            "physical resistor '{}' needs positive L and W",
                            element.name
                        ));
                    }
                    if let Some(layer_name) = layer {
                        let Some(model) = self
                            .resistor_layer_models
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
                            .map(|(_, model)| model)
                        else {
                            return Err(format!(
                                "resistor '{}' uses unknown layer model '{}'",
                                element.name, layer_name
                            ));
                        };
                        if length_m <= 0.0 || width_m <= 0.0 {
                            return Err(format!(
                                "layer-model resistor '{}' needs positive L and W",
                                element.name
                            ));
                        }
                        if width_m < model.minimum_width_m
                            || (model.maximum_width_m > 0.0 && width_m > model.maximum_width_m)
                        {
                            return Err(format!(
                                "resistor '{}' width is outside layer '{}' limits",
                                element.name, layer_name
                            ));
                        }
                        let (_, bin_width_offset) = model
                            .effective_sheet(width_m)
                            .map_err(|e| format!("resistor '{}': {}", element.name, e))?;
                        if width_m + bin_width_offset <= 0.0 {
                            return Err(format!(
                                "resistor '{}' has non-positive effective width",
                                element.name
                            ));
                        }
                    }
                    let delta = self.temperature_k - SPICE_NOMINAL_TEMPERATURE_K;
                    if let Some(layer_name) = layer {
                        let model = self
                            .resistor_layer_models
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(layer_name))
                            .map(|(_, model)| model)
                            .ok_or_else(|| {
                                format!("unknown resistor layer model '{layer_name}'")
                            })?;
                        let sheet_factor = 1.0
                            + model.first_temperature_coefficient_per_k * delta
                            + model.second_temperature_coefficient_per_k2 * delta * delta;
                        if !sheet_factor.is_finite() || sheet_factor <= 0.0 {
                            return Err(format!(
                                "layer model '{}' has non-positive sheet resistance at {} K",
                                layer_name, self.temperature_k
                            ));
                        }
                    }
                    let factor = 1.0
                        + first_temperature_coefficient_per_k * delta
                        + second_temperature_coefficient_per_k2 * delta * delta;
                    if !factor.is_finite() || factor <= 0.0 {
                        return Err(format!(
                            "resistor '{}' has non-positive resistance at {} K",
                            element.name, self.temperature_k
                        ));
                    }
                }
                SpiceElementKind::Capacitor {
                    capacitance,
                    initial_voltage,
                } => {
                    ensure_finite_positive(capacitance, "capacitance")?;
                    ensure_finite(initial_voltage, "capacitor initial voltage")?;
                }
                SpiceElementKind::Inductor {
                    inductance,
                    initial_current,
                } => {
                    ensure_finite_positive(inductance, "inductance")?;
                    ensure_finite(initial_current, "inductor initial current")?;
                }
                SpiceElementKind::VoltageSource {
                    ac_magnitude,
                    ac_phase_deg,
                    ..
                } => {
                    if let Some(magnitude) = ac_magnitude {
                        ensure_finite_nonnegative(magnitude, "AC magnitude")?;
                    }
                    ensure_finite(ac_phase_deg, "AC phase")?;
                }
                SpiceElementKind::CurrentSource {
                    ac_magnitude,
                    ac_phase_deg,
                    ..
                } => {
                    if let Some(magnitude) = ac_magnitude {
                        ensure_finite_nonnegative(magnitude, "AC magnitude")?;
                    }
                    ensure_finite(ac_phase_deg, "AC phase")?;
                }
                SpiceElementKind::VoltageControlledVoltageSource { gain }
                | SpiceElementKind::VoltageControlledCurrentSource {
                    transconductance: gain,
                } => {
                    ensure_finite(gain, "controlled-source gain")?;
                }
                SpiceElementKind::CurrentControlledVoltageSource {
                    ref controlling_source,
                    transresistance,
                } => {
                    ensure_finite(transresistance, "CCVS transresistance")?;
                    ensure_control_source(self, controlling_source.as_str(), &element.name)?;
                }
                SpiceElementKind::CurrentControlledCurrentSource {
                    ref controlling_source,
                    gain,
                } => {
                    ensure_finite(gain, "CCCS gain")?;
                    ensure_control_source(self, controlling_source.as_str(), &element.name)?;
                }
                SpiceElementKind::Diode { ref model } => {
                    if !self.diode_models.contains_key(model.as_str()) {
                        return Err(format!("unknown diode model '{}'", model));
                    }
                }
                SpiceElementKind::Mosfet {
                    ref model,
                    width_m,
                    length_m,
                    multiplier,
                } => {
                    self.mosfet_model_for(model, width_m, length_m)?;
                    ensure_finite_positive(width_m, "MOS W")?;
                    ensure_finite_positive(length_m, "MOS L")?;
                    ensure_finite_positive(multiplier, "MOS M")?;
                }
                SpiceElementKind::BipolarJunctionTransistor { ref model, area } => {
                    if !self.bjt_models.contains_key(model.as_str()) {
                        return Err(format!("unknown BJT model '{}'", model));
                    }
                    ensure_finite_positive(area, "BJT AREA")?;
                }
            }
        }
        for (name, model) in &self.diode_models {
            if name.trim().is_empty() {
                return Err("diode model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.mosfet_models {
            if name.trim().is_empty() {
                return Err("MOSFET model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.bjt_models {
            if name.trim().is_empty() {
                return Err("BJT model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, model) in &self.resistor_layer_models {
            if name.trim().is_empty() {
                return Err("resistor layer model name cannot be empty".into());
            }
            model.validate()?;
        }
        for (name, location) in &self.locations {
            if !self
                .elements
                .iter()
                .any(|element| element.name.eq_ignore_ascii_case(name))
            {
                return Err(format!("location target '{name}' does not exist"));
            }
            ensure_finite(location.x_m, "element location X")?;
            ensure_finite(location.y_m, "element location Y")?;
        }
        for (name, model) in &self.statistical_models {
            if name.trim().is_empty() {
                return Err("statistical model name cannot be empty".into());
            }
            model.validate()?;
            for parameter in &model.parameters {
                if !self.has_sensitivity_parameter(&parameter.parameter) {
                    return Err(format!(
                        "statistical model parameter '{}' does not exist",
                        parameter.parameter
                    ));
                }
            }
            for correlation in &model.correlations {
                if !self.has_sensitivity_parameter(&correlation.left)
                    || !self.has_sensitivity_parameter(&correlation.right)
                {
                    return Err(format!(
                        "statistical model correlation '{}' does not exist",
                        correlation.left
                    ));
                }
            }
        }
        for analysis in &self.analyses {
            if let Analysis::NoiseSweep {
                output_node,
                input_source,
                ..
            } = analysis
            {
                if !self.node_names().iter().any(|node| node == output_node) {
                    return Err(format!(
                        "noise output node '{}' does not exist",
                        output_node
                    ));
                }
                match self.element(input_source).map(|element| &element.kind) {
                    Some(
                        SpiceElementKind::VoltageSource { .. }
                        | SpiceElementKind::CurrentSource { .. },
                    ) => {}
                    _ => {
                        return Err(format!(
                            "noise input source '{}' does not exist",
                            input_source
                        ))
                    }
                }
            }
            if let Analysis::MonteCarlo(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "Monte Carlo output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "Monte Carlo target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for correlation in &options.correlations {
                    if !self.has_sensitivity_parameter(&correlation.left) {
                        return Err(format!(
                            "unknown Monte Carlo correlation parameter '{}'",
                            correlation.left
                        ));
                    }
                    if !self.has_sensitivity_parameter(&correlation.right) {
                        return Err(format!(
                            "unknown Monte Carlo correlation parameter '{}'",
                            correlation.right
                        ));
                    }
                }
                if let Some(model_name) = &options.statistical_model {
                    if !self.statistical_models.contains_key(model_name) {
                        return Err(format!(
                            "Monte Carlo statistical model '{model_name}' does not exist"
                        ));
                    }
                }
            }
            if let Analysis::Sensitivity(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "sensitivity output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "sensitivity target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
            if let Analysis::GlobalSensitivity(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "global sensitivity output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "global sensitivity target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
            let has_diode = self
                .elements
                .iter()
                .any(|element| matches!(element.kind, SpiceElementKind::Diode { .. }));
            if let Analysis::Derivative(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "derivative output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if options.target == DerivativeTarget::AcMagnitude {
                    let linear_ac = !has_diode
                        && self.elements.iter().all(|element| {
                            matches!(
                                element.kind,
                                SpiceElementKind::Resistor { .. }
                                    | SpiceElementKind::Capacitor { .. }
                                    | SpiceElementKind::Inductor { .. }
                                    | SpiceElementKind::VoltageSource { .. }
                                    | SpiceElementKind::CurrentSource { .. }
                            )
                        });
                    if has_diode {
                        let resistor_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(element.kind, SpiceElementKind::Resistor { .. })
                            })
                            .count();
                        let diode_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(element.kind, SpiceElementKind::Diode { .. })
                            })
                            .count();
                        let source_count = self
                            .elements
                            .iter()
                            .filter(|element| {
                                matches!(
                                    &element.kind,
                                    SpiceElementKind::VoltageSource {
                                        waveform: SourceWaveform::Dc(_),
                                        ac_magnitude: Some(_),
                                        ..
                                    }
                                )
                            })
                            .count();
                        if self.elements.len() != 3
                            || resistor_count != 1
                            || diode_count != 1
                            || source_count != 1
                            || !self
                                .analyses
                                .iter()
                                .any(|candidate| matches!(candidate, Analysis::AcSweep { .. }))
                        {
                            return Err(
                                "nonlinear AC derivatives support one series R, one diode, and one DC/AC V source"
                                    .into(),
                            );
                        }
                    } else if !linear_ac {
                        return Err(
                            "AC analytic derivatives support linear RCL and source circuits only"
                                .into(),
                        );
                    }
                } else if options.target == DerivativeTarget::TransientFinal {
                    if !self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                    {
                        return Err(
                            "transient derivative target lacks its base .tran analysis".into()
                        );
                    }
                    let linear = self.elements.iter().all(|element| {
                        matches!(
                            element.kind,
                            SpiceElementKind::Resistor { .. }
                                | SpiceElementKind::Capacitor { .. }
                                | SpiceElementKind::Inductor { .. }
                                | SpiceElementKind::VoltageSource { .. }
                                | SpiceElementKind::CurrentSource { .. }
                        )
                    });
                    if !linear {
                        return Err(
                            "transient analytic derivatives support linear RCL and source circuits only"
                                .into(),
                        );
                    }
                }
                for parameter in &options.parameters {
                    let supported_dc = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            field.eq_ignore_ascii_case("IS")
                                && self
                                    .diode_models
                                    .keys()
                                    .any(|model| model.eq_ignore_ascii_case(object))
                                || (field.eq_ignore_ascii_case("VTO")
                                    || field.eq_ignore_ascii_case("KP"))
                                    && self
                                        .mosfet_models
                                        .keys()
                                        .any(|model| model.eq_ignore_ascii_case(object))
                                || (field.eq_ignore_ascii_case("IS")
                                    || field.eq_ignore_ascii_case("BF"))
                                    && self
                                        .bjt_models
                                        .keys()
                                        .any(|model| model.eq_ignore_ascii_case(object))
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("R")
                                        && matches!(element.kind, SpiceElementKind::Resistor { .. })
                                })
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("AREA")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::BipolarJunctionTransistor { .. }
                                        )
                                })
                                || self.element(object).is_some_and(|element| {
                                    field.eq_ignore_ascii_case("DC")
                                        && match &element.kind {
                                            SpiceElementKind::VoltageSource {
                                                waveform, ..
                                            }
                                            | SpiceElementKind::CurrentSource {
                                                waveform, ..
                                            } => {
                                                matches!(waveform, SourceWaveform::Dc(_))
                                            }
                                            _ => false,
                                        }
                                })
                        })
                        .unwrap_or(false);
                    let supported_ac = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("C")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Capacitor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("L")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Inductor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("AC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            } | SpiceElementKind::CurrentSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            }
                                        ))
                            })
                        })
                        .unwrap_or(false);
                    let supported_nonlinear_ac =
                        parameter.rsplit_once('.').is_some_and(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("DC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            }
                                        ))
                                    || (field.eq_ignore_ascii_case("AC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                ac_magnitude: Some(_),
                                                ..
                                            }
                                        ))
                            }) || self
                                .diode_models
                                .keys()
                                .any(|model| model.eq_ignore_ascii_case(object))
                                && (field.eq_ignore_ascii_case("IS")
                                    || field.eq_ignore_ascii_case("TT")
                                    || field.eq_ignore_ascii_case("CJO")
                                    || field.eq_ignore_ascii_case("VJ")
                                    || field.eq_ignore_ascii_case("M")
                                    || field.eq_ignore_ascii_case("FC"))
                        });
                    let supported_transient = parameter
                        .rsplit_once('.')
                        .map(|(object, field)| {
                            self.element(object).is_some_and(|element| {
                                (field.eq_ignore_ascii_case("R")
                                    && matches!(element.kind, SpiceElementKind::Resistor { .. }))
                                    || (field.eq_ignore_ascii_case("C")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Capacitor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("L")
                                        && matches!(
                                            element.kind,
                                            SpiceElementKind::Inductor { .. }
                                        ))
                                    || (field.eq_ignore_ascii_case("DC")
                                        && matches!(
                                            &element.kind,
                                            SpiceElementKind::VoltageSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            } | SpiceElementKind::CurrentSource {
                                                waveform: SourceWaveform::Dc(_),
                                                ..
                                            }
                                        ))
                            })
                        })
                        .unwrap_or(false);
                    let supported = match options.target {
                        DerivativeTarget::Dc => supported_dc,
                        DerivativeTarget::AcMagnitude => {
                            if has_diode {
                                supported_nonlinear_ac
                            } else {
                                supported_ac
                            }
                        }
                        DerivativeTarget::TransientFinal => supported_transient,
                    };
                    if !supported {
                        return Err(match options.target {
                            DerivativeTarget::Dc => format!(
                                "derivative parameter '{parameter}' must be a resistor R, DC source value, diode model IS, MOS model VTO/KP, BJT model IS/BF, or BJT instance AREA"
                            ),
                            DerivativeTarget::AcMagnitude if has_diode => format!(
                                "nonlinear AC derivative parameter '{parameter}' must be R, diode IS/TT/CJO/VJ/M/FC, DC source, or AC source magnitude"
                            ),
                            DerivativeTarget::AcMagnitude => format!(
                                "AC derivative parameter '{parameter}' must be R, C, L, or an AC source magnitude"
                            ),
                            DerivativeTarget::TransientFinal => format!(
                                "transient derivative parameter '{parameter}' must be the series R, C, or source DC value"
                            ),
                        });
                    }
                }
            }
            if let Analysis::Optimization(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "optimization output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "optimization target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for parameter in &options.parameters {
                    if !self.has_sensitivity_parameter(&parameter.parameter) {
                        return Err(format!(
                            "unknown optimization parameter '{}'",
                            parameter.parameter
                        ));
                    }
                }
            }
            if let Analysis::Pareto(options) = analysis {
                for output_node in &options.output_nodes {
                    if !self.node_names().iter().any(|node| node == output_node) {
                        return Err(format!(
                            "Pareto optimization output node '{}' does not exist",
                            output_node
                        ));
                    }
                }
                for constraint in &options.constraints {
                    for variable in constraint.variable_names() {
                        if !self.node_names().iter().any(|node| node == &variable) {
                            return Err(format!(
                                "Pareto optimization constraint node '{}' does not exist",
                                variable
                            ));
                        }
                    }
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "Pareto optimization target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
                for parameter in &options.parameters {
                    if !self.has_sensitivity_parameter(&parameter.parameter) {
                        return Err(format!(
                            "unknown Pareto optimization parameter '{}'",
                            parameter.parameter
                        ));
                    }
                }
            }
            if let Analysis::PeriodicSteadyState(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "PSS output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if !self
                    .analyses
                    .iter()
                    .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                {
                    return Err("PSS requires a .tran analysis".into());
                }
            }
            if let Analysis::EnvelopeFollow(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "envelope output node '{}' does not exist",
                        options.output_node
                    ));
                }
                if !self
                    .analyses
                    .iter()
                    .any(|candidate| matches!(candidate, Analysis::Transient { .. }))
                {
                    return Err("envelope following requires a .tran analysis".into());
                }
            }
            if let Analysis::Aging(options) = analysis {
                if !self
                    .node_names()
                    .iter()
                    .any(|node| node == &options.output_node)
                {
                    return Err(format!(
                        "aging output node '{}' does not exist",
                        options.output_node
                    ));
                }
                let target_ok = match options.target {
                    MonteCarloTarget::OperatingPoint => true,
                    MonteCarloTarget::TransientFinal => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::Transient { .. })),
                    MonteCarloTarget::AcMagnitude => self
                        .analyses
                        .iter()
                        .any(|candidate| matches!(candidate, Analysis::AcSweep { .. })),
                };
                if !target_ok {
                    return Err(format!(
                        "aging target '{}' lacks its base analysis",
                        options.target.as_str()
                    ));
                }
            }
        }
        for analysis in &self.analyses {
            validate_analysis(analysis)?;
        }
        Ok(())
    }
}

pub(super) fn ensure_finite(value: f64, what: &str) -> Result<(), String> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(format!("{} must be finite", what))
    }
}

pub(super) fn ensure_finite_nonnegative(value: f64, what: &str) -> Result<(), String> {
    ensure_finite(value, what)?;
    if value < 0.0 {
        Err(format!("{} must be nonnegative", what))
    } else {
        Ok(())
    }
}

pub(super) fn ensure_finite_positive(value: f64, what: &str) -> Result<(), String> {
    ensure_finite(value, what)?;
    if value <= 0.0 {
        Err(format!("{} must be positive", what))
    } else {
        Ok(())
    }
}

pub(super) fn ensure_control_source(
    circuit: &SpiceCircuit,
    name: &str,
    user: &str,
) -> Result<(), String> {
    match circuit.element(name) {
        Some(element) if matches!(element.kind, SpiceElementKind::VoltageSource { .. }) => Ok(()),
        _ => Err(format!(
            "'{}' must control from a voltage source (found no '{}')",
            user, name
        )),
    }
}

pub(super) fn validate_analysis(analysis: &Analysis) -> Result<(), String> {
    match analysis {
        Analysis::OperatingPoint => Ok(()),
        Analysis::DcSweep {
            start, stop, step, ..
        } => {
            ensure_finite(*start, "DC sweep start")?;
            ensure_finite(*stop, "DC sweep stop")?;
            ensure_finite(*step, "DC sweep step")?;
            if *step == 0.0 || (*stop < *start && *step > 0.0) || (*stop > *start && *step < 0.0) {
                return Err("DC sweep direction and step must agree".into());
            }
            Ok(())
        }
        Analysis::Transient {
            step_s,
            stop_s,
            method,
        } => {
            ensure_finite_positive(*step_s, "transient step")?;
            ensure_finite_positive(*stop_s, "transient stop time")?;
            if step_s > stop_s {
                return Err("transient step cannot exceed stop time".into());
            }
            if IntegrationMethod::parse(method.as_str()).is_none() {
                return Err("unknown transient integration method".into());
            }
            Ok(())
        }
        Analysis::AcSweep {
            kind,
            points,
            start_hz,
            stop_hz,
        } => {
            if *points == 0 {
                return Err("AC sweep needs at least one point".into());
            }
            ensure_finite_positive(*start_hz, "AC start frequency")?;
            ensure_finite_positive(*stop_hz, "AC stop frequency")?;
            if start_hz > stop_hz {
                return Err("AC start frequency cannot exceed stop frequency".into());
            }
            let _ = kind;
            Ok(())
        }
        Analysis::NoiseSweep {
            output_node,
            input_source,
            kind,
            points,
            start_hz,
            stop_hz,
        } => {
            if output_node.trim().is_empty() {
                return Err("noise output node cannot be empty".into());
            }
            if input_source.trim().is_empty() {
                return Err("noise input source cannot be empty".into());
            }
            if *points == 0 {
                return Err("noise sweep needs at least one point".into());
            }
            ensure_finite_positive(*start_hz, "noise start frequency")?;
            ensure_finite_positive(*stop_hz, "noise stop frequency")?;
            if start_hz > stop_hz {
                return Err("noise start frequency cannot exceed stop frequency".into());
            }
            let _ = kind;
            Ok(())
        }
        Analysis::MonteCarlo(options) => options.validate(),
        Analysis::Sensitivity(options) => options.validate(),
        Analysis::GlobalSensitivity(options) => options.validate(),
        Analysis::Optimization(options) => options.validate(),
        Analysis::Derivative(options) => options.validate(),
        Analysis::Pareto(options) => options.validate(),
        Analysis::PeriodicSteadyState(options) => options.validate(),
        Analysis::EnvelopeFollow(options) => options.validate(),
        Analysis::Aging(options) => options.validate(),
    }
}
