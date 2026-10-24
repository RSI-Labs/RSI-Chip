//! Compare authoritative slang summaries against the repository IR.

use super::expressions::{
    contains_expected_shapes, design_expr_fingerprint, design_lvalue_name, design_stmt_assignments,
    slang_expression_fingerprint_with_parameters, summary_literal_i64, ExpressionFingerprintMode,
};
use super::*;
use crate::types::{memory_type, simple_type_shape};

fn stable_class_metadata_key(symbol: &str) -> String {
    symbol
        .rsplit_once(' ')
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| symbol.to_string())
}

impl SlangAstSummary {
    pub fn validate_against_design(&self, design: &Design) -> Result<(), String> {
        self.validate(design, true)
    }

    pub fn validate_against_design_legacy_compatibility(
        &self,
        design: &Design,
    ) -> Result<(), String> {
        self.validate(design, false)
    }

    fn validate(&self, design: &Design, strict_generated_hierarchy: bool) -> Result<(), String> {
        let fingerprint_mode = if strict_generated_hierarchy {
            ExpressionFingerprintMode::Direct
        } else {
            ExpressionFingerprintMode::BoundedCompatibility
        };
        if design.name != self.top_name {
            return Err(format!(
                "bounded elaborator top '{}' does not match slang top '{}'",
                design.name, self.top_name
            ) + &self.span_note("top"));
        }

        let actual_ports: Vec<(String, &str)> = design
            .ports
            .iter()
            .map(|port| {
                if design.memory_ports.iter().any(|memory_port| {
                    memory_port.name == port.name
                        && memory_port.direction == eda_core::ir::MemoryPortDirection::InOut
                }) || design.array_ports.iter().any(|array_port| {
                    array_port.name == port.name
                        && array_port.direction == eda_core::ir::MemoryPortDirection::InOut
                }) {
                    return (port.name.clone(), "InOut");
                }
                let direction = match port.dir {
                    eda_core::ir::Dir::In => "In",
                    eda_core::ir::Dir::Out => "Out",
                };
                (port.name.clone(), direction)
            })
            .collect();
        let expected_ports: Vec<(String, &str)> = self
            .ports
            .iter()
            .map(|port| (port.name.clone(), port.direction.as_str()))
            .collect();
        if actual_ports != expected_ports {
            return Err(format!(
                "bounded elaborator ports for '{}' do not match slang: internal {:?}, slang {:?}",
                self.top_name, actual_ports, expected_ports
            ) + &self.span_note("top"));
        }

        let mut expected_declarations: std::collections::BTreeMap<String, Option<usize>> =
            std::collections::BTreeMap::new();
        for declaration in &self.declarations {
            match expected_declarations.get(&declaration.name) {
                Some(existing_width) if *existing_width != declaration.width => {
                    return Err(format!(
                        "slang declares '{}' with inconsistent widths {:?} and {:?}",
                        declaration.name, existing_width, declaration.width
                    ));
                }
                _ => {
                    expected_declarations.insert(declaration.name.clone(), declaration.width);
                }
            }
        }
        let visible_design_declarations: std::collections::BTreeMap<String, usize> = design
            .net_decls
            .iter()
            .filter(|declaration| {
                !declaration.name.starts_with("__") && !declaration.name.contains('.')
            })
            .map(|declaration| (declaration.name.clone(), declaration.width))
            .collect();
        for (name, slang_width) in expected_declarations {
            // Class handles are symbol references such as `"42 Counter"`.
            // Direct lowering expands the handle into `object.property` fields,
            // so the placeholder handle has no scalar IR declaration.
            if self.declarations.iter().any(|declaration| {
                declaration.name == *name
                    && declaration
                        .type_name
                        .split_whitespace()
                        .next()
                        .is_some_and(|address| address.parse::<u64>().is_ok())
            }) {
                continue;
            }
            match visible_design_declarations.get(&name) {
                None => {
                    return Err(
                        format!("bounded elaborator dropped slang declaration '{name}'")
                            + &self.span_note(&format!("declaration:{name}")),
                    );
                }
                Some(actual_width) => {
                    if let Some(slang_width) = slang_width {
                        if *actual_width != slang_width {
                            return Err(format!(
                                "bounded elaborator declaration '{name}' width {actual_width} does not match slang width {slang_width}"
                            ) + &self.span_note(&format!("declaration:{name}")));
                        }
                    }
                }
            }
        }

        let mut parameter_names = std::collections::BTreeSet::new();
        for parameter in &self.parameters {
            if !parameter_names.insert(parameter.name.clone()) {
                return Err(format!(
                    "slang declares parameter '{}' more than once",
                    parameter.name
                ));
            }
        }

        let memory_names: std::collections::BTreeSet<String> = design
            .memories
            .iter()
            .map(|memory| memory.name.clone())
            .collect();
        let mut expected_continuous_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for target in &self.continuous_targets {
            expected_continuous_targets
                .entry(target.name.clone())
                .or_default()
                .push(
                    match slang_expression_fingerprint_with_parameters(
                        &target.rhs,
                        &self.parameters,
                        fingerprint_mode,
                    ) {
                        Some(fingerprint) => {
                            let fingerprint =
                                canonicalize_memory_fingerprint(&fingerprint, &memory_names);
                            if strict_generated_hierarchy
                                || (!fingerprint.starts_with("MemoryRead")
                                    && !fingerprint.contains("PackedSelect:"))
                                || memory_fingerprint_target_exists(&fingerprint, &memory_names)
                            {
                                fingerprint
                            } else {
                                // The bounded elaborator can legally lower a
                                // source-level memory read into flattened nets
                                // without retaining the source memory symbol.
                                // Preserve assignment-count checking until that
                                // representation carries an exact fingerprint.
                                "?".to_string()
                            }
                        }
                        None => "?".to_string(),
                    },
                );
        }
        let mut actual_continuous_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for assignment in &design.assigns {
            let Some(name) = design_lvalue_name(design, &assignment.lv) else {
                continue;
            };
            if name.starts_with("__") || name.contains('.') {
                continue;
            }
            actual_continuous_targets
                .entry(name)
                .or_default()
                .push(design_expr_fingerprint(design, &assignment.e));
        }
        for (name, expected_shapes) in expected_continuous_targets {
            let represented_as_sparse_memory = memory_names.contains(&name);
            if represented_as_sparse_memory {
                continue;
            }
            let actual_shapes = actual_continuous_targets
                .get(&name)
                .cloned()
                .unwrap_or_default();
            if !contains_expected_shapes(&actual_shapes, &expected_shapes) {
                return Err(format!(
                    "bounded elaborator continuous assignment shape for '{name}' does not match slang: internal {actual_shapes:?}, slang {expected_shapes:?}"
                ) + &self.span_note(&format!("continuous:{name}:")));
            }
        }

        let expected_initial_count = self
            .procedural_blocks
            .iter()
            .filter(|block| block.procedure_kind == "Initial")
            .count();
        if design.initials.len() < expected_initial_count {
            return Err(format!(
                "bounded elaborator dropped initial blocks: slang expects {expected_initial_count}, internal has {}",
                design.initials.len()
            ) + &self.span_note("procedural:"));
        }
        let expected_process_count = self
            .procedural_blocks
            .iter()
            .filter(|block| !matches!(block.procedure_kind.as_str(), "Initial" | "Final"))
            .count();
        if design.processes.len() < expected_process_count {
            return Err(format!(
                "bounded elaborator dropped procedural blocks: slang expects {expected_process_count}, internal has {}",
                design.processes.len()
            ) + &self.span_note("procedural:"));
        }
        let expected_final_count = self
            .procedural_blocks
            .iter()
            .filter(|block| block.procedure_kind == "Final")
            .count();
        if design.final_blocks.len() < expected_final_count {
            return Err(format!(
                "bounded elaborator dropped final blocks: slang expects {expected_final_count}, internal has {}",
                design.final_blocks.len()
            ) + &self.span_note("procedural:"));
        }

        let declaration_names: std::collections::BTreeSet<String> = self
            .declarations
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect();
        let mut expected_procedural_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let legacy_array_targets: std::collections::BTreeSet<String> = self
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.type_name.contains('$')
                    && !design
                        .memories
                        .iter()
                        .any(|memory| memory.name == declaration.name)
            })
            .map(|declaration| declaration.name.clone())
            .collect();
        for block in &self.procedural_blocks {
            for assignment in &block.assignments {
                for target in &assignment.target_names {
                    if declaration_names.contains(target) {
                        if !assignment.class_handle_dynamic_targets.is_empty() {
                            let source_shapes =
                                if assignment.class_handle_dynamic_sources.is_empty() {
                                    assignment
                                        .class_handle_source
                                        .iter()
                                        .map(|source| format!("ClassHandleCopy:{source}"))
                                        .collect::<Vec<_>>()
                                } else {
                                    assignment
                                        .class_handle_dynamic_sources
                                        .iter()
                                        .map(|source| format!("ClassHandleCopy:{source}"))
                                        .collect::<Vec<_>>()
                                };
                            if !source_shapes.is_empty() {
                                for target in &assignment.class_handle_dynamic_targets {
                                    expected_procedural_targets
                                        .entry(target.clone())
                                        .or_default()
                                        .extend(source_shapes.iter().cloned());
                                }
                                continue;
                            }
                        }
                        if let (Some(target_key), Some(source_key)) = (
                            assignment.class_handle_target_key.as_ref(),
                            assignment.class_handle_source_key.as_ref(),
                        ) {
                            let target_key = stable_class_metadata_key(target_key);
                            let source_key = stable_class_metadata_key(source_key);
                            expected_procedural_targets
                                .entry(target_key)
                                .or_default()
                                .push(format!("ClassHandleCopy:{source_key}"));
                            continue;
                        }
                        if !assignment.class_handle_dynamic_sources.is_empty() {
                            let shapes = assignment
                                .class_handle_dynamic_sources
                                .iter()
                                .map(|source| format!("ClassHandleCopy:{source}"))
                                .collect::<Vec<_>>();
                            expected_procedural_targets
                                .entry(target.clone())
                                .or_default()
                                .extend(shapes);
                            continue;
                        }
                        let legacy_array_shape =
                            !strict_generated_hierarchy && legacy_array_targets.contains(target);
                        if legacy_array_shape && !assignment.dynamic_array_lvalue_indexes.is_empty()
                        {
                            let shapes = dynamic_array_element_lvalue_fingerprints(
                                target,
                                &assignment.dynamic_array_lvalue_indexes,
                                &assignment.rhs,
                                &self.declarations,
                                &self.parameters,
                                fingerprint_mode,
                            )
                            .ok_or_else(|| {
                                format!(
                                    "lowering could not fingerprint dynamic array lvalue '{target}'"
                                )
                            })?;
                            expected_procedural_targets
                                .entry(target.clone())
                                .or_default()
                                .extend(shapes);
                            continue;
                        }
                        if legacy_array_shape && assignment.array_part_lvalue.is_some() {
                            let shapes = assignment
                                .array_part_lvalue
                                .as_ref()
                                .and_then(|part| {
                                    array_part_lvalue_fingerprints(
                                        target,
                                        part,
                                        &assignment.rhs,
                                        &self.declarations,
                                        &self.parameters,
                                        fingerprint_mode,
                                    )
                                })
                                .ok_or_else(|| {
                                    format!(
                                        "lowering could not fingerprint array part lvalue '{target}'"
                                    )
                                })?;
                            expected_procedural_targets
                                .entry(target.clone())
                                .or_default()
                                .extend(shapes);
                            continue;
                        }
                        let shape = if let Some(source) = assignment.class_handle_source.as_ref() {
                            format!("ClassHandleCopy:{source}")
                        } else {
                            if legacy_array_shape && !assignment.constant_array_element_lvalue {
                                // Small arrays still use the legacy bounded
                                // per-element expansion, whose expression tree is
                                // intentionally not structurally equivalent to the
                                // slang source.
                                "?".to_string()
                            } else if let Some(member) = assignment.memory_member_lvalue.as_ref() {
                                memory_member_lvalue_fingerprint(
                                    target,
                                    member,
                                    &assignment.rhs,
                                    &self.declarations,
                                    &self.type_layouts,
                                    &self.parameters,
                                    fingerprint_mode,
                                )
                                .unwrap_or_else(|| "?".to_string())
                            } else if let Some(selector) =
                                assignment.dynamic_packed_lvalue_selector.as_ref()
                            {
                                dynamic_packed_lvalue_fingerprint(
                                    target,
                                    selector,
                                    &assignment.rhs,
                                    &self.declarations,
                                    &self.parameters,
                                    fingerprint_mode,
                                )
                                .ok_or_else(|| {
                                    format!(
                                        "lowering could not fingerprint dynamic packed lvalue '{target}'"
                                    )
                                })?
                            } else {
                                match slang_expression_fingerprint_with_parameters(
                                    &assignment.rhs,
                                    &self.parameters,
                                    fingerprint_mode,
                                ) {
                                    Some(fingerprint) => {
                                        let fingerprint = canonicalize_memory_fingerprint(
                                            &fingerprint,
                                            &memory_names,
                                        );
                                        if strict_generated_hierarchy
                                            || (!fingerprint.starts_with("MemoryRead")
                                                && !fingerprint.contains("PackedSelect:"))
                                            || memory_fingerprint_target_exists(
                                                &fingerprint,
                                                &memory_names,
                                            )
                                        {
                                            fingerprint
                                        } else {
                                            "?".to_string()
                                        }
                                    }
                                    None => "?".to_string(),
                                }
                            }
                        };
                        expected_procedural_targets
                            .entry(target.clone())
                            .or_default()
                            .push(shape);
                    }
                }
            }
        }
        let mut actual_procedural_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for initial in &design.initials {
            for (name, shape) in design_stmt_assignments(design, initial) {
                if !name.starts_with("__") && !name.contains('.') {
                    actual_procedural_targets
                        .entry(name)
                        .or_default()
                        .push(shape);
                }
            }
        }
        for process in &design.processes {
            for (name, shape) in design_stmt_assignments(design, &process.body) {
                if !name.starts_with("__") && !name.contains('.') {
                    actual_procedural_targets
                        .entry(name)
                        .or_default()
                        .push(shape);
                }
            }
        }
        for final_block in &design.final_blocks {
            for (name, shape) in design_stmt_assignments(design, final_block) {
                if !name.starts_with("__") && !name.contains('.') {
                    actual_procedural_targets
                        .entry(name)
                        .or_default()
                        .push(shape);
                }
            }
        }
        for (name, expected_shapes) in expected_procedural_targets {
            let actual_shapes = actual_procedural_targets
                .get(&name)
                .cloned()
                .unwrap_or_default();
            if !contains_expected_shapes(&actual_shapes, &expected_shapes) {
                return Err(format!(
                    "bounded elaborator procedural assignment shape for '{name}' does not match slang: internal {actual_shapes:?}, slang {expected_shapes:?}"
                ) + &self.span_note("procedural:"));
            }
        }

        let actual_instances: std::collections::BTreeSet<(String, String)> = design
            .hierarchy
            .iter()
            .filter(|instance| instance.depth == 1)
            .map(|instance| {
                (
                    instance
                        .path
                        .strip_prefix(&format!("{}.", self.top_name))
                        .unwrap_or(&instance.path)
                        .to_string(),
                    instance.module.clone(),
                )
            })
            .collect();
        let expected_instances: std::collections::BTreeSet<(String, String)> = self
            .instances
            .iter()
            .map(|instance| {
                let instance_name = if strict_generated_hierarchy {
                    instance.instance_name.clone()
                } else {
                    instance
                        .instance_name
                        .split('.')
                        .next_back()
                        .unwrap_or(&instance.instance_name)
                        .to_string()
                };
                (instance_name, instance.definition_name.clone())
            })
            .collect();
        if actual_instances != expected_instances {
            return Err(format!(
                "bounded elaborator direct instances for '{}' do not match slang: internal {:?}, slang {:?}",
                self.top_name, actual_instances, expected_instances
            ) + &self.span_note("instance:"));
        }
        Ok(())
    }

    fn span_note(&self, key_prefix: &str) -> String {
        self.source_spans
            .iter()
            .find(|(key, _)| key.as_str() == key_prefix || key.starts_with(key_prefix))
            .map(|(key, span)| {
                format!(
                    " [slang {key} at {}:{}:{}-{}:{}]",
                    span.file, span.start_line, span.start_column, span.end_line, span.end_column
                )
            })
            .unwrap_or_default()
    }
}

fn canonicalize_memory_fingerprint(
    fingerprint: &str,
    memory_names: &std::collections::BTreeSet<String>,
) -> String {
    let Some(start) = fingerprint.find('[') else {
        return fingerprint.to_string();
    };
    let Some(end) = fingerprint[start..].find(',').map(|offset| start + offset) else {
        return fingerprint.to_string();
    };
    let Some(memory) = fingerprint.get(start + 1..end) else {
        return fingerprint.to_string();
    };
    if memory_names.contains(memory) {
        return fingerprint.to_string();
    }
    let hierarchical_matches = memory_names
        .iter()
        .filter(|candidate| candidate.ends_with(&format!(".{memory}")));
    let hierarchical_matches = hierarchical_matches.collect::<Vec<_>>();
    let Some(unique) = hierarchical_matches
        .first()
        .filter(|_| hierarchical_matches.len() == 1)
    else {
        return fingerprint.to_string();
    };
    format!(
        "{}{unique}{}",
        &fingerprint[..start + 1],
        &fingerprint[end..]
    )
}

fn memory_fingerprint_target_exists(
    fingerprint: &str,
    memory_names: &std::collections::BTreeSet<String>,
) -> bool {
    let Some(start) = fingerprint.find('[') else {
        return false;
    };
    let Some(end) = fingerprint[start..].find(',').map(|offset| start + offset) else {
        return false;
    };
    fingerprint
        .get(start + 1..end)
        .is_some_and(|memory| memory_names.contains(memory))
}

fn memory_member_lvalue_fingerprint(
    target_name: &str,
    member: &super::SlangMemoryMemberLvalueSummary,
    value: &super::SlangExpressionSummary,
    declarations: &[super::SlangDeclarationSummary],
    type_layouts: &std::collections::BTreeMap<String, super::SlangTypeLayoutSummary>,
    parameters: &[super::SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.name == target_name)?;
    let alias = super::type_layout_alias_key(&declaration.type_name)?;
    let mut current_alias = alias.to_string();
    let mut offset = 0_usize;
    let mut field_width = None;
    for member_name in &member.members {
        let layout = type_layouts.get(&current_alias)?;
        let field = layout
            .fields
            .iter()
            .find(|field| field.name == *member_name)?;
        offset += field.offset;
        field_width = Some(field.width);
        current_alias = super::type_layout_alias_key(&field.type_name)?.to_string();
    }
    let field_width = field_width?;
    let root_layout = type_layouts.get(alias)?;
    let element_width = root_layout.element_width;
    if offset + field_width > element_width {
        return None;
    }

    let indexes = member
        .indexes
        .iter()
        .map(|index| slang_expression_fingerprint_with_parameters(index, parameters, mode))
        .collect::<Option<Vec<_>>>()?;
    let old = format!(
        "MemoryRead:{element_width}:u[{target_name},{}]",
        indexes.join(",")
    );
    let value = slang_expression_fingerprint_with_parameters(value, parameters, mode)?;
    let mut parts = Vec::new();
    if offset + field_width < element_width {
        let lsb = offset + field_width;
        let msb = element_width - 1;
        parts.push(format!(
            "PackedSelect:{}:u[{old},Range:{msb}:{lsb}]",
            msb - lsb + 1
        ));
    }
    parts.push(value);
    if offset > 0 {
        let msb = offset - 1;
        parts.push(format!("PackedSelect:{}:u[{old},Range:{msb}:0]", msb + 1));
    }
    Some(format!("Concat:{element_width}:u[{}]", parts.join(",")))
}

fn dynamic_array_element_lvalue_fingerprints(
    target_name: &str,
    indexes: &[super::SlangExpressionSummary],
    value: &super::SlangExpressionSummary,
    declarations: &[super::SlangDeclarationSummary],
    parameters: &[super::SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<Vec<String>> {
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.name == target_name)?;
    let memory = memory_type(&declaration.type_name)?;
    if memory.dynamic || memory.queue || memory.associative {
        return None;
    }
    if indexes.len() != memory.dimensions.len() {
        return None;
    }
    let element_width = memory.element_width;
    let total_width = element_width.checked_mul(usize::try_from(memory.flat_length).ok()?)?;
    if total_width == 0 {
        return None;
    }
    let value_width = value.type_width?;
    if value_width != element_width {
        return None;
    }

    if memory.element_msb < memory.element_lsb {
        let whole_element_part = super::SlangArrayPartLvalueSummary {
            indexes: indexes.to_vec(),
            start: constant_zero_summary(),
            width: element_width,
            selection_kind: "IndexedUp".to_string(),
        };
        return array_part_lvalue_fingerprints(
            target_name,
            &whole_element_part,
            value,
            declarations,
            parameters,
            mode,
        );
    }

    let selector_fingerprints = indexes
        .iter()
        .map(|selector| slang_expression_fingerprint_with_parameters(selector, parameters, mode))
        .collect::<Option<Vec<_>>>()?;
    let value_fingerprint = slang_expression_fingerprint_with_parameters(value, parameters, mode)?;
    let const32 = |value: u64| format!("Const:32:u[32:{value:032b}]");
    let const_total_bits = |bits: &str| format!("Const:{total_width}:u[{total_width}:{bits}]");

    let mut in_range = "Const:1:u[1:1]".to_string();
    let mut physical_index = const32(0);
    let mut stride = memory.flat_length;
    for (selector_fingerprint, dimension) in selector_fingerprints.iter().zip(&memory.dimensions) {
        let base = dimension.base;
        let last = base + dimension.step * (dimension.length as i64 - 1);
        let lower = base.min(last);
        let upper = base.max(last);
        if lower < 0 || upper < 0 || lower > i64::from(u32::MAX) || upper > i64::from(u32::MAX) {
            return None;
        }

        let lower_bound = format!(
            "Ge:1:u[{selector_fingerprint},{}]",
            const32(lower.unsigned_abs())
        );
        let upper_bound = format!(
            "Le:1:u[{selector_fingerprint},{}]",
            const32(upper.unsigned_abs())
        );
        in_range = format!("LogAnd:1:u[{in_range},LogAnd:1:u[{lower_bound},{upper_bound}]]");
        let normalized = if base <= last {
            format!(
                "Sub:32:u[{selector_fingerprint},{}]",
                const32(base.unsigned_abs())
            )
        } else {
            format!(
                "Sub:32:u[{},{}]",
                const32(base.unsigned_abs()),
                selector_fingerprint
            )
        };
        let dimension_length = u64::try_from(dimension.length).ok()?;
        stride = stride.checked_div(dimension_length)?;
        physical_index = format!(
            "Add:32:u[{physical_index},Mul:32:u[{normalized},{}]]",
            const32(stride)
        );
    }
    let bit_offset = format!(
        "Mul:32:u[{physical_index},{}]",
        const32(element_width as u64)
    );

    let old = format!("Ref:{total_width}:u[{target_name}]");
    let wide_value = if value_width < total_width {
        let padding = total_width - value_width;
        let padding_zero = format!("Const:{padding}:u[{padding}:{:0padding$b}]", 0);
        format!(
            "Concat:{total_width}:u[{},{}]",
            padding_zero, value_fingerprint
        )
    } else {
        value_fingerprint
    };
    let shifted_value = format!(
        "Or:{total_width}:u[Shl:{total_width}:u[{wide_value},{bit_offset}],{}]",
        const_total_bits(&"0".repeat(total_width))
    );
    let selected_mask_bits = format!(
        "{}{}",
        "0".repeat(total_width - element_width),
        "1".repeat(element_width)
    );
    let selected_mask = format!(
        "Shl:{total_width}:u[{},{}]",
        const_total_bits(&selected_mask_bits),
        bit_offset
    );
    let merged = format!(
        "Or:{total_width}:u[And:{total_width}:u[{old},Not:{total_width}:u[{selected_mask}]],{shifted_value}]"
    );
    Some(vec![format!(
        "Cond:{total_width}:u[{in_range},{merged},{old}]"
    )])
}

fn constant_zero_summary() -> super::SlangExpressionSummary {
    super::SlangExpressionSummary {
        kind: "IntegerLiteral".to_string(),
        type_name: "int".to_string(),
        operator: String::new(),
        selection_kind: String::new(),
        type_width: Some(32),
        type_signed: Some(true),
        symbol: None,
        literal: Some(format!("32:{}", "0".repeat(32))),
        repeat_count: None,
        simulation_time_unit: None,
        enum_definition: None,
        operands: Vec::new(),
    }
}

fn array_part_lvalue_fingerprints(
    target_name: &str,
    part: &super::SlangArrayPartLvalueSummary,
    value: &super::SlangExpressionSummary,
    declarations: &[super::SlangDeclarationSummary],
    parameters: &[super::SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<Vec<String>> {
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.name == target_name)?;
    let memory = memory_type(&declaration.type_name)?;
    if memory.dynamic || memory.queue || memory.associative {
        return None;
    }
    if part.indexes.len() != memory.dimensions.len() {
        return None;
    }
    let element_width = memory.element_width;
    let flat_length = usize::try_from(memory.flat_length).ok()?;
    let total_width = element_width.checked_mul(flat_length)?;
    if total_width == 0 {
        return None;
    }
    if part.width == 0 || part.width > element_width {
        return None;
    }
    if value.type_width? != part.width {
        return None;
    }

    let index_fingerprints = part
        .indexes
        .iter()
        .map(|index| slang_expression_fingerprint_with_parameters(index, parameters, mode))
        .collect::<Option<Vec<_>>>()?;
    let start_fingerprint =
        slang_expression_fingerprint_with_parameters(&part.start, parameters, mode)?;
    let value_fingerprint = slang_expression_fingerprint_with_parameters(value, parameters, mode)?;
    let const32 = |value: u64| format!("Const:32:u[32:{value:032b}]");
    let logical_start = if part.selection_kind == "IndexedDown" {
        format!(
            "Sub:32:u[{start_fingerprint},{}]",
            const32(part.width as u64 - 1)
        )
    } else {
        start_fingerprint
    };
    let normalized_start = format!(
        "Sub:32:u[{logical_start},{}]",
        const32(if memory.element_msb < memory.element_lsb {
            memory.element_msb
        } else {
            memory.element_lsb
        } as u64)
    );

    let physical_layout = |physical_element: usize| -> Option<(usize, Vec<i64>)> {
        let mut physical_offset = physical_element;
        let mut element_index = 0_usize;
        let mut logical_indexes = Vec::with_capacity(memory.dimensions.len());
        let mut stride = flat_length;
        for dimension in &memory.dimensions {
            stride /= dimension.length;
            let dimension_offset = physical_offset / stride;
            physical_offset %= stride;
            if dimension_offset >= dimension.length {
                return None;
            }
            let logical_index = dimension.base + dimension.step * dimension_offset as i64;
            element_index = element_index * dimension.length + dimension_offset;
            logical_indexes.push(logical_index);
        }
        Some((element_index, logical_indexes))
    };

    let array_part_body = |base_offset: usize| {
        format!(
            "{target_name},Element:{element_width},Offset:{base_offset},Reversed:{},{normalized_start},{value_fingerprint}",
            memory.element_msb < memory.element_lsb,
        )
    };

    if part
        .indexes
        .iter()
        .all(|index| summary_literal_i64(index).is_some())
    {
        let index_values = part
            .indexes
            .iter()
            .map(summary_literal_i64)
            .collect::<Option<Vec<_>>>()?;
        let mut element_index = 0_usize;
        for (index_value, dimension) in index_values.iter().zip(&memory.dimensions) {
            let normalized_index = if dimension.step > 0 {
                index_value - dimension.base
            } else {
                dimension.base - index_value
            };
            if normalized_index < 0 || normalized_index >= dimension.length as i64 {
                return None;
            }
            element_index = element_index * dimension.length + normalized_index as usize;
        }
        let base_offset = element_index * element_width;
        return Some(vec![format!(
            "ArrayPartWrite:{}:u[{}]",
            part.width,
            array_part_body(base_offset)
        )]);
    }

    let mut shapes = Vec::with_capacity(flat_length);
    let mut excluded_conditions = None::<String>;
    for physical_element in (0..flat_length).rev() {
        let (element_index, logical_indexes) = physical_layout(physical_element)?;
        let mut candidate_condition = "Const:1:u[1:1]".to_string();
        for (index_fingerprint, logical_index) in index_fingerprints.iter().zip(logical_indexes) {
            candidate_condition = format!(
                "And:1:u[{candidate_condition},Eq:1:u[{index_fingerprint},{}]]",
                const32(logical_index as u64)
            );
        }

        let guard = match &excluded_conditions {
            Some(excluded) => format!("And:1:u[{excluded},{candidate_condition}]"),
            None => candidate_condition.clone(),
        };
        let base_offset = element_index * element_width;
        shapes.push(format!(
            "GuardedArrayPartWrite:{}:u[{guard},{}]",
            part.width,
            array_part_body(base_offset)
        ));

        excluded_conditions = match excluded_conditions {
            Some(excluded) => Some(format!(
                "And:1:u[{excluded},Not:1:u[{candidate_condition}]]"
            )),
            None => Some(format!("Not:1:u[{candidate_condition}]")),
        };
    }
    Some(shapes)
}

fn dynamic_packed_lvalue_fingerprint(
    target_name: &str,
    selector: &super::SlangExpressionSummary,
    value: &super::SlangExpressionSummary,
    declarations: &[super::SlangDeclarationSummary],
    parameters: &[super::SlangParameterSummary],
    mode: ExpressionFingerprintMode,
) -> Option<String> {
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.name == target_name)?;
    let shape = simple_type_shape(&declaration.type_name)?;
    let dimension = shape.dimensions.first()?;
    let base_width = shape.width;
    let group_width = dimension.element_width;
    let selector_width = selector.type_width?.max(1);
    let selector_fingerprint =
        slang_expression_fingerprint_with_parameters(selector, parameters, mode)?;
    let value_fingerprint = slang_expression_fingerprint_with_parameters(value, parameters, mode)?;
    let old_fingerprint = format!("Ref:{base_width}:u[{target_name}]");
    let mut physical_bits = vec![None; base_width];

    for coordinate in 0..dimension.length {
        let logical_index = if dimension.msb >= dimension.lsb {
            dimension.lsb + coordinate as i64
        } else {
            dimension.lsb - coordinate as i64
        };
        let logical_bits = logical_index as u64;
        let mut constant_bits = String::new();
        for bit in (0..selector_width).rev() {
            constant_bits.push(if (logical_bits >> bit) & 1 == 0 {
                '0'
            } else {
                '1'
            });
        }
        let condition = format!(
            "Eq:1:u[{selector_fingerprint},Const:{selector_width}:u[{selector_width}:{constant_bits}]]"
        );
        for bit in 0..group_width {
            let physical_bit = coordinate * group_width + bit;
            let updated = format!("PackedSelect:1:u[{value_fingerprint},Range:{bit}:{bit}]");
            let old =
                format!("PackedSelect:1:u[{old_fingerprint},Range:{physical_bit}:{physical_bit}]");
            physical_bits[physical_bit] = Some(format!("Cond:1:u[{condition},{updated},{old}]"));
        }
    }

    let operands = physical_bits
        .into_iter()
        .rev()
        .collect::<Option<Vec<_>>>()?;
    Some(format!("Concat:{base_width}:u[{}]", operands.join(",")))
}
