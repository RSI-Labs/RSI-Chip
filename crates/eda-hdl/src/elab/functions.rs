//! Function inlining, aggregate-state evaluation, and nested call expansion.

use super::*;

impl<'a> Elaborator<'a> {
    pub(super) fn inline_function_call(
        &self,
        name: &str,
        arguments: &[ast::Expr],
        env: &Env<'_>,
    ) -> Result<ExprIR, String> {
        let inlined = self.build_function_expression(name, arguments, env, &Vec::new())?;
        self.elab_expr(&inlined, env)
    }

    pub(super) fn build_function_expression(
        &self,
        name: &str,
        arguments: &[ast::Expr],
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        if call_stack.iter().any(|active| active == name) {
            return Err(format!(
                "recursive function call '{}' through {:?}",
                name, call_stack
            ));
        }
        let definition = env
            .functions
            .get(name)
            .ok_or_else(|| format!("unknown function '{}'", name))?;
        let arguments = self.expand_function_actuals(definition, arguments)?;
        let inlined_arguments = arguments
            .iter()
            .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
            .collect::<Result<Vec<_>, String>>()?;
        validate_function_body(definition)?;
        let mut argument_map = HashMap::new();
        for ((formal, range), actual) in definition.args.iter().zip(inlined_arguments) {
            let width = self.ast_range_width(range, env)?;
            let actual = ast::Expr::Resize {
                width,
                value: Box::new(actual),
            };
            argument_map.insert(formal.clone(), actual);
        }
        let mut nested_stack = call_stack.to_vec();
        nested_stack.push(name.to_string());
        self.evaluate_function_body(definition, &mut argument_map, env, &nested_stack)
    }

    pub(super) fn evaluate_function_body(
        &self,
        definition: &ast::FunctionDef,
        values: &mut HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        let mut return_expression = None;
        let mut aggregate_types = FunctionAggregateState::default();
        for (argument, range) in &definition.args {
            let width = self.ast_range_width(range, env)?;
            aggregate_types
                .scalar_widths
                .insert(argument.clone(), width);
            aggregate_types
                .scalar_ranges
                .insert(argument.clone(), self.ast_range_bounds(range, env)?);
        }
        self.evaluate_function_statements(
            &definition.body,
            definition,
            values,
            env,
            call_stack,
            &mut aggregate_types,
            &mut return_expression,
        )?;
        return_expression.ok_or_else(|| {
            format!(
                "function '{}' must assign its return value exactly once",
                definition.name
            )
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn evaluate_function_statements(
        &self,
        statements: &[ast::Stmt],
        definition: &ast::FunctionDef,
        values: &mut HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
        aggregate_types: &mut FunctionAggregateState,
        return_expression: &mut Option<ast::Expr>,
    ) -> Result<(), String> {
        for statement in statements {
            match statement {
                ast::Stmt::Block(nested) => self.evaluate_function_statements(
                    nested,
                    definition,
                    values,
                    env,
                    call_stack,
                    aggregate_types,
                    return_expression,
                )?,
                ast::Stmt::Local {
                    type_name,
                    range,
                    array_ranges,
                    names,
                } => {
                    for (name, initializer) in names {
                        let initializer = if let Some(type_name) = type_name {
                            if range.is_some() {
                                return Err(format!(
                                    "function '{}': typed local '{}' does not support an additional packed range",
                                    definition.name, name
                                ));
                            }
                            let definition = env.types.get(type_name).ok_or_else(|| {
                                format!(
                                    "function local '{}' uses unknown type '{}'",
                                    name, type_name
                                )
                            })?;
                            aggregate_types
                                .types
                                .insert(name.clone(), type_name.clone());
                            aggregate_types
                                .scalar_widths
                                .insert(name.clone(), definition.width());
                            aggregate_types
                                .scalar_ranges
                                .insert(name.clone(), (definition.width() as i64 - 1, 0));
                            match initializer {
                                Some(expression) => self.expand_function_expression(
                                    expression,
                                    values,
                                    env,
                                    call_stack,
                                    aggregate_types,
                                )?,
                                None => ast::Expr::Num {
                                    width: Some(definition.width()),
                                    word: LogicWord::zero(definition.width()),
                                    orig: "0".to_string(),
                                },
                            }
                        } else {
                            aggregate_types.types.remove(name);
                            if !array_ranges.is_empty() {
                                let element_width = self.ast_range_width(range, env)?;
                                let (element_msb, element_lsb) =
                                    self.ast_range_bounds(range, env)?;
                                aggregate_types.scalar_widths.remove(name);
                                aggregate_types.scalar_ranges.remove(name);
                                let mut dimensions = Vec::with_capacity(array_ranges.len());
                                let mut length = 1_usize;
                                for array_range in array_ranges {
                                    let Some((left, right)) = array_range else {
                                        return Err(format!(
                                            "function array '{}' has an empty dimension",
                                            name
                                        ));
                                    };
                                    let base =
                                        self.const_eval(left, env.params, env.module_name)?;
                                    let last =
                                        self.const_eval(right, env.params, env.module_name)?;
                                    let step = if base <= last { 1 } else { -1 };
                                    let dimension_length =
                                        (base - last).unsigned_abs() as usize + 1;
                                    if dimension_length == 0 || dimension_length > 256 {
                                        return Err(format!(
                                            "function array '{}' has invalid dimensions",
                                            name
                                        ));
                                    }
                                    length = length
                                        .checked_mul(dimension_length)
                                        .filter(|total| *total <= 4096)
                                        .ok_or_else(|| {
                                            format!("function array '{}' is too large", name)
                                        })?;
                                    dimensions.push(FunctionArrayDimension {
                                        base,
                                        step,
                                        length: dimension_length,
                                    });
                                }
                                let total_width = element_width
                                    .checked_mul(length)
                                    .ok_or_else(|| "function array is too large".to_string())?;
                                if total_width > 4096 {
                                    return Err(format!(
                                        "function array '{}' exceeds the packed-width limit",
                                        name
                                    ));
                                }
                                aggregate_types.arrays.insert(
                                    name.clone(),
                                    FunctionArrayLocal {
                                        dimensions,
                                        element_width,
                                        element_msb,
                                        element_lsb,
                                        length,
                                    },
                                );
                                aggregate_types.array_values.insert(
                                    name.clone(),
                                    vec![
                                        ast::Expr::Num {
                                            width: Some(element_width),
                                            word: LogicWord::zero(element_width),
                                            orig: "0".to_string(),
                                        };
                                        length
                                    ],
                                );
                                continue;
                            } else {
                                aggregate_types.arrays.remove(name);
                                aggregate_types.array_values.remove(name);
                                aggregate_types
                                    .scalar_widths
                                    .insert(name.clone(), self.ast_range_width(range, env)?);
                                aggregate_types
                                    .scalar_ranges
                                    .insert(name.clone(), self.ast_range_bounds(range, env)?);
                                match initializer {
                                    Some(expression) => self.expand_function_expression(
                                        expression,
                                        values,
                                        env,
                                        call_stack,
                                        aggregate_types,
                                    )?,
                                    None => self.local_zero(range, env)?,
                                }
                            }
                        };
                        let initializer =
                            if let Some(width) = aggregate_types.scalar_widths.get(name).copied() {
                                ast::Expr::Resize {
                                    width,
                                    value: Box::new(initializer),
                                }
                            } else {
                                initializer
                            };
                        values.insert(name.clone(), initializer);
                    }
                }
                ast::Stmt::BL { lv, e } => {
                    if let ast::Expr::ArrayIndexedPart {
                        name,
                        indexes,
                        start,
                        width,
                        ascending,
                    } = lv
                    {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        if indexes.len() != array.dimensions.len() {
                            return Err(format!(
                                "array expects {} index(es), received {}",
                                array.dimensions.len(),
                                indexes.len()
                            ));
                        }
                        let mut resolved_indexes = Vec::with_capacity(indexes.len());
                        let mut runtime_indexes = Vec::with_capacity(indexes.len());
                        let mut all_indexes_constant = true;
                        for index in indexes {
                            let index = self.expand_function_expression(
                                index,
                                values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            runtime_indexes.push(index.clone());
                            if let Ok(constant) =
                                self.const_eval(&index, env.params, env.module_name)
                            {
                                resolved_indexes.push(constant);
                            } else {
                                all_indexes_constant = false;
                            }
                        }
                        let reversed = array.element_msb < array.element_lsb;
                        let logical_base = if reversed {
                            array.element_msb
                        } else {
                            array.element_lsb
                        };

                        let start = self.expand_function_expression(
                            start,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let width = self.expand_function_expression(
                            width,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let width = self.const_eval(&width, env.params, env.module_name)?;
                        if width <= 0 {
                            return Err(format!(
                                "indexed part-select width must be positive, got {width}"
                            ));
                        }
                        let width = width as usize;
                        if width > array.element_width {
                            return Err(format!(
                                "indexed part-select width {width} exceeds array element width {}",
                                array.element_width
                            ));
                        }
                        let logical_start = if *ascending {
                            start
                        } else {
                            ast::Expr::Bin(
                                ast::BinOpSrc::Sub,
                                Box::new(start),
                                Box::new(constant_expr(width as i64 - 1)),
                            )
                        };
                        let offset = ast::Expr::Bin(
                            ast::BinOpSrc::Sub,
                            Box::new(logical_start),
                            Box::new(constant_expr(logical_base)),
                        );
                        let value = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let update_element = |old_element: ast::Expr| {
                            let normalized_old = if reversed {
                                Self::reverse_ast_bits(old_element.clone(), array.element_width)
                            } else {
                                old_element.clone()
                            };
                            let normalized_updated = Self::function_array_indexed_update(
                                normalized_old,
                                array.element_width,
                                offset.clone(),
                                width,
                                value.clone(),
                            );
                            if reversed {
                                Self::reverse_ast_bits(normalized_updated, array.element_width)
                            } else {
                                normalized_updated
                            }
                        };
                        if all_indexes_constant {
                            let element_index = array.element_index(&resolved_indexes)?;
                            let old_element =
                                aggregate_types.array_values[name][element_index].clone();
                            let updated = update_element(old_element);
                            let elements = aggregate_types
                                .array_values
                                .get_mut(name)
                                .ok_or_else(|| format!("array local '{name}' state is missing"))?;
                            elements[element_index] = updated;
                            continue;
                        }

                        let old_elements = aggregate_types.array_values[name].clone();
                        let mut updated_elements = Vec::with_capacity(array.length);
                        for (physical_element, old_element) in old_elements.iter().enumerate() {
                            let mut physical_offset = physical_element;
                            let mut scale = array.length;
                            let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
                            for dimension in &array.dimensions {
                                scale /= dimension.length;
                                let dimension_offset = physical_offset / scale;
                                physical_offset %= scale;
                                logical_indexes.push(
                                    dimension.base + dimension.step * dimension_offset as i64,
                                );
                            }
                            let mut candidate_valid = ast::Expr::Num {
                                width: Some(1),
                                word: LogicWord::from_u64(1, 1),
                                orig: "function_runtime_array_write_true".to_string(),
                            };
                            for (runtime_index, logical_index) in
                                runtime_indexes.iter().zip(&logical_indexes)
                            {
                                let equals = ast::Expr::Bin(
                                    ast::BinOpSrc::Eq,
                                    Box::new(runtime_index.clone()),
                                    Box::new(constant_expr(*logical_index)),
                                );
                                candidate_valid = ast::Expr::Bin(
                                    ast::BinOpSrc::LogAnd,
                                    Box::new(candidate_valid),
                                    Box::new(equals),
                                );
                            }
                            let updated = update_element(old_element.clone());
                            updated_elements.push(ast::Expr::Cond(
                                Box::new(candidate_valid),
                                Box::new(updated),
                                Box::new(old_element.clone()),
                            ));
                        }
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{name}' state is missing"))?;
                        *elements = updated_elements;
                        continue;
                    }
                    if let ast::Expr::ArraySelect(name, indexes) = lv {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        let mut resolved_indexes = Vec::with_capacity(indexes.len());
                        for index in indexes {
                            let index = self.expand_function_expression(
                                index,
                                values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            resolved_indexes.push(self.const_eval(
                                &index,
                                env.params,
                                env.module_name,
                            )?);
                        }
                        let element_index = array.element_index(&resolved_indexes)?;
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{}' state is missing", name))?;
                        elements[element_index] = ast::Expr::Resize {
                            width: array.element_width,
                            value: Box::new(expanded),
                        };
                        continue;
                    }
                    if let ast::Expr::Select(name, index) = lv {
                        let Some(array) = aggregate_types.arrays.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not an array local",
                                definition.name, name
                            ));
                        };
                        let index = self.expand_function_expression(
                            index,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let index = self.const_eval(&index, env.params, env.module_name)?;
                        let element_index = array.element_index(&[index])?;
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let elements = aggregate_types
                            .array_values
                            .get_mut(name)
                            .ok_or_else(|| format!("array local '{}' state is missing", name))?;
                        elements[element_index] = ast::Expr::Resize {
                            width: array.element_width,
                            value: Box::new(expanded),
                        };
                        continue;
                    }
                    if let ast::Expr::Member(name, field) = lv {
                        let Some(type_name) = aggregate_types.types.get(name).cloned() else {
                            return Err(format!(
                                "function '{}': assignment target '{}' is not a typed local",
                                definition.name, name
                            ));
                        };
                        let expanded = self.expand_function_expression(
                            e,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let updated = self.update_aggregate_field(
                            values[name].clone(),
                            env,
                            &type_name,
                            field,
                            expanded,
                        )?;
                        values.insert(name.clone(), updated);
                        continue;
                    }
                    let ast::Expr::Ident(name) = lv else {
                        return Err(format!(
                            "function '{}': only scalar/function or packed aggregate field assignments are supported",
                            definition.name
                        ));
                    };
                    let mut expanded = self.expand_function_expression(
                        e,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    if name == &definition.name {
                        let width = self.ast_range_width(&definition.return_range, env)?;
                        expanded = ast::Expr::Resize {
                            width,
                            value: Box::new(expanded),
                        };
                        if return_expression.replace(expanded).is_some() {
                            return Err(format!(
                                "function '{}' assigns its return value more than once",
                                definition.name
                            ));
                        }
                    } else if values.contains_key(name) {
                        let expanded =
                            if let Some(width) = aggregate_types.scalar_widths.get(name).copied() {
                                ast::Expr::Resize {
                                    width,
                                    value: Box::new(expanded),
                                }
                            } else {
                                expanded
                            };
                        values.insert(name.clone(), expanded);
                    } else {
                        return Err(format!(
                            "function '{}': assignment to undeclared local '{}'",
                            definition.name, name
                        ));
                    }
                }
                ast::Stmt::Repeat { count, body } => {
                    let count = self.expand_function_expression(
                        count,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let count = self.const_eval(&count, env.params, env.module_name)?;
                    if !(0..=256).contains(&count) {
                        return Err(format!(
                            "function '{}': repeat count must be between 0 and 256, got {}",
                            definition.name, count
                        ));
                    }
                    for _ in 0..count {
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                    }
                }
                ast::Stmt::While { cond, body } => {
                    let mut iterations = 0_usize;
                    loop {
                        let condition = self.expand_function_expression(
                            cond,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let condition = self
                            .const_eval(&condition, env.params, env.module_name)
                            .map_err(|error| {
                                format!(
                                    "function '{}': while condition must be constant during elaboration: {error}",
                                    definition.name
                                )
                            })?;
                        if condition == 0 {
                            break;
                        }
                        if iterations == 256 {
                            return Err(format!(
                                "function '{}': while loop exceeds the 256-iteration elaboration limit",
                                definition.name
                            ));
                        }
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                        iterations += 1;
                    }
                }
                ast::Stmt::For {
                    var,
                    start,
                    cond,
                    step,
                    body,
                } => {
                    let start = self.expand_function_expression(
                        start,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let start = self.const_eval(&start, env.params, env.module_name)?;
                    if !(0..=i32::MAX as i64).contains(&start) {
                        return Err(format!(
                            "function '{}': for-loop start for '{var}' must be a non-negative integer",
                            definition.name
                        ));
                    }
                    values.insert(var.clone(), loop_value(start));
                    let mut iterations = 0_usize;
                    loop {
                        if iterations == 256 {
                            return Err(format!(
                                "function '{}': for-loop '{var}' exceeds the 256-iteration elaboration limit",
                                definition.name
                            ));
                        }
                        let condition = self.expand_function_expression(
                            cond,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let condition = self.const_eval(&condition, env.params, env.module_name)?;
                        if condition == 0 {
                            break;
                        }
                        self.evaluate_function_statements(
                            std::slice::from_ref(body.as_ref()),
                            definition,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                            return_expression,
                        )?;
                        let step = self.expand_function_expression(
                            step,
                            values,
                            env,
                            call_stack,
                            aggregate_types,
                        )?;
                        let next = self.const_eval(&step, env.params, env.module_name)?;
                        if !(0..=i32::MAX as i64).contains(&next) {
                            return Err(format!(
                                "function '{}': for-loop '{var}' step must remain a non-negative integer",
                                definition.name
                            ));
                        }
                        let current = match &values[var] {
                            ast::Expr::Num { word, .. } => word.to_u64().unwrap_or(0) as i64,
                            _ => return Err(format!(
                                "function '{}': for-loop variable '{var}' must remain constant during elaboration",
                                definition.name
                            )),
                        };
                        if next == current {
                            return Err(format!(
                                "function '{}': for-loop '{var}' does not advance",
                                definition.name
                            ));
                        }
                        values.insert(var.clone(), loop_value(next));
                        iterations += 1;
                    }
                    values.remove(var);
                }
                ast::Stmt::If { c, t, f } => {
                    let condition = self.expand_function_expression(
                        c,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let mut then_values = values.clone();
                    let mut then_aggregate_types = aggregate_types.clone();
                    let mut then_return = return_expression.clone();
                    self.evaluate_function_statements(
                        std::slice::from_ref(t.as_ref()),
                        definition,
                        &mut then_values,
                        env,
                        call_stack,
                        &mut then_aggregate_types,
                        &mut then_return,
                    )?;
                    let mut else_values = values.clone();
                    let mut else_aggregate_types = aggregate_types.clone();
                    let mut else_return = return_expression.clone();
                    if let Some(false_branch) = f {
                        self.evaluate_function_statements(
                            std::slice::from_ref(false_branch.as_ref()),
                            definition,
                            &mut else_values,
                            env,
                            call_stack,
                            &mut else_aggregate_types,
                            &mut else_return,
                        )?;
                    }
                    if then_return.is_some() != else_return.is_some() {
                        return Err(format!(
                            "function '{}': if branches must both assign or both preserve the return value",
                            definition.name
                        ));
                    }
                    for key in then_values.keys().cloned().collect::<Vec<_>>() {
                        let then_value = then_values.remove(&key).expect("known then value");
                        let else_value = else_values.remove(&key).expect("known else value");
                        let merged = if then_value == else_value {
                            then_value
                        } else {
                            ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(then_value),
                                Box::new(else_value),
                            )
                        };
                        values.insert(key, merged);
                    }
                    Self::merge_array_states(
                        aggregate_types,
                        then_aggregate_types.array_values,
                        else_aggregate_types.array_values,
                        &condition,
                    );
                    *return_expression = match (then_return, else_return) {
                        (Some(then_value), Some(else_value)) => Some(if then_value == else_value {
                            then_value
                        } else {
                            ast::Expr::Cond(
                                Box::new(condition),
                                Box::new(then_value),
                                Box::new(else_value),
                            )
                        }),
                        (None, None) => None,
                        _ => unreachable!("branch return states were checked"),
                    };
                }
                ast::Stmt::Case {
                    e,
                    arms,
                    dflt,
                    kind,
                } => {
                    let selector = self.expand_function_expression(
                        e,
                        values,
                        env,
                        call_stack,
                        aggregate_types,
                    )?;
                    let entry_values = values.clone();
                    let entry_return = return_expression.clone();
                    let mut branches = Vec::new();
                    for (labels, body) in arms {
                        let mut condition = None;
                        for label in labels {
                            let label = self.expand_function_expression(
                                label,
                                &entry_values,
                                env,
                                call_stack,
                                aggregate_types,
                            )?;
                            let comparison = if *kind == ast::CaseKind::Exact {
                                ast::Expr::Bin(
                                    ast::BinOpSrc::Eq,
                                    Box::new(selector.clone()),
                                    Box::new(label),
                                )
                            } else {
                                ast::Expr::WildcardMatch {
                                    kind: *kind,
                                    selector: Box::new(selector.clone()),
                                    label: Box::new(label),
                                }
                            };
                            condition = Some(match condition {
                                Some(existing) => ast::Expr::Bin(
                                    ast::BinOpSrc::LogOr,
                                    Box::new(existing),
                                    Box::new(comparison),
                                ),
                                None => comparison,
                            });
                        }
                        let Some(condition) = condition else {
                            return Err(format!(
                                "function '{}': case arm has no labels",
                                definition.name
                            ));
                        };
                        let mut branch_values = entry_values.clone();
                        let mut branch_aggregate_types = aggregate_types.clone();
                        let mut branch_return = entry_return.clone();
                        self.evaluate_function_statements(
                            std::slice::from_ref(body),
                            definition,
                            &mut branch_values,
                            env,
                            call_stack,
                            &mut branch_aggregate_types,
                            &mut branch_return,
                        )?;
                        branches.push((
                            condition,
                            branch_values,
                            branch_return,
                            branch_aggregate_types.array_values,
                        ));
                    }
                    let mut unmatched_values = entry_values.clone();
                    let mut unmatched_aggregate_types = aggregate_types.clone();
                    let mut unmatched_return = entry_return.clone();
                    if let Some(default_body) = dflt {
                        self.evaluate_function_statements(
                            std::slice::from_ref(default_body),
                            definition,
                            &mut unmatched_values,
                            env,
                            call_stack,
                            &mut unmatched_aggregate_types,
                            &mut unmatched_return,
                        )?;
                    }
                    for key in unmatched_values.keys().cloned().collect::<Vec<_>>() {
                        let mut merged = unmatched_values
                            .remove(&key)
                            .expect("known unmatched case value");
                        for (condition, branch_values, _, _) in branches.iter().rev() {
                            let branch_value = branch_values
                                .get(&key)
                                .cloned()
                                .expect("known case branch value");
                            merged = ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(branch_value),
                                Box::new(merged),
                            );
                        }
                        values.insert(key, merged);
                    }
                    let mut return_states = Vec::with_capacity(branches.len() + 1);
                    return_states.push(unmatched_return.as_ref());
                    for (_, _, branch_return, _) in &branches {
                        return_states.push(branch_return.as_ref());
                    }
                    if return_states.iter().any(Option::is_some)
                        && return_states.iter().any(Option::is_none)
                    {
                        return Err(format!(
                            "function '{}': every case path must assign or preserve the return value",
                            definition.name
                        ));
                    }
                    let merged_return = if return_states.iter().all(Option::is_some) {
                        let mut merged = unmatched_return.clone().expect("checked case return");
                        for (condition, _, branch_return, _) in branches.iter().rev() {
                            let branch_value = branch_return.clone().expect("checked return");
                            merged = ast::Expr::Cond(
                                Box::new(condition.clone()),
                                Box::new(branch_value),
                                Box::new(merged),
                            );
                        }
                        Some(merged)
                    } else {
                        None
                    };
                    aggregate_types.array_values = unmatched_aggregate_types.array_values;
                    for (condition, _, _, branch_arrays) in branches.into_iter().rev() {
                        let unmatched_arrays = aggregate_types.array_values.clone();
                        Self::merge_array_states(
                            aggregate_types,
                            branch_arrays,
                            unmatched_arrays,
                            &condition,
                        );
                    }
                    *return_expression = merged_return;
                }
                other => {
                    return Err(format!(
                        "function '{}': unsupported statement {:?} in straight-line function body",
                        definition.name, other
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn expand_function_expression(
        &self,
        expression: &ast::Expr,
        values: &HashMap<String, ast::Expr>,
        env: &Env<'_>,
        call_stack: &[String],
        aggregate_types: &FunctionAggregateState,
    ) -> Result<ast::Expr, String> {
        let substituted = substitute_function_arguments(expression, values);
        let substituted =
            self.resolve_aggregate_references(&substituted, values, aggregate_types, env)?;
        self.inline_nested_function_calls(&substituted, env, call_stack)
    }

    pub(super) fn resolve_aggregate_references(
        &self,
        expression: &ast::Expr,
        values: &HashMap<String, ast::Expr>,
        aggregate_types: &FunctionAggregateState,
        env: &Env<'_>,
    ) -> Result<ast::Expr, String> {
        Ok(match expression {
            ast::Expr::Member(object, field)
                if values.contains_key(object) && aggregate_types.types.contains_key(object) =>
            {
                let type_name = &aggregate_types.types[object];
                let definition = env.types.get(type_name).ok_or_else(|| {
                    format!(
                        "function local '{}' uses unknown type '{}'",
                        object, type_name
                    )
                })?;
                let definition = match definition {
                    ResolvedType::Struct(definition) | ResolvedType::Union(definition) => {
                        definition
                    }
                    ResolvedType::Enum(_) => {
                        return Err(format!(
                            "function local '{}' has enum type and no field '{}'",
                            object, field
                        ));
                    }
                };
                let &(offset, width) = definition
                    .fields
                    .get(field)
                    .ok_or_else(|| format!("type '{}' has no field '{}'", type_name, field))?;
                ast::Expr::Slice {
                    value: Box::new(values[object].clone()),
                    msb: Box::new(constant_expr((offset + width - 1) as i64)),
                    lsb: Box::new(constant_expr(offset as i64)),
                }
            }
            ast::Expr::Ident(name) if aggregate_types.arrays.contains_key(name) => {
                ast::Expr::Concat(aggregate_types.array_values[name].clone())
            }
            ast::Expr::Ident(_)
            | ast::Expr::ScopedIdent { .. }
            | ast::Expr::Num { .. }
            | ast::Expr::Str(_)
            | ast::Expr::Member(_, _)
            | ast::Expr::MemberChain(_) => expression.clone(),
            ast::Expr::Select(name, index) if aggregate_types.arrays.contains_key(name) => {
                let array = &aggregate_types.arrays[name];
                let index =
                    self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                let index = self.const_eval(&index, env.params, env.module_name)?;
                let element_index = array.element_index(&[index])?;
                aggregate_types.array_values[name][element_index].clone()
            }
            ast::Expr::ArraySelect(name, indexes) if aggregate_types.arrays.contains_key(name) => {
                let mut resolved_indexes = Vec::with_capacity(indexes.len());
                for index in indexes {
                    let index =
                        self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                    resolved_indexes.push(self.const_eval(&index, env.params, env.module_name)?);
                }
                let array = &aggregate_types.arrays[name];
                let element_index = array.element_index(&resolved_indexes)?;
                aggregate_types.array_values[name][element_index].clone()
            }
            ast::Expr::Select(name, index) => ast::Expr::Select(
                name.clone(),
                Box::new(self.resolve_aggregate_references(index, values, aggregate_types, env)?),
            ),
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => ast::Expr::MethodCall {
                object: object.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => ast::Expr::MethodCallChain {
                path: path.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::New(size) => ast::Expr::New(Box::new(self.resolve_aggregate_references(
                size,
                values,
                aggregate_types,
                env,
            )?)),
            ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
                arguments
                    .iter()
                    .map(|argument| {
                        self.resolve_aggregate_references(argument, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
                name.clone(),
                indexes
                    .iter()
                    .map(|index| {
                        self.resolve_aggregate_references(index, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArrayPart {
                name,
                indexes,
                msb,
                lsb,
            } => {
                let indexes = indexes
                    .iter()
                    .map(|index| {
                        self.resolve_aggregate_references(index, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let msb = self.resolve_aggregate_references(msb, values, aggregate_types, env)?;
                let lsb = self.resolve_aggregate_references(lsb, values, aggregate_types, env)?;
                let normalized = self.normalize_array_part(name, &indexes, &msb, &lsb, env)?;
                self.resolve_aggregate_references(&normalized, values, aggregate_types, env)?
            }
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => {
                let Some(array) = aggregate_types.arrays.get(name) else {
                    return Err(format!(
                        "indexed part-select uses unknown function array '{}'",
                        name
                    ));
                };
                let mut resolved_indexes = Vec::with_capacity(indexes.len());
                let mut runtime_indexes = Vec::with_capacity(indexes.len());
                let mut all_indexes_constant = true;
                for index in indexes {
                    let index =
                        self.resolve_aggregate_references(index, values, aggregate_types, env)?;
                    runtime_indexes.push(index.clone());
                    if let Ok(constant) = self.const_eval(&index, env.params, env.module_name) {
                        resolved_indexes.push(constant);
                    } else {
                        all_indexes_constant = false;
                    }
                }
                let mut array_index_valid = None;
                let element = if all_indexes_constant {
                    let element_index = array.element_index(&resolved_indexes)?;
                    aggregate_types.array_values[name][element_index].clone()
                } else {
                    let mut selected_element = ast::Expr::Num {
                        width: Some(array.element_width),
                        word: LogicWord::xes(array.element_width),
                        orig: "function_runtime_array_x".to_string(),
                    };
                    let mut any_index_valid = ast::Expr::Num {
                        width: Some(1),
                        word: LogicWord::from_u64(1, 0),
                        orig: "function_runtime_array_false".to_string(),
                    };
                    for physical_element in 0..array.length {
                        let mut physical_offset = physical_element;
                        let mut scale = array.length;
                        let mut logical_indexes = Vec::with_capacity(array.dimensions.len());
                        for dimension in &array.dimensions {
                            scale /= dimension.length;
                            let dimension_offset = physical_offset / scale;
                            physical_offset %= scale;
                            logical_indexes
                                .push(dimension.base + dimension.step * dimension_offset as i64);
                        }

                        let mut candidate_valid = ast::Expr::Num {
                            width: Some(1),
                            word: LogicWord::from_u64(1, 1),
                            orig: "function_runtime_array_true".to_string(),
                        };
                        for (runtime_index, logical_index) in
                            runtime_indexes.iter().zip(&logical_indexes)
                        {
                            let equals = ast::Expr::Bin(
                                ast::BinOpSrc::Eq,
                                Box::new(runtime_index.clone()),
                                Box::new(constant_expr(*logical_index)),
                            );
                            candidate_valid = ast::Expr::Bin(
                                ast::BinOpSrc::LogAnd,
                                Box::new(candidate_valid),
                                Box::new(equals),
                            );
                        }
                        any_index_valid = ast::Expr::Bin(
                            ast::BinOpSrc::LogOr,
                            Box::new(any_index_valid),
                            Box::new(candidate_valid.clone()),
                        );
                        selected_element = ast::Expr::Cond(
                            Box::new(candidate_valid),
                            Box::new(aggregate_types.array_values[name][physical_element].clone()),
                            Box::new(selected_element),
                        );
                    }
                    array_index_valid = Some(any_index_valid);
                    selected_element
                };
                let element_width = array.element_width;
                let reversed = array.element_msb < array.element_lsb;
                let logical_base = if reversed {
                    array.element_msb
                } else {
                    array.element_lsb
                };
                let normalized_element = if reversed {
                    Self::reverse_ast_bits(element, element_width)
                } else {
                    element
                };

                let start =
                    self.resolve_aggregate_references(start, values, aggregate_types, env)?;
                let width =
                    self.resolve_aggregate_references(width, values, aggregate_types, env)?;
                let width = self.const_eval(&width, env.params, env.module_name)?;
                if width <= 0 {
                    return Err(format!(
                        "indexed part-select width must be positive, got {width}"
                    ));
                }
                let width = width as usize;
                if width > element_width {
                    return Err(format!(
                        "indexed part-select width {width} exceeds array element width {element_width}"
                    ));
                }
                let logical_start = if *ascending {
                    start
                } else {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(start),
                        Box::new(constant_expr(width as i64 - 1)),
                    )
                };
                let offset = ast::Expr::Bin(
                    ast::BinOpSrc::Sub,
                    Box::new(logical_start),
                    Box::new(constant_expr(logical_base)),
                );
                if let Ok(logical_start) = self.const_eval(&offset, env.params, env.module_name) {
                    if logical_start < 0 || logical_start + width as i64 > element_width as i64 {
                        return Err(format!(
                            "indexed part-select [{logical_start}+:{}] is out of array-element range",
                            width
                        ));
                    }
                    let selected = ast::Expr::Slice {
                        value: Box::new(normalized_element),
                        msb: Box::new(constant_expr(logical_start + width as i64 - 1)),
                        lsb: Box::new(constant_expr(logical_start)),
                    };
                    return Ok(if let Some(valid) = array_index_valid {
                        ast::Expr::Cond(
                            Box::new(valid),
                            Box::new(selected),
                            Box::new(ast::Expr::Num {
                                width: Some(width),
                                word: LogicWord::xes(width),
                                orig: "function_runtime_array_index_x".to_string(),
                            }),
                        )
                    } else {
                        selected
                    });
                }

                let max_offset = element_width - width;
                let in_range = ast::Expr::Bin(
                    ast::BinOpSrc::LogAnd,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Ge,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(0)),
                    )),
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Le,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(max_offset as i64)),
                    )),
                );
                let selected = ast::Expr::Bin(
                    ast::BinOpSrc::And,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Shr,
                        Box::new(normalized_element),
                        Box::new(offset),
                    )),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::ones(width),
                        orig: "function_array_runtime_mask".to_string(),
                    }),
                );
                let packed_result = ast::Expr::Cond(
                    Box::new(in_range),
                    Box::new(selected),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::xes(width),
                        orig: "function_array_runtime_x".to_string(),
                    }),
                );
                return Ok(if let Some(valid) = array_index_valid {
                    ast::Expr::Cond(
                        Box::new(valid),
                        Box::new(packed_result),
                        Box::new(ast::Expr::Num {
                            width: Some(width),
                            word: LogicWord::xes(width),
                            orig: "function_runtime_array_index_x".to_string(),
                        }),
                    )
                } else {
                    packed_result
                });
            }
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => {
                if aggregate_types.arrays.contains_key(name) {
                    return Err(format!(
                        "indexed part-select is not supported on function array '{}'",
                        name
                    ));
                }
                let start =
                    self.resolve_aggregate_references(start, values, aggregate_types, env)?;
                let width =
                    self.resolve_aggregate_references(width, values, aggregate_types, env)?;
                let width = self.const_eval(&width, env.params, env.module_name)?;
                if width <= 0 {
                    return Err(format!(
                        "indexed part-select width must be positive, got {width}"
                    ));
                }
                let width = width as usize;
                let value = values
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("indexed part-select uses unknown local '{}'", name))?;
                let value_width = *aggregate_types.scalar_widths.get(name).ok_or_else(|| {
                    format!("indexed part-select local '{}' has unknown width", name)
                })?;
                if width > value_width {
                    return Err(format!(
                        "indexed part-select width {width} exceeds local width {value_width}"
                    ));
                }
                let (logical_msb, logical_lsb) = aggregate_types
                    .scalar_ranges
                    .get(name)
                    .copied()
                    .unwrap_or((value_width as i64 - 1, 0));
                let reversed = logical_msb < logical_lsb;
                let normalized_value = if reversed {
                    let mut physical_parts = Vec::with_capacity(value_width);
                    for physical_bit in 0..value_width {
                        physical_parts.push(ast::Expr::Slice {
                            value: Box::new(value.clone()),
                            msb: Box::new(constant_expr(physical_bit as i64)),
                            lsb: Box::new(constant_expr(physical_bit as i64)),
                        });
                    }
                    ast::Expr::Concat(physical_parts)
                } else {
                    value
                };
                if let Ok(start) = self.const_eval(&start, env.params, env.module_name) {
                    let logical_start = if *ascending {
                        start
                    } else {
                        start - width as i64 + 1
                    };
                    let normalized_start = if reversed {
                        logical_start - logical_msb
                    } else {
                        logical_start
                    };
                    return Ok(ast::Expr::Slice {
                        value: Box::new(normalized_value),
                        msb: Box::new(constant_expr(normalized_start + width as i64 - 1)),
                        lsb: Box::new(constant_expr(normalized_start)),
                    });
                }

                let offset = if *ascending {
                    start
                } else {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(start),
                        Box::new(constant_expr(width as i64 - 1)),
                    )
                };
                let offset = if reversed {
                    ast::Expr::Bin(
                        ast::BinOpSrc::Sub,
                        Box::new(offset),
                        Box::new(constant_expr(logical_msb)),
                    )
                } else {
                    offset
                };
                let max_offset = value_width - width;
                let in_range = ast::Expr::Bin(
                    ast::BinOpSrc::LogAnd,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Ge,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(0)),
                    )),
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Le,
                        Box::new(offset.clone()),
                        Box::new(constant_expr(max_offset as i64)),
                    )),
                );
                let selected = ast::Expr::Bin(
                    ast::BinOpSrc::And,
                    Box::new(ast::Expr::Bin(
                        ast::BinOpSrc::Shr,
                        Box::new(normalized_value),
                        Box::new(offset),
                    )),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::ones(width),
                        orig: "runtime_function_indexed_mask".to_string(),
                    }),
                );
                ast::Expr::Cond(
                    Box::new(in_range),
                    Box::new(selected),
                    Box::new(ast::Expr::Num {
                        width: Some(width),
                        word: LogicWord::xes(width),
                        orig: "runtime_function_indexed_x".to_string(),
                    }),
                )
            }
            ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
                name.clone(),
                Box::new(self.resolve_aggregate_references(msb, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(lsb, values, aggregate_types, env)?),
            ),
            ast::Expr::Un(op, value) => ast::Expr::Un(
                *op,
                Box::new(self.resolve_aggregate_references(value, values, aggregate_types, env)?),
            ),
            ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
                *op,
                Box::new(self.resolve_aggregate_references(left, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(right, values, aggregate_types, env)?),
            ),
            ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
                Box::new(self.resolve_aggregate_references(
                    condition,
                    values,
                    aggregate_types,
                    env,
                )?),
                Box::new(self.resolve_aggregate_references(
                    true_value,
                    values,
                    aggregate_types,
                    env,
                )?),
                Box::new(self.resolve_aggregate_references(
                    false_value,
                    values,
                    aggregate_types,
                    env,
                )?),
            ),
            ast::Expr::Concat(parts) => ast::Expr::Concat(
                parts
                    .iter()
                    .map(|part| {
                        self.resolve_aggregate_references(part, values, aggregate_types, env)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
                Box::new(self.resolve_aggregate_references(count, values, aggregate_types, env)?),
                Box::new(self.resolve_aggregate_references(value, values, aggregate_types, env)?),
            ),
            ast::Expr::Call(name, args) => ast::Expr::Call(
                name.clone(),
                args.iter()
                    .map(|arg| self.resolve_aggregate_references(arg, values, aggregate_types, env))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
                value: Box::new(self.resolve_aggregate_references(
                    value,
                    values,
                    aggregate_types,
                    env,
                )?),
                msb: Box::new(self.resolve_aggregate_references(
                    msb,
                    values,
                    aggregate_types,
                    env,
                )?),
                lsb: Box::new(self.resolve_aggregate_references(
                    lsb,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
            ast::Expr::Resize { width, value } => ast::Expr::Resize {
                width: *width,
                value: Box::new(self.resolve_aggregate_references(
                    value,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => ast::Expr::WildcardMatch {
                kind: *kind,
                selector: Box::new(self.resolve_aggregate_references(
                    selector,
                    values,
                    aggregate_types,
                    env,
                )?),
                label: Box::new(self.resolve_aggregate_references(
                    label,
                    values,
                    aggregate_types,
                    env,
                )?),
            },
        })
    }

    pub(super) fn update_aggregate_field(
        &self,
        current: ast::Expr,
        env: &Env<'_>,
        type_name: &str,
        field: &str,
        value: ast::Expr,
    ) -> Result<ast::Expr, String> {
        let definition = env
            .types
            .get(type_name)
            .ok_or_else(|| format!("unknown aggregate type '{}'", type_name))?;
        let definition = match definition {
            ResolvedType::Struct(definition) | ResolvedType::Union(definition) => definition,
            ResolvedType::Enum(_) => {
                return Err(format!("type '{}' is not a packed struct/union", type_name));
            }
        };
        let &(offset, width) = definition
            .fields
            .get(field)
            .ok_or_else(|| format!("type '{}' has no field '{}'", type_name, field))?;
        let total_width = definition.width;
        let mut parts = Vec::new();
        if offset + width < total_width {
            parts.push(ast::Expr::Slice {
                value: Box::new(current.clone()),
                msb: Box::new(constant_expr((total_width - 1) as i64)),
                lsb: Box::new(constant_expr((offset + width) as i64)),
            });
        }
        parts.push(ast::Expr::Resize {
            width,
            value: Box::new(value),
        });
        if offset > 0 {
            parts.push(ast::Expr::Slice {
                value: Box::new(current),
                msb: Box::new(constant_expr((offset - 1) as i64)),
                lsb: Box::new(constant_expr(0)),
            });
        }
        Ok(ast::Expr::Concat(parts))
    }

    pub(super) fn merge_array_states(
        target: &mut FunctionAggregateState,
        mut then_state: HashMap<String, Vec<ast::Expr>>,
        else_state: HashMap<String, Vec<ast::Expr>>,
        condition: &ast::Expr,
    ) {
        let names = then_state
            .keys()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        for name in names {
            let Some(mut then_values) = then_state.remove(&name) else {
                continue;
            };
            let Some(else_values) = else_state.get(&name) else {
                continue;
            };
            for (then_value, else_value) in then_values.iter_mut().zip(else_values) {
                if *then_value != *else_value {
                    *then_value = ast::Expr::Cond(
                        Box::new(condition.clone()),
                        Box::new(then_value.clone()),
                        Box::new(else_value.clone()),
                    );
                }
            }
            target.array_values.insert(name, then_values);
        }
    }

    pub(super) fn local_zero(
        &self,
        range: &ast::RangeAst,
        env: &Env<'_>,
    ) -> Result<ast::Expr, String> {
        let width = match range {
            Some((msb, lsb)) => {
                let hi = self.const_eval(msb, env.params, env.module_name)?;
                let lo = self.const_eval(lsb, env.params, env.module_name)?;
                (hi - lo).unsigned_abs() as usize + 1
            }
            None => 1,
        };
        if width == 0 || width > 64 {
            return Err(format!(
                "function local variable width must be between 1 and 64, got {}",
                width
            ));
        }
        Ok(ast::Expr::Num {
            width: Some(width),
            word: LogicWord::from_u64(width, 0),
            orig: "0".to_string(),
        })
    }

    pub(super) fn inline_nested_function_calls(
        &self,
        expression: &ast::Expr,
        env: &Env<'_>,
        call_stack: &[String],
    ) -> Result<ast::Expr, String> {
        Ok(match expression {
            ast::Expr::Call(name, arguments) if name != "$signed" && name != "$unsigned" => {
                self.build_function_expression(name, arguments, env, call_stack)?
            }
            ast::Expr::Call(name, arguments) => ast::Expr::Call(
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Ident(_)
            | ast::Expr::ScopedIdent { .. }
            | ast::Expr::Num { .. }
            | ast::Expr::Str(_)
            | ast::Expr::Member(_, _)
            | ast::Expr::MemberChain(_) => expression.clone(),
            ast::Expr::Select(name, index) => ast::Expr::Select(
                name.clone(),
                Box::new(self.inline_nested_function_calls(index, env, call_stack)?),
            ),
            ast::Expr::ArraySelect(name, indexes) => ast::Expr::ArraySelect(
                name.clone(),
                indexes
                    .iter()
                    .map(|index| self.inline_nested_function_calls(index, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::ArrayIndexedPart {
                name,
                indexes,
                start,
                width,
                ascending,
            } => ast::Expr::ArrayIndexedPart {
                name: name.clone(),
                indexes: indexes
                    .iter()
                    .map(|index| self.inline_nested_function_calls(index, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
                start: Box::new(self.inline_nested_function_calls(start, env, call_stack)?),
                width: Box::new(self.inline_nested_function_calls(width, env, call_stack)?),
                ascending: *ascending,
            },
            ast::Expr::ArrayPart {
                name,
                indexes,
                msb,
                lsb,
            } => ast::Expr::ArrayPart {
                name: name.clone(),
                indexes: indexes
                    .iter()
                    .map(|index| self.inline_nested_function_calls(index, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
                msb: Box::new(self.inline_nested_function_calls(msb, env, call_stack)?),
                lsb: Box::new(self.inline_nested_function_calls(lsb, env, call_stack)?),
            },
            ast::Expr::Part(name, msb, lsb) => ast::Expr::Part(
                name.clone(),
                Box::new(self.inline_nested_function_calls(msb, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(lsb, env, call_stack)?),
            ),
            ast::Expr::IndexedPart {
                name,
                start,
                width,
                ascending,
            } => ast::Expr::IndexedPart {
                name: name.clone(),
                start: Box::new(self.inline_nested_function_calls(start, env, call_stack)?),
                width: Box::new(self.inline_nested_function_calls(width, env, call_stack)?),
                ascending: *ascending,
            },
            ast::Expr::Un(op, value) => ast::Expr::Un(
                *op,
                Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            ),
            ast::Expr::Bin(op, left, right) => ast::Expr::Bin(
                *op,
                Box::new(self.inline_nested_function_calls(left, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(right, env, call_stack)?),
            ),
            ast::Expr::Cond(condition, true_value, false_value) => ast::Expr::Cond(
                Box::new(self.inline_nested_function_calls(condition, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(true_value, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(false_value, env, call_stack)?),
            ),
            ast::Expr::Concat(parts) => ast::Expr::Concat(
                parts
                    .iter()
                    .map(|part| self.inline_nested_function_calls(part, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Replicate(count, value) => ast::Expr::Replicate(
                Box::new(self.inline_nested_function_calls(count, env, call_stack)?),
                Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            ),
            ast::Expr::Slice { value, msb, lsb } => ast::Expr::Slice {
                value: Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
                msb: Box::new(self.inline_nested_function_calls(msb, env, call_stack)?),
                lsb: Box::new(self.inline_nested_function_calls(lsb, env, call_stack)?),
            },
            ast::Expr::MethodCall {
                object,
                method,
                arguments,
            } => ast::Expr::MethodCall {
                object: object.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::MethodCallChain {
                path,
                method,
                arguments,
            } => ast::Expr::MethodCallChain {
                path: path.clone(),
                method: method.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            },
            ast::Expr::New(size) => ast::Expr::New(Box::new(
                self.inline_nested_function_calls(size, env, call_stack)?,
            )),
            ast::Expr::NewClass(arguments) => ast::Expr::NewClass(
                arguments
                    .iter()
                    .map(|argument| self.inline_nested_function_calls(argument, env, call_stack))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ast::Expr::Resize { width, value } => ast::Expr::Resize {
                width: *width,
                value: Box::new(self.inline_nested_function_calls(value, env, call_stack)?),
            },
            ast::Expr::WildcardMatch {
                kind,
                selector,
                label,
            } => ast::Expr::WildcardMatch {
                kind: *kind,
                selector: Box::new(self.inline_nested_function_calls(selector, env, call_stack)?),
                label: Box::new(self.inline_nested_function_calls(label, env, call_stack)?),
            },
        })
    }
}
