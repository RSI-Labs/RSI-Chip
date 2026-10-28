//! Procedural block, statement, assignment, and memory-file lowering.

use super::expressions::{constant_i64, continuous_assignment_delay};
use super::hierarchy::full_range;
use super::{declaration_name, AstLoweredSymbol, SlangAstLowerer};
use crate::ast_util::{
    bool_field, expression_symbol, resolve_memory_file_path, resolve_memory_output_path,
    slang_constant_u64, string_field, unwrap_conversion, usize_field, with_source_span,
};
use crate::types::{is_dynamic_container_type, memory_type, AstPackedDimension};
use eda_core::logic::{LogicValue, LogicWord};
use serde_json::Value;

impl SlangAstLowerer {
    pub(super) fn collect_named_block_flags(
        &mut self,
        statement: &serde_json::Value,
    ) -> Result<(), String> {
        self.collect_named_block_flags_inner(statement)
            .and_then(|()| {
                if self.named_disable_seen {
                    self.reject_unsafe_named_control(statement, false)
                } else {
                    Ok(())
                }
            })
    }

    pub(super) fn lower_clocking_block(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let name = declaration_name(member, "ClockingBlock")?;
        let event = member
            .get("event")
            .ok_or_else(|| format!("clocking block '{name}' has no event"))?;
        let watches = event_watches(&self.lower_sensitivity(event)?)?;
        if watches.len() != 1 {
            return Err(format!(
                "clocking block '{name}' must have exactly one scalar event"
            ));
        }
        let clock_watch = watches[0];
        let empty_members = Vec::new();
        let variables = member
            .get("members")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_members);
        let mut inputs = Vec::new();
        for variable in variables {
            if string_field(variable, "kind").as_deref() != Some("ClockVar") {
                return Err(format!(
                    "clocking block '{name}' contains an unsupported member kind"
                ));
            }
            let variable_name = declaration_name(variable, "ClockVar")?;
            let direction = match string_field(variable, "direction").as_deref() {
                Some("In") => eda_core::ir::Dir::In,
                Some("Out") => eda_core::ir::Dir::Out,
                other => {
                    return Err(format!(
                        "clocking variable '{name}.{variable_name}' has unsupported direction {other:?}"
                    ))
                }
            };
            if variable.get("inputSkew").is_some() || variable.get("outputSkew").is_some() {
                return Err(format!(
                    "clocking variable '{name}.{variable_name}' skew is not supported"
                ));
            }
            let initializer = variable.get("initializer").ok_or_else(|| {
                format!("clocking variable '{name}.{variable_name}' has no target")
            })?;
            let target = self.lower_lvalue(initializer)?;
            let type_name = string_field(variable, "type").unwrap_or_default();
            let (shape, signed, _) = self.net_type(&type_name)?;
            if target.range.width() != shape.width {
                return Err(format!(
                    "clocking variable '{name}.{variable_name}' width {} does not match target width {}",
                    shape.width,
                    target.range.width()
                ));
            }
            let address = usize_field(variable, "addr").ok_or_else(|| {
                format!("clocking variable '{name}.{variable_name}' has no symbol address")
            })?;
            let symbol = if direction == eda_core::ir::Dir::In {
                let sample_base = self.design.n_nets;
                self.design.n_nets += shape.width;
                self.design.net_decls.push(eda_core::ir::NetDecl {
                    name: format!("__sample_{name}_{variable_name}"),
                    width: shape.width,
                    is_reg: true,
                    is_signed: signed,
                    base: sample_base,
                });
                let sample = eda_core::ir::LValue {
                    net: sample_base,
                    decl_width: shape.width,
                    range: full_range(shape.width),
                };
                inputs.push(eda_core::ir::ClockingInput { target, sample });
                AstLoweredSymbol {
                    base: sample_base,
                    width: shape.width,
                    signed,
                    bounds: shape.bounds,
                    dimensions: shape.dimensions,
                    memory: None,
                    struct_fields: std::collections::BTreeMap::new(),
                }
            } else {
                AstLoweredSymbol {
                    base: target.net,
                    width: shape.width,
                    signed,
                    bounds: shape.bounds,
                    dimensions: shape.dimensions,
                    memory: None,
                    struct_fields: std::collections::BTreeMap::new(),
                }
            };
            self.clocking_variables.insert(
                format!("{address} {variable_name}"),
                super::AstClockingVariable { direction, symbol },
            );
        }
        self.clocking_events.insert(name.clone(), clock_watch);
        if bool_field(member, "isDefault") {
            self.default_clocking_event = Some(clock_watch);
        }
        self.design
            .clocking_blocks
            .push(eda_core::ir::ClockingBlock {
                name,
                event: clock_watch,
                inputs,
            });
        Ok(())
    }

    fn collect_named_block_flags_inner(
        &mut self,
        statement: &serde_json::Value,
    ) -> Result<(), String> {
        let kind = string_field(statement, "kind").unwrap_or_default();
        if kind == "Block" {
            let named_id =
                string_field(statement, "block").filter(|symbol| !symbol.trim().is_empty());
            if let Some(named_id) = &named_id {
                if !self.named_block_flags.contains_key(named_id) {
                    let label = named_id.split_whitespace().next_back().unwrap_or("block");
                    let address = named_id.split_whitespace().next().unwrap_or("0");
                    let base = self.design.n_nets;
                    self.design.n_nets += 1;
                    self.design.net_decls.push(eda_core::ir::NetDecl {
                        name: format!("__disable_{address}_{label}"),
                        width: 1,
                        is_reg: true,
                        is_signed: false,
                        base,
                    });
                    self.named_block_flags.insert(named_id.clone(), base);
                }
            }
            if let Some(body) = statement.get("body") {
                self.collect_named_block_flags_inner(body)?;
            }
            return Ok(());
        }
        if kind == "Disable" {
            self.named_disable_seen = true;
        }
        if let Some(fields) = statement.as_object() {
            for nested in fields.values() {
                self.collect_named_block_flags_inner(nested)?;
            }
        }
        if let Some(values) = statement.as_array() {
            for nested in values {
                self.collect_named_block_flags_inner(nested)?;
            }
        }
        Ok(())
    }

    fn reject_unsafe_named_control(
        &self,
        statement: &serde_json::Value,
        inside_named_block: bool,
    ) -> Result<(), String> {
        let kind = string_field(statement, "kind").unwrap_or_default();
        if kind == "Block" {
            if inside_named_block
                && matches!(
                    string_field(statement, "blockKind")
                        .unwrap_or_default()
                        .as_str(),
                    "JoinAll" | "JoinAny" | "JoinNone"
                )
            {
                return Err(
                    "named blocks containing fork are not supported by direct lowering yet".into(),
                );
            }
            let named = string_field(statement, "block")
                .filter(|symbol| !symbol.trim().is_empty())
                .is_some();
            if let Some(body) = statement.get("body") {
                self.reject_unsafe_named_control(body, inside_named_block || named)?;
            }
            return Ok(());
        }
        if inside_named_block && kind == "ForeverLoop" {
            return Err("named blocks containing forever loops are not supported yet".into());
        }
        if let Some(fields) = statement.as_object() {
            for nested in fields.values() {
                self.reject_unsafe_named_control(nested, inside_named_block)?;
            }
        }
        if let Some(values) = statement.as_array() {
            for nested in values {
                self.reject_unsafe_named_control(nested, inside_named_block)?;
            }
        }
        Ok(())
    }

    pub(super) fn lower_continuous_assignment(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let delay = self.scale_delay(continuous_assignment_delay(member)?)?;
        let assignment = member
            .get("assignment")
            .ok_or_else(|| "ContinuousAssign has no assignment".to_string())?;
        let left = assignment
            .get("left")
            .ok_or_else(|| "continuous assignment has no left side".to_string())?;
        let right = assignment
            .get("right")
            .ok_or_else(|| "continuous assignment has no right side".to_string())?;
        if self.memory_element_select(left)?.is_some() || self.memory_range_select(left)?.is_some()
        {
            if delay != 0 {
                return Err(
                    "delayed sparse-memory continuous assignments are not supported".to_string(),
                );
            }
            let body = self.lower_assignment(assignment)?;
            self.design.processes.push(eda_core::ir::Process {
                sens: eda_core::ir::Sens::Comb,
                body,
            });
            return Ok(());
        }
        let lv = self.lower_lvalue(left)?;
        self.record_implicit_width_mismatch(&lv, right)?;
        let rhs = self.lower_expression(right)?;
        self.design
            .assigns
            .push(eda_core::ir::Assign { lv, e: rhs, delay });
        Ok(())
    }

    fn record_implicit_width_mismatch(
        &mut self,
        lvalue: &eda_core::ir::LValue,
        expression: &Value,
    ) -> Result<(), String> {
        if string_field(expression, "kind").as_deref() != Some("Conversion") {
            return Ok(());
        }
        let Some(operand) = expression.get("operand") else {
            return Ok(());
        };

        // slang gives an implicit assignment conversion the same source span as
        // its operand. An explicit cast starts at the cast operator instead.
        let implicit_conversion = [
            "source_file_start",
            "source_line_start",
            "source_column_start",
            "source_file_end",
            "source_line_end",
            "source_column_end",
        ]
        .iter()
        .all(|field| expression.get(field) == operand.get(field));
        if !implicit_conversion {
            return Ok(());
        }

        let target_type = string_field(expression, "type").unwrap_or_default();
        let source_type = string_field(operand, "type").unwrap_or_default();
        let (target_shape, _, _) = self.net_type(&target_type)?;
        let (source_shape, _, _) = self.net_type(&source_type)?;
        let difference = target_shape.width.abs_diff(source_shape.width);
        if difference == 0 {
            return Ok(());
        }

        let first_bit = match lvalue.range {
            eda_core::ir::Range::Bit(bit) => lvalue.net + bit,
            eda_core::ir::Range::Span { msb, lsb } => lvalue.net + lsb.min(msb),
        };
        let bit_name = self.design.net_name(first_bit);
        let net = bit_name.split('[').next().unwrap_or(&bit_name).to_string();
        let issue = eda_core::ir::LintIssue {
            kind: eda_core::ir::LintIssueKind::WidthMismatch,
            net,
            count: difference,
        };
        if let Some(existing) = self
            .design
            .recorded_width_mismatches
            .iter_mut()
            .find(|existing| existing.net == issue.net)
        {
            existing.count = existing.count.max(issue.count);
        } else {
            self.design.recorded_width_mismatches.push(issue);
        }
        Ok(())
    }

    pub(super) fn lower_procedural_block(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
        let body = member
            .get("body")
            .ok_or_else(|| "slang ProceduralBlock has no body".to_string())?;
        self.named_disable_seen = false;
        self.collect_named_block_flags(body)?;
        self.active_named_blocks.borrow_mut().clear();
        match procedure_kind.as_str() {
            "Initial" => {
                let statement = self.lower_statement(body)?;
                self.design.initials.push(statement);
            }
            "Final" => {
                let statement = self.lower_statement(body)?;
                self.design.final_blocks.push(statement);
            }
            "AlwaysComb" => {
                let statement = self.lower_statement(body)?;
                self.design.processes.push(eda_core::ir::Process {
                    sens: eda_core::ir::Sens::Comb,
                    body: statement,
                });
            }
            "Always" | "AlwaysFF" | "AlwaysLatch" => {
                if string_field(body, "kind").as_deref() == Some("Timed") {
                    let timing = body
                        .get("timing")
                        .ok_or_else(|| "timed procedure has no timing control".to_string())?;
                    let is_delay = string_field(timing, "kind").as_deref() == Some("Delay");
                    if is_delay {
                        if procedure_kind != "Always" {
                            return Err(format!(
                                "direct slang-AST lowering does not support delays in {procedure_kind}"
                            ));
                        }
                        let statement = self.lower_statement(body)?;
                        self.design.processes.push(eda_core::ir::Process {
                            sens: eda_core::ir::Sens::Timed,
                            body: eda_core::ir::Stmt::Forever(Box::new(statement)),
                        });
                    } else {
                        let statement = body
                            .get("stmt")
                            .ok_or_else(|| "timed procedure has no statement".to_string())?;
                        let sens = self.lower_sensitivity(timing)?;
                        let statement = self.lower_statement(statement)?;
                        self.design.processes.push(eda_core::ir::Process {
                            sens,
                            body: statement,
                        });
                    }
                } else {
                    let statement = self.lower_statement(body)?;
                    self.design.processes.push(eda_core::ir::Process {
                        sens: eda_core::ir::Sens::Comb,
                        body: statement,
                    });
                }
            }
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support procedure kind {other:?}"
                ))
            }
        }
        Ok(())
    }

    pub(super) fn lower_sensitivity(
        &self,
        timing: &serde_json::Value,
    ) -> Result<eda_core::ir::Sens, String> {
        match string_field(timing, "kind").as_deref() {
            Some("ImplicitEvent") => return Ok(eda_core::ir::Sens::Comb),
            Some("EventList") => {
                let events = timing
                    .get("events")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| "event list has no events".to_string())?;
                if events.is_empty() {
                    return Err("event list is empty".into());
                }
                let list = events
                    .iter()
                    .map(|event| self.lower_sensitivity(event))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(eda_core::ir::Sens::List(list));
            }
            Some("SignalEvent") => {}
            _ => {
                return Err(
                    "direct slang-AST lowering supports signal and event-list sensitivity".into(),
                )
            }
        }
        let expression = timing
            .get("expr")
            .ok_or_else(|| "signal event has no expression".to_string())?;
        let signal = self.lower_lvalue(expression)?;
        let net = sensitivity_net(&signal)?;
        match string_field(timing, "edge").as_deref() {
            Some("None") => Ok(eda_core::ir::Sens::Any(net)),
            Some("PosEdge") => Ok(eda_core::ir::Sens::Posedge(net)),
            Some("NegEdge") => Ok(eda_core::ir::Sens::Negedge(net)),
            _ => Err("direct slang-AST lowering requires a scalar signal edge".into()),
        }
    }

    pub(super) fn lower_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        if let Some(statement) = self.lower_uvm_callback_macro(statement)? {
            return Ok(statement);
        }
        self.lower_statement_inner(statement)
            .map_err(|error| with_source_span(statement, error))
    }

    fn active_named_block_ids(&self) -> Vec<String> {
        self.active_named_blocks.borrow().clone()
    }

    fn named_block_disabled(&self, named_ids: &[String]) -> eda_core::ir::Expr {
        let mut nets = named_ids.iter().map(|named_id| {
            let net = self
                .named_block_flags
                .get(named_id)
                .copied()
                .unwrap_or_default();
            eda_core::ir::Expr::Ref {
                net,
                decl_width: 1,
                range: full_range(1),
                signed: false,
            }
        });
        let Some(mut result) = nets.next() else {
            return eda_core::ir::Expr::Const(LogicWord::zero(1));
        };
        for net in nets {
            result = eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::LogOr,
                a: Box::new(result),
                b: Box::new(net),
            };
        }
        result
    }

    fn guard_named_block_statement(
        &self,
        statement: eda_core::ir::Stmt,
        named_ids: &[String],
    ) -> eda_core::ir::Stmt {
        if named_ids.is_empty() {
            return statement;
        }
        eda_core::ir::Stmt::If {
            c: self.named_block_disabled(named_ids),
            t: Box::new(eda_core::ir::Stmt::Block(Vec::new())),
            f: Some(Box::new(statement)),
        }
    }

    fn lower_loop_body(&self, body: &serde_json::Value) -> Result<eda_core::ir::Stmt, String> {
        let lowered = self.lower_statement(body)?;
        let active_ids = self.active_named_block_ids();
        if active_ids.is_empty() {
            return Ok(lowered);
        }
        Ok(eda_core::ir::Stmt::If {
            c: self.named_block_disabled(&active_ids),
            t: Box::new(eda_core::ir::Stmt::Break),
            f: Some(Box::new(lowered)),
        })
    }

    fn lower_named_disable(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let target = statement
            .get("target")
            .ok_or_else(|| "disable statement has no target".to_string())?;
        let named_id = string_field(target, "symbol")
            .filter(|symbol| !symbol.trim().is_empty())
            .ok_or_else(|| "disable target is not a named block".to_string())?;
        let net = self
            .named_block_flags
            .get(&named_id)
            .copied()
            .ok_or_else(|| format!("direct lowering cannot disable target '{named_id}'"))?;
        Ok(eda_core::ir::Stmt::Bl {
            lv: eda_core::ir::LValue {
                net,
                decl_width: 1,
                range: full_range(1),
            },
            e: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
        })
    }

    fn lower_statement_inner(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let kind = string_field(statement, "kind").unwrap_or_default();
        match kind.as_str() {
            "Block" => {
                match string_field(statement, "blockKind").as_deref() {
                    None | Some("Sequential") => {}
                    Some(block_kind @ ("JoinAll" | "JoinAny" | "JoinNone")) => {
                        if self.named_disable_seen && !self.active_named_block_ids().is_empty() {
                            return Err(
                                "fork inside a named block is not supported by direct lowering yet"
                                    .into(),
                            );
                        }
                        let body = statement
                            .get("body")
                            .ok_or_else(|| "fork block has no body".to_string())?;
                        let branches = if let Some(list) =
                            body.get("list").and_then(serde_json::Value::as_array)
                        {
                            list.iter()
                                .map(|branch| self.lower_statement(branch))
                                .collect::<Result<Vec<_>, _>>()?
                        } else if matches!(
                            string_field(body, "kind").as_deref(),
                            Some("Timed" | "Block")
                        ) {
                            vec![self.lower_statement(body)?]
                        } else {
                            return Err(
                                "fork block body is not a statement list".to_string()
                            );
                        };
                        let mode = match block_kind {
                            "JoinAny" => eda_core::ir::ForkMode::JoinAny,
                            "JoinNone" => eda_core::ir::ForkMode::JoinNone,
                            _ => eda_core::ir::ForkMode::JoinAll,
                        };
                        return Ok(Stmt::Fork { branches, mode });
                    }
                    Some(other) => {
                        return Err(format!(
                            "direct slang-AST lowering does not support concurrency block kind '{other}'"
                        ));
                    }
                }
                if let Some(statement) = self.lower_uvm_macro_report_list(statement)? {
                    return Ok(statement);
                }
                let named_id = string_field(statement, "block")
                    .filter(|symbol| !symbol.trim().is_empty());
                if let Some(named_id) = &named_id {
                    self.active_named_blocks.borrow_mut().push(named_id.clone());
                }
                let active_ids = self.active_named_block_ids();
                if let Some(nested) = statement.get("body") {
                    if string_field(nested, "kind").as_deref() == Some("List") {
                        let local_ids = statement
                            .get("block")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .and_then(|block| self.local_blocks.get(block))
                            .cloned()
                            .unwrap_or_default();
                        let mut statements = Vec::new();
                        if let Some(named_id) = &named_id {
                            let net = self
                                .named_block_flags
                                .get(named_id)
                                .copied()
                                .ok_or_else(|| "named block has no disable flag".to_string())?;
                            statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        for symbol_id in &local_ids {
                            let initializer = self
                                .local_initializers
                                .get(symbol_id)
                                .ok_or_else(|| "procedure-local initializer was not collected".to_string())?;
                            let symbol = self
                                .local_symbols
                                .get(symbol_id)
                                .ok_or_else(|| "procedure-local symbol was not collected".to_string())?;
                            let rhs = self.lower_expression(initializer)?;
                            statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: symbol.base,
                                    decl_width: symbol.width,
                                    range: full_range(symbol.width),
                                },
                                e: rhs,
                            });
                        }
                        let items = nested
                            .get("list")
                            .and_then(Value::as_array)
                            .ok_or_else(|| "statement list has no items".to_string())?;
                        for item in items {
                            if !local_ids.is_empty()
                                && string_field(item, "kind").as_deref() == Some("VariableDeclaration")
                            {
                                continue;
                            }
                            let lowered = self.lower_statement(item)?;
                            statements
                                .push(self.guard_named_block_statement(lowered, &active_ids));
                        }
                        if named_id.is_some() {
                            self.active_named_blocks.borrow_mut().pop();
                        }
                        return Ok(Stmt::Block(statements));
                    }
                    let nested = self.lower_statement(nested)?;
                    let nested = self.guard_named_block_statement(nested, &active_ids);
                    if named_id.is_some() {
                        self.active_named_blocks.borrow_mut().pop();
                    }
                    return Ok(Stmt::Block(vec![nested]));
                }
                if named_id.is_some() {
                    self.active_named_blocks.borrow_mut().pop();
                }
                Ok(Stmt::Block(Vec::new()))
            }
            "List" => {
                if let Some(statement) = self.lower_uvm_macro_report_list(statement)? {
                    return Ok(statement);
                }
                let mut statements = Vec::new();
                let active_ids = self.active_named_block_ids();
                if let Some(items) = statement.get("list").and_then(Value::as_array) {
                    for item in items {
                        let lowered = self.lower_statement(item)?;
                        statements.push(self.guard_named_block_statement(lowered, &active_ids));
                    }
                }
                Ok(Stmt::Block(statements))
            }
            "ExpressionStatement" => {
                let expression = statement
                    .get("expr")
                    .ok_or_else(|| "expression statement has no expression".to_string())?;
                if string_field(expression, "kind").as_deref() == Some("Call") {
                    if let Some(statement) = self.lower_task_call(expression)? {
                        return Ok(statement);
                    }
                    return self.lower_memory_file_call(expression, statement);
                }
                if matches!(
                    string_field(expression, "op").as_deref(),
                    Some("Postincrement" | "Preincrement" | "Postdecrement" | "Predecrement")
                ) {
                    return self.lower_increment_statement(expression);
                }
                if string_field(expression, "kind").as_deref() != Some("Assignment") {
                    return Err(
                        "direct slang-AST lowering supports only assignment expression statements"
                            .into(),
                    );
                }
                self.lower_assignment(expression)
            }
            "ProceduralAssign" if bool_field(statement, "isForce") => {
                let assignment = statement
                    .get("assignment")
                    .ok_or_else(|| "force statement has no assignment".to_string())?;
                let left = assignment
                    .get("left")
                    .ok_or_else(|| "force statement has no left side".to_string())?;
                let right = assignment
                    .get("right")
                    .ok_or_else(|| "force statement has no right side".to_string())?;
                Ok(Stmt::Force {
                    lv: self.lower_lvalue(left)?,
                    e: self.lower_expression(right)?,
                })
            }
            "ProceduralDeassign" if bool_field(statement, "isRelease") => {
                let left = statement
                    .get("lvalue")
                    .ok_or_else(|| "release statement has no lvalue".to_string())?;
                Ok(Stmt::Release {
                    lv: self.lower_lvalue(left)?,
                })
            }
            "Assignment" => self.lower_assignment(statement),
            "VariableDeclaration" => Err(
                "direct slang-AST procedure locals are not supported; declare loop variables in module scope"
                    .into(),
            ),
            "Disable" => self.lower_named_disable(statement),
            "Conditional" => {
                if let Some(statement) = self.lower_value_plusarg_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_resource_read_type_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_resource_write_type_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_resource_write_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_resource_read_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_config_exists_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_config_get_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_uvm_factory_cast_conditional(statement)? {
                    return Ok(statement);
                }
                if let Some(statement) = self.lower_class_cast_conditional(statement)? {
                    return Ok(statement);
                }
                self.lower_conditional_statement(statement)
            }
            "Case" => {
                self.lower_case_statement(statement)
            }
            "Timed" => {
                let timing = statement
                    .get("timing")
                    .ok_or_else(|| "timed statement has no timing control".to_string())?;
                if string_field(timing, "kind").as_deref() == Some("EventList") {
                    let body = statement
                        .get("stmt")
                        .ok_or_else(|| "event control has no statement".to_string())?;
                    let events = timing
                        .get("events")
                        .and_then(Value::as_array)
                        .ok_or_else(|| "event list has no events".to_string())?;
                    if events.is_empty() {
                        return Err("event list is empty".into());
                    }
                    let mut watches = Vec::with_capacity(events.len());
                    for event in events {
                        watches.extend(event_watches(&self.lower_sensitivity(event)?)?);
                    }
                    return Ok(Stmt::EventWait {
                        watches,
                        body: Box::new(self.lower_statement(body)?),
                    });
                }
                if string_field(timing, "kind").as_deref() == Some("SignalEvent") {
                    let body = statement
                        .get("stmt")
                        .ok_or_else(|| "event control has no statement".to_string())?;
                    let event = timing
                        .get("expr")
                        .ok_or_else(|| "event control has no event expression".to_string())?;
                    if string_field(event, "kind").as_deref() == Some("ArbitrarySymbol") {
                        let clocking_name = expression_symbol(event).ok_or_else(|| {
                            "clocking-block event control has no symbol".to_string()
                        })?;
                        if let Some(watch) = self.clocking_events.get(&clocking_name).copied() {
                            return Ok(Stmt::EventWait {
                                watches: vec![watch],
                                body: Box::new(self.lower_statement(body)?),
                            });
                        }
                    }
                    if let Some(selection) = self.event_array_selection(event)? {
                        let edge = match string_field(timing, "edge").as_deref() {
                            Some("None") => eda_core::ir::EventEdge::Any,
                            Some("PosEdge") => eda_core::ir::EventEdge::Posedge,
                            Some("NegEdge") => eda_core::ir::EventEdge::Negedge,
                            _ => {
                                return Err(
                                    "direct slang-AST lowering requires a scalar signal edge"
                                        .into(),
                                )
                            }
                        };
                        return self.lower_event_array_wait(&selection, edge, body);
                    }
                    let lvalue = self.lower_lvalue(event)?;
                    if lvalue.range.width() != 1 {
                        return Err("event control expression must be scalar".into());
                    }
                    let edge = match string_field(timing, "edge").as_deref() {
                        Some("None") => eda_core::ir::EventEdge::Any,
                        Some("PosEdge") => eda_core::ir::EventEdge::Posedge,
                        Some("NegEdge") => eda_core::ir::EventEdge::Negedge,
                        _ => {
                            return Err(
                                "direct slang-AST lowering requires a scalar signal edge".into(),
                            )
                        }
                    };
                    return Ok(Stmt::EventWait {
                        watches: vec![eda_core::ir::EventWatch {
                            net: sensitivity_net(&lvalue)?,
                            edge,
                        }],
                        body: Box::new(self.lower_statement(body)?),
                    });
                }
                if string_field(timing, "kind").as_deref() == Some("CycleDelay") {
                    let body = statement
                        .get("stmt")
                        .ok_or_else(|| "cycle delay has no statement".to_string())?;
                    let expression = timing
                        .get("expr")
                        .ok_or_else(|| "cycle delay has no count".to_string())?;
                    let cycles = slang_constant_u64(expression)
                        .map_err(|_| "cycle delay count must be an integer constant".to_string())?;
                    let Some(watch) = self.default_clocking_event else {
                        return Err(
                            "cycle delay requires a default clocking block".to_string()
                        );
                    };
                    let body = self.lower_statement(body)?;
                    if cycles == 0 {
                        return Ok(body);
                    }
                    let wait = Stmt::EventWait {
                        watches: vec![watch],
                        body: Box::new(Stmt::Block(Vec::new())),
                    };
                    return Ok(Stmt::Block(vec![
                        Stmt::Repeat {
                            count: eda_core::ir::Expr::Const(LogicWord::from_u64(64, cycles)),
                            body: Box::new(wait),
                        },
                        body,
                    ]));
                }
                let delay = self.lower_delay_control(timing)?;
                let body = statement
                    .get("stmt")
                    .ok_or_else(|| "timed statement has no body".to_string())?;
                Ok(Stmt::Delay {
                    ns: delay,
                    body: Box::new(self.lower_statement(body)?),
                })
            }
            "ForeverLoop" => {
                let body = statement
                    .get("body")
                    .ok_or_else(|| "forever loop has no body".to_string())?;
                if self.named_disable_seen && !self.active_named_block_ids().is_empty() {
                    return Err("forever loops inside named blocks are not supported yet".into());
                }
                Ok(Stmt::Forever(Box::new(self.lower_statement(body)?)))
            }
            "RepeatLoop" => {
                let count = statement
                    .get("count")
                    .ok_or_else(|| "repeat loop has no count".to_string())?;
                let body = statement
                    .get("body")
                    .ok_or_else(|| "repeat loop has no body".to_string())?;
                Ok(Stmt::Repeat {
                    count: self.lower_expression(count)?,
                    body: Box::new(self.lower_loop_body(body)?),
                })
            }
            "WhileLoop" => {
                let condition = statement
                    .get("cond")
                    .ok_or_else(|| "while loop has no condition".to_string())?;
                let body = statement
                    .get("body")
                    .ok_or_else(|| "while loop has no body".to_string())?;
                Ok(Stmt::While {
                    cond: self.lower_expression(condition)?,
                    body: Box::new(self.lower_loop_body(body)?),
                    steps: Vec::new(),
                })
            }
            "DoWhileLoop" => {
                let condition = statement
                    .get("cond")
                    .ok_or_else(|| "do-while loop has no condition".to_string())?;
                let body = statement
                    .get("body")
                    .ok_or_else(|| "do-while loop has no body".to_string())?;
                let first_execution = self.lower_loop_body(body)?;
                let repeated_execution = Stmt::While {
                    cond: self.lower_expression(condition)?,
                    body: Box::new(self.lower_loop_body(body)?),
                    steps: Vec::new(),
                };
                Ok(Stmt::Block(vec![first_execution, repeated_execution]))
            }
            "ForLoop" => {
                let initializers = statement
                    .get("initializers")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "for loop has no initializers".to_string())?;
                if initializers
                    .iter()
                    .any(|initializer| string_field(initializer, "kind").as_deref()
                        == Some("VariableDeclaration"))
                {
                    return Err(
                        "direct slang-AST for loops require module-scope loop variables".into(),
                    );
                }
                let condition = statement
                    .get("stopExpr")
                    .ok_or_else(|| "for loop has no stop expression".to_string())?;
                let body = statement
                    .get("body")
                    .ok_or_else(|| "for loop has no body".to_string())?;
                let steps = statement
                    .get("steps")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "for loop has no steps".to_string())?;

                let mut statements = Vec::with_capacity(initializers.len() + 1);
                for initializer in initializers {
                    statements.push(self.lower_statement(initializer)?);
                }
                let loop_body = self.lower_loop_body(body)?;
                let lowered_steps = steps
                    .iter()
                    .map(|step| self.lower_update_statement(step))
                    .collect::<Result<Vec<_>, _>>()?;
                statements.push(Stmt::While {
                    cond: self.lower_expression(condition)?,
                    body: Box::new(loop_body),
                    steps: lowered_steps,
                });
                Ok(Stmt::Block(statements))
            }
            "Break" => Ok(Stmt::Break),
            "Continue" => Ok(Stmt::Continue),
            "Return" => {
                if statement
                    .get("expr")
                    .is_some_and(|expression| !expression.is_null())
                {
                    return Err(
                        "direct slang-AST statement lowering supports only void task return".into(),
                    );
                }
                Ok(Stmt::Return)
            }
            "Empty" => Ok(Stmt::Block(Vec::new())),
            "EventTrigger" => self.lower_event_trigger(statement),
            "Wait" => {
                let condition = statement
                    .get("cond")
                    .ok_or_else(|| "wait statement has no condition".to_string())?;
                let body = statement
                    .get("stmt")
                    .ok_or_else(|| "wait statement has no body".to_string())?;
                Ok(Stmt::Wait {
                    cond: self.lower_expression(condition)?,
                    body: Box::new(self.lower_statement(body)?),
                })
            }
            "WaitOrder" => {
                let event_values = statement
                    .get("events")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "wait_order has no events".to_string())?;
                if event_values.is_empty() {
                    return Err("wait_order requires at least one event".into());
                }
                let mut events = Vec::with_capacity(event_values.len());
                for event in event_values {
                    let target = event
                        .get("target")
                        .ok_or_else(|| "wait_order event has no target".to_string())?;
                    let event_type = target
                        .get("type")
                        .ok_or_else(|| "wait_order event target has no type".to_string())?;
                    let is_event = match event_type {
                        Value::String(type_name) => type_name == "event",
                        Value::Object(fields) => {
                            fields.get("name").and_then(Value::as_str) == Some("event")
                                || fields.get("kind").and_then(Value::as_str)
                                    == Some("EventType")
                        }
                        _ => false,
                    };
                    if !is_event {
                        return Err("wait_order targets must be events".into());
                    }
                    let lvalue = self.lower_lvalue(target)?;
                    if lvalue.range.width() != 1 {
                        return Err("wait_order event targets must be scalar".into());
                    }
                    let net = lvalue.net
                        + match lvalue.range {
                            eda_core::ir::Range::Bit(bit) => bit,
                            _ => 0,
                        };
                    events.push(net);
                }
                let lower_action = |action: &str| -> Result<Option<Box<Stmt>>, String> {
                    match statement.get(action) {
                        Some(action) => Ok(Some(Box::new(self.lower_statement(action)?))),
                        None => Ok(None),
                    }
                };
                Ok(Stmt::WaitOrder {
                    events,
                    success: lower_action("ifTrue")?,
                    failure: lower_action("ifFalse")?,
                })
            }
            other => Err(format!(
                "direct slang-AST lowering does not support statement kind '{other}'"
            )),
        }
    }

    fn lower_conditional_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::{Expr, Stmt};
        let check = string_field(statement, "check").unwrap_or_default();
        match check.as_str() {
            "None" | "Priority" | "Unique" | "Unique0" => {}
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support {other} conditional semantics"
                ))
            }
        }
        let mut chain_conditions = Vec::new();
        let mut has_terminal_else = false;
        let mut current = statement;
        loop {
            let conditions = current
                .get("conditions")
                .and_then(Value::as_array)
                .ok_or_else(|| "conditional statement has no conditions".to_string())?;
            if conditions.len() != 1 {
                return Err("direct slang-AST lowering supports one condition per if chain".into());
            }
            let condition = conditions
                .first()
                .and_then(|condition| condition.get("expr"))
                .ok_or_else(|| "conditional statement has no condition expression".to_string())?;
            chain_conditions.push(condition);
            match current.get("ifFalse") {
                Some(nested) if string_field(nested, "kind").as_deref() == Some("Conditional") => {
                    current = nested;
                }
                Some(_) => {
                    has_terminal_else = true;
                    break;
                }
                None => break,
            }
        }

        let lowered_conditions = chain_conditions
            .iter()
            .map(|condition| self.lower_expression(condition))
            .collect::<Result<Vec<_>, _>>()?;
        let selection = self.lower_unchecked_conditional_statement(statement)?;
        let mut statements = Vec::new();
        if check == "Priority" && !has_terminal_else {
            let conditions_are_known = all_expressions_known(&lowered_conditions);
            let definite_matches = lowered_conditions
                .iter()
                .map(definite_true)
                .collect::<Vec<_>>();
            statements.push(Stmt::Assert {
                cond: Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(Expr::Un {
                        op: eda_core::ir::UnOp::Not,
                        a: Box::new(conditions_are_known),
                    }),
                    b: Box::new(logical_or(&definite_matches)),
                },
                message: Some("priority if has no matching condition".into()),
            });
        }
        if check == "Unique" || check == "Unique0" {
            let conditions_are_known = all_expressions_known(&lowered_conditions);
            for left in 0..lowered_conditions.len() {
                for right in (left + 1)..lowered_conditions.len() {
                    let definite_overlap = Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(definite_true(&lowered_conditions[left])),
                        b: Box::new(definite_true(&lowered_conditions[right])),
                    };
                    statements.push(Stmt::Assert {
                        cond: Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(conditions_are_known.clone()),
                            }),
                            b: Box::new(Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(definite_overlap),
                            }),
                        },
                        message: Some("unique if has multiple matching conditions".into()),
                    });
                }
            }
            if check == "Unique" && !has_terminal_else {
                let definite_matches = lowered_conditions
                    .iter()
                    .map(definite_true)
                    .collect::<Vec<_>>();
                statements.push(Stmt::Assert {
                    cond: Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: Box::new(conditions_are_known),
                        }),
                        b: Box::new(logical_or(&definite_matches)),
                    },
                    message: Some("unique if has no matching condition".into()),
                });
            }
        }
        if statements.is_empty() {
            return Ok(selection);
        }
        statements.push(selection);
        Ok(Stmt::Block(statements))
    }

    fn lower_unchecked_conditional_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let conditions = statement
            .get("conditions")
            .and_then(Value::as_array)
            .ok_or_else(|| "conditional statement has no conditions".to_string())?;
        if conditions.len() != 1 {
            return Err("direct slang-AST lowering supports one condition per if".into());
        }
        let condition = conditions
            .first()
            .and_then(|condition| condition.get("expr"))
            .ok_or_else(|| "conditional statement has no condition expression".to_string())?;
        let true_statement = statement
            .get("ifTrue")
            .ok_or_else(|| "conditional statement has no true branch".to_string())?;
        let false_statement = match statement.get("ifFalse") {
            Some(nested) if string_field(nested, "kind").as_deref() == Some("Conditional") => Some(
                Box::new(self.lower_unchecked_conditional_statement(nested)?),
            ),
            Some(nested) => Some(Box::new(self.lower_statement(nested)?)),
            None => None,
        };
        Ok(eda_core::ir::Stmt::If {
            c: self.lower_expression(condition)?,
            t: Box::new(self.lower_statement(true_statement)?),
            f: false_statement,
        })
    }

    fn lower_case_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let condition = string_field(statement, "condition").unwrap_or_default();
        let check = string_field(statement, "check").unwrap_or_default();
        match condition.as_str() {
            "WildcardJustZ" | "WildcardXOrZ" => {
                if check != "None" {
                    return Err(format!(
                        "direct slang-AST lowering does not support {check} wildcard case semantics"
                    ));
                }
                self.lower_wildcard_case_statement(statement, condition == "WildcardXOrZ")
            }
            "Inside" => {
                if check != "None" {
                    return Err(format!(
                        "direct slang-AST lowering does not support {check} case-inside semantics"
                    ));
                }
                self.lower_inside_case_statement(statement)
            }
            "Normal" => self.lower_normal_case_statement(statement, &check),
            other => Err(format!(
                "direct slang-AST lowering does not support {other} case semantics"
            )),
        }
    }

    fn lower_normal_case_statement(
        &self,
        statement: &serde_json::Value,
        check: &str,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::{BinOp, Expr, Stmt};
        match check {
            "None" | "Priority" | "Unique" | "Unique0" => {}
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support {other} case semantics"
                ))
            }
        }
        let expression = statement
            .get("expr")
            .ok_or_else(|| "case statement has no expression".to_string())?;
        let case_expression = self.lower_expression(expression)?;
        let case_is_known = expression_is_known(&case_expression);
        let items = statement
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "case statement has no items".to_string())?;
        let mut arms = Vec::new();
        for item in items {
            let body = item
                .get("stmt")
                .ok_or_else(|| "case item has no statement".to_string())?;
            let body = self.lower_statement(body)?;
            let expressions = item
                .get("expressions")
                .and_then(Value::as_array)
                .ok_or_else(|| "case item has no expressions".to_string())?;
            for expression in expressions {
                arms.push((self.lower_expression(expression)?, body.clone()));
            }
        }
        let matches = arms
            .iter()
            .map(|(label, _)| Expr::Bin {
                op: BinOp::CaseEq,
                a: Box::new(case_expression.clone()),
                b: Box::new(label.clone()),
            })
            .collect::<Vec<_>>();
        let default = match statement.get("defaultCase") {
            Some(statement) => Some(Box::new(self.lower_statement(statement)?)),
            None => None,
        };
        let has_default = default.is_some();
        let selection = Stmt::Case {
            e: case_expression,
            arms,
            dflt: default,
        };
        let mut statements = Vec::new();
        if check == "Priority" && !has_default {
            let definite_matches = matches.iter().map(definite_true).collect::<Vec<_>>();
            statements.push(Stmt::Assert {
                cond: Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(Expr::Un {
                        op: eda_core::ir::UnOp::Not,
                        a: Box::new(case_is_known.clone()),
                    }),
                    b: Box::new(logical_or(&definite_matches)),
                },
                message: Some("priority case has no matching item".into()),
            });
        }
        if check == "Unique" || check == "Unique0" {
            for left in 0..matches.len() {
                for right in (left + 1)..matches.len() {
                    let definite_overlap = Expr::Bin {
                        op: BinOp::LogAnd,
                        a: Box::new(definite_true(&matches[left])),
                        b: Box::new(definite_true(&matches[right])),
                    };
                    statements.push(Stmt::Assert {
                        cond: Expr::Bin {
                            op: BinOp::LogOr,
                            a: Box::new(Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(case_is_known.clone()),
                            }),
                            b: Box::new(Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(definite_overlap),
                            }),
                        },
                        message: Some("unique case has multiple matching items".into()),
                    });
                }
            }
            if check == "Unique" && !has_default {
                let definite_matches = matches.iter().map(definite_true).collect::<Vec<_>>();
                statements.push(Stmt::Assert {
                    cond: Expr::Bin {
                        op: BinOp::LogOr,
                        a: Box::new(Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: Box::new(case_is_known),
                        }),
                        b: Box::new(logical_or(&definite_matches)),
                    },
                    message: Some("unique case has no matching item".into()),
                });
            }
        }
        if statements.is_empty() {
            return Ok(selection);
        }
        statements.push(selection);
        Ok(Stmt::Block(statements))
    }

    fn lower_wildcard_case_statement(
        &self,
        statement: &serde_json::Value,
        ignore_x: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::{BinOp, Expr, Stmt};

        let case_expression = statement
            .get("expr")
            .ok_or_else(|| "wildcard case statement has no expression".to_string())?;
        let case_expression = self.lower_expression(case_expression)?;
        let items = statement
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "wildcard case statement has no items".to_string())?;
        let mut result = match statement.get("defaultCase") {
            Some(default) => self.lower_statement(default)?,
            None => Stmt::Block(Vec::new()),
        };
        for item in items.iter().rev() {
            let body = item
                .get("stmt")
                .ok_or_else(|| "wildcard case item has no statement".to_string())?;
            let body = self.lower_statement(body)?;
            let expressions = item
                .get("expressions")
                .and_then(Value::as_array)
                .ok_or_else(|| "wildcard case item has no expressions".to_string())?;
            let mut alternatives = Vec::new();
            for expression in expressions {
                let label = self.lower_expression(expression)?;
                alternatives.push(self.wildcard_case_match(
                    case_expression.clone(),
                    label,
                    ignore_x,
                )?);
            }
            let Some(first) = alternatives.first().cloned() else {
                return Err("wildcard case item has no usable expressions".into());
            };
            let mut condition = first;
            for alternative in alternatives.into_iter().skip(1) {
                condition = Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(condition),
                    b: Box::new(alternative),
                };
            }
            result = Stmt::If {
                c: condition,
                t: Box::new(body),
                f: Some(Box::new(result)),
            };
        }
        Ok(result)
    }

    fn wildcard_case_match(
        &self,
        case_expression: eda_core::ir::Expr,
        label: eda_core::ir::Expr,
        ignore_x: bool,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let width = case_expression.width().max(label.width()).max(1);
        let mut result = Expr::Const(LogicWord::ones(1));
        for bit in 0..width {
            let case_bit = Self::case_bit(case_expression.clone(), bit);
            let label_bit = Self::case_bit(label.clone(), bit);
            let selector_wildcard = Self::bit_is(case_bit.clone(), LogicValue::Z);
            let mut wildcard = selector_wildcard;
            if ignore_x {
                wildcard = Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(wildcard),
                    b: Box::new(Self::bit_is(case_bit.clone(), LogicValue::X)),
                };
            }
            let label_wildcard = Self::bit_is(label_bit.clone(), LogicValue::Z);
            let label_wildcard = if ignore_x {
                Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(label_wildcard),
                    b: Box::new(Self::bit_is(label_bit.clone(), LogicValue::X)),
                }
            } else {
                label_wildcard
            };
            wildcard = Expr::Bin {
                op: BinOp::LogOr,
                a: Box::new(wildcard),
                b: Box::new(label_wildcard),
            };
            let matches = Expr::Bin {
                op: BinOp::LogOr,
                a: Box::new(wildcard),
                b: Box::new(Expr::Bin {
                    op: BinOp::CaseEq,
                    a: Box::new(case_bit),
                    b: Box::new(label_bit),
                }),
            };
            result = Expr::Bin {
                op: BinOp::LogAnd,
                a: Box::new(result),
                b: Box::new(matches),
            };
        }
        Ok(result)
    }

    fn case_bit(expression: eda_core::ir::Expr, bit: usize) -> eda_core::ir::Expr {
        if bit >= expression.width() {
            return eda_core::ir::Expr::Const(LogicWord::zero(1));
        }
        eda_core::ir::Expr::Index {
            e: Box::new(expression),
            msb: bit,
            lsb: bit,
        }
    }

    fn bit_is(expression: eda_core::ir::Expr, value: LogicValue) -> eda_core::ir::Expr {
        eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::CaseEq,
            a: Box::new(expression),
            b: Box::new(eda_core::ir::Expr::Const(LogicWord::new_const(1, value))),
        }
    }

    fn lower_inside_case_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::{BinOp, Expr, Stmt};

        let case_expression = statement
            .get("expr")
            .ok_or_else(|| "case inside statement has no expression".to_string())?;
        let case_expression = self.lower_expression(case_expression)?;
        let items = statement
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "case inside statement has no items".to_string())?;
        let mut result = match statement.get("defaultCase") {
            Some(default) => self.lower_statement(default)?,
            None => Stmt::Block(Vec::new()),
        };
        for item in items.iter().rev() {
            let body = item
                .get("stmt")
                .ok_or_else(|| "case inside item has no statement".to_string())?;
            let body = self.lower_statement(body)?;
            let expressions = item
                .get("expressions")
                .and_then(Value::as_array)
                .ok_or_else(|| "case inside item has no expressions".to_string())?;
            let mut alternatives = Vec::new();
            for expression in expressions {
                if string_field(expression, "kind").as_deref() == Some("ValueRange") {
                    if string_field(expression, "rangeKind").as_deref() != Some("Simple") {
                        return Err(
                            "direct slang-AST lowering supports only simple case-inside ranges"
                                .into(),
                        );
                    }
                    let lower = expression
                        .get("left")
                        .ok_or_else(|| "case-inside range has no lower bound".to_string())?;
                    let upper = expression
                        .get("right")
                        .ok_or_else(|| "case-inside range has no upper bound".to_string())?;
                    let lower = self.lower_expression(lower)?;
                    let upper = self.lower_expression(upper)?;
                    alternatives.push(Expr::Bin {
                        op: BinOp::LogAnd,
                        a: Box::new(Expr::Bin {
                            op: BinOp::Ge,
                            a: Box::new(case_expression.clone()),
                            b: Box::new(lower),
                        }),
                        b: Box::new(Expr::Bin {
                            op: BinOp::Le,
                            a: Box::new(case_expression.clone()),
                            b: Box::new(upper),
                        }),
                    });
                } else {
                    alternatives.push(Expr::Bin {
                        op: BinOp::Eq,
                        a: Box::new(case_expression.clone()),
                        b: Box::new(self.lower_expression(expression)?),
                    });
                }
            }
            let Some(first) = alternatives.first().cloned() else {
                return Err("case inside item has no usable expressions".into());
            };
            let mut condition = first;
            for alternative in alternatives.into_iter().skip(1) {
                condition = Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(condition),
                    b: Box::new(alternative),
                };
            }
            result = Stmt::If {
                c: condition,
                t: Box::new(body),
                f: Some(Box::new(result)),
            };
        }
        Ok(result)
    }

    fn lower_delay_control(&self, timing: &serde_json::Value) -> Result<u64, String> {
        if string_field(timing, "kind").as_deref() != Some("Delay") {
            return Err("direct slang-AST lowering supports only constant Delay controls".into());
        }
        if timing.get("minValue").is_some()
            || timing.get("typicalValue").is_some()
            || timing.get("maxValue").is_some()
        {
            return Err("min:typ:max delay controls are not supported".into());
        }
        let expression = timing
            .get("expr")
            .ok_or_else(|| "delay control has no expression".to_string())?;
        if string_field(expression, "kind").as_deref() == Some("MinTypMax") {
            return Err("min:typ:max delay controls are not supported".into());
        }
        let delay = slang_constant_u64(expression)
            .map_err(|_| "delay control must be an integer constant".to_string())?;
        self.scale_delay(delay)
    }

    fn lower_event_trigger(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let nonblocking = bool_field(statement, "isNonBlocking");
        let target = statement
            .get("target")
            .ok_or_else(|| "event trigger has no target".to_string())?;
        let event_type = target
            .get("type")
            .ok_or_else(|| "event trigger target has no type".to_string())?;
        let is_event = match event_type {
            Value::String(type_name) => type_name == "event",
            Value::Object(fields) => {
                fields.get("name").and_then(Value::as_str) == Some("event")
                    || fields.get("kind").and_then(Value::as_str) == Some("EventType")
            }
            _ => false,
        };
        if !is_event {
            return Err("event trigger target must be an event".into());
        }
        if let Some(selection) = self.event_array_selection(target)? {
            return self.lower_event_array_trigger(&selection, nonblocking);
        }
        let lvalue = self.lower_lvalue(target)?;
        let current = self.lower_expression(target)?;
        let unknown = eda_core::ir::Expr::Const(LogicWord::new_const(1, LogicValue::X));
        let zero = eda_core::ir::Expr::Const(LogicWord::zero(1));
        let next = eda_core::ir::Expr::Cond {
            c: Box::new(eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::CaseEq,
                a: Box::new(current.clone()),
                b: Box::new(unknown),
            }),
            t: Box::new(zero),
            f: Box::new(eda_core::ir::Expr::Un {
                op: eda_core::ir::UnOp::Not,
                a: Box::new(current),
            }),
        };
        if nonblocking {
            Ok(eda_core::ir::Stmt::NBl {
                lv: lvalue,
                e: next,
            })
        } else {
            Ok(eda_core::ir::Stmt::Bl {
                lv: lvalue,
                e: next,
            })
        }
    }

    fn lower_event_array_trigger(
        &self,
        selection: &super::AstEventArraySelection,
        nonblocking: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        let candidates = self.event_array_candidates(selection, &selection.selectors)?;
        let mut result = eda_core::ir::Stmt::Block(Vec::new());
        for candidate in candidates {
            let branch = event_array_toggle(candidate.net, nonblocking);
            result = eda_core::ir::Stmt::If {
                c: event_array_condition(&candidate.conditions),
                t: Box::new(branch),
                f: Some(Box::new(result)),
            };
        }
        Ok(result)
    }

    fn lower_event_array_wait(
        &self,
        selection: &super::AstEventArraySelection,
        edge: eda_core::ir::EventEdge,
        body: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let candidates = self.event_array_candidates(selection, &selection.selectors)?;
        let body = self.lower_statement(body)?;
        let mut result = eda_core::ir::Stmt::Assert {
            cond: eda_core::ir::Expr::Const(LogicWord::zero(1)),
            message: Some("event array index is out of range".into()),
        };
        for candidate in candidates {
            let branch = eda_core::ir::Stmt::EventWait {
                watches: vec![eda_core::ir::EventWatch {
                    net: candidate.net,
                    edge,
                }],
                body: Box::new(body.clone()),
            };
            result = eda_core::ir::Stmt::If {
                c: event_array_condition(&candidate.conditions),
                t: Box::new(branch),
                f: Some(Box::new(result)),
            };
        }
        Ok(result)
    }

    fn lower_increment_statement(
        &self,
        expression: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let operand = expression
            .get("operand")
            .ok_or_else(|| "increment expression has no operand".to_string())?;
        let lv = self.lower_lvalue(operand)?;
        let value = self.lower_expression(operand)?;
        let width = lv.range.width();
        let one = eda_core::ir::Expr::Const(LogicWord::from_u64(width, 1));
        let add = matches!(
            string_field(expression, "op").as_deref(),
            Some("Postincrement" | "Preincrement")
        );
        Ok(eda_core::ir::Stmt::Bl {
            lv,
            e: eda_core::ir::Expr::Bin {
                op: if add {
                    eda_core::ir::BinOp::Add
                } else {
                    eda_core::ir::BinOp::Sub
                },
                a: Box::new(value),
                b: Box::new(one),
            },
        })
    }

    fn lower_update_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        match string_field(statement, "kind").as_deref() {
            Some("Assignment") => self.lower_assignment(statement),
            Some(
                kind @ ("UnaryOp" | "Postincrement" | "Preincrement" | "Postdecrement"
                | "Predecrement"),
            ) if kind == "UnaryOp"
                || matches!(
                    string_field(statement, "op").as_deref(),
                    Some("Postincrement" | "Preincrement" | "Postdecrement" | "Predecrement")
                ) =>
            {
                self.lower_increment_statement(statement)
            }
            _ => Err("direct slang-AST for-loop steps must be assignments or increments".into()),
        }
    }

    pub(super) fn lower_memory_file_call(
        &self,
        call: &serde_json::Value,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let subroutine = string_field(call, "subroutine").unwrap_or_default();
        let is_write = matches!(subroutine.as_str(), "$writememh" | "$writememb");
        let radix = match subroutine.as_str() {
            "$readmemh" | "$writememh" => 16,
            "$readmemb" | "$writememb" => 2,
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support system call {other:?}"
                ))
            }
        };
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .ok_or_else(|| "memory-file call has no arguments".to_string())?;
        if arguments.len() < 2 || arguments.len() > 4 {
            return Err(format!(
                "direct slang-AST lowering supports two to four arguments for memory-file tasks, got {}",
                arguments.len()
            ));
        }
        let raw_path = string_field(&arguments[0], "literal")
            .ok_or_else(|| "memory-file call requires a string literal path".to_string())?;
        let target = if is_write {
            &arguments[1]
        } else {
            arguments[1]
                .get("left")
                .ok_or_else(|| "memory-file call has no target".to_string())?
        };
        let name = expression_symbol(target)
            .ok_or_else(|| "memory-file call target has no symbol".to_string())?;
        let symbol = self
            .symbols
            .get(&name)
            .ok_or_else(|| format!("unknown memory-file target '{name}'"))?;
        let memory = symbol
            .memory
            .ok_or_else(|| format!("memory-file target '{name}' is not a sparse memory"))?;
        let start = arguments.get(2).map(slang_constant_u64).transpose()?;
        let finish = arguments.get(3).map(slang_constant_u64).transpose()?;
        let declaration = &self.design.memories[memory];
        let path = if is_write {
            resolve_memory_output_path(&raw_path, statement)
        } else {
            resolve_memory_file_path(&raw_path, statement)
        };
        if is_write {
            return Ok(eda_core::ir::Stmt::WriteMem {
                radix,
                path: path.to_string_lossy().into_owned(),
                memory,
                start: start.map(|start| {
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, start))
                }),
                finish: finish.map(|finish| {
                    eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(32, finish))
                }),
            });
        }
        let values = eda_core::memory_file::read_memory_file(
            &path,
            radix,
            declaration.element_width,
            start,
        )?;
        let values = eda_core::memory_file::filter_memory_file_finish(values, start, finish)?;
        let mut statements = Vec::with_capacity(values.len());
        for physical_index in values.keys().copied().collect::<Vec<_>>() {
            let logical_indexes =
                eda_core::memory_file::physical_to_logical_indexes(declaration, physical_index)?;
            statements.push(eda_core::ir::Stmt::MemoryBl {
                memory,
                indexes: logical_indexes
                    .into_iter()
                    .map(|index| {
                        eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(
                            32,
                            index as u64,
                        ))
                    })
                    .collect(),
                e: eda_core::ir::Expr::Const(values[&physical_index].clone()),
            });
        }
        Ok(eda_core::ir::Stmt::Block(statements))
    }

    pub(super) fn lower_assignment(
        &self,
        assignment: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let left = assignment
            .get("left")
            .ok_or_else(|| "assignment has no left side".to_string())?;
        let right = assignment
            .get("right")
            .ok_or_else(|| "assignment has no right side".to_string())?;
        if self.is_external_input_lvalue(left)? {
            return Err("modport input variable cannot be assigned directly".into());
        }
        if let Some(statement) = self.lower_associative_traversal_assignment(
            left,
            right,
            bool_field(assignment, "isNonBlocking"),
        )? {
            return Ok(statement);
        }
        if let Some(statement) =
            self.whole_memory_assignment(left, right, bool_field(assignment, "isNonBlocking"))?
        {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_class_parent_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) =
            self.lower_uvm_factory_create_assignment(assignment, left, right)?
        {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_value_plusarg_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_class_null_assignment(assignment)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_class_constructor(assignment)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_class_handle_assignment(assignment)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_class_property_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.memory_struct_member_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.queue_pop_assignment(left, right)? {
            return Ok(statement);
        }
        if let Some(statement) =
            self.dynamic_array_allocation(left, right, bool_field(assignment, "isNonBlocking"))?
        {
            return Ok(statement);
        }
        if let Some(statement) =
            self.memory_pattern_assignment(left, right, bool_field(assignment, "isNonBlocking"))?
        {
            return Ok(statement);
        }
        if let Some(statement) = self.dynamic_packed_element_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some((memory, indexes)) = self.memory_element_select(left)? {
            let expression = self.lower_expression(right)?;
            if self.design.memories[memory].associative {
                let [index] = indexes.as_slice() else {
                    return Err("associative array assignment requires one index".into());
                };
                return Ok(Stmt::AssocWrite {
                    memory,
                    index: index.clone(),
                    value: expression,
                    nonblocking: bool_field(assignment, "isNonBlocking"),
                });
            }
            return Ok(if bool_field(assignment, "isNonBlocking") {
                Stmt::MemoryNBl {
                    memory,
                    indexes,
                    e: expression,
                }
            } else {
                Stmt::MemoryBl {
                    memory,
                    indexes,
                    e: expression,
                }
            });
        }
        if let Some((memory, indexes, start, width, ascending)) = self.memory_range_select(left)? {
            let expression = self.lower_expression(right)?;
            return Ok(if bool_field(assignment, "isNonBlocking") {
                Stmt::MemoryNBlPart {
                    memory,
                    indexes,
                    start: Box::new(start),
                    width,
                    ascending,
                    e: expression,
                }
            } else {
                Stmt::MemoryBlPart {
                    memory,
                    indexes,
                    start: Box::new(start),
                    width,
                    ascending,
                    e: expression,
                }
            });
        }
        let lv = self.lower_lvalue(left)?;
        let expression = self.lower_expression(right)?;
        if bool_field(assignment, "isNonBlocking") {
            Ok(Stmt::NBl { lv, e: expression })
        } else {
            Ok(Stmt::Bl { lv, e: expression })
        }
    }

    fn lower_value_plusarg_output(
        &self,
        call: &Value,
    ) -> Result<(eda_core::ir::Stmt, eda_core::ir::Expr), String> {
        use eda_core::ir::{Expr, Stmt};
        let plan = super::declarations::value_plusarg_plan(call)?;
        if !self.plusargs.contains_key(&plan.query) {
            return Err(format!("plusarg '{}' was not prepared", plan.query));
        }
        let value = self
            .plusarg_values
            .get(&plan.query)
            .ok_or_else(|| format!("plusarg value '{}' was not prepared", plan.query))?;
        let target = self.lower_lvalue(plan.target)?;
        let parsed = match plan.specifier {
            's' => Expr::Ref {
                net: value.base,
                decl_width: value.width,
                range: full_range(value.width),
                signed: false,
            },
            specifier => {
                if target.decl_width > 32 {
                    return Err(format!(
                        "$value$plusargs '%{specifier}' currently supports targets up to 32 bits"
                    ));
                }
                let radix = match specifier {
                    'd' => 10,
                    'h' => 16,
                    'o' => 8,
                    'b' => 2,
                    _ => unreachable!("validated $value$plusargs specifier"),
                };
                Expr::StringToRadix {
                    e: Box::new(Expr::Ref {
                        net: value.base,
                        decl_width: value.width,
                        range: full_range(value.width),
                        signed: false,
                    }),
                    radix,
                }
            }
        };
        let status = self.lower_expression(call)?;
        let output = Stmt::If {
            c: status.clone(),
            t: Box::new(Stmt::Bl {
                lv: target,
                e: parsed,
            }),
            f: None,
        };
        Ok((output, status))
    }

    fn lower_value_plusarg_assignment(
        &self,
        assignment: &Value,
        left: &Value,
        right: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        use eda_core::ir::Stmt;
        let call = unwrap_conversion(right);
        if string_field(call, "kind").as_deref() != Some("Call")
            || string_field(call, "subroutine").as_deref() != Some("$value$plusargs")
        {
            return Ok(None);
        }
        let (output, _) = self.lower_value_plusarg_output(call)?;
        let status = self.lower_expression(right)?;
        let status_lv = self.lower_lvalue(left)?;
        let status_assignment = if bool_field(assignment, "isNonBlocking") {
            Stmt::NBl {
                lv: status_lv,
                e: status,
            }
        } else {
            Stmt::Bl {
                lv: status_lv,
                e: status,
            }
        };
        Ok(Some(Stmt::Block(vec![output, status_assignment])))
    }

    fn lower_value_plusarg_conditional(
        &self,
        statement: &Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        use eda_core::ir::Stmt;
        let conditions = statement
            .get("conditions")
            .and_then(Value::as_array)
            .ok_or_else(|| "conditional statement has no conditions".to_string())?;
        if conditions.len() != 1 {
            return Ok(None);
        }
        let Some(condition) = conditions.first().and_then(|value| value.get("expr")) else {
            return Ok(None);
        };
        let call = unwrap_conversion(condition);
        if string_field(call, "kind").as_deref() != Some("Call")
            || string_field(call, "subroutine").as_deref() != Some("$value$plusargs")
        {
            return Ok(None);
        }
        let (output, _) = self.lower_value_plusarg_output(call)?;
        let selection = self.lower_unchecked_conditional_statement(statement)?;
        Ok(Some(Stmt::Block(vec![output, selection])))
    }

    fn whole_memory_assignment(
        &self,
        left: &serde_json::Value,
        right: &serde_json::Value,
        nonblocking: bool,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        if !matches!(
            (
                string_field(left, "kind").as_deref(),
                string_field(right, "kind").as_deref(),
            ),
            (
                Some("NamedValue" | "HierarchicalValue"),
                Some("NamedValue" | "HierarchicalValue"),
            )
        ) {
            return Ok(None);
        }
        if !is_dynamic_container_type(&string_field(left, "type").unwrap_or_default())
            && memory_type(&string_field(left, "type").unwrap_or_default()).is_none()
        {
            return Ok(None);
        }
        if !is_dynamic_container_type(&string_field(right, "type").unwrap_or_default())
            && memory_type(&string_field(right, "type").unwrap_or_default()).is_none()
        {
            return Ok(None);
        }
        let left_symbol = self.named_symbol(left)?;
        let right_symbol = self.named_symbol(right)?;
        let Some(destination) = left_symbol.memory else {
            return Ok(None);
        };
        let Some(source) = right_symbol.memory else {
            return Ok(None);
        };
        let source_memory = &self.design.memories[source];
        let destination_memory = &self.design.memories[destination];
        let elements_match = source_memory.element_width == destination_memory.element_width
            && source_memory.element_msb == destination_memory.element_msb
            && source_memory.element_lsb == destination_memory.element_lsb;
        let kind = if source_memory.associative {
            if !destination_memory.associative
                || !elements_match
                || source_memory.index_width != destination_memory.index_width
                || source_memory.index_signed != destination_memory.index_signed
                || source_memory.flat_length != destination_memory.flat_length
            {
                return Err(
                    "whole associative-array assignment source and destination shapes do not match"
                        .into(),
                );
            }
            eda_core::ir::MemoryCopyKind::Associative
        } else if source_memory.queue {
            if !destination_memory.queue || !elements_match {
                return Err(
                    "whole queue assignment source and destination element shapes do not match"
                        .into(),
                );
            }
            eda_core::ir::MemoryCopyKind::Queue
        } else if source_memory.dynamic {
            if !destination_memory.dynamic
                || destination_memory.queue
                || destination_memory.associative
                || !elements_match
                || source_memory.dimensions != destination_memory.dimensions
                || source_memory.flat_length != destination_memory.flat_length
            {
                return Err(
                    "whole dynamic-array assignment source and destination shapes do not match"
                        .into(),
                );
            }
            eda_core::ir::MemoryCopyKind::DynamicArray
        } else {
            if destination_memory.dynamic
                || !elements_match
                || source_memory.dimensions != destination_memory.dimensions
                || source_memory.flat_length != destination_memory.flat_length
            {
                return Err(
                    "whole unpacked-array assignment source and destination shapes do not match"
                        .into(),
                );
            }
            if source_memory.flat_length > 4096 {
                return Err(
                    "whole unpacked-array assignment exceeds the 4096-element support limit".into(),
                );
            }
            eda_core::ir::MemoryCopyKind::FixedArray
        };
        Ok(Some(eda_core::ir::Stmt::MemoryCopy {
            source,
            destination,
            nonblocking,
            kind,
        }))
    }

    fn dynamic_array_allocation(
        &self,
        left: &serde_json::Value,
        right: &serde_json::Value,
        nonblocking: bool,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        if string_field(right, "kind").as_deref() != Some("NewArray") {
            return Ok(None);
        }
        if !matches!(
            string_field(left, "kind").as_deref(),
            Some("NamedValue" | "HierarchicalValue")
        ) {
            return Err("dynamic-array allocation requires a named array target".into());
        }
        let destination = self.named_symbol(left)?;
        let Some(memory) = destination.memory else {
            return Err("dynamic-array allocation target is not a dynamic array".into());
        };
        if !self.design.memories[memory].dynamic {
            return Err("new[] target is not a dynamic array".into());
        }
        let size = right
            .get("sizeExpr")
            .ok_or_else(|| "dynamic-array allocation has no size".to_string())?;
        let size = self.lower_expression(size)?;
        if size.width() > u64::BITS as usize {
            return Err("dynamic-array allocation size exceeds 64 bits".into());
        }
        Ok(Some(eda_core::ir::Stmt::MemoryNew {
            memory,
            size,
            nonblocking,
        }))
    }

    fn queue_pop_assignment(
        &self,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        if string_field(right, "kind").as_deref() != Some("Call") {
            return Ok(None);
        }
        let front = match string_field(right, "subroutine").as_deref() {
            Some("pop_front") => true,
            Some("pop_back") => false,
            _ => return Ok(None),
        };
        let arguments = right
            .get("arguments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if arguments.len() != 1 {
            return Err("queue pop expects exactly one receiver".into());
        }
        let receiver = arguments
            .first()
            .ok_or_else(|| "queue pop has no receiver".to_string())?;
        let name = expression_symbol(receiver)
            .ok_or_else(|| "queue pop receiver has no symbol".to_string())?;
        let Some(symbol) = self.symbols.get(&name) else {
            return Ok(None);
        };
        let Some(memory) = symbol.memory else {
            return Ok(None);
        };
        if !self.design.memories[memory].queue {
            return Ok(None);
        }
        let destination = self.lower_lvalue(left)?;
        if destination.range.width() != self.design.memories[memory].element_width {
            return Err(format!(
                "queue pop destination width {} does not match element width {}",
                destination.range.width(),
                self.design.memories[memory].element_width
            ));
        }
        Ok(Some(eda_core::ir::Stmt::QueuePop {
            memory,
            destination,
            front,
        }))
    }

    fn memory_pattern_assignment(
        &self,
        left: &serde_json::Value,
        right: &serde_json::Value,
        nonblocking: bool,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let right_kind = string_field(right, "kind").unwrap_or_default();
        if !matches!(
            string_field(left, "kind").as_deref(),
            Some("NamedValue" | "HierarchicalValue")
        ) || !matches!(
            right_kind.as_str(),
            "SimpleAssignmentPattern" | "StructuredAssignmentPattern"
        ) {
            return Ok(None);
        }
        let destination = self.named_symbol(left)?;
        let Some(memory) = destination.memory else {
            return Ok(None);
        };
        if self.design.memories[memory].dynamic {
            return Err("unpacked assignment patterns are not supported for dynamic arrays".into());
        }
        let dimensions = self.design.memories[memory].dimensions.clone();
        let flat_length = self.design.memories[memory].flat_length;
        if flat_length > 4096 {
            return Err(
                "unpacked assignment pattern exceeds the 4096-element support limit".into(),
            );
        }
        if right_kind == "StructuredAssignmentPattern" {
            let [dimension] = dimensions.as_slice() else {
                return Err(
                    "structured multidimensional unpacked patterns are not supported".to_string(),
                );
            };
            return self
                .structured_memory_pattern_assignment(memory, dimension, right, nonblocking)
                .map(Some);
        }

        fn visit(
            lowerer: &SlangAstLowerer,
            memory: usize,
            dimensions: &[eda_core::ir::MemoryDimension],
            pattern: &serde_json::Value,
            indexes: &mut Vec<i64>,
            statements: &mut Vec<eda_core::ir::Stmt>,
            nonblocking: bool,
        ) -> Result<(), String> {
            if indexes.len() == dimensions.len() {
                let value = lowerer.lower_expression(pattern)?;
                let indexes = indexes
                    .iter()
                    .map(|index| {
                        u64::try_from(*index).map(|index| {
                            eda_core::ir::Expr::Const(eda_core::logic::LogicWord::from_u64(
                                32, index,
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "unpacked assignment pattern index is negative".to_string())?;
                statements.push(if nonblocking {
                    eda_core::ir::Stmt::MemoryNBl {
                        memory,
                        indexes,
                        e: value,
                    }
                } else {
                    eda_core::ir::Stmt::MemoryBl {
                        memory,
                        indexes,
                        e: value,
                    }
                });
                return Ok(());
            }

            let elements = pattern
                .get("elements")
                .and_then(Value::as_array)
                .ok_or_else(|| "unpacked assignment pattern has no elements".to_string())?;
            let dimension = dimensions[indexes.len()];
            if elements.len() != dimension.length {
                return Err(format!(
                    "unpacked assignment-pattern dimension has {} elements, expected {}",
                    elements.len(),
                    dimension.length
                ));
            }
            for (position, element) in elements.iter().enumerate() {
                let logical_index = dimension.base + dimension.step * position as i64;
                indexes.push(logical_index);
                visit(
                    lowerer,
                    memory,
                    dimensions,
                    element,
                    indexes,
                    statements,
                    nonblocking,
                )?;
                indexes.pop();
            }
            Ok(())
        }

        let mut statements = Vec::new();
        let mut indexes = Vec::new();
        visit(
            self,
            memory,
            &dimensions,
            right,
            &mut indexes,
            &mut statements,
            nonblocking,
        )?;
        Ok(Some(eda_core::ir::Stmt::Block(statements)))
    }

    fn structured_memory_pattern_assignment(
        &self,
        memory: usize,
        dimension: &eda_core::ir::MemoryDimension,
        pattern: &serde_json::Value,
        nonblocking: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        if pattern.get("memberSetters").is_some() {
            return Err("member-based unpacked assignment patterns are not supported".into());
        }
        let default = pattern
            .get("defaultSetter")
            .ok_or_else(|| "structured unpacked pattern has no default value".to_string())?;
        let empty_setters = Vec::new();
        let setters = pattern
            .get("indexSetters")
            .and_then(Value::as_array)
            .unwrap_or(&empty_setters);
        let mut values_by_index = std::collections::BTreeMap::new();
        for setter in setters {
            let index_value = setter
                .get("index")
                .ok_or_else(|| "structured unpacked pattern setter has no index".to_string())?;
            let index = slang_constant_u64(index_value).map_err(|_| {
                "structured unpacked pattern indexes must be nonnegative integer constants"
                    .to_string()
            })?;
            let expression = setter
                .get("expr")
                .ok_or_else(|| "structured unpacked pattern setter has no value".to_string())?;
            if values_by_index.insert(index, expression).is_some() {
                return Err(format!(
                    "duplicate structured unpacked pattern index {index}"
                ));
            }
        }

        let last = dimension.base + dimension.step * (dimension.length as i64 - 1);
        let lower = dimension.base.min(last);
        let upper = dimension.base.max(last);
        for index in values_by_index.keys() {
            if *index < lower as u64 || *index > upper as u64 {
                return Err(format!(
                    "structured unpacked pattern index {index} is outside {lower}..={upper}"
                ));
            }
        }

        let mut statements = Vec::new();
        for position in 0..dimension.length {
            let logical_index = dimension.base + dimension.step * position as i64;
            if logical_index < 0 {
                return Err(
                    "negative structured unpacked pattern indexes are not yet lowered".to_string(),
                );
            }
            let expression = values_by_index
                .get(&(logical_index as u64))
                .copied()
                .unwrap_or(default);
            let value = self.lower_expression(expression)?;
            let indexes = vec![eda_core::ir::Expr::Const(
                eda_core::logic::LogicWord::from_u64(32, logical_index as u64),
            )];
            statements.push(if nonblocking {
                eda_core::ir::Stmt::MemoryNBl {
                    memory,
                    indexes,
                    e: value,
                }
            } else {
                eda_core::ir::Stmt::MemoryBl {
                    memory,
                    indexes,
                    e: value,
                }
            });
        }
        Ok(eda_core::ir::Stmt::Block(statements))
    }

    fn dynamic_packed_element_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        use eda_core::ir::{LValue, Stmt};

        if string_field(left, "kind").as_deref() != Some("ElementSelect")
            || self.memory_element_select(left)?.is_some()
        {
            return Ok(None);
        }
        let selected = left
            .get("value")
            .ok_or_else(|| "packed assignment select has no selected value".to_string())?;
        let selector = left
            .get("selector")
            .ok_or_else(|| "packed assignment select has no selector".to_string())?;
        let Some(symbol) = self.direct_packed_symbol(selected)? else {
            return Ok(None);
        };
        let Some(dimension) = symbol.dimensions.first() else {
            return Ok(None);
        };
        if constant_i64(selector).is_ok() {
            return Ok(None);
        }

        self.lower_dynamic_packed_group(symbol.width, dimension, selector, selected, right)
            .map(|expression| {
                let lv = LValue {
                    net: symbol.base,
                    decl_width: symbol.width,
                    range: full_range(symbol.width),
                };
                if bool_field(assignment, "isNonBlocking") {
                    Stmt::NBl { lv, e: expression }
                } else {
                    Stmt::Bl { lv, e: expression }
                }
            })
            .map(Some)
    }

    fn memory_struct_member_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        use eda_core::ir::{Expr, Stmt};

        if string_field(left, "kind").as_deref() != Some("MemberAccess") {
            return Ok(None);
        }
        let Some(selection) = self.memory_member_selection(left)? else {
            return Ok(None);
        };
        let mut offset = 0_usize;
        let mut field_width = None;
        for (selected, member) in &selection.members {
            let field = self.struct_field_for_type(selected, member)?;
            offset += field.offset;
            field_width = Some(field.width);
        }
        let Some(field_width) = field_width else {
            return Ok(None);
        };
        let memory = selection.memory;
        let indexes = selection.indexes;
        let member = selection
            .members
            .last()
            .map(|(_, member)| member.clone())
            .unwrap_or_default();
        let element_width = self.design.memories[memory].element_width;
        if offset + field_width > element_width {
            return Err(format!(
                "struct member '{member}' exceeds memory element width {element_width}"
            ));
        }

        let value = self.lower_expression(right)?;
        if value.width() != field_width {
            return Err(format!(
                "memory struct member '{member}' width {} does not match assignment width {}",
                field_width,
                value.width()
            ));
        }
        let old = Expr::MemoryRead {
            memory,
            indexes: indexes.clone(),
            width: element_width,
            signed: false,
        };
        let mut parts = Vec::new();
        if offset + field_width < element_width {
            parts.push(Expr::Index {
                e: Box::new(old.clone()),
                msb: element_width - 1,
                lsb: offset + field_width,
            });
        }
        parts.push(value);
        if offset > 0 {
            parts.push(Expr::Index {
                e: Box::new(old),
                msb: offset - 1,
                lsb: 0,
            });
        }
        let updated = Expr::Concat(parts);
        Ok(Some(if bool_field(assignment, "isNonBlocking") {
            Stmt::MemoryNBl {
                memory,
                indexes,
                e: updated,
            }
        } else {
            Stmt::MemoryBl {
                memory,
                indexes,
                e: updated,
            }
        }))
    }

    fn lower_dynamic_packed_group(
        &self,
        base_width: usize,
        dimension: &AstPackedDimension,
        selector: &serde_json::Value,
        selected: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        use eda_core::ir::{BinOp, Expr};

        let group_width = dimension.element_width;
        let value = self.lower_expression(right)?;
        if value.width() != group_width {
            return Err(format!(
                "dynamic packed assignment width {} does not match selected dimension width {group_width}",
                value.width()
            ));
        }
        let selector_width = string_field(selector, "type")
            .and_then(|type_name| crate::types::simple_type_width(&type_name))
            .unwrap_or(32)
            .max(1);
        let selector_value = self.lower_expression(selector)?;
        let old_value = self.lower_expression(selected)?;
        let mut physical_bits = vec![None; base_width];

        for coordinate in 0..dimension.length {
            let logical_index = if dimension.msb >= dimension.lsb {
                dimension.lsb + coordinate as i64
            } else {
                dimension.lsb - coordinate as i64
            };
            let condition = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(selector_value.clone()),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    selector_width,
                    logical_index as u64,
                ))),
            };
            for bit in 0..group_width {
                let physical_bit = coordinate * group_width + bit;
                let updated = Expr::Index {
                    e: Box::new(value.clone()),
                    msb: bit,
                    lsb: bit,
                };
                let old = Expr::Index {
                    e: Box::new(old_value.clone()),
                    msb: physical_bit,
                    lsb: physical_bit,
                };
                physical_bits[physical_bit] = Some(Expr::Cond {
                    c: Box::new(condition.clone()),
                    t: Box::new(updated),
                    f: Box::new(old),
                });
            }
        }

        let mut bits = Vec::with_capacity(base_width);
        for bit in physical_bits.into_iter().rev() {
            bits.push(bit.ok_or_else(|| {
                "dynamic packed assignment did not cover a physical bit".to_string()
            })?);
        }
        Ok(Expr::Concat(bits))
    }

    pub(super) fn push_assignment(&mut self, name: String, rhs: eda_core::ir::Expr) {
        let symbol = self.symbols[&name].clone();
        self.design.assigns.push(eda_core::ir::Assign {
            lv: eda_core::ir::LValue {
                net: symbol.base,
                decl_width: symbol.width,
                range: full_range(symbol.width),
            },
            e: rhs,
            delay: 0,
        });
    }

    fn memory_member_selection(
        &self,
        value: &serde_json::Value,
    ) -> Result<Option<MemoryMemberSelection>, String> {
        match string_field(value, "kind").as_deref() {
            Some("ElementSelect") => {
                let Some((memory, indexes)) = self.memory_element_select(value)? else {
                    return Ok(None);
                };
                Ok(Some(MemoryMemberSelection {
                    memory,
                    indexes,
                    members: Vec::new(),
                }))
            }
            Some("MemberAccess") => {
                let selected = value
                    .get("value")
                    .ok_or_else(|| "struct member access has no selected value".to_string())?;
                let Some(mut selection) = self.memory_member_selection(selected)? else {
                    return Ok(None);
                };
                let member = string_field(value, "member")
                    .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
                    .ok_or_else(|| "struct member access has no member".to_string())?;
                selection.members.push((selected.clone(), member));
                Ok(Some(selection))
            }
            _ => Ok(None),
        }
    }
}

struct MemoryMemberSelection {
    memory: usize,
    indexes: Vec<eda_core::ir::Expr>,
    members: Vec<(serde_json::Value, String)>,
}

fn sensitivity_net(signal: &eda_core::ir::LValue) -> Result<usize, String> {
    match signal.range {
        eda_core::ir::Range::Bit(bit) => Ok(signal.net + bit),
        eda_core::ir::Range::Span { msb, lsb } if msb == lsb => Ok(signal.net + lsb),
        _ => Err("sensitivity expression must be one bit wide".into()),
    }
}

fn event_watches(
    sensitivity: &eda_core::ir::Sens,
) -> Result<Vec<eda_core::ir::EventWatch>, String> {
    use eda_core::ir::{EventEdge, EventWatch, Sens};

    match sensitivity {
        Sens::Comb => Err("event control cannot use implicit @* sensitivity".into()),
        Sens::Timed => Err("event control cannot use timed-process sensitivity".into()),
        Sens::Any(net) => Ok(vec![EventWatch {
            net: *net,
            edge: EventEdge::Any,
        }]),
        Sens::Posedge(net) => Ok(vec![EventWatch {
            net: *net,
            edge: EventEdge::Posedge,
        }]),
        Sens::Negedge(net) => Ok(vec![EventWatch {
            net: *net,
            edge: EventEdge::Negedge,
        }]),
        Sens::List(nested) => {
            let mut watches = Vec::new();
            for nested in nested {
                watches.extend(event_watches(nested)?);
            }
            Ok(watches)
        }
    }
}

fn event_array_toggle(net: usize, nonblocking: bool) -> eda_core::ir::Stmt {
    let lv = eda_core::ir::LValue {
        net,
        decl_width: 1,
        range: full_range(1),
    };
    let current = eda_core::ir::Expr::Ref {
        net,
        decl_width: 1,
        range: full_range(1),
        signed: false,
    };
    let unknown = eda_core::ir::Expr::Const(LogicWord::new_const(1, LogicValue::X));
    let zero = eda_core::ir::Expr::Const(LogicWord::zero(1));
    let next = eda_core::ir::Expr::Cond {
        c: Box::new(eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::CaseEq,
            a: Box::new(current.clone()),
            b: Box::new(unknown),
        }),
        t: Box::new(zero),
        f: Box::new(eda_core::ir::Expr::Un {
            op: eda_core::ir::UnOp::Not,
            a: Box::new(current),
        }),
    };
    if nonblocking {
        eda_core::ir::Stmt::NBl { lv, e: next }
    } else {
        eda_core::ir::Stmt::Bl { lv, e: next }
    }
}

pub(super) fn event_array_condition(conditions: &[eda_core::ir::Expr]) -> eda_core::ir::Expr {
    let mut conditions = conditions.iter().cloned();
    let Some(mut result) = conditions.next() else {
        return eda_core::ir::Expr::Const(LogicWord::ones(1));
    };
    for condition in conditions {
        result = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogAnd,
            a: Box::new(result),
            b: Box::new(condition),
        };
    }
    result
}

fn logical_or(expressions: &[eda_core::ir::Expr]) -> eda_core::ir::Expr {
    let Some(first) = expressions.first() else {
        return eda_core::ir::Expr::Const(LogicWord::zero(1));
    };
    let mut result = first.clone();
    for expression in expressions.iter().skip(1) {
        result = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogOr,
            a: Box::new(result),
            b: Box::new(expression.clone()),
        };
    }
    result
}

fn expression_is_known(expression: &eda_core::ir::Expr) -> eda_core::ir::Expr {
    let mut unknown_bits = Vec::new();
    for bit in 0..expression.width() {
        let selected = eda_core::ir::Expr::Index {
            e: Box::new(expression.clone()),
            msb: bit,
            lsb: bit,
        };
        unknown_bits.push(eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::CaseEq,
            a: Box::new(selected.clone()),
            b: Box::new(eda_core::ir::Expr::Const(LogicWord::new_const(
                1,
                LogicValue::X,
            ))),
        });
        unknown_bits.push(eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::CaseEq,
            a: Box::new(selected),
            b: Box::new(eda_core::ir::Expr::Const(LogicWord::new_const(
                1,
                LogicValue::Z,
            ))),
        });
    }
    eda_core::ir::Expr::Un {
        op: eda_core::ir::UnOp::Not,
        a: Box::new(logical_or(&unknown_bits)),
    }
}

fn all_expressions_known(expressions: &[eda_core::ir::Expr]) -> eda_core::ir::Expr {
    let mut result = eda_core::ir::Expr::Const(LogicWord::ones(1));
    let mut references = Vec::new();
    for expression in expressions {
        collect_expression_references(expression, &mut references);
    }
    for reference in &references {
        result = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogAnd,
            a: Box::new(result),
            b: Box::new(expression_is_known(reference)),
        };
    }
    result
}

fn collect_expression_references(
    expression: &eda_core::ir::Expr,
    references: &mut Vec<eda_core::ir::Expr>,
) {
    use eda_core::ir::Expr;
    match expression {
        Expr::Const(_) | Expr::EventTriggered { .. } | Expr::SimulationTime { .. } => {}
        Expr::Ref { .. } => references.push(expression.clone()),
        Expr::Un { a, .. }
        | Expr::Signed { e: a, .. }
        | Expr::StringLength { e: a }
        | Expr::StringCase { e: a, .. }
        | Expr::StringToRadix { e: a, .. } => collect_expression_references(a, references),
        Expr::Bin { a, b, .. } => {
            collect_expression_references(a, references);
            collect_expression_references(b, references);
        }
        Expr::Cond { c, t, f } => {
            collect_expression_references(c, references);
            collect_expression_references(t, references);
            collect_expression_references(f, references);
        }
        Expr::Concat(parts) => parts
            .iter()
            .for_each(|part| collect_expression_references(part, references)),
        Expr::Repeat { e, .. } => collect_expression_references(e, references),
        Expr::MemoryRead { indexes, .. } => indexes
            .iter()
            .for_each(|index| collect_expression_references(index, references)),
        Expr::MemoryReadPart { indexes, start, .. } => {
            indexes
                .iter()
                .for_each(|index| collect_expression_references(index, references));
            collect_expression_references(start, references);
        }
        Expr::MemorySize { .. } => {}
        Expr::AssocRead { index, .. } | Expr::AssocExists { index, .. } => {
            collect_expression_references(index, references)
        }
        Expr::Index { e, .. } => collect_expression_references(e, references),
        Expr::StringConcat { left, right } | Expr::StringCompare { left, right, .. } => {
            collect_expression_references(left, references);
            collect_expression_references(right, references);
        }
        Expr::StringGetByte { e, index } | Expr::StringPutc { e, index, .. } => {
            collect_expression_references(e, references);
            collect_expression_references(index, references);
        }
        Expr::StringSubstr { e, left, right } => {
            collect_expression_references(e, references);
            collect_expression_references(left, references);
            collect_expression_references(right, references);
        }
        Expr::StringFromRadix { value, .. } => collect_expression_references(value, references),
    }
}

fn definite_true(expression: &eda_core::ir::Expr) -> eda_core::ir::Expr {
    eda_core::ir::Expr::Bin {
        op: eda_core::ir::BinOp::CaseEq,
        a: Box::new(expression.clone()),
        b: Box::new(eda_core::ir::Expr::Const(LogicWord::ones(1))),
    }
}
