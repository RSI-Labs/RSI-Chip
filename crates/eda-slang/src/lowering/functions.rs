//! Straight-line function inlining from slang's elaborated AST.

use super::statements::event_array_condition;
use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{bool_field, expression_symbol, string_field};
use serde_json::{json, Value};

#[derive(Clone)]
struct TaskLocal {
    type_name: String,
    initializer: Option<Value>,
}

impl SlangAstLowerer {
    pub(super) fn collect_subroutine(&mut self, member: &Value) -> Result<(), String> {
        let name = declaration_name(member, "Subroutine")?;
        if self.subroutines.contains_key(&name) {
            return Err(format!("duplicate slang subroutine '{name}'"));
        }
        self.subroutines.insert(name, member.clone());
        Ok(())
    }

    pub(super) fn lower_function_call(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Err("function call has no subroutine symbol".into());
        };
        let function_name = subroutine
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        if matches!(function_name, "$time" | "$stime") {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if !arguments.is_empty() {
                return Err(format!("{function_name} does not take arguments"));
            }
            return Ok(if function_name == "$time" {
                eda_core::ir::Expr::SimulationTime {
                    width: 64,
                    signed: true,
                    unit_exponent: self.time_scale.normalized_unit_exponent(),
                }
            } else {
                eda_core::ir::Expr::SimulationTime {
                    width: 32,
                    signed: false,
                    unit_exponent: self.time_scale.normalized_unit_exponent(),
                }
            });
        }
        match subroutine.as_str() {
            "next" | "prev" => {
                return self.lower_enum_next_prev(value, subroutine == "next");
            }
            "first" | "last" => return self.lower_enum_first_last(value, subroutine == "first"),
            "num" => return self.lower_enum_num(value),
            "name" => return self.lower_enum_name(value),
            "len" => return self.lower_string_len(value),
            "getc" => return self.lower_string_getc(value),
            "toupper" | "tolower" => return self.lower_string_case(value, subroutine == "toupper"),
            "compare" | "icompare" => {
                return self.lower_string_compare(value, subroutine == "icompare")
            }
            "substr" => return self.lower_string_substr(value),
            "triggered" => return self.lower_event_triggered(value),
            "atoi" | "atohex" | "atobin" => {
                let radix = match subroutine.as_str() {
                    "atohex" => 16,
                    "atobin" => 2,
                    _ => 10,
                };
                return self.lower_string_to_radix(value, radix);
            }
            _ => {}
        }
        if let Some(expression) = self.lower_static_class_function_call(value)? {
            return Ok(expression);
        }
        if let Some(expression) = self.lower_class_function_call(value)? {
            return Ok(expression);
        }
        let subroutine_name = expression_symbol(value)
            .or_else(|| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .ok_or_else(|| "function call has no subroutine name".to_string())?;
        let definition = self
            .subroutines
            .get(&subroutine_name)
            .ok_or_else(|| format!("unknown direct-lowering function '{subroutine_name}'"))?;
        if string_field(definition, "subroutineKind").as_deref() != Some("Function") {
            return Err(format!(
                "direct slang-AST lowering does not support task call '{subroutine_name}'"
            ));
        }

        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let formals = definition
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("function '{subroutine_name}' has no formal arguments"))?;
        if arguments.len() != formals.len() {
            return Err(format!(
                "function '{subroutine_name}' expects {} arguments, found {}",
                formals.len(),
                arguments.len()
            ));
        }

        let mut environment = std::collections::BTreeMap::new();
        for (formal, actual) in formals.iter().zip(arguments) {
            let formal_name = formal
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("function '{subroutine_name}' has an unnamed argument"))?;
            let formal_type = formal.get("type").and_then(Value::as_str).ok_or_else(|| {
                format!("function argument '{formal_name}' has no elaborated type")
            })?;
            environment.insert(
                formal_name.to_string(),
                json!({
                    "kind": "Conversion",
                    "type": formal_type,
                    "operand": actual,
                }),
            );
        }
        load_variable_initializers(definition, &mut environment);

        let function_name = declaration_name(definition, "Subroutine")?;
        {
            let mut stack = self.inline_stack.borrow_mut();
            if stack.iter().any(|name| name == &function_name) {
                return Err(format!(
                    "recursive function '{function_name}' is not supported"
                ));
            }
            stack.push(function_name.clone());
        }

        let body = definition
            .get("body")
            .ok_or_else(|| format!("function '{function_name}' has no body"))?;
        let return_type = string_field(definition, "returnType")
            .ok_or_else(|| format!("function '{function_name}' has no return type"))?;
        let result = match self.inline_function_body(body, &mut environment, &function_name) {
            Ok(return_expression) => {
                let converted = json!({
                    "kind": "Conversion",
                    "type": return_type,
                    "operand": return_expression,
                });
                self.lower_expression(&converted)
            }
            Err(error) => Err(error),
        };
        self.inline_stack.borrow_mut().pop();
        result
    }

    pub(super) fn lower_task_call(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        let task_name = subroutine
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .to_string();
        if task_name == "$finish" {
            return Ok(Some(eda_core::ir::Stmt::Finish));
        }
        if task_name == "$cast" {
            return self.lower_class_cast_call(value);
        }
        if let Some(statement) = self.lower_uvm_timeout_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_factory_type_alias_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_factory_override_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_run_test_call(value)? {
            return Ok(Some(statement));
        }
        if matches!(
            task_name.as_str(),
            "$display"
                | "$displayb"
                | "$displayh"
                | "$write"
                | "$writeb"
                | "$writeh"
                | "$monitor"
                | "$monitorb"
                | "$monitorh"
        ) {
            return self.lower_display_call(value, &task_name).map(Some);
        }
        if task_name == "$timeformat" {
            return self.lower_timeformat_call(value).map(Some);
        }
        if task_name == "$printtimescale" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if !arguments.is_empty() {
                return Err("$printtimescale does not take arguments".into());
            }
            return Ok(Some(eda_core::ir::Stmt::Display {
                parts: vec![eda_core::ir::DispPart::Str(format!(
                    "time unit = {}, time precision = {}\n",
                    format_time_component(
                        self.time_scale.unit_magnitude,
                        self.time_scale.unit_exponent
                    ),
                    format_time_precision(self.time_scale.precision_exponent)
                ))],
                args: Vec::new(),
                monitor: false,
            }));
        }
        if task_name == "putc" {
            return self.lower_string_putc(value).map(Some);
        }
        if matches!(task_name.as_str(), "itoa" | "hextoa" | "bintoa") {
            let radix = match task_name.as_str() {
                "hextoa" => 16,
                "bintoa" => 2,
                _ => 10,
            };
            return self.lower_string_from_radix(value, radix).map(Some);
        }
        if let Some(statement) = self.lower_static_class_task_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_class_task_call(value)? {
            return Ok(Some(statement));
        }
        let Some(definition) = self.subroutines.get(&task_name) else {
            return Ok(None);
        };
        if string_field(definition, "subroutineKind").as_deref() != Some("Task") {
            return Ok(None);
        }

        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("task call '{task_name}' has no arguments"))?;
        let formals = definition
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("task '{task_name}' has no formal arguments"))?;
        if arguments.len() != formals.len() {
            return Err(format!(
                "task '{}' expects {} arguments, found {}",
                task_name,
                formals.len(),
                arguments.len()
            ));
        }

        let mut environment = std::collections::BTreeMap::new();
        let locals = collect_task_locals(definition);
        for (formal, actual) in formals.iter().zip(arguments) {
            let formal_name = formal
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("task '{task_name}' has an unnamed argument"))?;
            let direction = string_field(formal, "direction").unwrap_or_default();
            match direction.as_str() {
                "In" => {
                    let formal_type = formal.get("type").cloned().ok_or_else(|| {
                        format!("task argument '{formal_name}' has no elaborated type")
                    })?;
                    environment.insert(
                        formal_name.to_string(),
                        json!({
                            "kind": "Conversion",
                            "type": formal_type,
                            "operand": actual,
                        }),
                    );
                }
                "Out" | "InOut" | "Ref" => {
                    let actual_kind = string_field(actual, "kind").unwrap_or_default();
                    let target = if actual_kind == "Assignment" {
                        actual
                            .get("left")
                            .ok_or_else(|| format!("task argument '{formal_name}' has no target"))?
                    } else if direction == "Ref"
                        && matches!(
                            actual_kind.as_str(),
                            "NamedValue" | "ElementSelect" | "MemberAccess"
                        )
                    {
                        actual
                    } else {
                        return Err(format!(
                            "task '{task_name}' argument '{formal_name}' must be connected to an assignable target"
                        ));
                    };
                    let formal_type = formal.get("type").cloned().unwrap_or_default();
                    let target_type = target.get("type").cloned().unwrap_or_default();
                    if formal_type != target_type {
                        return Err(format!(
                            "task '{task_name}' argument '{formal_name}' type {formal_type} does not match target type {target_type}"
                        ));
                    }
                    if !matches!(direction.as_str(), "InOut" | "Ref")
                        && task_value_reads_formal(
                            definition
                                .get("body")
                                .ok_or_else(|| format!("task '{task_name}' has no body"))?,
                            formal_name,
                        )
                    {
                        return Err(format!(
                            "direct task inlining does not support reading output '{formal_name}'"
                        ));
                    }
                    environment.insert(formal_name.to_string(), target.clone());
                }
                other => {
                    return Err(format!(
                        "direct task inlining does not support argument direction '{other}'"
                    ));
                }
            }
        }
        self.initialize_task_locals(&locals, &mut environment)?;

        let body = definition
            .get("body")
            .ok_or_else(|| format!("task '{task_name}' has no body"))?;
        let body = self
            .inline_task_body(body, &mut environment, &locals)?
            .unwrap_or_else(|| json!({"kind": "Block", "blockKind": "Sequential"}));
        {
            let mut stack = self.inline_stack.borrow_mut();
            if stack.iter().any(|name| name == &task_name) {
                return Err(format!("recursive task '{task_name}' is not supported"));
            }
            stack.push(task_name.clone());
        }
        let result = self.lower_statement(&body);
        self.inline_stack.borrow_mut().pop();
        result.map(Some)
    }

    fn lower_display_call(
        &self,
        value: &Value,
        task_name: &str,
    ) -> Result<eda_core::ir::Stmt, String> {
        let default_spec = match task_name {
            "$displayb" | "$writeb" | "$monitorb" => 'b',
            "$displayh" | "$writeh" | "$monitorh" => 'h',
            _ => 'd',
        };
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let mut literal = String::new();
        let mut has_format_literal = false;
        let value_arguments = if let Some(first) = arguments.first() {
            if string_field(first, "kind").as_deref() == Some("StringLiteral") {
                has_format_literal = true;
                literal = string_field(first, "literal").unwrap_or_default();
                &arguments[1..]
            } else {
                arguments
            }
        } else {
            arguments
        };
        if !has_format_literal && !value_arguments.is_empty() {
            literal = value_arguments
                .iter()
                .map(|_| format!("%{default_spec}"))
                .collect::<Vec<_>>()
                .join(" ");
        }
        if !task_name.starts_with("$write") && !task_name.starts_with("$monitor") {
            literal.push('\n');
        }
        let parts = parse_display_format(&literal, value_arguments.len())?;
        let args = value_arguments
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(eda_core::ir::Stmt::Display {
            parts,
            args,
            monitor: task_name.starts_with("$monitor"),
        })
    }

    fn lower_timeformat_call(&self, value: &Value) -> Result<eda_core::ir::Stmt, String> {
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() > 4 {
            return Err("$timeformat supports at most four arguments".into());
        }
        let mut fields = [None, None, None, None];
        for (argument, field) in arguments.iter().zip(fields.iter_mut()) {
            *field = Some(argument);
        }

        let units = match fields[0] {
            Some(argument) => {
                let units = constant_ast_i64(argument)?;
                if !matches!(units, -15 | -12 | -9 | -6 | -3 | 0) {
                    return Err(format!("$timeformat has unsupported time unit '{units}'"));
                }
                i8::try_from(units)
                    .map_err(|_| format!("$timeformat unit '{units}' is out of range"))?
            }
            None => -9,
        };
        let precision = match fields[1] {
            Some(argument) => {
                let precision = constant_ast_i64(argument)?;
                if !(0..=9).contains(&precision) {
                    return Err(format!(
                        "$timeformat precision {precision} is outside 0..=9"
                    ));
                }
                u8::try_from(precision)
                    .map_err(|_| format!("$timeformat precision '{precision}' is invalid"))?
            }
            None => 0,
        };
        let suffix = match fields[2] {
            Some(argument) => timeformat_suffix(argument)?,
            None => String::new(),
        };
        let min_width = match fields[3] {
            Some(argument) => {
                let width = constant_ast_i64(argument)?;
                usize::try_from(width)
                    .map_err(|_| format!("$timeformat minimum width '{width}' is invalid"))?
            }
            None => 0,
        };
        Ok(eda_core::ir::Stmt::TimeFormat {
            units,
            precision,
            suffix,
            min_width,
        })
    }

    fn initialize_task_locals(
        &self,
        locals: &std::collections::BTreeMap<String, TaskLocal>,
        environment: &mut std::collections::BTreeMap<String, Value>,
    ) -> Result<(), String> {
        for (name, local) in locals {
            let (shape, _, _) = self.net_type(&local.type_name)?;
            let width = shape.width.max(1);
            let initializer = local.initializer.clone().unwrap_or_else(|| {
                let two_state = ["bit", "byte", "shortint", "int", "longint"]
                    .iter()
                    .any(|type_name| local.type_name.contains(type_name));
                let digits = if two_state {
                    format!("{width}'d0")
                } else {
                    format!("{width}'b{}", "x".repeat(width))
                };
                json!({
                    "kind": "IntegerLiteral",
                    "type": local.type_name,
                    "value": digits,
                })
            });
            environment.insert(
                name.clone(),
                json!({
                    "kind": "Conversion",
                    "type": local.type_name,
                    "operand": initializer,
                }),
            );
        }
        Ok(())
    }

    fn inline_task_body(
        &self,
        value: &Value,
        environment: &mut std::collections::BTreeMap<String, Value>,
        locals: &std::collections::BTreeMap<String, TaskLocal>,
    ) -> Result<Option<Value>, String> {
        match string_field(value, "kind").as_deref() {
            Some("List") => {
                let mut statements = Vec::new();
                if let Some(items) = value.get("list").and_then(Value::as_array) {
                    for item in items {
                        if let Some(statement) = self.inline_task_body(item, environment, locals)? {
                            statements.push(statement);
                        }
                    }
                }
                Ok(Some(json!({
                    "kind": "List",
                    "list": statements,
                })))
            }
            Some("Block") => {
                let Some(body) = value.get("body") else {
                    return Ok(Some(json!({
                        "kind": "Block",
                        "blockKind": "Sequential",
                    })));
                };
                self.inline_task_body(body, environment, locals)
            }
            Some("VariableDeclaration") => Ok(None),
            Some("ExpressionStatement") => {
                let Some(expression) = value.get("expr") else {
                    return Err("task expression statement has no expression".into());
                };
                let lowered =
                    self.inline_task_body(expression, environment, locals)?
                        .map(|expression| {
                            json!({
                                "kind": "ExpressionStatement",
                                "expr": expression,
                            })
                        });
                Ok(lowered)
            }
            Some("Assignment") => {
                let target = value
                    .get("left")
                    .and_then(expression_symbol)
                    .ok_or_else(|| "task assignment has no named target".to_string())?;
                let rhs = value
                    .get("right")
                    .ok_or_else(|| "task assignment has no right side".to_string())?;
                let rhs = substitute_named_values(rhs, environment);
                if let Some(local) = locals.get(&target) {
                    environment.insert(
                        target,
                        json!({
                            "kind": "Conversion",
                            "type": local.type_name,
                            "operand": rhs,
                        }),
                    );
                    return Ok(None);
                }
                let mut assignment = substitute_named_values(value, environment);
                if let Some(object) = assignment.get_mut("right") {
                    *object = rhs;
                }
                Ok(Some(assignment))
            }
            Some("Conditional") => {
                if string_field(value, "check").as_deref() != Some("None") {
                    return Err(
                        "direct task inlining does not support unique/priority conditionals".into(),
                    );
                }
                let conditions = value
                    .get("conditions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "task conditional has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err("direct task inlining supports one task condition".into());
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| "task conditional has no expression".to_string())?;
                let condition = substitute_named_values(condition, environment);
                let true_statement = value
                    .get("ifTrue")
                    .ok_or_else(|| "task conditional has no true branch".to_string())?;
                let mut true_environment = environment.clone();
                let true_body =
                    self.inline_task_body(true_statement, &mut true_environment, locals)?;
                let mut false_environment = environment.clone();
                let false_body = match value.get("ifFalse") {
                    Some(statement) => {
                        self.inline_task_body(statement, &mut false_environment, locals)?
                    }
                    None => None,
                };
                merge_conditional_environment(
                    environment,
                    true_environment,
                    false_environment,
                    condition.clone(),
                )?;
                if true_body.is_none() && false_body.is_none() {
                    return Ok(None);
                }
                let empty_block = json!({
                    "kind": "Block",
                    "blockKind": "Sequential",
                });
                Ok(Some(json!({
                    "kind": "Conditional",
                    "conditions": [{"expr": condition}],
                    "check": "None",
                    "ifTrue": true_body.unwrap_or_else(|| empty_block.clone()),
                    "ifFalse": false_body.unwrap_or(empty_block),
                })))
            }
            Some("ForLoop") => {
                const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
                let mut statements = Vec::new();
                if let Some(initializers) = value.get("initializers").and_then(Value::as_array) {
                    for initializer in initializers {
                        if let Some(statement) =
                            self.inline_task_body(initializer, environment, locals)?
                        {
                            statements.push(statement);
                        }
                    }
                }
                let stop = value
                    .get("stopExpr")
                    .ok_or_else(|| "task for-loop has no stop expression".to_string())?;
                let body = value
                    .get("body")
                    .ok_or_else(|| "task for-loop has no body".to_string())?;
                let steps = value
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "task for-loop has no steps".to_string())?;
                for _ in 0..=MAX_INLINE_TASK_LOOP_STEPS {
                    let stop = substitute_named_values(stop, environment);
                    let stop_value = constant_ast_i64(&stop)
                        .map_err(|_| "task for-loop stop must be an inline integer constant")?;
                    if stop_value == 0 {
                        return Ok(Some(json!({
                            "kind": "List",
                            "list": statements,
                        })));
                    }
                    if let Some(statement) = self.inline_task_body(body, environment, locals)? {
                        statements.push(statement);
                    }
                    for step in steps {
                        if let Some(statement) = self.inline_task_body(step, environment, locals)? {
                            statements.push(statement);
                        }
                    }
                }
                Err("task for-loop exceeds the 256-iteration inlining limit".into())
            }
            Some("WhileLoop") => {
                const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
                let condition = value
                    .get("cond")
                    .ok_or_else(|| "task while-loop has no condition".to_string())?;
                let body = value
                    .get("body")
                    .ok_or_else(|| "task while-loop has no body".to_string())?;
                let mut statements = Vec::new();
                for _ in 0..=MAX_INLINE_TASK_LOOP_STEPS {
                    let condition = substitute_named_values(condition, environment);
                    let condition_value = constant_ast_i64(&condition).map_err(|_| {
                        "task while-loop condition must be an inline integer constant"
                    })?;
                    if condition_value == 0 {
                        return Ok(Some(json!({
                            "kind": "List",
                            "list": statements,
                        })));
                    }
                    if let Some(statement) = self.inline_task_body(body, environment, locals)? {
                        statements.push(statement);
                    }
                }
                Err("task while-loop exceeds the 256-iteration inlining limit".into())
            }
            Some("RepeatLoop") => {
                const MAX_INLINE_TASK_LOOP_STEPS: usize = 256;
                let count = value
                    .get("count")
                    .ok_or_else(|| "task repeat-loop has no count".to_string())?;
                let count = substitute_named_values(count, environment);
                let count = constant_ast_i64(&count)
                    .map_err(|_| "task repeat-loop count must be an inline integer constant")?;
                let count = usize::try_from(count)
                    .map_err(|_| "task repeat-loop count is negative".to_string())?;
                if count > MAX_INLINE_TASK_LOOP_STEPS {
                    return Err("task repeat-loop exceeds the 256-iteration inlining limit".into());
                }
                let body = value
                    .get("body")
                    .ok_or_else(|| "task repeat-loop has no body".to_string())?;
                let mut statements = Vec::new();
                for _ in 0..count {
                    if let Some(statement) = self.inline_task_body(body, environment, locals)? {
                        statements.push(statement);
                    }
                }
                Ok(Some(json!({
                    "kind": "List",
                    "list": statements,
                })))
            }
            Some(kind) => {
                if task_contains_local_assignment(value, locals) {
                    return Err(format!(
                        "direct task inlining does not support local assignment inside '{kind}'"
                    ));
                }
                Ok(Some(substitute_named_values(value, environment)))
            }
            None => Err("task statement has no kind".into()),
        }
    }

    fn lower_enum_next_prev(
        &self,
        value: &Value,
        next: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "enum method call has no receiver".to_string())?;
        if arguments.is_empty() || arguments.len() > 2 {
            return Err("enum next/prev expects one receiver and at most one count".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "enum next/prev has no receiver".to_string())?;
        let receiver_type = string_field(receiver, "type").unwrap_or_default();
        let enum_name = receiver_type
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
            .next()
            .unwrap_or_default();
        let definition = self
            .type_enums
            .get(enum_name)
            .ok_or_else(|| format!("enum next/prev receiver is not an enum: '{receiver_type}'"))?;
        if definition.values.is_empty() {
            return Err("enum next/prev requires a nonempty enum".into());
        }

        let mut count = 1_i64;
        if let Some(count_value) = arguments.get(1) {
            count = constant_ast_i64(count_value)
                .map_err(|_| "enum next/prev count must be an inline integer constant")?;
        }
        if !next {
            count = -count;
        }
        let length = definition.values.len() as i64;
        let offset = count.rem_euclid(length);

        let receiver = self.lower_expression(receiver)?;
        let mut result = Expr::Const(eda_core::logic::LogicWord::xes(definition.width));
        for (index, current) in definition.values.iter().enumerate() {
            let target_index = (index as i64 + offset) % length;
            let target = &definition.values[target_index as usize];
            result = Expr::Cond {
                c: Box::new(Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(receiver.clone()),
                    b: Box::new(Expr::Const(current.clone())),
                }),
                t: Box::new(Expr::Const(target.clone())),
                f: Box::new(result),
            };
        }
        Ok(result)
    }

    fn lower_enum_first_last(
        &self,
        value: &Value,
        first: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        let definition = self.enum_method_definition(value)?;
        let value = if first {
            definition.values.first()
        } else {
            definition.values.last()
        }
        .ok_or_else(|| "enum first/last requires a nonempty enum".to_string())?;
        Ok(eda_core::ir::Expr::Const(value.clone()))
    }

    fn lower_enum_num(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let definition = self.enum_method_definition(value)?;
        let count = u64::try_from(definition.values.len())
            .map_err(|_| "enum num count is out of range".to_string())?;
        let word = eda_core::logic::LogicWord::from_u64(32, count);
        Ok(eda_core::ir::Expr::Signed {
            e: Box::new(eda_core::ir::Expr::Const(word)),
            signed: true,
        })
    }

    fn lower_enum_name(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let definition = self.enum_method_definition(value)?;
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "enum name call has no receiver".to_string())?;
        let receiver = arguments
            .first()
            .ok_or_else(|| "enum name call has no receiver".to_string())?;
        let receiver = self.lower_expression(receiver)?;
        let mut result = Expr::Const(eda_core::logic::LogicWord::zero(crate::types::STRING_WIDTH));
        for (value, name) in definition.values.iter().zip(&definition.names) {
            let rendered = Expr::Const(eda_core::logic::LogicWord::from_bytes_le(
                crate::types::STRING_WIDTH,
                name.as_bytes(),
            ));
            result = Expr::Cond {
                c: Box::new(Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(receiver.clone()),
                    b: Box::new(Expr::Const(value.clone())),
                }),
                t: Box::new(rendered),
                f: Box::new(result),
            };
        }
        Ok(result)
    }

    fn lower_string_len(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string len call has no receiver".to_string())?;
        if arguments.len() != 1 {
            return Err("string len expects one string receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string len call has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string len receiver is not a string".into());
        }
        let receiver = self.lower_expression(receiver)?;
        self.lower_string_length(receiver)
    }

    fn lower_string_getc(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string getc call has no arguments".to_string())?;
        if arguments.len() != 2 {
            return Err("string getc expects a string receiver and one index".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string getc call has no receiver".to_string())?;
        let index = arguments
            .get(1)
            .ok_or_else(|| "string getc call has no index".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string getc receiver is not a string".into());
        }
        self.lower_string_get_byte(receiver, index)
    }

    fn lower_string_case(&self, value: &Value, upper: bool) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string case call has no receiver".to_string())?;
        if arguments.len() != 1 {
            return Err("string case conversion expects one string receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string case call has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string case receiver is not a string".into());
        }
        let receiver = self.lower_expression(receiver)?;
        if receiver.width() != crate::types::STRING_WIDTH {
            return Err("string case conversion requires a 512-bit string".into());
        }
        Ok(eda_core::ir::Expr::StringCase {
            e: Box::new(receiver),
            upper,
        })
    }

    fn lower_string_compare(
        &self,
        value: &Value,
        ignore_case: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string compare call has no arguments".to_string())?;
        if arguments.len() != 2 {
            return Err("string compare expects two string receivers".into());
        }
        let left = arguments
            .first()
            .ok_or_else(|| "string compare call has no left receiver".to_string())?;
        let right = arguments
            .get(1)
            .ok_or_else(|| "string compare call has no right receiver".to_string())?;
        self.lower_string_compare_operands(left, right, ignore_case)
    }

    fn lower_string_substr(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string substr call has no arguments".to_string())?;
        if arguments.len() != 3 {
            return Err("string substr expects a receiver and two bounds".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string substr has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string substr receiver is not a string".into());
        }
        let left = arguments
            .get(1)
            .ok_or_else(|| "string substr has no left bound".to_string())?;
        let right = arguments
            .get(2)
            .ok_or_else(|| "string substr has no right bound".to_string())?;
        let receiver = self.lower_expression(receiver)?;
        if receiver.width() != crate::types::STRING_WIDTH {
            return Err("string substr requires a 512-bit string".into());
        }
        let left = self.lower_expression(left)?;
        let right = self.lower_expression(right)?;
        if left.width() > 32 || right.width() > 32 {
            return Err("string substr bounds exceed 32 bits".into());
        }
        Ok(eda_core::ir::Expr::StringSubstr {
            e: Box::new(receiver),
            left: Box::new(left),
            right: Box::new(right),
        })
    }

    fn lower_string_putc(&self, value: &Value) -> Result<eda_core::ir::Stmt, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string putc call has no arguments".to_string())?;
        if arguments.len() != 3 {
            return Err("string putc expects receiver, index, and byte".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string putc has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string putc receiver is not a string".into());
        }
        let index = arguments
            .get(1)
            .ok_or_else(|| "string putc has no index".to_string())?;
        let byte = arguments
            .get(2)
            .ok_or_else(|| "string putc has no byte".to_string())?;
        let lv = self.lower_lvalue(receiver)?;
        let receiver = self.lower_expression(receiver)?;
        if receiver.width() != crate::types::STRING_WIDTH {
            return Err("string putc requires a 512-bit string".into());
        }
        let index = self.lower_expression(index)?;
        if index.width() > 32 {
            return Err("string putc index exceeds 32 bits".into());
        }
        let byte = self.lower_expression(byte)?;
        if byte.width() != 8 {
            return Err("string putc byte must be 8 bits".into());
        }
        Ok(eda_core::ir::Stmt::Bl {
            lv,
            e: eda_core::ir::Expr::StringPutc {
                e: Box::new(receiver),
                index: Box::new(index),
                value: Box::new(byte),
            },
        })
    }

    fn lower_string_to_radix(
        &self,
        value: &Value,
        radix: u32,
    ) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string radix call has no arguments".to_string())?;
        if arguments.len() != 1 {
            return Err("string radix conversion expects one string receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string radix conversion has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string radix conversion receiver is not a string".into());
        }
        let receiver = self.lower_expression(receiver)?;
        if receiver.width() != crate::types::STRING_WIDTH {
            return Err("string radix conversion requires a 512-bit string".into());
        }
        Ok(eda_core::ir::Expr::StringToRadix {
            e: Box::new(receiver),
            radix,
        })
    }

    fn lower_string_from_radix(
        &self,
        value: &Value,
        radix: u32,
    ) -> Result<eda_core::ir::Stmt, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "string radix formatting call has no arguments".to_string())?;
        if arguments.len() != 2 {
            return Err("string radix formatting expects receiver and integer".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "string radix formatting has no receiver".to_string())?;
        if string_field(receiver, "type").as_deref() != Some("string") {
            return Err("string radix formatting receiver is not a string".into());
        }
        let integer = arguments
            .get(1)
            .ok_or_else(|| "string radix formatting has no integer".to_string())?;
        let lv = self.lower_lvalue(receiver)?;
        let receiver = self.lower_expression(receiver)?;
        if receiver.width() != crate::types::STRING_WIDTH {
            return Err("string radix formatting requires a 512-bit string".into());
        }
        let integer = self.lower_expression(integer)?;
        if integer.width() > 64 {
            return Err("string radix formatting integer exceeds 64 bits".into());
        }
        Ok(eda_core::ir::Stmt::Bl {
            lv,
            e: eda_core::ir::Expr::StringFromRadix {
                value: Box::new(integer),
                radix,
            },
        })
    }

    fn lower_event_triggered(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "event triggered check has no receiver".to_string())?;
        if arguments.len() != 1 {
            return Err("event triggered check expects one event receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "event triggered check has no receiver".to_string())?;
        let event_type = receiver
            .get("type")
            .ok_or_else(|| "event triggered receiver has no type".to_string())?;
        let is_event = match event_type {
            Value::String(type_name) => type_name == "event",
            Value::Object(fields) => {
                fields.get("name").and_then(Value::as_str) == Some("event")
                    || fields.get("kind").and_then(Value::as_str) == Some("EventType")
            }
            _ => false,
        };
        if !is_event {
            return Err("event triggered receiver must be an event".into());
        }
        if let Some(selection) = self.event_array_selection(receiver)? {
            let candidates = self.event_array_candidates(&selection, &selection.selectors)?;
            let mut result = eda_core::ir::Expr::Const(eda_core::logic::LogicWord::xes(1));
            for candidate in candidates {
                result = eda_core::ir::Expr::Cond {
                    c: Box::new(event_array_condition(&candidate.conditions)),
                    t: Box::new(eda_core::ir::Expr::EventTriggered { net: candidate.net }),
                    f: Box::new(result),
                };
            }
            return Ok(result);
        }
        let lvalue = self.lower_lvalue(receiver)?;
        if lvalue.range.width() != 1 {
            return Err("event triggered receiver must be scalar".into());
        }
        let net = lvalue.net
            + match lvalue.range {
                eda_core::ir::Range::Bit(bit) => bit,
                _ => 0,
            };
        Ok(eda_core::ir::Expr::EventTriggered { net })
    }

    fn enum_method_definition(&self, value: &Value) -> Result<&super::AstEnumType, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "enum method call has no receiver".to_string())?;
        if arguments.len() != 1 {
            return Err("enum method expects one enum receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "enum method has no receiver".to_string())?;
        let receiver_type = string_field(receiver, "type").unwrap_or_default();
        let enum_name = receiver_type
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
            .next()
            .unwrap_or_default();
        self.type_enums
            .get(enum_name)
            .ok_or_else(|| format!("enum method receiver is not an enum: '{receiver_type}'"))
    }

    fn inline_function_body(
        &self,
        value: &Value,
        environment: &mut std::collections::BTreeMap<String, Value>,
        function_name: &str,
    ) -> Result<Value, String> {
        if let Some(result) = self.inline_function_statement(value, environment)? {
            return Ok(result);
        }
        environment.get(function_name).cloned().ok_or_else(|| {
            format!("function '{function_name}' has no return value on a control path")
        })
    }

    fn inline_function_statement(
        &self,
        value: &Value,
        environment: &mut std::collections::BTreeMap<String, Value>,
    ) -> Result<Option<Value>, String> {
        match string_field(value, "kind").as_deref() {
            Some("List") => {
                let Some(items) = value.get("list").and_then(Value::as_array) else {
                    return Err("function statement list has no items".into());
                };
                for item in items {
                    if let Some(result) = self.inline_function_statement(item, environment)? {
                        return Ok(Some(result));
                    }
                }
                Ok(None)
            }
            Some("Block") => {
                let Some(body) = value.get("body") else {
                    return Err("function block has no body".into());
                };
                self.inline_function_statement(body, environment)
            }
            Some("VariableDeclaration") => Ok(None),
            Some("ExpressionStatement") => {
                let Some(expression) = value.get("expr") else {
                    return Err("function expression statement has no expression".into());
                };
                self.inline_function_statement(expression, environment)
            }
            Some("Assignment") => {
                apply_function_assignment(value, environment)?;
                Ok(None)
            }
            Some("Conditional") => {
                if string_field(value, "check").as_deref() != Some("None") {
                    return Err(
                        "direct function inlining does not support unique/priority conditionals"
                            .into(),
                    );
                }
                let conditions = value
                    .get("conditions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "function conditional has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err(
                        "direct function inlining supports one unqualified condition".into(),
                    );
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| "function conditional has no expression".to_string())?;
                let condition = substitute_named_values(condition, environment);
                let true_statement = value
                    .get("ifTrue")
                    .ok_or_else(|| "function conditional has no true branch".to_string())?;
                let mut true_environment = environment.clone();
                let true_result =
                    self.inline_function_statement(true_statement, &mut true_environment)?;
                let mut false_environment = environment.clone();
                let false_result = match value.get("ifFalse") {
                    Some(statement) => {
                        self.inline_function_statement(statement, &mut false_environment)?
                    }
                    None => None,
                };

                match (true_result, false_result) {
                    (Some(true_value), Some(false_value)) => Ok(Some(json!({
                        "kind": "ConditionalOp",
                        "type": string_field(&true_value, "type").unwrap_or_default(),
                        "conditions": [{"expr": condition}],
                        "left": true_value,
                        "right": false_value,
                    }))),
                    (Some(_), None) | (None, Some(_)) => Err(
                        "direct function inlining does not support mixed return and fall-through branches"
                            .into(),
                    ),
                    (None, None) => {
                        merge_conditional_environment(
                            environment,
                            true_environment,
                            false_environment,
                            condition,
                        )?;
                        Ok(None)
                    }
                }
            }
            Some("Return") => {
                let expression = value
                    .get("expr")
                    .ok_or_else(|| "function return has no expression".to_string())?;
                Ok(Some(substitute_named_values(expression, environment)))
            }
            Some("ForLoop") => {
                const MAX_INLINE_FUNCTION_LOOP_STEPS: usize = 256;
                if let Some(initializers) = value.get("initializers").and_then(Value::as_array) {
                    for initializer in initializers {
                        self.inline_function_statement(initializer, environment)?;
                    }
                }
                let stop = value
                    .get("stopExpr")
                    .ok_or_else(|| "function for-loop has no stop expression".to_string())?;
                let body = value
                    .get("body")
                    .ok_or_else(|| "function for-loop has no body".to_string())?;
                let steps = value
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "function for-loop has no steps".to_string())?;
                for _ in 0..=MAX_INLINE_FUNCTION_LOOP_STEPS {
                    let stop = substitute_named_values(stop, environment);
                    let stop_value = constant_ast_i64(&stop)
                        .map_err(|_| "function for-loop stop must be an inline integer constant")?;
                    if stop_value == 0 {
                        return Ok(None);
                    }
                    if let Some(result) = self.inline_function_statement(body, environment)? {
                        return Ok(Some(result));
                    }
                    for step in steps {
                        self.inline_function_statement(step, environment)?;
                    }
                }
                Err("function for-loop exceeds the 256-iteration inlining limit".into())
            }
            Some("WhileLoop") => {
                const MAX_INLINE_FUNCTION_LOOP_STEPS: usize = 256;
                let condition = value
                    .get("cond")
                    .ok_or_else(|| "function while-loop has no condition".to_string())?;
                let body = value
                    .get("body")
                    .ok_or_else(|| "function while-loop has no body".to_string())?;
                for _ in 0..=MAX_INLINE_FUNCTION_LOOP_STEPS {
                    let condition = substitute_named_values(condition, environment);
                    let condition_value = constant_ast_i64(&condition).map_err(|_| {
                        "function while-loop condition must be an inline integer constant"
                    })?;
                    if condition_value == 0 {
                        return Ok(None);
                    }
                    if let Some(result) = self.inline_function_statement(body, environment)? {
                        return Ok(Some(result));
                    }
                }
                Err("function while-loop exceeds the 256-iteration inlining limit".into())
            }
            Some("RepeatLoop") => {
                const MAX_INLINE_FUNCTION_LOOP_STEPS: usize = 256;
                let count = value
                    .get("count")
                    .ok_or_else(|| "function repeat-loop has no count".to_string())?;
                let count = substitute_named_values(count, environment);
                let count = constant_ast_i64(&count)
                    .map_err(|_| "function repeat-loop count must be an inline integer constant")?;
                let count = usize::try_from(count)
                    .map_err(|_| "function repeat-loop count is negative".to_string())?;
                if count > MAX_INLINE_FUNCTION_LOOP_STEPS {
                    return Err(
                        "function repeat-loop exceeds the 256-iteration inlining limit".into(),
                    );
                }
                let body = value
                    .get("body")
                    .ok_or_else(|| "function repeat-loop has no body".to_string())?;
                for _ in 0..count {
                    if let Some(result) = self.inline_function_statement(body, environment)? {
                        return Ok(Some(result));
                    }
                }
                Ok(None)
            }
            Some(kind) => Err(format!(
                "direct function inlining does not support statement kind '{kind}'"
            )),
            None => Err("function statement has no kind".into()),
        }
    }
}

fn apply_function_assignment(
    value: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    let target = value
        .get("left")
        .and_then(expression_symbol)
        .ok_or_else(|| "function assignment has no named target".to_string())?;
    let rhs = value
        .get("right")
        .ok_or_else(|| "function assignment has no right side".to_string())?;
    let rhs = substitute_named_values(rhs, environment);
    environment.insert(target, rhs);
    Ok(())
}

fn load_variable_initializers(
    value: &Value,
    environment: &mut std::collections::BTreeMap<String, Value>,
) {
    if let Value::Object(fields) = value {
        if matches!(
            string_field(value, "kind").as_deref(),
            Some("Variable" | "StatementBlock")
        ) {
            if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
                if let Some(initializer) = value.get("initializer") {
                    environment.insert(name, substitute_named_values(initializer, environment));
                }
            }
        }
        for nested in fields.values() {
            load_variable_initializers(nested, environment);
        }
    } else if let Value::Array(values) = value {
        for nested in values {
            load_variable_initializers(nested, environment);
        }
    }
}

fn merge_conditional_environment(
    environment: &mut std::collections::BTreeMap<String, Value>,
    true_environment: std::collections::BTreeMap<String, Value>,
    false_environment: std::collections::BTreeMap<String, Value>,
    condition: Value,
) -> Result<(), String> {
    let mut names = std::collections::BTreeSet::new();
    names.extend(true_environment.keys().cloned());
    names.extend(false_environment.keys().cloned());
    for name in names {
        let (true_value, false_value) =
            match (true_environment.get(&name), false_environment.get(&name)) {
                (Some(true_value), Some(false_value)) => (true_value, false_value),
                _ => {
                    return Err(format!(
                        "function conditional does not define local '{name}' on both branches"
                    ))
                }
            };
        if true_value == false_value {
            environment.insert(name, true_value.clone());
        } else {
            environment.insert(
                name,
                json!({
                    "kind": "ConditionalOp",
                    "type": string_field(true_value, "type").unwrap_or_default(),
                    "conditions": [{"expr": condition.clone()}],
                    "left": true_value,
                    "right": false_value,
                }),
            );
        }
    }
    Ok(())
}

fn substitute_named_values(
    value: &Value,
    environment: &std::collections::BTreeMap<String, Value>,
) -> Value {
    match value {
        Value::Object(fields) => {
            if string_field(value, "kind").as_deref() == Some("NamedValue") {
                if let Some(name) = expression_symbol(value) {
                    if let Some(replacement) = environment.get(&name) {
                        return replacement.clone();
                    }
                }
            }
            fields
                .iter()
                .map(|(key, nested)| (key.clone(), substitute_named_values(nested, environment)))
                .collect::<serde_json::Map<String, Value>>()
                .into()
        }
        Value::Array(values) => values
            .iter()
            .map(|nested| substitute_named_values(nested, environment))
            .collect::<Vec<_>>()
            .into(),
        scalar => scalar.clone(),
    }
}

fn collect_task_locals(value: &Value) -> std::collections::BTreeMap<String, TaskLocal> {
    let mut locals = std::collections::BTreeMap::new();
    if let Value::Object(fields) = value {
        if string_field(value, "kind").as_deref() == Some("Variable") && !bool_field(value, "flags")
        {
            if let Some(name) = string_field(value, "name").filter(|name| !name.is_empty()) {
                let type_name = string_field(value, "type").unwrap_or_default();
                let initializer = value.get("initializer").cloned();
                locals.insert(
                    name,
                    TaskLocal {
                        type_name,
                        initializer,
                    },
                );
            }
        }
        for nested in fields.values() {
            locals.extend(collect_task_locals(nested));
        }
    } else if let Value::Array(values) = value {
        for nested in values {
            locals.extend(collect_task_locals(nested));
        }
    }
    locals
}

fn task_contains_local_assignment(
    value: &Value,
    locals: &std::collections::BTreeMap<String, TaskLocal>,
) -> bool {
    match value {
        Value::Object(fields) => {
            if string_field(value, "kind").as_deref() == Some("Assignment") {
                if let Some(target) = value.get("left").and_then(expression_symbol) {
                    if locals.contains_key(&target) {
                        return true;
                    }
                }
            }
            fields
                .values()
                .any(|nested| task_contains_local_assignment(nested, locals))
        }
        Value::Array(values) => values
            .iter()
            .any(|nested| task_contains_local_assignment(nested, locals)),
        _ => false,
    }
}

fn task_value_reads_formal(value: &Value, formal_name: &str) -> bool {
    match value {
        Value::Object(fields) => {
            if string_field(value, "kind").as_deref() == Some("Assignment") {
                if let Some(right) = value.get("right") {
                    if json_contains_named_value(right, formal_name) {
                        return true;
                    }
                }
                return fields
                    .iter()
                    .filter(|(key, _)| key.as_str() != "left" && key.as_str() != "right")
                    .any(|(_, nested)| task_value_reads_formal(nested, formal_name));
            }
            fields
                .values()
                .any(|nested| task_value_reads_formal(nested, formal_name))
        }
        Value::Array(values) => values
            .iter()
            .any(|nested| task_value_reads_formal(nested, formal_name)),
        _ => false,
    }
}

fn json_contains_named_value(value: &Value, name: &str) -> bool {
    match value {
        Value::Object(fields) => {
            if string_field(value, "kind").as_deref() == Some("NamedValue")
                && expression_symbol(value).as_deref() == Some(name)
            {
                return true;
            }
            fields
                .values()
                .any(|nested| json_contains_named_value(nested, name))
        }
        Value::Array(values) => values
            .iter()
            .any(|nested| json_contains_named_value(nested, name)),
        _ => false,
    }
}

fn parse_display_format(
    format: &str,
    argument_count: usize,
) -> Result<Vec<eda_core::ir::DispPart>, String> {
    let mut parts = Vec::new();
    let mut literal = String::new();
    let mut specifiers = 0_usize;
    let mut characters = format.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '%' {
            literal.push(character);
            continue;
        }
        if characters.peek() == Some(&'%') {
            characters.next();
            literal.push('%');
            continue;
        }
        let specifier = characters
            .next()
            .ok_or_else(|| "display format has a trailing '%'".to_string())?;
        match specifier {
            'b' | 'B' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 'b'));
                specifiers += 1;
            }
            'd' | 'D' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 'd'));
                specifiers += 1;
            }
            'h' | 'H' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 'h'));
                specifiers += 1;
            }
            's' | 'S' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 's'));
                specifiers += 1;
            }
            't' | 'T' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Time(specifiers));
                specifiers += 1;
            }
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support display specifier '%{other}'"
                ))
            }
        }
    }
    if !literal.is_empty() {
        parts.push(eda_core::ir::DispPart::Str(literal));
    }
    if specifiers != argument_count {
        return Err(format!(
            "display has {specifiers} format specifiers but {argument_count} arguments"
        ));
    }
    Ok(parts)
}

fn push_display_literal(parts: &mut Vec<eda_core::ir::DispPart>, literal: &mut String) {
    if !literal.is_empty() {
        parts.push(eda_core::ir::DispPart::Str(std::mem::take(literal)));
    }
}

fn timeformat_suffix(value: &Value) -> Result<String, String> {
    if string_field(value, "kind").as_deref() == Some("Conversion") {
        let operand = value
            .get("operand")
            .ok_or_else(|| "timeformat suffix conversion has no operand".to_string())?;
        return timeformat_suffix(operand);
    }
    if string_field(value, "kind").as_deref() == Some("StringLiteral") {
        return string_field(value, "literal")
            .ok_or_else(|| "timeformat suffix literal has no value".to_string());
    }
    Err("$timeformat suffix must be a string literal".into())
}

fn format_time_component(magnitude: u64, exponent: i8) -> String {
    let unit = match exponent {
        0 => "s",
        -3 => "ms",
        -6 => "us",
        -9 => "ns",
        -12 => "ps",
        -15 => "fs",
        _ => "s",
    };
    format!("{magnitude}{unit}")
}

fn format_time_precision(exponent: i8) -> String {
    let exponent = i16::from(exponent);
    let base = ((exponent - 2) / 3) * 3;
    let remainder = exponent - base;
    let magnitude = 10_u64
        .checked_pow(u32::try_from(remainder).unwrap_or(u32::MAX))
        .unwrap_or(1);
    format_time_component(magnitude, i8::try_from(base).unwrap_or(-9))
}

fn constant_ast_i64(value: &Value) -> Result<i64, String> {
    if let Some(word) = crate::ast_util::integer_literal_word(value)
        .or_else(|| crate::ast_util::constant_json_word(value))
    {
        return word_i64(&word, &string_field(value, "type").unwrap_or_default());
    }

    match string_field(value, "kind").as_deref() {
        Some("Conversion") => value
            .get("operand")
            .ok_or_else(|| "conversion constant has no operand".to_string())
            .and_then(constant_ast_i64),
        Some("UnaryOp") => {
            let operand = value
                .get("operand")
                .ok_or_else(|| "unary constant has no operand".to_string())
                .and_then(constant_ast_i64)?;
            match string_field(value, "op").as_deref() {
                Some("Plus") => Ok(operand),
                Some("Minus") => operand
                    .checked_neg()
                    .ok_or_else(|| "unary constant negation overflows".to_string()),
                _ => Err("unsupported unary constant operator".into()),
            }
        }
        Some("BinaryOp") => {
            let left = value
                .get("left")
                .ok_or_else(|| "binary constant has no left operand".to_string())
                .and_then(constant_ast_i64)?;
            let right = value
                .get("right")
                .ok_or_else(|| "binary constant has no right operand".to_string())
                .and_then(constant_ast_i64)?;
            let result = match string_field(value, "op").as_deref() {
                Some("Add") => left.checked_add(right),
                Some("Subtract") => left.checked_sub(right),
                Some("Multiply") => left.checked_mul(right),
                Some("Divide") if right != 0 => Some(left.wrapping_div(right)),
                Some("Mod") if right != 0 => Some(left.wrapping_rem(right)),
                Some("LessThan") => Some((left < right) as i64),
                Some("LessThanEqual") => Some((left <= right) as i64),
                Some("GreaterThan") => Some((left > right) as i64),
                Some("GreaterThanEqual") => Some((left >= right) as i64),
                Some("Equality") => Some((left == right) as i64),
                Some("Inequality") => Some((left != right) as i64),
                Some("LogicalAnd") => Some((left != 0 && right != 0) as i64),
                Some("LogicalOr") => Some((left != 0 || right != 0) as i64),
                _ => return Err("unsupported binary constant operator".into()),
            };
            result.ok_or_else(|| "binary constant operation overflows".to_string())
        }
        Some("ConditionalOp") => {
            let condition = value
                .get("conditions")
                .and_then(Value::as_array)
                .and_then(|conditions| conditions.first())
                .and_then(|condition| condition.get("expr"))
                .ok_or_else(|| "conditional constant has no condition".to_string())
                .and_then(constant_ast_i64)?;
            let selected = if condition != 0 { "left" } else { "right" };
            value
                .get(selected)
                .ok_or_else(|| "conditional constant has no selected value".to_string())
                .and_then(constant_ast_i64)
        }
        _ => Err("expression is not an inline integer constant".into()),
    }
}

fn word_i64(word: &eda_core::logic::LogicWord, type_name: &str) -> Result<i64, String> {
    if word.width() == 0 || word.width() > u64::BITS as usize {
        return Err("inline integer constant width is out of range".into());
    }
    if !word.is_fully_known() {
        return Err("inline integer constant is unknown".into());
    }
    let mut magnitude = 0_i128;
    for (bit, value) in word.bits.iter().enumerate() {
        if value.as_bool() == Some(true) {
            magnitude |= 1_i128 << bit;
        }
    }
    let signed = crate::types::simple_type_signed(type_name).unwrap_or(false);
    let sign_bit = 1_i128 << (word.width() - 1);
    let signed_value = if signed && magnitude & sign_bit != 0 {
        magnitude - (1_i128 << word.width())
    } else {
        magnitude
    };
    i64::try_from(signed_value).map_err(|_| "inline integer constant is out of range".to_string())
}
