//! Procedural block, statement, assignment, and memory-file lowering.

use super::expressions::{constant_i64, continuous_assignment_delay};
use super::hierarchy::full_range;
use super::SlangAstLowerer;
use crate::ast_util::{
    bool_field, expression_symbol, resolve_memory_file_path, resolve_memory_output_path,
    slang_constant_u64, string_field,
};
use crate::types::AstPackedDimension;
use eda_core::logic::LogicWord;
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
                    let statement = body
                        .get("stmt")
                        .ok_or_else(|| "timed procedure has no statement".to_string())?;
                    let sens = self.lower_sensitivity(timing)?;
                    let statement = self.lower_statement(statement)?;
                    self.design.processes.push(eda_core::ir::Process {
                        sens,
                        body: statement,
                    });
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
        if string_field(timing, "kind").as_deref() == Some("ImplicitEvent") {
            return Ok(eda_core::ir::Sens::Comb);
        }
        if string_field(timing, "kind").as_deref() != Some("SignalEvent") {
            return Err(
                "direct slang-AST lowering supports only a single signal event sensitivity".into(),
            );
        }
        let expression = timing
            .get("expr")
            .ok_or_else(|| "signal event has no expression".to_string())?;
        let signal = self.lower_lvalue(expression)?;
        match string_field(timing, "edge").as_deref() {
            Some("PosEdge") => Ok(eda_core::ir::Sens::Posedge(signal.net)),
            Some("NegEdge") => Ok(eda_core::ir::Sens::Negedge(signal.net)),
            _ => Err("direct slang-AST lowering requires PosEdge or NegEdge sensitivity".into()),
        }
    }

    pub(super) fn lower_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let kind = string_field(statement, "kind").unwrap_or_default();
        match kind.as_str() {
            "Block" => {
                match string_field(statement, "blockKind").as_deref() {
                    None | Some("Sequential") => {}
                    Some(other) => {
                        return Err(format!(
                            "direct slang-AST lowering does not support concurrency block kind '{other}'"
                        ));
                    }
                }
                if let Some(nested) = statement.get("body") {
                    if string_field(nested, "kind").as_deref() == Some("List") {
                        return self.lower_statement(nested);
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
                    return self.lower_memory_file_call(expression, statement);
                }
                if string_field(expression, "kind").as_deref() != Some("Assignment") {
                    return Err(
                        "direct slang-AST lowering supports only assignment expression statements"
                            .into(),
                    );
                }
                self.lower_assignment(expression)
            }
            "Assignment" => self.lower_assignment(statement),
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
            other => Err(format!(
                "direct slang-AST lowering does not support statement kind '{other}'"
            )),
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
