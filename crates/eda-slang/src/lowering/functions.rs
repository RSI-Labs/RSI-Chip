//! Straight-line function inlining from slang's elaborated AST.

use super::statements::event_array_condition;
use super::{declaration_name, SlangAstLowerer};
use crate::ast_util::{
    bool_field, expression_symbol, string_field, string_literal, unwrap_conversion,
};
use crate::types::{memory_type, simple_type_shape};
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
        if function_name == "$test$plusargs" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$test$plusargs requires one string literal argument".into());
            }
            let name = string_literal(&arguments[0])
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "$test$plusargs requires a non-empty string literal".to_string())?;
            let symbol = self
                .plusargs
                .get(&name)
                .ok_or_else(|| format!("plusarg '{name}' was not prepared"))?;
            return Ok(eda_core::ir::Expr::Ref {
                net: symbol.base,
                decl_width: 1,
                range: eda_core::ir::Range::Bit(0),
                signed: false,
            });
        }
        if function_name == "$value$plusargs" {
            let plan = super::declarations::value_plusarg_plan(value)?;
            let symbol = self
                .plusargs
                .get(&plan.query)
                .ok_or_else(|| format!("plusarg '{}' was not prepared", plan.query))?;
            return Ok(eda_core::ir::Expr::Ref {
                net: symbol.base,
                decl_width: 1,
                range: eda_core::ir::Range::Bit(0),
                signed: false,
            });
        }
        if matches!(
            function_name,
            "$time" | "$stime" | "$realtime" | "$timeunit" | "$timeprecision"
        ) {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if !arguments.is_empty() {
                return Err(format!("{function_name} does not take arguments"));
            }
            if function_name == "$time" {
                return Ok(eda_core::ir::Expr::SimulationTime {
                    width: 64,
                    signed: true,
                    unit_exponent: self.time_scale.normalized_unit_exponent(),
                });
            }
            if function_name == "$stime" {
                return Ok(eda_core::ir::Expr::SimulationTime {
                    width: 32,
                    signed: false,
                    unit_exponent: self.time_scale.normalized_unit_exponent(),
                });
            }
            if function_name == "$realtime" {
                return Ok(eda_core::ir::Expr::RealTime {
                    unit_exponent: self.time_scale.normalized_unit_exponent(),
                });
            }
            if function_name == "$timeunit" {
                return Ok(signed_time_exponent_const(
                    self.time_scale.normalized_unit_exponent(),
                ));
            }
            return Ok(signed_time_exponent_const(self.global_time_precision));
        }
        if matches!(function_name, "$random" | "$urandom" | "$urandom_range") {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            let expected_count = if function_name == "$urandom_range" {
                if arguments.len() != 1 && arguments.len() != 2 {
                    return Err("$urandom_range requires one or two arguments".to_string());
                }
                arguments.len()
            } else if arguments.is_empty() {
                0
            } else {
                return Err(format!(
                    "seeded '{function_name}' is not supported because its inout seed update is not modeled"
                ));
            };
            let args = arguments
                .iter()
                .map(|argument| self.lower_expression(argument))
                .collect::<Result<Vec<_>, String>>()?;
            debug_assert_eq!(args.len(), expected_count);
            return Ok(eda_core::ir::Expr::Random {
                function: match function_name {
                    "$random" => eda_core::ir::RandomFunction::Random,
                    "$urandom" => eda_core::ir::RandomFunction::Urandom,
                    _ => eda_core::ir::RandomFunction::UrandomRange,
                },
                args,
                width: 32,
                signed: function_name == "$random",
            });
        }
        if function_name == "$sformatf" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            let Some(format_source) = arguments.first() else {
                return Err("$sformatf requires a format string".into());
            };
            let format = string_literal(unwrap_conversion(format_source))
                .ok_or_else(|| "$sformatf format must be a string literal".to_string())?;
            let value_arguments = &arguments[1..];
            // Raw `%u/%z` are valid in a string result: the simulator removes
            // NUL bytes while packing the bounded string.
            let parts = parse_display_format(
                &format,
                value_arguments.len(),
                true,
                &self.current_format_hierarchy_path(),
                &self.current_format_library_scope(),
                value_arguments,
            )?;
            let args = self.lower_display_arguments(&parts, value_arguments)?;
            return Ok(eda_core::ir::Expr::StringFormat {
                format,
                parts,
                args,
            });
        }
        if function_name == "$fopen" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 2 {
                return Err("$fopen requires a path and read/write/append mode".to_string());
            }
            let literal_path = string_literal(unwrap_conversion(&arguments[0]));
            let literal_mode = string_literal(unwrap_conversion(&arguments[1]));
            if let (Some(path), Some(mode)) = (literal_path, literal_mode) {
                let path = if path.is_empty() {
                    return Err("$fopen paths must be nonempty strings".to_string());
                } else {
                    path
                };
                let (append, read, truncate) = match mode.as_str() {
                    "r" | "rb" => (false, true, false),
                    "r+" | "r+b" | "rb+" => (true, true, false),
                    "w" | "wb" => (false, false, true),
                    "w+" | "w+b" | "wb+" => (false, true, true),
                    "a" | "ab" => (true, false, false),
                    "a+" | "a+b" | "ab+" => (true, true, true),
                    other => {
                        return Err(format!(
                            "$fopen mode '{other}' is unsupported; only checked text/binary fopen modes execute"
                        ));
                    }
                };
                return Ok(eda_core::ir::Expr::FileOpen {
                    path,
                    append,
                    read,
                    truncate,
                });
            }
            let path = self.lower_expression(&arguments[0])?;
            let mode = self.lower_expression(&arguments[1])?;
            if path.width() != crate::types::STRING_WIDTH
                || mode.width() != crate::types::STRING_WIDTH
            {
                return Err(
                    "dynamic $fopen path and mode must be 512-bit string expressions".to_string(),
                );
            }
            return Ok(eda_core::ir::Expr::FileOpenDynamic {
                path: Box::new(path),
                mode: Box::new(mode),
            });
        }
        if function_name == "$fgetc" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$fgetc requires exactly one file descriptor".to_string());
            }
            return Ok(eda_core::ir::Expr::FileGetc {
                fd: Box::new(self.lower_expression(&arguments[0])?),
            });
        }
        if function_name == "$ungetc" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 2 {
                return Err("$ungetc requires a byte value and file descriptor".to_string());
            }
            return Ok(eda_core::ir::Expr::FileUngetc {
                value: Box::new(self.lower_expression(&arguments[0])?),
                fd: Box::new(self.lower_expression(&arguments[1])?),
            });
        }
        if function_name == "$feof" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$feof requires exactly one file descriptor".to_string());
            }
            return Ok(eda_core::ir::Expr::FileEof {
                fd: Box::new(self.lower_expression(&arguments[0])?),
            });
        }
        if function_name == "$ftell" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$ftell requires exactly one file descriptor".to_string());
            }
            return Ok(eda_core::ir::Expr::FileTell {
                fd: Box::new(self.lower_expression(&arguments[0])?),
            });
        }
        if function_name == "$fseek" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 3 {
                return Err("$fseek requires descriptor, offset, and whence".to_string());
            }
            return Ok(eda_core::ir::Expr::FileSeek {
                fd: Box::new(self.lower_expression(&arguments[0])?),
                offset: Box::new(self.lower_expression(&arguments[1])?),
                whence: Box::new(self.lower_expression(&arguments[2])?),
            });
        }
        if function_name == "$rewind" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$rewind requires exactly one file descriptor".to_string());
            }
            return Ok(eda_core::ir::Expr::FileSeek {
                fd: Box::new(self.lower_expression(&arguments[0])?),
                offset: Box::new(eda_core::ir::Expr::Const(eda_core::logic::LogicWord::zero(
                    32,
                ))),
                whence: Box::new(eda_core::ir::Expr::Const(eda_core::logic::LogicWord::zero(
                    32,
                ))),
            });
        }
        if function_name == "$size" {
            return self.lower_fixed_array_size(value);
        }
        if matches!(
            function_name,
            "$left" | "$right" | "$low" | "$high" | "$increment"
        ) {
            return self.lower_fixed_array_bound(value, function_name);
        }
        if matches!(function_name, "$dimensions" | "$unpacked_dimensions") {
            return self.lower_array_dimension_count(value, function_name);
        }
        if function_name == "$bits" {
            return self.lower_bits(value);
        }
        if subroutine == "size" {
            if let Some(expression) = self.lower_dynamic_array_size(value)? {
                return Ok(expression);
            }
        }
        if subroutine == "num" {
            if let Some(expression) = self.lower_associative_array_num(value)? {
                return Ok(expression);
            }
        }
        if subroutine == "exists" {
            if let Some(expression) = self.lower_associative_array_exists(value)? {
                return Ok(expression);
            }
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
        if self.uvm_config_db_type_name(value, "get").is_some() {
            return Err(
                "direct UVM config DB get is supported only as a conditional expression".into(),
            );
        }
        if self.uvm_config_db_type_name(value, "exists").is_some() {
            return Err(
                "direct UVM config DB exists is supported only as a conditional expression".into(),
            );
        }
        if self
            .uvm_resource_db_type_name(value, "read_by_name")
            .is_some()
        {
            return Err(
                "direct UVM resource DB read_by_name is supported only as a conditional expression"
                    .into(),
            );
        }
        if self
            .uvm_resource_db_type_name(value, "write_by_name")
            .is_some()
        {
            return Err(
                "direct UVM resource DB write_by_name is supported only as a conditional expression"
                    .into(),
            );
        }
        for method_name in ["read_by_type", "write_by_type"] {
            if self.uvm_resource_db_type_name(value, method_name).is_some() {
                return Err(format!(
                    "direct UVM resource DB {method_name} is supported only as a conditional expression"
                ));
            }
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
        if task_name == "delete" {
            if let Some(statement) = self.lower_associative_array_delete(value)? {
                return Ok(Some(statement));
            }
            if let Some(statement) = self.lower_dynamic_array_delete(value)? {
                return Ok(Some(statement));
            }
            if let Some(statement) = self.lower_queue_delete(value)? {
                return Ok(Some(statement));
            }
        }
        if matches!(task_name.as_str(), "push_back" | "push_front") {
            if let Some(statement) = self.lower_queue_push(value, task_name == "push_front")? {
                return Ok(Some(statement));
            }
        }
        if task_name == "insert" {
            if let Some(statement) = self.lower_queue_insert(value)? {
                return Ok(Some(statement));
            }
        }
        if task_name == "$finish" {
            return Ok(Some(eda_core::ir::Stmt::Finish));
        }
        if task_name == "$stop" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if !arguments.is_empty() {
                return Err("argument-bearing '$stop' is not supported".into());
            }
            return Ok(Some(eda_core::ir::Stmt::Stop));
        }
        if matches!(
            task_name.as_str(),
            "$asserton" | "$assertoff" | "$assertkill"
        ) {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if !arguments.is_empty() {
                return Err(format!(
                    "argument-scoped '{task_name}' is not supported; use the no-argument global form"
                ));
            }
            return Ok(Some(eda_core::ir::Stmt::AssertControl(
                match task_name.as_str() {
                    "$asserton" => eda_core::ir::AssertControl::On,
                    "$assertoff" => eda_core::ir::AssertControl::Off,
                    _ => eda_core::ir::AssertControl::Kill,
                },
            )));
        }
        if task_name == "$cast" {
            return self.lower_class_cast_call(value);
        }
        if let Some(statement) = self.lower_uvm_super_phase_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_timeout_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_report_catcher_registration(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_report_catcher_delete_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_report_catcher_delete_by_name_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_report_catcher_mode_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_callback_registration(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_callback_mode_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_callback_delete_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_callback_delete_by_name_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_report_dispatch(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_factory_type_alias_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_factory_instance_alias_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_factory_override_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_config_wait_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_config_set_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_resource_set_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_run_test_call(value)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_sequence_item_call(value, &task_name)? {
            return Ok(Some(statement));
        }
        if let Some(statement) = self.lower_uvm_sequence_response_call(value, &task_name)? {
            return Ok(Some(statement));
        }
        if task_name == "$fclose" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$fclose requires exactly one file descriptor".to_string());
            }
            return Ok(Some(eda_core::ir::Stmt::FileClose {
                fd: self.lower_expression(&arguments[0])?,
            }));
        }
        if task_name == "$fflush" {
            let empty_arguments = Vec::new();
            let arguments = value
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("$fflush requires exactly one file descriptor".to_string());
            }
            return Ok(Some(eda_core::ir::Stmt::FileFlush {
                fd: self.lower_expression(&arguments[0])?,
            }));
        }
        if matches!(
            task_name.as_str(),
            "$fdisplay"
                | "$fdisplayo"
                | "$fdisplayb"
                | "$fdisplayh"
                | "$fwrite"
                | "$fwriteo"
                | "$fwriteb"
                | "$fwriteh"
                | "$fstrobe"
                | "$fstrobeo"
                | "$fstrobeb"
                | "$fstrobeh"
        ) {
            return self.lower_file_display_call(value, &task_name).map(Some);
        }
        if matches!(
            task_name.as_str(),
            "$display"
                | "$displayo"
                | "$displayb"
                | "$displayh"
                | "$write"
                | "$writeo"
                | "$writeb"
                | "$writeh"
                | "$monitor"
                | "$monitoro"
                | "$monitorb"
                | "$monitorh"
                | "$strobe"
                | "$strobeo"
                | "$strobeb"
                | "$strobeh"
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
                strobe: false,
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
        let return_type = definition.get("returnType");
        let returns_void = return_type
            .and_then(Value::as_str)
            .is_some_and(|type_name| type_name == "void")
            || return_type
                .and_then(|type_name| type_name.get("kind"))
                .and_then(Value::as_str)
                == Some("VoidType");
        let is_task = string_field(definition, "subroutineKind").as_deref() == Some("Task");
        let is_void_function = string_field(definition, "subroutineKind").as_deref()
            == Some("Function")
            && returns_void;
        if !is_task && !is_void_function {
            return Ok(None);
        }

        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let arguments = arguments.as_slice();
        let formals = definition
            .get("arguments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let formals = formals.as_slice();
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
        result.map(|statement| Some(eda_core::ir::Stmt::Scope(Box::new(statement))))
    }

    fn lower_file_display_call(
        &self,
        value: &Value,
        task_name: &str,
    ) -> Result<eda_core::ir::Stmt, String> {
        let default_spec = match task_name {
            "$fdisplayo" | "$fwriteo" | "$fstrobeo" => 'o',
            "$fdisplayb" | "$fwriteb" | "$fstrobeb" => 'b',
            "$fdisplayh" | "$fwriteh" | "$fstrobeh" => 'h',
            _ => 'd',
        };
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() {
            return Err(format!("{task_name} requires a file descriptor"));
        }
        let fd = self.lower_expression(&arguments[0])?;
        let value_arguments = &arguments[1..];
        let mut literal = String::new();
        let has_format_literal = value_arguments
            .first()
            .and_then(|first| string_field(first, "literal"))
            .is_some_and(|format| {
                literal = format.to_string();
                true
            });
        let value_arguments = if has_format_literal {
            &value_arguments[1..]
        } else {
            value_arguments
        };
        if !has_format_literal && !value_arguments.is_empty() {
            literal = value_arguments
                .iter()
                .map(|_| format!("%{default_spec}"))
                .collect::<Vec<_>>()
                .join(" ");
        }
        let parts = parse_display_format(
            &literal,
            value_arguments.len(),
            true,
            &self.current_format_hierarchy_path(),
            &self.current_format_library_scope(),
            value_arguments,
        )?;
        let args = self.lower_display_arguments(&parts, value_arguments)?;
        Ok(eda_core::ir::Stmt::FileDisplay {
            fd,
            parts,
            args,
            newline: !task_name.starts_with("$fwrite"),
            strobe: task_name.starts_with("$fstrobe"),
        })
    }

    fn lower_display_call(
        &self,
        value: &Value,
        task_name: &str,
    ) -> Result<eda_core::ir::Stmt, String> {
        let default_spec = match task_name {
            "$displayo" | "$writeo" | "$monitoro" | "$strobeo" => 'o',
            "$displayb" | "$writeb" | "$monitorb" | "$strobeb" => 'b',
            "$displayh" | "$writeh" | "$monitorh" | "$strobeh" => 'h',
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
        let parts = parse_display_format(
            &literal,
            value_arguments.len(),
            false,
            &self.current_format_hierarchy_path(),
            &self.current_format_library_scope(),
            value_arguments,
        )?;
        let args = self.lower_display_arguments(&parts, value_arguments)?;
        Ok(eda_core::ir::Stmt::Display {
            parts,
            args,
            monitor: task_name.starts_with("$monitor"),
            strobe: task_name.starts_with("$strobe"),
        })
    }

    fn lower_display_arguments(
        &self,
        parts: &[eda_core::ir::DispPart],
        sources: &[Value],
    ) -> Result<Vec<eda_core::ir::Expr>, String> {
        sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let pattern_spec = parts.iter().find_map(|part| {
                    if let eda_core::ir::DispPart::Arg(part_index, spec) = part {
                        (*part_index == index).then_some(*spec)
                    } else if let eda_core::ir::DispPart::FormattedArg {
                        index: part_index,
                        spec,
                        ..
                    } = part
                    {
                        (*part_index == index).then_some(*spec)
                    } else {
                        None
                    }
                });
                if matches!(pattern_spec, Some('r' | 'R')) {
                    self.lower_memory_pattern(source, pattern_spec == Some('R'))
                } else if matches!(pattern_spec, Some('p' | 'P' | 'e' | 'E' | 'f' | 'g'))
                    && is_real_value(source)
                {
                    if string_field(unwrap_conversion(source), "kind").as_deref()
                        == Some("RealLiteral")
                    {
                        self.lower_real_pattern(source)
                    } else {
                        self.lower_expression(source)
                    }
                } else if matches!(pattern_spec, Some('e' | 'E')) {
                    let type_name =
                        string_field(unwrap_conversion(source), "type").unwrap_or_default();
                    if let Some(definition) = self.type_alias_definition(&type_name) {
                        if definition.is_union && !definition.is_unpacked {
                            return self.lower_expression(source);
                        }
                        self.lower_struct_pattern(source, pattern_spec == Some('E'))
                    } else {
                        self.lower_enum_pattern(source)
                    }
                } else {
                    self.lower_expression(source)
                }
            })
            .collect()
    }

    fn lower_memory_pattern(
        &self,
        source: &Value,
        abbreviated: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        let source = unwrap_conversion(source);
        let Some(name) = expression_symbol(source) else {
            return Err("%p array arguments must name a whole fixed array".into());
        };
        let Some(symbol) = self.symbols.get(&name) else {
            return Err(format!("unknown %p array argument '{name}'"));
        };
        let Some(memory) = symbol.memory else {
            return Err(format!("'{name}' is not an unpacked array"));
        };
        let declaration = &self.design.memories[memory];
        let runtime_sized = declaration.dynamic || declaration.queue;
        if declaration.flat_length > 4096 {
            return Err("%p arrays are currently limited to 4,096 elements".into());
        }
        if runtime_sized && declaration.dimensions.len() != 1 {
            return Err("%p dynamic arrays and queues must be one-dimensional".into());
        }
        if declaration.associative
            && (declaration.dimensions.len() != 1
                || declaration.index_width == 0
                || declaration.index_width > 64)
        {
            return Err(
                "%p associative arrays must be one-dimensional with scalar integral keys up to 64 bits"
                    .into(),
            );
        }
        let mut shape_length = 1_u64;
        for dimension in &declaration.dimensions {
            shape_length = shape_length
                .checked_mul(dimension.length as u64)
                .ok_or_else(|| "%p fixed-array shape is too large".to_string())?;
        }
        if shape_length != declaration.flat_length {
            return Err(format!(
                "%p fixed-array shape length {shape_length} does not match memory length {}",
                declaration.flat_length
            ));
        }
        let type_name = string_field(source, "type").unwrap_or_default();
        let element_type_name = type_name
            .split('$')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        let element = if let Some(definition) = self.type_alias_definition(&element_type_name) {
            if !definition.flat_pattern
                || definition.fields.is_empty()
                || definition.width != declaration.element_width
            {
                return Err(
                    "%p aggregate-array elements must be flat structs/unions with matching width"
                        .into(),
                );
            }
            if definition.is_union && !definition.is_unpacked {
                eda_core::ir::MemoryPatternElement::PackedUnion {
                    signed: definition.signed,
                }
            } else if definition.is_union {
                eda_core::ir::MemoryPatternElement::UnpackedUnion {
                    fields: self.lower_struct_pattern_fields(definition, 0)?,
                }
            } else {
                eda_core::ir::MemoryPatternElement::Struct {
                    fields: self.lower_struct_pattern_fields(definition, 0)?,
                }
            }
        } else {
            if declaration.element_width == 0 || declaration.element_width > 64 {
                return Err("%p array elements must be scalar integrals up to 64 bits".into());
            }
            eda_core::ir::MemoryPatternElement::Scalar {
                signed: symbol.signed,
            }
        };
        Ok(eda_core::ir::Expr::MemoryPattern {
            memory,
            element,
            abbreviated,
        })
    }

    pub(super) fn lower_real_pattern(&self, source: &Value) -> Result<eda_core::ir::Expr, String> {
        let source = unwrap_conversion(source);
        let real = source
            .get("value")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| "real %p literal has no numeric value".to_string())?;
        if !real.is_finite() {
            return Err("real %p literals must be finite".into());
        }
        Ok(eda_core::ir::Expr::RealConst(real))
    }

    fn lower_enum_pattern(&self, source: &Value) -> Result<eda_core::ir::Expr, String> {
        let type_name = string_field(unwrap_conversion(source), "type").unwrap_or_default();
        let enum_name = type_name
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .rsplit(['.', '#', ':'])
            .next()
            .unwrap_or_default();
        let definition = self.type_enums.get(enum_name).ok_or_else(|| {
            format!("$sformatf/display %p argument has unsupported non-enum type '{type_name}'")
        })?;
        let value = self.lower_expression(source)?;
        if value.width() != definition.width {
            return Err(format!(
                "enum %p argument width {} does not match enum '{enum_name}' width {}",
                value.width(),
                definition.width
            ));
        }
        Ok(eda_core::ir::Expr::EnumPattern {
            value: Box::new(value),
            values: definition.values.clone(),
            names: definition.names.clone(),
        })
    }

    fn lower_struct_pattern(
        &self,
        source: &Value,
        abbreviated: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        let type_name = string_field(unwrap_conversion(source), "type").unwrap_or_default();
        let definition = self.type_alias_definition(&type_name).ok_or_else(|| {
            format!("$sformatf/display %p argument has unsupported non-struct type '{type_name}'")
        })?;
        if definition.is_union && !definition.is_unpacked {
            let value = self.lower_expression(source)?;
            if value.width() != definition.width {
                return Err(format!(
                    "union %p argument width {} does not match type width {}",
                    value.width(),
                    definition.width
                ));
            }
            return Ok(value);
        }
        if !definition.flat_pattern {
            return Err(
                "$sformatf/display %p currently supports flat packed/unpacked structs, scalar-field unpacked unions, and scalar packed unions"
                    .into(),
            );
        }
        let value = self.lower_expression(source)?;
        if value.width() != definition.width {
            return Err(format!(
                "struct %p argument width {} does not match type width {}",
                value.width(),
                definition.width
            ));
        }
        let fields = self.lower_struct_pattern_fields(definition, 0)?;
        Ok(eda_core::ir::Expr::StructPattern {
            value: Box::new(value),
            fields,
            abbreviated,
        })
    }

    fn lower_struct_pattern_fields(
        &self,
        definition: &crate::lowering::AstStructType,
        base_offset: usize,
    ) -> Result<Vec<eda_core::ir::StructPatternField>, String> {
        let mut fields = definition
            .fields
            .iter()
            .map(|(name, field)| {
                (
                    field.field_index,
                    name.clone(),
                    field,
                    base_offset.checked_add(field.offset),
                )
            })
            .collect::<Vec<_>>();
        fields.sort_by_key(|(index, _, _, _)| *index);
        fields
            .into_iter()
            .map(|(_, name, field, offset)| {
                let offset = offset.ok_or_else(|| {
                    format!("struct %p field '{name}' has an invalid physical offset")
                })?;
                let value = if let Some(nested_type) = &field.nested_type {
                    let nested = match nested_type {
                        crate::lowering::AstNestedStruct::Alias(type_name) => {
                            self.type_alias_definition(type_name).ok_or_else(|| {
                                format!("struct %p field '{name}' has unknown nested type")
                            })?
                        }
                        crate::lowering::AstNestedStruct::Inline(definition) => definition,
                    };
                    if !nested.flat_pattern {
                        return Err(format!(
                            "struct %p field '{name}' has an unsupported nested aggregate"
                        ));
                    }
                    if nested.width != field.width {
                        return Err(format!(
                            "struct %p field '{name}' width {} does not match nested type width {}",
                            field.width, nested.width
                        ));
                    }
                    if nested.is_union && !nested.is_unpacked {
                        eda_core::ir::StructPatternValue::Scalar {
                            signed: nested.signed,
                        }
                    } else {
                        eda_core::ir::StructPatternValue::Struct {
                            fields: self.lower_struct_pattern_fields(nested, offset)?,
                        }
                    }
                } else {
                    eda_core::ir::StructPatternValue::Scalar {
                        signed: field.signed,
                    }
                };
                Ok(eda_core::ir::StructPatternField {
                    name,
                    offset,
                    width: field.width,
                    value,
                })
            })
            .collect()
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

    fn lower_fixed_array_size(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = fixed_array_arguments(value, "$size")?;
        if arguments.is_empty() || arguments.len() > 2 {
            return Err("$size expects one array argument and at most one dimension".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "$size has no array receiver".to_string())?;
        if let Some(name) = expression_symbol(receiver) {
            if let Some(symbol) = self.symbols.get(&name) {
                if let Some(memory) = symbol.memory {
                    if self.design.memories[memory].dynamic {
                        if arguments.len() != 1 {
                            return Err("$size does not take a dimension for dynamic arrays".into());
                        }
                        return Ok(eda_core::ir::Expr::MemorySize {
                            memory,
                            width: 32,
                            signed: true,
                        });
                    }
                }
            }
        }
        let (dimensions, dimension) = self.fixed_array_dimension(arguments, "$size")?;
        let length = dimensions[dimension as usize - 1].length as u64;
        Ok(eda_core::ir::Expr::Signed {
            e: Box::new(eda_core::ir::Expr::Const(
                eda_core::logic::LogicWord::from_u64(32, length),
            )),
            signed: true,
        })
    }

    fn lower_dynamic_array_size(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 1 {
            return Ok(None);
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "dynamic-array size has no receiver".to_string())?;
        let name = expression_symbol(receiver)
            .ok_or_else(|| "dynamic-array size receiver has no symbol".to_string())?;
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if !self.design.memories[memory].dynamic {
            return Ok(None);
        }
        Ok(Some(eda_core::ir::Expr::MemorySize {
            memory,
            width: 32,
            signed: true,
        }))
    }

    fn lower_dynamic_array_delete(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 1 {
            return Ok(None);
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "dynamic-array delete has no receiver".to_string())?;
        let name = expression_symbol(receiver)
            .ok_or_else(|| "dynamic-array delete receiver has no symbol".to_string())?;
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if !self.design.memories[memory].dynamic {
            return Ok(None);
        }
        Ok(Some(eda_core::ir::Stmt::MemoryDelete { memory }))
    }

    fn associative_receiver(
        &self,
        value: &Value,
        function_name: &str,
    ) -> Result<Option<usize>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let receiver = arguments
            .first()
            .ok_or_else(|| format!("{function_name} has no receiver"))?;
        let Some(name) = expression_symbol(receiver) else {
            return Ok(None);
        };
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if self.design.memories[memory].associative {
            Ok(Some(memory))
        } else {
            Ok(None)
        }
    }

    fn lower_associative_array_num(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let Some(memory) = self.associative_receiver(value, "associative num")? else {
            return Ok(None);
        };
        Ok(Some(eda_core::ir::Expr::MemorySize {
            memory,
            width: 32,
            signed: true,
        }))
    }

    pub(super) fn lower_associative_traversal_assignment(
        &self,
        left: &Value,
        right: &Value,
        nonblocking: bool,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        if string_field(right, "kind").as_deref() != Some("Call") {
            return Ok(None);
        }
        let Some(subroutine) = string_field(right, "subroutine") else {
            return Ok(None);
        };
        let method = subroutine
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        let operation = match method {
            "first" => eda_core::ir::AssocTraversalOp::First,
            "last" => eda_core::ir::AssocTraversalOp::Last,
            "next" => eda_core::ir::AssocTraversalOp::Next,
            "prev" => eda_core::ir::AssocTraversalOp::Prev,
            _ => return Ok(None),
        };
        let Some(memory) = self.associative_receiver(right, "associative traversal")? else {
            return Ok(None);
        };
        let arguments = right
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 2 {
            return Err(format!(
                "associative-array {method} expects one receiver and one ref key"
            ));
        }
        if nonblocking {
            return Err(format!(
                "associative-array {method} is not supported as a nonblocking assignment"
            ));
        }
        let key_argument = arguments
            .get(1)
            .ok_or_else(|| format!("associative-array {method} has no ref key"))?;
        let key = self.lower_lvalue(key_argument)?;
        let destination = self.lower_lvalue(left)?;
        if key.range.width() != self.design.memories[memory].index_width {
            return Err(format!(
                "associative-array {method} key width {} does not match index width {}",
                key.range.width(),
                self.design.memories[memory].index_width
            ));
        }
        if destination.range.width() == 0 || destination.range.width() > 32 {
            return Err(format!(
                "associative-array {method} status destination must be 1..=32 bits"
            ));
        }
        Ok(Some(eda_core::ir::Stmt::AssocTraversal {
            memory,
            operation,
            key,
            destination,
        }))
    }

    fn lower_associative_array_exists(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let Some(memory) = self.associative_receiver(value, "associative exists")? else {
            return Ok(None);
        };
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 2 {
            return Err("associative exists expects one receiver and one key".into());
        }
        let key = arguments
            .get(1)
            .ok_or_else(|| "associative exists has no key".to_string())?;
        let index = self.lower_expression(key)?;
        if index.width() > self.design.memories[memory].index_width {
            return Err(format!(
                "associative exists key width {} exceeds index width {}",
                index.width(),
                self.design.memories[memory].index_width
            ));
        }
        Ok(Some(eda_core::ir::Expr::AssocExists {
            memory,
            index: Box::new(index),
        }))
    }

    fn lower_associative_array_delete(
        &self,
        value: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let Some(memory) = self.associative_receiver(value, "associative delete")? else {
            return Ok(None);
        };
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() > 2 {
            return Err("associative delete expects at most one key".into());
        }
        let index = match arguments.get(1) {
            Some(key) => {
                let index = self.lower_expression(key)?;
                if index.width() > self.design.memories[memory].index_width {
                    return Err(format!(
                        "associative delete key width {} exceeds index width {}",
                        index.width(),
                        self.design.memories[memory].index_width
                    ));
                }
                Some(index)
            }
            None => None,
        };
        Ok(Some(eda_core::ir::Stmt::AssocDelete { memory, index }))
    }

    fn lower_queue_push(
        &self,
        value: &Value,
        front: bool,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 2 {
            return Ok(None);
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "queue push has no receiver".to_string())?;
        let pushed = arguments
            .get(1)
            .ok_or_else(|| "queue push has no value".to_string())?;
        let name = expression_symbol(receiver)
            .ok_or_else(|| "queue push receiver has no symbol".to_string())?;
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if !self.design.memories[memory].queue {
            return Ok(None);
        }
        let value = self.lower_queue_element_value(pushed)?;
        Ok(Some(eda_core::ir::Stmt::QueuePush {
            memory,
            value,
            front,
        }))
    }

    fn lower_queue_insert(&self, value: &Value) -> Result<Option<eda_core::ir::Stmt>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 3 {
            return Ok(None);
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| String::from("queue insert has no receiver"))?;
        let Some(memory) = self.queue_receiver(receiver, "queue insert")? else {
            return Ok(None);
        };
        let index = arguments
            .get(1)
            .ok_or_else(|| String::from("queue insert has no index"))?;
        let inserted = arguments
            .get(2)
            .ok_or_else(|| String::from("queue insert has no value"))?;
        let index = self.lower_expression(index)?;
        if index.width() > u64::BITS as usize {
            return Err("queue insert index exceeds 64 bits".into());
        }
        let value = self.lower_queue_element_value(inserted)?;
        Ok(Some(eda_core::ir::Stmt::QueueInsert {
            memory,
            index,
            value,
        }))
    }

    fn lower_queue_element_value(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let type_name = string_field(value, "type").unwrap_or_default();
        if super::expressions::type_is_realtime_compatible(&type_name) {
            return self.lower_real_arithmetic_operand(value);
        }
        if type_name.trim() == "shortreal" {
            return self.lower_shortreal_arithmetic_operand(value);
        }
        self.lower_expression(value)
    }

    fn lower_queue_delete(&self, value: &Value) -> Result<Option<eda_core::ir::Stmt>, String> {
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 2 {
            return Ok(None);
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| String::from("queue delete has no receiver"))?;
        let Some(memory) = self.queue_receiver(receiver, "queue delete")? else {
            return Ok(None);
        };
        let index = arguments
            .get(1)
            .ok_or_else(|| String::from("queue delete has no index"))?;
        let index = self.lower_expression(index)?;
        if index.width() > u64::BITS as usize {
            return Err("queue delete index exceeds 64 bits".into());
        }
        Ok(Some(eda_core::ir::Stmt::QueueDelete { memory, index }))
    }

    fn queue_receiver(
        &self,
        receiver: &Value,
        function_name: &str,
    ) -> Result<Option<usize>, String> {
        let Some(name) = expression_symbol(receiver) else {
            return Ok(None);
        };
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if !self.design.memories[memory].queue {
            return Ok(None);
        }
        let _ = function_name;
        Ok(Some(memory))
    }

    fn lower_fixed_array_bound(
        &self,
        value: &Value,
        function_name: &str,
    ) -> Result<eda_core::ir::Expr, String> {
        let arguments = fixed_array_arguments(value, function_name)?;
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(format!(
                "{function_name} expects one array argument and at most one dimension"
            ));
        }
        let (dimensions, dimension) = self.fixed_array_dimension(arguments, function_name)?;
        let dimension = dimensions[dimension as usize - 1];
        let last = dimension.base + dimension.step * (dimension.length as i64 - 1);
        let result = match function_name {
            "$left" => dimension.base,
            "$right" => last,
            "$low" => dimension.base.min(last),
            "$high" => dimension.base.max(last),
            "$increment" => dimension.step,
            _ => unreachable!("fixed-array bound function dispatch"),
        };
        Ok(eda_core::ir::Expr::Signed {
            e: Box::new(eda_core::ir::Expr::Const(
                eda_core::logic::LogicWord::from_u64(32, result as u64),
            )),
            signed: true,
        })
    }

    fn lower_array_dimension_count(
        &self,
        value: &Value,
        function_name: &str,
    ) -> Result<eda_core::ir::Expr, String> {
        let arguments = fixed_array_arguments(value, function_name)?;
        if arguments.len() != 1 {
            return Err(format!("{function_name} expects exactly one type argument"));
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| format!("{function_name} has no receiver"))?;
        let receiver_type = string_field(receiver, "type").unwrap_or_default();
        let (packed_dimensions, unpacked_dimensions) =
            if let Some(array) = self.declared_memory_type(&receiver_type) {
                let separator = receiver_type.find('$').unwrap_or_default();
                let packed = simple_type_shape(&receiver_type[..separator])
                    .map(|shape| shape.dimensions.len())
                    .unwrap_or_default();
                (packed, array.dimensions.len())
            } else {
                let packed = simple_type_shape(&receiver_type)
                    .map(|shape| shape.dimensions.len())
                    .ok_or_else(|| {
                        format!("{function_name} receiver has unsupported type: '{receiver_type}'")
                    })?;
                (packed, 0)
            };
        let count = if function_name == "$dimensions" {
            packed_dimensions + unpacked_dimensions
        } else {
            unpacked_dimensions
        };
        Ok(eda_core::ir::Expr::Signed {
            e: Box::new(eda_core::ir::Expr::Const(
                eda_core::logic::LogicWord::from_u64(32, count as u64),
            )),
            signed: true,
        })
    }

    fn lower_bits(&self, value: &Value) -> Result<eda_core::ir::Expr, String> {
        let arguments = fixed_array_arguments(value, "$bits")?;
        if arguments.len() != 1 {
            return Err("$bits expects exactly one argument".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "$bits has no receiver".to_string())?;
        let receiver_type = string_field(receiver, "type").unwrap_or_default();
        if let Some(array) = self.declared_memory_type(&receiver_type) {
            if array.dynamic {
                let name = expression_symbol(receiver)
                    .ok_or_else(|| "$bits receiver has no symbol".to_string())?;
                let symbol = self
                    .symbols
                    .get(&name)
                    .ok_or_else(|| format!("unknown $bits receiver '{name}'"))?;
                let memory = symbol
                    .memory
                    .ok_or_else(|| format!("$bits receiver '{name}' is not a dynamic array"))?;
                let element_width = u32::try_from(array.element_width).map_err(|_| {
                    format!("$bits element width is too large: {}", array.element_width)
                })?;
                let size = eda_core::ir::Expr::MemorySize {
                    memory,
                    width: 32,
                    signed: true,
                };
                return Ok(eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::Mul,
                    a: Box::new(size),
                    b: Box::new(eda_core::ir::Expr::Signed {
                        e: Box::new(eda_core::ir::Expr::Const(
                            eda_core::logic::LogicWord::from_u64(32, u64::from(element_width)),
                        )),
                        signed: true,
                    }),
                });
            }
        }
        let bits = if let Some(array) = self.declared_memory_type(&receiver_type) {
            u128::from(array.flat_length) * array.element_width as u128
        } else {
            self.lower_expression(receiver)?.width() as u128
        };
        let bits = u32::try_from(bits).map_err(|_| format!("$bits result is too large: {bits}"))?;
        Ok(eda_core::ir::Expr::Signed {
            e: Box::new(eda_core::ir::Expr::Const(
                eda_core::logic::LogicWord::from_u64(32, u64::from(bits)),
            )),
            signed: true,
        })
    }

    fn fixed_array_dimension(
        &self,
        arguments: &[Value],
        function_name: &str,
    ) -> Result<(Vec<eda_core::ir::MemoryDimension>, i64), String> {
        let receiver = arguments
            .first()
            .ok_or_else(|| format!("{function_name} has no array receiver"))?;
        let receiver_type = string_field(receiver, "type").unwrap_or_default();
        let array = self.declared_memory_type(&receiver_type).ok_or_else(|| {
            format!("{function_name} receiver is not a fixed unpacked array: '{receiver_type}'")
        })?;
        if array.dynamic {
            return Err(format!(
                "{function_name} requires a fixed unpacked array, found a dynamic array"
            ));
        }
        let mut dimension = 1_i64;
        if let Some(argument) = arguments.get(1) {
            dimension = constant_ast_i64(argument)
                .map_err(|_| format!("{function_name} dimension must be an integer constant"))?;
        }
        if dimension < 1 || dimension as usize > array.dimensions.len() {
            return Err(format!(
                "{function_name} dimension {dimension} is outside 1..={}",
                array.dimensions.len()
            ));
        }
        Ok((array.dimensions, dimension))
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
    allow_raw_binary: bool,
    hierarchy_path: &str,
    library_scope: &str,
    value_arguments: &[Value],
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
        let mut left_justify = false;
        let mut zero_modifier = false;
        loop {
            match characters.peek() {
                Some('-') if !left_justify => {
                    left_justify = true;
                    characters.next();
                }
                Some('0') if !zero_modifier => {
                    zero_modifier = true;
                    characters.next();
                }
                _ => break,
            }
        }
        let mut width_text = String::new();
        while matches!(characters.peek(), Some(c) if c.is_ascii_digit()) {
            width_text.push(characters.next().expect("peeked width digit"));
        }
        let mut precision_text = String::new();
        if characters.peek() == Some(&'.') {
            characters.next();
            while matches!(characters.peek(), Some(c) if c.is_ascii_digit()) {
                precision_text.push(characters.next().expect("peeked precision digit"));
            }
            if precision_text.is_empty() {
                return Err("display format precision requires at least one digit".into());
            }
        }
        let precision = if precision_text.is_empty() {
            None
        } else {
            let precision = precision_text
                .parse::<usize>()
                .map_err(|_| "display format precision is too large".to_string())?;
            if precision > 512 {
                return Err("display format precision must be at most 512".into());
            }
            Some(precision)
        };
        let explicit_width = if width_text.is_empty() {
            None
        } else {
            let width = width_text
                .parse::<usize>()
                .map_err(|_| "display format width is too large".to_string())?;
            if width == 0 {
                return Err("display format width must be positive".into());
            }
            if width > 512 {
                return Err("display format width must be at most 512".into());
            }
            Some(width)
        };
        let specifier = characters
            .next()
            .ok_or_else(|| "display format has a trailing '%'".to_string())?;
        if allow_raw_binary
            && matches!(specifier, 'u' | 'U' | 'z' | 'Z')
            && (left_justify || zero_modifier || explicit_width.is_some())
        {
            return Err("raw binary file-output specifiers cannot take a width modifier".into());
        }
        if matches!(specifier, 'v' | 'V')
            && (left_justify || zero_modifier || explicit_width.is_some())
        {
            return Err("display strength specifier '%v' cannot take a width modifier".into());
        }
        if matches!(specifier, 'm' | 'M')
            && (left_justify || zero_modifier || explicit_width.is_some())
        {
            return Err("display hierarchy specifier '%m' cannot take a width modifier".into());
        }
        if matches!(specifier, 'l' | 'L')
            && (left_justify || zero_modifier || explicit_width.is_some())
        {
            return Err("display library specifier '%l' cannot take a width modifier".into());
        }
        if matches!(specifier, 'p' | 'P') && (left_justify || explicit_width.is_some()) {
            return Err("display pattern specifier '%p' cannot take a width modifier".into());
        }
        let is_real_format = matches!(specifier, 'e' | 'E' | 'f' | 'F' | 'g' | 'G');
        if is_real_format {
            let Some(argument) = value_arguments.get(specifiers) else {
                return Err("display real format specifier has no argument".into());
            };
            if !is_real_value(argument) {
                return Err("display real formats require a real argument".into());
            }
        } else if precision.is_some() {
            return Err("display format precision is supported only for real formats".into());
        }
        let formatted_width = explicit_width;
        match specifier {
            'b' | 'B' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'b',
                    formatted_width,
                    None,
                    left_justify,
                );
                specifiers += 1;
            }
            'd' | 'D' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'd',
                    formatted_width,
                    None,
                    left_justify,
                );
                specifiers += 1;
            }
            'h' | 'H' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'h',
                    formatted_width,
                    None,
                    left_justify,
                );
                specifiers += 1;
            }
            'o' | 'O' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'o',
                    formatted_width,
                    None,
                    left_justify,
                );
                specifiers += 1;
            }
            's' | 'S' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    's',
                    formatted_width,
                    None,
                    left_justify,
                );
                specifiers += 1;
            }
            'e' | 'E' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'e',
                    formatted_width,
                    precision,
                    left_justify,
                );
                specifiers += 1;
            }
            'f' | 'F' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'f',
                    formatted_width,
                    precision,
                    left_justify,
                );
                specifiers += 1;
            }
            'g' | 'G' => {
                push_display_literal(&mut parts, &mut literal);
                push_display_argument(
                    &mut parts,
                    specifiers,
                    'g',
                    formatted_width,
                    precision,
                    left_justify,
                );
                specifiers += 1;
            }
            't' | 'T' => {
                push_display_literal(&mut parts, &mut literal);
                if let Some(width) = formatted_width {
                    parts.push(eda_core::ir::DispPart::FormattedArg {
                        index: specifiers,
                        spec: 't',
                        width,
                        precision: None,
                        left_justify,
                    });
                } else {
                    parts.push(eda_core::ir::DispPart::Time(specifiers));
                }
                specifiers += 1;
            }
            'c' | 'C' if formatted_width.is_none() && !left_justify && !zero_modifier => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 'c'));
                specifiers += 1;
            }
            'v' | 'V' => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, 'v'));
                specifiers += 1;
            }
            'm' | 'M' => {
                push_display_literal(&mut parts, &mut literal);
                if !hierarchy_path.is_empty() {
                    parts.push(eda_core::ir::DispPart::Str(hierarchy_path.to_string()));
                }
            }
            'l' | 'L' => {
                push_display_literal(&mut parts, &mut literal);
                if !library_scope.is_empty() {
                    parts.push(eda_core::ir::DispPart::Str(library_scope.to_string()));
                }
            }
            'p' | 'P' => {
                let Some(argument) = value_arguments.get(specifiers) else {
                    return Err("display pattern specifier '%p' has no argument".into());
                };
                let mut pattern_spec = pattern_argument_specifier(argument)?;
                if zero_modifier && matches!(pattern_spec, 'r' | 'e') {
                    pattern_spec = if pattern_spec == 'r' { 'R' } else { 'E' };
                }
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(specifiers, pattern_spec));
                specifiers += 1;
            }
            'u' | 'U' | 'z' | 'Z' if allow_raw_binary => {
                push_display_literal(&mut parts, &mut literal);
                parts.push(eda_core::ir::DispPart::Arg(
                    specifiers,
                    specifier.to_ascii_lowercase(),
                ));
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

fn is_real_value(value: &Value) -> bool {
    let value = unwrap_conversion(value);
    string_field(value, "kind").as_deref() == Some("RealLiteral")
        || string_field(value, "type")
            .as_deref()
            .is_some_and(|type_name| matches!(type_name.trim(), "real" | "realtime" | "shortreal"))
}

fn push_display_argument(
    parts: &mut Vec<eda_core::ir::DispPart>,
    index: usize,
    spec: char,
    width: Option<usize>,
    precision: Option<usize>,
    left_justify: bool,
) {
    if width.is_some() || precision.is_some() {
        parts.push(eda_core::ir::DispPart::FormattedArg {
            index,
            spec,
            width: width.unwrap_or(0),
            precision,
            left_justify,
        });
    } else {
        parts.push(eda_core::ir::DispPart::Arg(index, spec));
    }
}

fn pattern_argument_specifier(argument: &Value) -> Result<char, String> {
    let argument = unwrap_conversion(argument);
    let type_name = string_field(argument, "type").unwrap_or_default();
    if string_field(argument, "kind").as_deref() == Some("StringLiteral")
        || type_name.trim() == "string"
    {
        return Ok('q');
    }
    if string_field(argument, "kind").as_deref() == Some("RealLiteral") {
        return Ok('p');
    }
    if let Some(memory) = memory_type(&type_name) {
        if !memory.dynamic && !memory.queue && !memory.associative && memory.element_width <= 64 {
            return Ok('r');
        }
    }
    if crate::types::is_dynamic_container_type(&type_name) {
        return Ok('r');
    }
    if type_name.contains('$') {
        return Ok('r');
    }
    if crate::types::simple_type_width(&type_name).is_some_and(|width| width <= 64) {
        return Ok('p');
    }
    if matches!(
        string_field(argument, "kind").as_deref(),
        Some(
            "FixedSizeUnpackedArrayType"
                | "DynamicArrayType"
                | "QueueType"
                | "AssociativeArrayType"
        )
    ) {
        return Ok('r');
    }
    if !type_name.trim().is_empty() {
        return Ok('e');
    }
    Err(
        "$sformatf/display %p currently supports bounded strings, scalar integrals up to 64 bits, multidimensional fixed integral arrays, one-dimensional dynamic arrays, queues, and scalar-key associative arrays"
            .into(),
    )
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

fn fixed_array_arguments<'a>(value: &'a Value, function_name: &str) -> Result<&'a [Value], String> {
    value
        .get("arguments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{function_name} requires a fixed unpacked-array receiver"))
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

fn signed_time_exponent_const(exponent: i8) -> eda_core::ir::Expr {
    let bits = u64::from(i32::from(exponent) as u32);
    eda_core::ir::Expr::Signed {
        e: Box::new(eda_core::ir::Expr::Const(
            eda_core::logic::LogicWord::from_u64(32, bits),
        )),
        signed: true,
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
