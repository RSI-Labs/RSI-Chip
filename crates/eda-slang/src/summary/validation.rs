//! Compare authoritative slang summaries against the repository IR.

use super::expressions::{
    contains_expected_shapes, design_expr_fingerprint, design_lvalue_name, design_stmt_assignments,
    slang_expression_fingerprint_with_parameters,
};
use super::*;

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

        let mut expected_continuous_targets: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for target in &self.continuous_targets {
            expected_continuous_targets
                .entry(target.name.clone())
                .or_default()
                .push(
                    slang_expression_fingerprint_with_parameters(&target.rhs, &self.parameters)
                        .unwrap_or_else(|| "?".to_string()),
                );
        }
        let memory_names: std::collections::BTreeSet<String> = design
            .memories
            .iter()
            .map(|memory| memory.name.clone())
            .collect();
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
                        let shape = if legacy_array_targets.contains(target)
                            || assignment.dynamic_packed_lvalue
                        {
                            // Small arrays still use the legacy bounded
                            // per-element expansion, whose expression tree is
                            // intentionally not structurally equivalent to the
                            // slang source. Preserve assignment-count checking
                            // without guessing an equivalence.
                            "?".to_string()
                        } else {
                            slang_expression_fingerprint_with_parameters(
                                &assignment.rhs,
                                &self.parameters,
                            )
                            .unwrap_or_else(|| "?".to_string())
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
