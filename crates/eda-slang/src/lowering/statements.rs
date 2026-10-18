//! Procedural block, statement, assignment, and memory-file lowering.

use super::expressions::{constant_i64, continuous_assignment_delay};
use super::hierarchy::full_range;
use super::SlangAstLowerer;
use crate::ast_util::{
    bool_field, expression_symbol, resolve_memory_file_path, resolve_memory_output_path,
    slang_constant_u64, string_field, with_source_span,
};
use crate::types::AstPackedDimension;
use eda_core::logic::{LogicValue, LogicWord};
use serde_json::Value;

impl SlangAstLowerer {
    pub(super) fn lower_continuous_assignment(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let delay = continuous_assignment_delay(member)?;
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
        let rhs = self.lower_expression(right)?;
        self.design
            .assigns
            .push(eda_core::ir::Assign { lv, e: rhs, delay });
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
        match procedure_kind.as_str() {
            "Initial" => {
                let statement = self.lower_statement(body)?;
                self.design.initials.push(statement);
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
        self.lower_statement_inner(statement)
            .map_err(|error| with_source_span(statement, error))
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
                        let body = statement
                            .get("body")
                            .ok_or_else(|| "fork block has no body".to_string())?;
                        let list = body
                            .get("list")
                            .and_then(serde_json::Value::as_array)
                            .ok_or_else(|| "fork block body is not a statement list".to_string())?;
                        let branches = list
                            .iter()
                            .map(|branch| self.lower_statement(branch))
                            .collect::<Result<Vec<_>, _>>()?;
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
                            statements.push(self.lower_statement(item)?);
                        }
                        return Ok(Stmt::Block(statements));
                    }
                    let nested = self.lower_statement(nested)?;
                    return Ok(Stmt::Block(vec![nested]));
                }
                Ok(Stmt::Block(Vec::new()))
            }
            "List" => {
                let mut statements = Vec::new();
                if let Some(items) = statement.get("list").and_then(Value::as_array) {
                    for item in items {
                        statements.push(self.lower_statement(item)?);
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
            "Conditional" => {
                if string_field(statement, "check").as_deref() != Some("None") {
                    let check = string_field(statement, "check").unwrap_or_default();
                    return Err(format!(
                        "direct slang-AST lowering does not support {check} conditional semantics"
                    ));
                }
                let conditions = statement
                    .get("conditions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "conditional statement has no conditions".to_string())?;
                if conditions.len() != 1 {
                    return Err(
                        "direct slang-AST lowering supports one unqualified if condition".into(),
                    );
                }
                let condition = conditions
                    .first()
                    .and_then(|condition| condition.get("expr"))
                    .ok_or_else(|| {
                        "conditional statement has no condition expression".to_string()
                    })?;
                let true_statement = statement
                    .get("ifTrue")
                    .ok_or_else(|| "conditional statement has no true branch".to_string())?;
                let false_statement = match statement.get("ifFalse") {
                    Some(statement) => Some(Box::new(self.lower_statement(statement)?)),
                    None => None,
                };
                Ok(Stmt::If {
                    c: self.lower_expression(condition)?,
                    t: Box::new(self.lower_statement(true_statement)?),
                    f: false_statement,
                })
            }
            "Case" => {
                let condition = string_field(statement, "condition").unwrap_or_default();
                if condition != "Normal" {
                    return Err(format!(
                        "direct slang-AST lowering does not support {condition} case semantics"
                    ));
                }
                let check = string_field(statement, "check").unwrap_or_default();
                if check != "None" {
                    return Err(format!(
                        "direct slang-AST lowering does not support {check} case semantics"
                    ));
                }
                let expression = statement
                    .get("expr")
                    .ok_or_else(|| "case statement has no expression".to_string())?;
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
                let default = match statement.get("defaultCase") {
                    Some(statement) => Some(Box::new(self.lower_statement(statement)?)),
                    None => None,
                };
                Ok(Stmt::Case {
                    e: self.lower_expression(expression)?,
                    arms,
                    dflt: default,
                })
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
                    let mut nets = Vec::with_capacity(events.len());
                    for event in events {
                        if string_field(event, "edge").as_deref() != Some("None") {
                            return Err(
                                "event-list statement controls support any-edge events".into()
                            );
                        }
                        let expression = event
                            .get("expr")
                            .ok_or_else(|| "event list entry has no expression".to_string())?;
                        let signal = self.lower_lvalue(expression)?;
                        nets.push(sensitivity_net(&signal)?);
                    }
                    return Ok(Stmt::EventWait {
                        nets,
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
                    let lvalue = self.lower_lvalue(event)?;
                    if lvalue.range.width() != 1 {
                        return Err("event control expression must be scalar".into());
                    }
                    return Ok(Stmt::EventWait {
                        nets: vec![sensitivity_net(&lvalue)?],
                        body: Box::new(self.lower_statement(body)?),
                    });
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
                    body: Box::new(self.lower_statement(body)?),
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
                    body: Box::new(self.lower_statement(body)?),
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
                let first_execution = self.lower_statement(body)?;
                let repeated_execution = Stmt::While {
                    cond: self.lower_expression(condition)?,
                    body: Box::new(self.lower_statement(body)?),
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
                let loop_body = self.lower_statement(body)?;
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
        slang_constant_u64(expression)
            .map_err(|_| "delay control must be an integer constant".to_string())
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
        if let Some(statement) = self.memory_struct_member_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.dynamic_packed_element_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some((memory, indexes)) = self.memory_element_select(left)? {
            let expression = self.lower_expression(right)?;
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
        let selected = left
            .get("value")
            .ok_or_else(|| "struct member access has no selected value".to_string())?;
        let Some((memory, indexes)) = self.memory_element_select(selected)? else {
            return Ok(None);
        };
        let member = string_field(left, "member")
            .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "struct member access has no member".to_string())?;
        let field = self.struct_field_for_type(selected, &member)?;
        let element_width = self.design.memories[memory].element_width;
        if field.offset + field.width > element_width {
            return Err(format!(
                "struct member '{member}' exceeds memory element width {element_width}"
            ));
        }

        let value = self.lower_expression(right)?;
        if value.width() != field.width {
            return Err(format!(
                "memory struct member '{member}' width {} does not match assignment width {}",
                field.width,
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
        if field.offset + field.width < element_width {
            parts.push(Expr::Index {
                e: Box::new(old.clone()),
                msb: element_width - 1,
                lsb: field.offset + field.width,
            });
        }
        parts.push(value);
        if field.offset > 0 {
            parts.push(Expr::Index {
                e: Box::new(old),
                msb: field.offset - 1,
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
}

fn sensitivity_net(signal: &eda_core::ir::LValue) -> Result<usize, String> {
    match signal.range {
        eda_core::ir::Range::Bit(bit) => Ok(signal.net + bit),
        eda_core::ir::Range::Span { msb, lsb } if msb == lsb => Ok(signal.net + lsb),
        _ => Err("sensitivity expression must be one bit wide".into()),
    }
}
