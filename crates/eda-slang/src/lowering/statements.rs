//! Procedural block, statement, assignment, and memory-file lowering.

use super::expressions::{constant_i64, continuous_assignment_delay};
use super::hierarchy::full_range;
use super::{declaration_name, AstLoweredSymbol, SlangAstLowerer};
use crate::ast_util::{
    bool_field, expression_symbol, resolve_memory_file_path, resolve_memory_output_path,
    slang_constant_u64, string_field, unwrap_conversion, usize_field, with_source_span,
};
use crate::types::{
    is_dynamic_container_type, memory_type, simple_type_shape, simple_type_signed,
    AstPackedDimension,
};
use eda_core::ir::Stmt;
use eda_core::logic::{LogicValue, LogicWord};
use serde_json::Value;

struct ConcurrentProperty {
    antecedent: eda_core::ir::Expr,
    antecedent_sequence: Option<ConcurrentAntecedentSequence>,
    antecedent_goto: Option<(eda_core::ir::Expr, usize)>,
    antecedent_consecutive_range: Option<(eda_core::ir::Expr, usize, usize)>,
    antecedent_goto_range: Option<(eda_core::ir::Expr, usize, usize)>,
    antecedent_nonconsecutive_range: Option<(eda_core::ir::Expr, usize, usize)>,
    antecedent_immediate: Option<eda_core::ir::Expr>,
    local_samples: Vec<ConcurrentLocalSample>,
    consequent: eda_core::ir::Expr,
    delay: ConcurrentDelay,
}

#[derive(Clone)]
struct ConcurrentAntecedentSequence {
    expressions: Vec<eda_core::ir::Expr>,
    delays: Vec<usize>,
    range: Option<(usize, usize)>,
    range_element: usize,
    local_samples: Vec<ConcurrentLocalSample>,
}

#[derive(Clone)]
struct ConcurrentLocalSample {
    width: usize,
    signed: bool,
    trigger: eda_core::ir::Expr,
    value: eda_core::ir::Expr,
    nets: Vec<usize>,
    histories: Vec<ConcurrentLocalHistory>,
}

#[derive(Clone)]
struct ConcurrentLocalHistory {
    trigger: eda_core::ir::Expr,
    nets: Vec<usize>,
}

struct ScalarLocalRangeLowering {
    prefix: eda_core::ir::Expr,
    prefix_miss: eda_core::ir::Expr,
    start: usize,
    end: usize,
    boundaries: Vec<eda_core::ir::Expr>,
    prefix_includes_antecedent: bool,
    samples: Vec<ConcurrentLocalSample>,
}

struct ScalarLocalNonterminalRangeLowering {
    prefix: eda_core::ir::Expr,
    prefix_miss: eda_core::ir::Expr,
    range_start: usize,
    range_end: usize,
    suffix_delays: Vec<usize>,
    middle_boundaries: Vec<eda_core::ir::Expr>,
    suffix_boundaries: Vec<Vec<eda_core::ir::Expr>>,
    final_boundaries: Vec<eda_core::ir::Expr>,
    samples: Vec<ConcurrentLocalSample>,
}

struct ScalarLocalAssignment {
    local_index: usize,
    element: usize,
    age: usize,
    value: ScalarLocalAssignmentValue,
}

#[derive(Clone)]
enum ScalarLocalAssignmentValue {
    Ready(eda_core::ir::Expr),
    Pending(serde_json::Value),
}

fn antecedent_reduction(expression: &eda_core::ir::Expr) -> eda_core::ir::Expr {
    eda_core::ir::Expr::Un {
        op: eda_core::ir::UnOp::OrR,
        a: Box::new(expression.clone()),
    }
}

fn json_references_local_symbol(value: &serde_json::Value, local_name: &str) -> bool {
    if let Some(symbol) = value.get("symbol").and_then(Value::as_str) {
        if symbol == local_name || symbol.ends_with(&format!(" {local_name}")) {
            return true;
        }
    }
    match value {
        Value::Array(values) => values
            .iter()
            .any(|nested| json_references_local_symbol(nested, local_name)),
        Value::Object(fields) => fields
            .values()
            .any(|nested| json_references_local_symbol(nested, local_name)),
        _ => false,
    }
}

fn rewrite_sequence_local_references(
    value: &serde_json::Value,
    local_names: &[String],
    property_index: usize,
    age: usize,
) -> (serde_json::Value, Vec<usize>) {
    let mut references = Vec::new();
    if let Value::Object(fields) = value {
        if string_field(value, "kind").as_deref() == Some("NamedValue") {
            if let Some(local_index) = expression_symbol(value).and_then(|symbol| {
                local_names
                    .iter()
                    .position(|name| symbol == *name || symbol.ends_with(&format!(" {name}")))
            }) {
                let mut rewritten = fields.clone();
                rewritten.insert(
                    "symbol".into(),
                    Value::String(format!("__eda_slseq_{property_index}_{local_index}_{age}")),
                );
                references.push(local_index);
                return (Value::Object(rewritten), references);
            }
        }
        let mut rewritten = serde_json::Map::new();
        for (key, nested) in fields {
            let (nested, mut nested_references) =
                rewrite_sequence_local_references(nested, local_names, property_index, age);
            references.append(&mut nested_references);
            rewritten.insert(key.clone(), nested);
        }
        return (Value::Object(rewritten), references);
    }
    if let Value::Array(values) = value {
        let mut rewritten = Vec::with_capacity(values.len());
        for nested in values {
            let (nested, mut nested_references) =
                rewrite_sequence_local_references(nested, local_names, property_index, age);
            references.append(&mut nested_references);
            rewritten.push(nested);
        }
        return (Value::Array(rewritten), references);
    }
    (value.clone(), references)
}

#[derive(Clone, Debug)]
enum ConcurrentDelay {
    SameCycle,
    Fixed(usize),
    Range(usize, usize),
    Consecutive(usize),
    Goto(usize),
    Nonconsecutive(usize),
    ConsecutiveRangeSequence {
        prefix: Box<eda_core::ir::Expr>,
        repeated: Box<eda_core::ir::Expr>,
        suffix: Box<eda_core::ir::Expr>,
        start: usize,
        end: usize,
    },
    GotoRangeSequence {
        prefix: Box<eda_core::ir::Expr>,
        repeated: Box<eda_core::ir::Expr>,
        suffix: Box<eda_core::ir::Expr>,
        start: usize,
        end: usize,
    },
    NonconsecutiveRangeSequence {
        prefix: Box<eda_core::ir::Expr>,
        repeated: Box<eda_core::ir::Expr>,
        suffix: Box<eda_core::ir::Expr>,
        start: usize,
        end: usize,
    },
    Sequence(Vec<eda_core::ir::Expr>, Vec<usize>),
    SequenceOrImmediate(Vec<eda_core::ir::Expr>, Vec<usize>, Box<eda_core::ir::Expr>),
    PrefixRange(
        Box<eda_core::ir::Expr>,
        Box<eda_core::ir::Expr>,
        usize,
        usize,
        Vec<eda_core::ir::Expr>,
        bool,
    ),
    NonterminalPrefixRange {
        prefix: Box<eda_core::ir::Expr>,
        prefix_miss: Box<eda_core::ir::Expr>,
        range_start: usize,
        range_end: usize,
        suffix_delays: Vec<usize>,
        middle: Box<eda_core::ir::Expr>,
        suffix_expressions: Vec<eda_core::ir::Expr>,
    },
    LocalNonterminalPrefixRange {
        prefix: Box<eda_core::ir::Expr>,
        prefix_miss: Box<eda_core::ir::Expr>,
        range_start: usize,
        range_end: usize,
        suffix_delays: Vec<usize>,
        middle_boundaries: Vec<eda_core::ir::Expr>,
        suffix_boundaries: Vec<Vec<eda_core::ir::Expr>>,
        final_boundaries: Vec<eda_core::ir::Expr>,
    },
}

impl SlangAstLowerer {
    pub(super) fn collect_named_block_flags(
        &mut self,
        statement: &serde_json::Value,
    ) -> Result<(), String> {
        let definitions = self.subroutines.values().cloned().collect::<Vec<_>>();
        self.named_disable_seen = Self::statement_contains_disable(statement)
            || definitions.iter().any(|definition| {
                definition
                    .get("body")
                    .is_some_and(Self::statement_contains_disable)
            });
        if self.named_disable_seen {
            for definition in definitions {
                self.register_named_subroutine_flag(&definition)?;
            }
        }
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
        if kind == "Subroutine" && self.named_disable_seen {
            self.register_named_subroutine_flag(statement)?;
        }
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

    fn register_named_subroutine_flag(&mut self, definition: &Value) -> Result<(), String> {
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
            return Ok(());
        }
        let name = declaration_name(definition, "Subroutine")?;
        let address = definition
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("subroutine '{name}' has no symbol address"))?;
        let named_id = format!("{address} {name}");
        if self.named_block_flags.contains_key(&named_id) {
            return Ok(());
        }
        let base = self.design.n_nets;
        self.design.n_nets += 1;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: format!("__disable_{address}_{name}"),
            width: 1,
            is_reg: true,
            is_signed: false,
            base,
        });
        self.named_block_flags.insert(named_id, base);
        Ok(())
    }

    fn statement_contains_disable(statement: &serde_json::Value) -> bool {
        if string_field(statement, "kind").as_deref() == Some("Disable") {
            return true;
        }
        match statement {
            serde_json::Value::Object(fields) => {
                fields.values().any(Self::statement_contains_disable)
            }
            serde_json::Value::Array(values) => values.iter().any(Self::statement_contains_disable),
            _ => false,
        }
    }

    fn reject_unsafe_named_control(
        &self,
        statement: &serde_json::Value,
        inside_named_block: bool,
    ) -> Result<(), String> {
        let kind = string_field(statement, "kind").unwrap_or_default();
        if kind == "Block" {
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

    fn prepare_foreach_loop_iterators(&mut self, value: &serde_json::Value) -> Result<(), String> {
        if let Value::Array(values) = value {
            for nested in values {
                self.prepare_foreach_loop_iterators(nested)?;
            }
            return Ok(());
        }
        let Value::Object(fields) = value else {
            return Ok(());
        };
        if string_field(value, "kind").as_deref() == Some("ForeachLoop") {
            let loop_dims = value
                .get("loopDims")
                .and_then(Value::as_array)
                .ok_or_else(|| "foreach loop has no dimensions".to_string())?;
            for (dimension_index, loop_dim) in loop_dims.iter().enumerate() {
                let variable = loop_dim
                    .get("var")
                    .ok_or_else(|| "foreach dimension has no iterator".to_string())?;
                let name = string_field(variable, "name")
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| "foreach iterator has no name".to_string())?;
                let address = variable
                    .get("addr")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "foreach iterator has no address".to_string())?;
                let symbol_id = format!("{address} {name}");
                if self.symbols.contains_key(&symbol_id) {
                    continue;
                }
                let base = self.design.n_nets;
                self.design.n_nets += 32;
                self.design.net_decls.push(eda_core::ir::NetDecl {
                    name: format!("__foreach_iterator_{address}_{dimension_index}_{name}"),
                    width: 32,
                    is_reg: true,
                    is_signed: true,
                    base,
                });
                self.symbols.insert(
                    symbol_id,
                    AstLoweredSymbol {
                        base,
                        width: 32,
                        signed: true,
                        bounds: Some((31, 0)),
                        dimensions: Vec::new(),
                        memory: None,
                        struct_fields: std::collections::BTreeMap::new(),
                    },
                );
            }
        }
        for nested in fields.values() {
            self.prepare_foreach_loop_iterators(nested)?;
        }
        Ok(())
    }

    pub(super) fn lower_continuous_assignment(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let delay = self.scale_delay_constant(
            continuous_assignment_delay(member)?.unwrap_or(super::DelayConstant::Integer(0)),
        )?;
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
        self.record_implicit_type_mismatch(&lv, right)?;
        let rhs = self.lower_real_assignment_expression(left, right)?;
        self.design
            .assigns
            .push(eda_core::ir::Assign { lv, e: rhs, delay });
        Ok(())
    }

    /// Lower a built-in gate primitive instance (`and`, `or`, `xor`, `not`,
    /// `buf`, `nand`, `nor`, `xnor`) to a continuous assignment. Slang exposes
    /// the gate type as `primitiveType` and the terminals as `ports`; the
    /// first terminal is the output, represented as an Assignment whose left
    /// side is the output lvalue.
    pub(super) fn lower_primitive_instance(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        use eda_core::ir::{BinOp, Expr, UnOp};
        let primitive = string_field(member, "primitiveType")
            .unwrap_or_default()
            .split_whitespace()
            .next_back()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let ports = member
            .get("ports")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "primitive instance has no ports".to_string())?;
        let first = ports
            .first()
            .ok_or_else(|| "primitive instance has no output terminal".to_string())?;
        let output = if string_field(first, "kind").as_deref() == Some("Assignment") {
            first
                .get("left")
                .ok_or_else(|| "primitive output assignment has no left side".to_string())?
        } else {
            first
        };
        let inputs = ports
            .iter()
            .skip(1)
            .map(|port| {
                if string_field(port, "kind").as_deref() == Some("EmptyArgument") {
                    return Err("primitive instance has an empty input terminal".to_string());
                }
                self.lower_expression(port)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let fold = |op: BinOp| {
            inputs
                .iter()
                .cloned()
                .reduce(|a, b| Expr::Bin {
                    op,
                    a: Box::new(a),
                    b: Box::new(b),
                })
                .ok_or_else(|| format!("primitive '{primitive}' has no input terminals"))
        };
        let expression = match primitive.as_str() {
            "buf" if inputs.len() == 1 => inputs[0].clone(),
            "not" if inputs.len() == 1 => Expr::Un {
                op: UnOp::Not,
                a: Box::new(inputs[0].clone()),
            },
            "and" => fold(BinOp::And)?,
            "or" => fold(BinOp::Or)?,
            "xor" => fold(BinOp::Xor)?,
            "nand" => Expr::Un {
                op: UnOp::Not,
                a: Box::new(fold(BinOp::And)?),
            },
            "nor" => Expr::Un {
                op: UnOp::Not,
                a: Box::new(fold(BinOp::Or)?),
            },
            "xnor" => Expr::Un {
                op: UnOp::Not,
                a: Box::new(fold(BinOp::Xor)?),
            },
            other => {
                return Err(format!(
                    "direct slang-AST lowering does not support primitive '{other}'"
                ))
            }
        };
        let lv = self.lower_lvalue(output)?;
        self.design.assigns.push(eda_core::ir::Assign {
            lv,
            e: expression,
            delay: 0,
        });
        Ok(())
    }

    pub(super) fn record_implicit_type_mismatch(
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
        if string_field(operand, "kind").as_deref() == Some("Streaming") {
            // Streaming conversions are sized by the streaming lowering path;
            // their operand has void type and is not an ordinary implicit
            // integral conversion.
            return Ok(());
        }

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
        let (target_shape, target_signed, _) = self.net_type(&target_type)?;
        let (source_shape, source_signed, _) = self.net_type(&source_type)?;
        let width_difference = target_shape.width.abs_diff(source_shape.width);
        let first_bit = match lvalue.range {
            eda_core::ir::Range::Bit(bit) => lvalue.net + bit,
            eda_core::ir::Range::Span { msb, lsb } => lvalue.net + lsb.min(msb),
        };
        let bit_name = self.design.net_name(first_bit);
        let net = bit_name.split('[').next().unwrap_or(&bit_name).to_string();

        if width_difference != 0 {
            self.record_type_mismatch(eda_core::ir::LintIssue {
                kind: eda_core::ir::LintIssueKind::WidthMismatch,
                net: net.clone(),
                count: width_difference,
            });
        }
        if target_signed != source_signed {
            self.record_type_mismatch(eda_core::ir::LintIssue {
                kind: eda_core::ir::LintIssueKind::SignednessMismatch,
                net,
                count: 1,
            });
        }
        Ok(())
    }

    pub(super) fn record_type_mismatch(&mut self, issue: eda_core::ir::LintIssue) {
        if let Some(existing) = self
            .design
            .recorded_type_mismatches
            .iter_mut()
            .find(|existing| existing.kind == issue.kind && existing.net == issue.net)
        {
            existing.count = existing.count.max(issue.count);
        } else {
            self.design.recorded_type_mismatches.push(issue);
        }
    }

    pub(super) fn lower_procedural_block(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let procedure_kind = string_field(member, "procedureKind").unwrap_or_default();
        let body = member
            .get("body")
            .ok_or_else(|| "slang ProceduralBlock has no body".to_string())?;
        if !matches!(procedure_kind.as_str(), "Initial" | "Final") {
            let local_ids = string_field(body, "block")
                .as_deref()
                .map(str::trim)
                .and_then(|block| self.local_blocks.get(block))
                .cloned()
                .unwrap_or_default();
            if local_ids
                .iter()
                .any(|symbol| self.local_static_symbols.contains(symbol))
            {
                return Err(
                    "static procedure locals are supported only in initial or final blocks".into(),
                );
            }
        }
        self.named_disable_seen = false;
        self.collect_named_block_flags(body)?;
        self.active_named_blocks.borrow_mut().clear();
        self.active_assertion_scopes.borrow_mut().clear();
        self.validate_manual_covergroup_lifetime(body)?;
        self.prepare_foreach_loop_iterators(body)?;
        if matches!(
            procedure_kind.as_str(),
            "Always" | "AlwaysFF" | "AlwaysLatch"
        ) && string_field(body, "kind").as_deref() == Some("ConcurrentAssertion")
        {
            return self.lower_concurrent_assertion(body);
        }
        if matches!(
            procedure_kind.as_str(),
            "Always" | "AlwaysFF" | "AlwaysLatch"
        ) && string_field(body, "kind").as_deref() == Some("Block")
        {
            if let Some(assertion) = body.get("body") {
                if string_field(assertion, "kind").as_deref() == Some("ConcurrentAssertion") {
                    let scope = string_field(body, "block").filter(|scope| !scope.is_empty());
                    if let Some(scope) = scope {
                        self.active_assertion_scopes.borrow_mut().push(scope);
                    }
                    let result = self.lower_concurrent_assertion(assertion);
                    if body
                        .get("block")
                        .is_some_and(|scope| scope.as_str().is_some_and(|scope| !scope.is_empty()))
                    {
                        self.active_assertion_scopes.borrow_mut().pop();
                    }
                    return result;
                }
            }
        }
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
                        self.past_sampling_allowed = Self::sensitivity_has_clock_edge(&sens);
                        self.past_sampling_event = match &sens {
                            eda_core::ir::Sens::Posedge(_) | eda_core::ir::Sens::Negedge(_) => {
                                Some(sens.clone())
                            }
                            _ => None,
                        };
                        let statement = self.lower_statement(statement)?;
                        self.past_sampling_allowed = false;
                        self.past_sampling_event = None;
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

    pub(super) fn lower_concurrent_assertion(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let property_spec = member
            .get("propertySpec")
            .ok_or_else(|| "concurrent assertion has no property spec".to_string())?;
        let mut sequence_carried_property_expr = None;
        let sequence_carried_local_instance;
        let clocking = match string_field(property_spec, "kind").as_deref() {
            Some("Clocking") => property_spec
                .get("clocking")
                .ok_or_else(|| "clocked property has no clocking event".to_string())?,
            Some("Simple") => {
                let expression = property_spec
                    .get("expr")
                    .ok_or_else(|| "simple property has no expression".to_string())?;
                let instance =
                    if string_field(expression, "kind").as_deref() == Some("AssertionInstance") {
                        Some(expression)
                    } else {
                        expression.get("expr")
                    }
                    .filter(|instance| {
                        string_field(instance, "kind").as_deref() == Some("AssertionInstance")
                    })
                    .ok_or_else(|| {
                        "direct concurrent assertion lowering requires an explicit clocking event"
                            .to_string()
                    })?;
                let has_local_variables = instance
                    .get("localVars")
                    .and_then(Value::as_array)
                    .is_some_and(|locals| !locals.is_empty());
                let body = instance
                    .get("body")
                    .ok_or_else(|| "assertion instance has no elaborated body".to_string())?;
                if string_field(body, "kind").as_deref() != Some("Clocking") {
                    return Err(
                        "direct concurrent assertion lowering requires an explicit clocking event"
                            .into(),
                    );
                }
                let body_expression = body
                    .get("expr")
                    .ok_or_else(|| "clocked sequence has no expression".to_string())?;
                if has_local_variables {
                    let mut promoted_instance = instance.clone();
                    if let Value::Object(fields) = &mut promoted_instance {
                        fields.insert("body".into(), body_expression.clone());
                    }
                    sequence_carried_local_instance = Some(promoted_instance);
                    sequence_carried_property_expr = sequence_carried_local_instance.as_ref();
                } else {
                    sequence_carried_property_expr = Some(body_expression);
                }
                body.get("clocking")
                    .ok_or_else(|| "clocked sequence has no clocking event".to_string())?
            }
            _ => {
                return Err(
                    "direct concurrent assertion lowering requires an explicit clocking event"
                        .into(),
                )
            }
        };
        let sensitivity = self.lower_sensitivity(clocking)?;
        if !matches!(
            sensitivity,
            eda_core::ir::Sens::Posedge(_) | eda_core::ir::Sens::Negedge(_)
        ) {
            return Err("concurrent assertion requires a single posedge or negedge clock".into());
        }

        let mut property_expr = sequence_carried_property_expr.unwrap_or_else(|| {
            property_spec
                .get("expr")
                .expect("validated concurrent assertion property has an expression")
        });
        let mut disable = None;
        if string_field(property_expr, "kind").as_deref() == Some("DisableIff") {
            let condition = property_expr
                .get("condition")
                .ok_or_else(|| "disable iff has no condition".to_string())?;
            disable = Some(self.lower_expression(condition)?);
            property_expr = property_expr
                .get("expr")
                .ok_or_else(|| "disable iff has no property expression".to_string())?;
        }
        let pass_action = self.lower_optional_assertion_action(member.get("ifTrue"))?;
        let fail_action = self.lower_optional_assertion_action(member.get("ifFalse"))?;
        let pass_action_template = pass_action.clone();
        let fail_action_template = fail_action.clone();
        let property_index = self.design.processes.len();
        let property =
            self.lower_simple_concurrent_property(member, property_index, property_expr)?;
        let kind = match string_field(member, "assertionKind").as_deref() {
            Some("Assert") => eda_core::ir::ImmediateAssertionKind::Assert,
            Some("Assume") => eda_core::ir::ImmediateAssertionKind::Assume,
            Some("CoverProperty") => eda_core::ir::ImmediateAssertionKind::Cover,
            other => return Err(format!("unsupported concurrent assertion kind {other:?}")),
        };
        let assertion_scope = self.current_assertion_scope();
        let assertion = Stmt::ImmediateAssertion {
            kind,
            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
            scope: assertion_scope.clone(),
            cond: property.consequent.clone(),
            pass_action,
            fail_action,
        };
        let mut pending_nets = Vec::new();
        let mut statements = Vec::new();
        if let Some((expression, start, end)) = property.antecedent_nonconsecutive_range.clone() {
            let sampled = antecedent_reduction(&expression);
            let not_sampled = eda_core::ir::Expr::Un {
                op: eda_core::ir::UnOp::Not,
                a: Box::new(sampled.clone()),
            };
            for seen_count in 1..=end {
                pending_nets.push(self.declare_property_pending_net(
                    member,
                    property_index,
                    seen_count - 1,
                ));
            }
            let references = pending_nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                })
                .collect::<Vec<_>>();
            let true_endpoint = (start..=end)
                .map(|count| {
                    if count == 1 {
                        sampled.clone()
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(sampled.clone()),
                        }
                    }
                })
                .reduce(|left, right| eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(left),
                    b: Box::new(right),
                })
                .ok_or_else(|| "nonconsecutive antecedent has no true endpoints".to_string())?;
            let false_endpoint = references[start - 1..end]
                .iter()
                .cloned()
                .reduce(|left, right| eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(left),
                    b: Box::new(right),
                })
                .ok_or_else(|| "nonconsecutive antecedent has no false endpoints".to_string())?;
            let endpoint = eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::LogOr,
                a: Box::new(true_endpoint),
                b: Box::new(false_endpoint),
            };
            statements.push(Stmt::If {
                c: endpoint,
                t: Box::new(assertion),
                f: None,
            });
            let mut updates = Vec::with_capacity(pending_nets.len());
            updates.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: pending_nets[0],
                    decl_width: 1,
                    range: full_range(1),
                },
                e: eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(sampled.clone()),
                    b: Box::new(eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(references[0].clone()),
                        b: Box::new(not_sampled.clone()),
                    }),
                },
            });
            for seen_count in 2..=end {
                let advance = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(references[seen_count - 2].clone()),
                    b: Box::new(sampled.clone()),
                };
                let retain = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(references[seen_count - 1].clone()),
                    b: Box::new(not_sampled.clone()),
                };
                updates.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: pending_nets[seen_count - 1],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(advance),
                        b: Box::new(retain),
                    },
                });
            }
            statements.push(Stmt::Block(updates));
        } else if let Some((expression, start, end)) = property.antecedent_goto_range.clone() {
            let sampled = antecedent_reduction(&expression);
            for seen in 0..end - 1 {
                pending_nets.push(self.declare_property_pending_net(member, property_index, seen));
            }
            let references = pending_nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                })
                .collect::<Vec<_>>();
            let endpoint = (start..=end)
                .map(|count| {
                    if count == 1 {
                        sampled.clone()
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(sampled.clone()),
                        }
                    }
                })
                .reduce(|left, right| eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(left),
                    b: Box::new(right),
                })
                .ok_or_else(|| "goto antecedent range has no candidates".to_string())?;
            statements.push(Stmt::If {
                c: endpoint,
                t: Box::new(assertion),
                f: None,
            });
            let mut updates = Vec::with_capacity(pending_nets.len());
            updates.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: pending_nets[0],
                    decl_width: 1,
                    range: full_range(1),
                },
                e: eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(sampled.clone()),
                    b: Box::new(eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(references[0].clone()),
                        b: Box::new(eda_core::ir::Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: Box::new(sampled.clone()),
                        }),
                    }),
                },
            });
            for seen in 1..end - 1 {
                let advance = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(references[seen - 1].clone()),
                    b: Box::new(sampled.clone()),
                };
                let retain = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(references[seen].clone()),
                    b: Box::new(eda_core::ir::Expr::Un {
                        op: eda_core::ir::UnOp::Not,
                        a: Box::new(sampled.clone()),
                    }),
                };
                updates.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: pending_nets[seen],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(advance),
                        b: Box::new(retain),
                    },
                });
            }
            statements.push(Stmt::Block(updates));
        } else if let Some((expression, start, end)) = property.antecedent_consecutive_range.clone()
        {
            let sampled = antecedent_reduction(&expression);
            for length in 0..end - 1 {
                pending_nets.push(self.declare_property_pending_net(
                    member,
                    property_index,
                    length,
                ));
            }
            let references = pending_nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                })
                .collect::<Vec<_>>();
            let endpoint = (start..=end)
                .map(|count| {
                    if count == 1 {
                        sampled.clone()
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(sampled.clone()),
                        }
                    }
                })
                .reduce(|left, right| eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(left),
                    b: Box::new(right),
                })
                .ok_or_else(|| "consecutive antecedent range has no candidates".to_string())?;
            statements.push(Stmt::If {
                c: endpoint,
                t: Box::new(assertion),
                f: None,
            });
            let mut updates = Vec::with_capacity(pending_nets.len());
            updates.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: pending_nets[0],
                    decl_width: 1,
                    range: full_range(1),
                },
                e: sampled.clone(),
            });
            for length in 1..end - 1 {
                updates.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: pending_nets[length],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(references[length - 1].clone()),
                        b: Box::new(sampled.clone()),
                    },
                });
            }
            if !updates.is_empty() {
                statements.push(Stmt::Block(updates));
            }
        } else if let Some((expression, count)) = property.antecedent_goto.clone() {
            let sampled = antecedent_reduction(&expression);
            if count == 1 {
                statements.push(Stmt::If {
                    c: sampled,
                    t: Box::new(assertion),
                    f: None,
                });
            } else {
                for seen in 0..count - 1 {
                    pending_nets.push(self.declare_property_pending_net(
                        member,
                        property_index,
                        seen,
                    ));
                }
                let references = pending_nets
                    .iter()
                    .map(|&net| eda_core::ir::Expr::Ref {
                        net,
                        decl_width: 1,
                        range: full_range(1),
                        signed: false,
                    })
                    .collect::<Vec<_>>();
                let endpoint = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(references[count - 2].clone()),
                    b: Box::new(sampled.clone()),
                };
                statements.push(Stmt::If {
                    c: endpoint,
                    t: Box::new(assertion),
                    f: None,
                });
                let mut updates = Vec::with_capacity(pending_nets.len());
                updates.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: pending_nets[0],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(sampled.clone()),
                        b: Box::new(eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[0].clone()),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(sampled.clone()),
                            }),
                        }),
                    },
                });
                for seen in 1..count - 1 {
                    let advance = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(references[seen - 1].clone()),
                        b: Box::new(sampled.clone()),
                    };
                    let retain = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(references[seen].clone()),
                        b: Box::new(eda_core::ir::Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: Box::new(sampled.clone()),
                        }),
                    };
                    updates.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[seen],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(advance),
                            b: Box::new(retain),
                        },
                    });
                }
                statements.push(Stmt::Block(updates));
            }
        } else if let Some(antecedent_sequence) = property.antecedent_sequence.clone() {
            let expressions = antecedent_sequence.expressions;
            let delays = antecedent_sequence.delays;
            let mut cumulative = Vec::with_capacity(expressions.len());
            let mut total_delay = 0_usize;
            for delay in &delays {
                total_delay = total_delay
                    .checked_add(*delay)
                    .ok_or_else(|| "antecedent sequence delay total overflows".to_string())?;
                cumulative.push(total_delay);
            }
            let final_index = expressions.len() - 1;
            let nonterminal_suffix = antecedent_sequence
                .range
                .is_some_and(|_| antecedent_sequence.range_element + 2 == expressions.len());
            let raw_delay = if nonterminal_suffix {
                cumulative[antecedent_sequence.range_element]
            } else {
                total_delay
            };
            for stage in 0..raw_delay {
                pending_nets.push(self.declare_property_pending_net(member, property_index, stage));
            }
            let references = pending_nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                })
                .collect::<Vec<_>>();
            let antecedent_match = if nonterminal_suffix {
                let suffix_delay = delays[final_index];
                let suffix_base = pending_nets.len();
                let mut suffix_references = Vec::new();
                for stage in 0..suffix_delay {
                    let net = self.declare_property_pending_net(
                        member,
                        property_index,
                        suffix_base + stage,
                    );
                    pending_nets.push(net);
                    suffix_references.push(eda_core::ir::Expr::Ref {
                        net,
                        decl_width: 1,
                        range: full_range(1),
                        signed: false,
                    });
                }
                eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(suffix_references[suffix_delay - 1].clone()),
                    b: Box::new(expressions[final_index].clone()),
                }
            } else {
                let final_candidate = Self::antecedent_possible_boundaries(
                    final_index,
                    &cumulative,
                    antecedent_sequence.range,
                    antecedent_sequence.range_element,
                )?
                .into_iter()
                .map(|boundary| {
                    Self::antecedent_candidate_at_boundary(
                        boundary,
                        &property.antecedent,
                        &references,
                    )
                })
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .reduce(|left, right| eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(left),
                    b: Box::new(right),
                })
                .ok_or_else(|| "antecedent range has no candidates".to_string())?;
                eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(final_candidate),
                    b: Box::new(expressions[final_index].clone()),
                }
            };
            let antecedent_match = if let Some(immediate) = &property.antecedent_immediate {
                eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogOr,
                    a: Box::new(antecedent_match),
                    b: Box::new(immediate.clone()),
                }
            } else {
                antecedent_match
            };
            let mut base_antecedent_shifts = if nonterminal_suffix {
                let range_index = antecedent_sequence.range_element;
                Self::antecedent_sequence_shifts(
                    &property.antecedent,
                    &expressions[..=range_index],
                    &cumulative[..=range_index],
                    antecedent_sequence.range,
                    antecedent_sequence.range_element,
                    &references,
                    &pending_nets[..raw_delay],
                )?
            } else {
                Self::antecedent_sequence_shifts(
                    &property.antecedent,
                    &expressions,
                    &cumulative,
                    antecedent_sequence.range,
                    antecedent_sequence.range_element,
                    &references,
                    &pending_nets[..raw_delay],
                )?
            };
            if nonterminal_suffix {
                let range_index = antecedent_sequence.range_element;
                let range_boundaries = Self::antecedent_possible_boundaries(
                    range_index,
                    &cumulative,
                    antecedent_sequence.range,
                    antecedent_sequence.range_element,
                )?;
                let range_match = range_boundaries
                    .into_iter()
                    .map(|boundary| {
                        Self::antecedent_candidate_at_boundary(
                            boundary,
                            &property.antecedent,
                            &references,
                        )
                    })
                    .collect::<Result<Vec<_>, String>>()?
                    .into_iter()
                    .map(|candidate| eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(candidate),
                        b: Box::new(expressions[range_index].clone()),
                    })
                    .reduce(|left, right| eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(left),
                        b: Box::new(right),
                    })
                    .ok_or_else(|| "nonterminal antecedent range has no candidates".to_string())?;
                let suffix_delay = delays[final_index];
                let suffix_base = raw_delay;
                let suffix_references = pending_nets[suffix_base..suffix_base + suffix_delay]
                    .iter()
                    .map(|&net| eda_core::ir::Expr::Ref {
                        net,
                        decl_width: 1,
                        range: full_range(1),
                        signed: false,
                    })
                    .collect::<Vec<_>>();
                for stage in (1..suffix_delay).rev() {
                    base_antecedent_shifts.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[suffix_base + stage],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: suffix_references[stage - 1].clone(),
                    });
                }
                base_antecedent_shifts.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: pending_nets[suffix_base],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: antecedent_reduction(&range_match),
                });
            }
            let mut shift_statements = Vec::new();

            let special_consequent_handled =
                if let ConcurrentDelay::Sequence(ref consequent_expressions, ref delays) =
                    property.delay
                {
                    let mut consequent_cumulative =
                        Vec::with_capacity(consequent_expressions.len());
                    let mut consequent_total_delay = 0_usize;
                    for delay in delays {
                        consequent_total_delay =
                            consequent_total_delay.checked_add(*delay).ok_or_else(|| {
                                "composed consequent sequence delay total overflows".to_string()
                            })?;
                        consequent_cumulative.push(consequent_total_delay);
                    }
                    if consequent_total_delay > 256 {
                        return Err(
                            "composed consequent sequence total delay must stay within 0..=256"
                                .into(),
                        );
                    }
                    let antecedent_state_count = pending_nets.len();
                    let mut consequent_references = Vec::new();
                    for stage in 0..consequent_total_delay {
                        let net = self.declare_property_pending_net(
                            member,
                            property_index,
                            antecedent_state_count + stage,
                        );
                        pending_nets.push(net);
                        consequent_references.push(eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        });
                    }
                    let candidate = |index: usize| {
                        let boundary = consequent_cumulative[index];
                        if boundary == 0 {
                            antecedent_match.clone()
                        } else {
                            consequent_references[boundary - 1].clone()
                        }
                    };
                    let consequent_final_index = consequent_expressions.len() - 1;
                    let success = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(candidate(consequent_final_index)),
                        b: Box::new(consequent_expressions[consequent_final_index].clone()),
                    };
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    let misses = consequent_expressions
                        .iter()
                        .enumerate()
                        .map(|(index, expression)| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(candidate(index)),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(expression.clone()),
                            }),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| {
                            "composed consequent sequence has no expressions".to_string()
                        })?;
                    let antecedent_shifts = base_antecedent_shifts.clone();
                    let mut failure_statements = consequent_references
                        .iter()
                        .map(|reference| {
                            let net = match reference {
                                eda_core::ir::Expr::Ref { net, .. } => *net,
                                _ => unreachable!("consequent pending reference"),
                            };
                            Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            }
                        })
                        .collect::<Vec<_>>();
                    failure_statements.extend(antecedent_shifts.clone());
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                    }
                    let mut composed_shifts = antecedent_shifts;
                    for stage in (1..consequent_total_delay).rev() {
                        composed_shifts.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[antecedent_state_count + stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: consequent_references[stage - 1].clone(),
                        });
                    }
                    if consequent_total_delay > 0 {
                        composed_shifts.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[antecedent_state_count],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: antecedent_reduction(&antecedent_match),
                        });
                    }
                    statements.push(Stmt::If {
                        c: misses,
                        t: Box::new(Stmt::Block(failure_statements)),
                        f: Some(Box::new(Stmt::Block(composed_shifts))),
                    });
                    true
                } else if let ConcurrentDelay::Goto(count) = property.delay {
                    let antecedent_state_count = pending_nets.len();
                    let mut consequent_references = Vec::new();
                    for seen in 1..count {
                        let net = self.declare_property_pending_net(
                            member,
                            property_index,
                            antecedent_state_count + seen - 1,
                        );
                        pending_nets.push(net);
                        consequent_references.push(eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        });
                    }
                    let success = if count == 1 {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(antecedent_match.clone()),
                            b: Box::new(property.consequent.clone()),
                        }
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(consequent_references[count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        }
                    };
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    shift_statements.extend(base_antecedent_shifts.clone());
                    if count > 1 {
                        let not_consequent = eda_core::ir::Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: Box::new(property.consequent.clone()),
                        };
                        let start_or_retain_first = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(antecedent_match.clone()),
                                b: Box::new(property.consequent.clone()),
                            }),
                            b: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(consequent_references[0].clone()),
                                b: Box::new(not_consequent.clone()),
                            }),
                        };
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[antecedent_state_count],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: start_or_retain_first,
                        });
                        for seen in 2..count {
                            let advance = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(consequent_references[seen - 2].clone()),
                                b: Box::new(property.consequent.clone()),
                            };
                            let retain = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(consequent_references[seen - 1].clone()),
                                b: Box::new(not_consequent.clone()),
                            };
                            shift_statements.push(Stmt::NBl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[antecedent_state_count + seen - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogOr,
                                    a: Box::new(advance),
                                    b: Box::new(retain),
                                },
                            });
                        }
                    }
                    true
                } else if let ConcurrentDelay::Nonconsecutive(count) = property.delay {
                    let antecedent_state_count = pending_nets.len();
                    let mut consequent_references = Vec::new();
                    for seen_count in 1..=count {
                        let net = self.declare_property_pending_net(
                            member,
                            property_index,
                            antecedent_state_count + seen_count - 1,
                        );
                        pending_nets.push(net);
                        consequent_references.push(eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        });
                    }
                    let true_endpoint = if count == 1 {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(antecedent_match.clone()),
                            b: Box::new(property.consequent.clone()),
                        }
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(consequent_references[count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        }
                    };
                    let not_consequent = eda_core::ir::Expr::Un {
                        op: eda_core::ir::UnOp::Not,
                        a: Box::new(property.consequent.clone()),
                    };
                    let false_endpoint = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(consequent_references[count - 1].clone()),
                        b: Box::new(not_consequent.clone()),
                    };
                    let endpoint = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(true_endpoint),
                        b: Box::new(false_endpoint),
                    };
                    statements.push(Stmt::If {
                        c: endpoint,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    shift_statements.extend(base_antecedent_shifts.clone());
                    let start_or_retain_first = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogOr,
                        a: Box::new(eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(antecedent_match.clone()),
                            b: Box::new(property.consequent.clone()),
                        }),
                        b: Box::new(eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(consequent_references[0].clone()),
                            b: Box::new(not_consequent.clone()),
                        }),
                    };
                    shift_statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[antecedent_state_count],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: start_or_retain_first,
                    });
                    for seen_count in 2..=count {
                        let advance = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(consequent_references[seen_count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        let retain = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(consequent_references[seen_count - 1].clone()),
                            b: Box::new(not_consequent.clone()),
                        };
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[antecedent_state_count + seen_count - 1],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(advance),
                                b: Box::new(retain),
                            },
                        });
                    }
                    true
                } else {
                    false
                };

            if !special_consequent_handled {
                let (consequent_start, consequent_end) = match property.delay {
                    ConcurrentDelay::SameCycle => (0, 0),
                    ConcurrentDelay::Fixed(delay) => (delay, delay),
                    ConcurrentDelay::Range(start, end) => (start, end),
                    other => {
                        return Err(format!(
                            "temporal sequence antecedents support only same-cycle, fixed-delay, delay-range, sequence, goto, or nonconsecutive consequents, not {other:?}"
                        ));
                    }
                };
                let antecedent_state_count = pending_nets.len();
                let mut consequent_references = Vec::new();
                if consequent_end > 0 {
                    for stage in 0..consequent_end {
                        let net = self.declare_property_pending_net(
                            member,
                            property_index,
                            antecedent_state_count + stage,
                        );
                        pending_nets.push(net);
                        consequent_references.push(eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        });
                    }
                }
                let consequent_candidate = |delay: usize| {
                    if delay == 0 {
                        antecedent_match.clone()
                    } else {
                        consequent_references[delay - 1].clone()
                    }
                };
                if consequent_start == 0 && consequent_end == 0 {
                    statements.push(Stmt::If {
                        c: antecedent_match.clone(),
                        t: Box::new(assertion),
                        f: None,
                    });
                } else {
                    for delay in consequent_start..=consequent_end {
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        if delay > 0 {
                            success_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[antecedent_state_count + delay - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        statements.push(Stmt::If {
                            c: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(consequent_candidate(delay)),
                                b: Box::new(property.consequent.clone()),
                            },
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                    }
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let expired = consequent_candidate(consequent_end);
                        let failure_condition = if consequent_end == 0 {
                            eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(expired),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            }
                        } else {
                            expired
                        };
                        let mut failure_statements = Vec::new();
                        if consequent_end > 0 {
                            failure_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[antecedent_state_count + consequent_end - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        failure_statements.push(assertion.clone());
                        statements.push(Stmt::If {
                            c: failure_condition,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }
                }
                shift_statements.extend(base_antecedent_shifts.clone());
                if consequent_end > 0 {
                    for stage in (1..consequent_end).rev() {
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[antecedent_state_count + stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: consequent_references[stage - 1].clone(),
                        });
                    }
                    let mut pending_value = antecedent_reduction(&antecedent_match);
                    if consequent_start == 0 {
                        pending_value = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(pending_value),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(property.consequent.clone()),
                            }),
                        };
                    }
                    shift_statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[antecedent_state_count],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: pending_value,
                    });
                }
            }
            if !shift_statements.is_empty() {
                statements.push(Stmt::Block(shift_statements));
            }
        } else {
            match property.delay {
                ConcurrentDelay::SameCycle => {
                    statements.push(Stmt::If {
                        c: property.antecedent.clone(),
                        t: Box::new(assertion),
                        f: None,
                    });
                }
                ConcurrentDelay::Fixed(delay_cycles) => {
                    for stage in 0..delay_cycles {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let trigger = references[delay_cycles - 1].clone();
                    statements.push(Stmt::If {
                        c: trigger,
                        t: Box::new(assertion),
                        f: None,
                    });
                    for stage in (1..delay_cycles).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: references[stage - 1].clone(),
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[0],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: antecedent_reduction(&property.antecedent),
                    });
                }
                ConcurrentDelay::PrefixRange(
                    prefix,
                    prefix_miss,
                    start,
                    end,
                    boundary_expressions,
                    prefix_includes_antecedent,
                ) => {
                    for stage in 0..end {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let sequence_start = if prefix_includes_antecedent {
                        antecedent_reduction(&prefix)
                    } else {
                        eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(antecedent_reduction(&property.antecedent)),
                            b: Box::new(antecedent_reduction(&prefix)),
                        }
                    };
                    let candidate = |delay: usize| {
                        if delay == 0 {
                            sequence_start.clone()
                        } else {
                            references[delay - 1].clone()
                        }
                    };
                    for delay in start..=end {
                        let boundary_expression = boundary_expressions
                            .get(delay - start)
                            .cloned()
                            .ok_or_else(|| {
                                "prefix range consequent has no boundary expression".to_string()
                            })?;
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        if delay > 0 {
                            success_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[delay - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        statements.push(Stmt::If {
                            c: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(candidate(delay)),
                                b: Box::new(boundary_expression),
                            },
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                    }
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        statements.push(Stmt::If {
                            c: (*prefix_miss).clone(),
                            t: Box::new(Stmt::ImmediateAssertion {
                                kind,
                                deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                                scope: assertion_scope.clone(),
                                cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                                pass_action: pass_action_template.clone(),
                                fail_action: fail_action_template.clone(),
                            }),
                            f: None,
                        });
                        let expired = candidate(end);
                        let mut failure_statements = Vec::new();
                        if end > 0 {
                            failure_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[end - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                        statements.push(Stmt::If {
                            c: expired,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }
                    for stage in (1..end).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: references[stage - 1].clone(),
                        });
                    }
                    if end > 0 {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: sequence_start,
                        });
                    }
                }
                ConcurrentDelay::NonterminalPrefixRange {
                    prefix,
                    prefix_miss,
                    range_start,
                    range_end,
                    suffix_delays,
                    middle,
                    suffix_expressions,
                } => {
                    let suffix_total_delay =
                        suffix_delays.iter().try_fold(0_usize, |total, delay| {
                            total.checked_add(*delay).ok_or_else(|| {
                                "nonterminal matched range suffix delay total overflows".to_string()
                            })
                        })?;
                    let horizon = range_end.checked_add(suffix_total_delay).ok_or_else(|| {
                        "nonterminal matched range delay total overflows".to_string()
                    })?;
                    if horizon > 256 {
                        return Err(
                            "nonterminal matched range total delay must stay within 0..=256".into(),
                        );
                    }

                    for stage in 0..horizon {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let base_references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let suffix_base = pending_nets.len();
                    for stage in 0..suffix_total_delay {
                        let net = self.declare_property_pending_net(
                            member,
                            property_index,
                            suffix_base + stage,
                        );
                        pending_nets.push(net);
                    }
                    let suffix_references = pending_nets[suffix_base..]
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();

                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(antecedent_reduction(&prefix)),
                    };
                    let middle_match = (range_start..=range_end)
                        .map(|delay| {
                            if delay == 0 {
                                sequence_start.clone()
                            } else {
                                base_references[delay - 1].clone()
                            }
                        })
                        .map(|candidate| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(candidate),
                            b: Box::new(antecedent_reduction(&middle)),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| "nonterminal matched range has no boundaries".to_string())?;
                    let final_suffix_expression = suffix_expressions
                        .last()
                        .ok_or_else(|| "nonterminal matched range has no suffix".to_string())?;
                    let success = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(suffix_references[suffix_total_delay - 1].clone()),
                        b: Box::new(antecedent_reduction(final_suffix_expression)),
                    };
                    let clear_all = |pending_nets: &[usize]| {
                        pending_nets
                            .iter()
                            .map(|&net| Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            })
                            .collect::<Vec<_>>()
                    };

                    let mut success_statements = vec![Stmt::ImmediateAssertion {
                        kind,
                        deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                        scope: assertion_scope.clone(),
                        cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                        pass_action: pass_action_template.clone(),
                        fail_action: fail_action_template.clone(),
                    }];
                    success_statements.extend(clear_all(&pending_nets));
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::Block(success_statements)),
                        f: None,
                    });

                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let mut prefix_miss_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        prefix_miss_statements.extend(clear_all(&pending_nets));
                        statements.push(Stmt::If {
                            c: (*prefix_miss).clone(),
                            t: Box::new(Stmt::Block(prefix_miss_statements)),
                            f: None,
                        });

                        let expired = base_references[horizon - 1].clone();
                        let mut failure_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        failure_statements.extend(clear_all(&pending_nets));
                        statements.push(Stmt::If {
                            c: expired,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }

                    let mut suffix_cumulative = 0_usize;
                    let mut suffix_boundaries =
                        std::collections::BTreeMap::<usize, &eda_core::ir::Expr>::new();
                    for (index, (delay, expression)) in suffix_delays
                        .iter()
                        .zip(suffix_expressions.iter())
                        .enumerate()
                    {
                        suffix_cumulative =
                            suffix_cumulative.checked_add(*delay).ok_or_else(|| {
                                "nonterminal matched range suffix delay total overflows".to_string()
                            })?;
                        if index + 1 < suffix_expressions.len() {
                            suffix_boundaries.insert(suffix_cumulative, expression);
                        }
                    }
                    for stage in (1..suffix_total_delay).rev() {
                        let mut progress = suffix_references[stage - 1].clone();
                        if let Some(expression) = suffix_boundaries.get(&stage) {
                            progress = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(progress),
                                b: Box::new(antecedent_reduction(expression)),
                            };
                        }
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[suffix_base + stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: progress,
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[suffix_base],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: middle_match,
                    });
                    for stage in (1..horizon).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: base_references[stage - 1].clone(),
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[0],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: sequence_start,
                    });
                }
                ConcurrentDelay::LocalNonterminalPrefixRange {
                    prefix,
                    prefix_miss,
                    range_start,
                    range_end,
                    suffix_delays,
                    middle_boundaries,
                    suffix_boundaries,
                    final_boundaries,
                } => {
                    let suffix_total_delay =
                        suffix_delays.iter().try_fold(0_usize, |total, delay| {
                            total.checked_add(*delay).ok_or_else(|| {
                                "scalar-local nonterminal matched suffix delay total overflows"
                                    .to_string()
                            })
                        })?;
                    let horizon = range_end.checked_add(suffix_total_delay).ok_or_else(|| {
                        "scalar-local nonterminal matched range delay total overflows".to_string()
                    })?;
                    let alternative_count = range_end - range_start + 1;
                    if middle_boundaries.len() != alternative_count
                        || final_boundaries.len() != alternative_count
                        || suffix_boundaries.len() != alternative_count
                    {
                        return Err(
                            "scalar-local nonterminal matched range has inconsistent boundaries"
                                .into(),
                        );
                    }

                    for stage in 0..horizon {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let base_references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let alternative_base = pending_nets.len();
                    let mut alternative_references = Vec::with_capacity(alternative_count);
                    for alternative in 0..alternative_count {
                        let first_net = pending_nets.len();
                        for stage in 0..suffix_total_delay {
                            pending_nets.push(self.declare_property_pending_net(
                                member,
                                property_index,
                                alternative_base + alternative * suffix_total_delay + stage,
                            ));
                        }
                        alternative_references.push(
                            pending_nets[first_net..]
                                .iter()
                                .map(|&net| eda_core::ir::Expr::Ref {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                    signed: false,
                                })
                                .collect::<Vec<_>>(),
                        );
                    }

                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(antecedent_reduction(&prefix)),
                    };
                    let success = (0..alternative_count)
                        .map(|alternative| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(
                                alternative_references[alternative][suffix_total_delay - 1].clone(),
                            ),
                            b: Box::new(final_boundaries[alternative].clone()),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| {
                            "scalar-local nonterminal matched range has no alternatives".to_string()
                        })?;
                    let clear_all = |pending_nets: &[usize]| {
                        pending_nets
                            .iter()
                            .map(|&net| Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            })
                            .collect::<Vec<_>>()
                    };

                    let mut success_statements = vec![Stmt::ImmediateAssertion {
                        kind,
                        deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                        scope: assertion_scope.clone(),
                        cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                        pass_action: pass_action_template.clone(),
                        fail_action: fail_action_template.clone(),
                    }];
                    success_statements.extend(clear_all(&pending_nets));
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::Block(success_statements)),
                        f: None,
                    });

                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let mut prefix_miss_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        prefix_miss_statements.extend(clear_all(&pending_nets));
                        statements.push(Stmt::If {
                            c: (*prefix_miss).clone(),
                            t: Box::new(Stmt::Block(prefix_miss_statements)),
                            f: None,
                        });

                        let expired = base_references[horizon - 1].clone();
                        let mut failure_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        failure_statements.extend(clear_all(&pending_nets));
                        statements.push(Stmt::If {
                            c: expired,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }

                    let mut cumulative_boundaries = Vec::with_capacity(suffix_delays.len());
                    let mut cumulative = 0_usize;
                    for delay in &suffix_delays {
                        cumulative += delay;
                        cumulative_boundaries.push(cumulative);
                    }
                    for alternative in 0..alternative_count {
                        let range_delay = range_start + alternative;
                        let seed = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(base_references[range_delay - 1].clone()),
                            b: Box::new(middle_boundaries[alternative].clone()),
                        };
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets
                                    [alternative_base + alternative * suffix_total_delay],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: seed,
                        });
                        for stage in 1..suffix_total_delay {
                            let mut progress =
                                alternative_references[alternative][stage - 1].clone();
                            if let Some(suffix_index) = cumulative_boundaries
                                .iter()
                                .position(|&boundary| boundary == stage)
                            {
                                if suffix_index < suffix_boundaries[alternative].len() {
                                    progress = eda_core::ir::Expr::Bin {
                                        op: eda_core::ir::BinOp::LogAnd,
                                        a: Box::new(progress),
                                        b: Box::new(
                                            suffix_boundaries[alternative][suffix_index].clone(),
                                        ),
                                    };
                                }
                            }
                            statements.push(Stmt::NBl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[alternative_base
                                        + alternative * suffix_total_delay
                                        + stage],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: progress,
                            });
                        }
                    }
                    for stage in (1..horizon).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: base_references[stage - 1].clone(),
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[0],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: sequence_start,
                    });
                }
                ConcurrentDelay::Range(start, end) => {
                    for stage in 0..end {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let candidate = |delay: usize| {
                        if delay == 0 {
                            property.antecedent.clone()
                        } else {
                            references[delay - 1].clone()
                        }
                    };
                    for delay in start..=end {
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        if delay > 0 {
                            success_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[delay - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        statements.push(Stmt::If {
                            c: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(candidate(delay)),
                                b: Box::new(property.consequent.clone()),
                            },
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                    }
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let expired = candidate(end);
                        let failure_condition = if end == 0 {
                            eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(expired),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            }
                        } else {
                            expired
                        };
                        let mut failure_statements = Vec::new();
                        if end > 0 {
                            failure_statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[end - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            });
                        }
                        failure_statements.push(assertion.clone());
                        statements.push(Stmt::If {
                            c: failure_condition,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }
                    for stage in (1..end).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: references[stage - 1].clone(),
                        });
                    }
                    if end > 0 {
                        let mut pending_value = antecedent_reduction(&property.antecedent);
                        if start == 0 {
                            pending_value = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(pending_value),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            };
                        }
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: pending_value,
                        });
                    }
                }
                ConcurrentDelay::ConsecutiveRangeSequence {
                    prefix,
                    repeated,
                    suffix,
                    start,
                    end,
                } => {
                    for age in 0..=end {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            age,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(antecedent_reduction(&prefix)),
                    };
                    let suffix = antecedent_reduction(&suffix);
                    let repeated = antecedent_reduction(&repeated);
                    let success = (start..=end)
                        .map(|age| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[age].clone()),
                            b: Box::new(suffix.clone()),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| {
                            "consecutive range sequence has no boundaries".to_string()
                        })?;
                    let mut success_statements = vec![Stmt::ImmediateAssertion {
                        kind,
                        deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                        scope: assertion_scope.clone(),
                        cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                        pass_action: pass_action_template.clone(),
                        fail_action: fail_action_template.clone(),
                    }];
                    success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                        lv: eda_core::ir::LValue {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                    }));
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::Block(success_statements)),
                        f: None,
                    });
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let failure = (0..end)
                            .map(|age| {
                                let suffix_available = age >= start;
                                let can_extend = age < end;
                                let success_here = if suffix_available {
                                    suffix.clone()
                                } else {
                                    eda_core::ir::Expr::Const(LogicWord::zero(1))
                                };
                                let extend_here = if can_extend {
                                    repeated.clone()
                                } else {
                                    eda_core::ir::Expr::Const(LogicWord::zero(1))
                                };
                                let viable = eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogOr,
                                    a: Box::new(success_here),
                                    b: Box::new(extend_here),
                                };
                                eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(references[age].clone()),
                                    b: Box::new(eda_core::ir::Expr::Un {
                                        op: eda_core::ir::UnOp::Not,
                                        a: Box::new(viable),
                                    }),
                                }
                            })
                            .reduce(|left, right| eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(left),
                                b: Box::new(right),
                            })
                            .ok_or_else(|| "consecutive range sequence has no ages".to_string())?;
                        let mut failure_statements = pending_nets
                            .iter()
                            .map(|&net| Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            })
                            .collect::<Vec<_>>();
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                        statements.push(Stmt::If {
                            c: failure,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }
                    for age in (1..=end).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[age],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[age - 1].clone()),
                                b: Box::new(repeated.clone()),
                            },
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[0],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: sequence_start,
                    });
                }
                ConcurrentDelay::GotoRangeSequence {
                    prefix,
                    repeated,
                    suffix,
                    start,
                    end,
                } => {
                    let seed_index = 0_usize;
                    let seen_base = 1_usize;
                    let ready_base = end + 1;
                    for stage in 0..(2 * end + 1) {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(antecedent_reduction(&prefix)),
                    };
                    let repeated = antecedent_reduction(&repeated);
                    let suffix = antecedent_reduction(&suffix);
                    let success = (start..=end)
                        .map(|seen| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[ready_base + seen - 1].clone()),
                            b: Box::new(suffix.clone()),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| "goto range sequence has no boundaries".to_string())?;
                    let mut success_statements = vec![Stmt::ImmediateAssertion {
                        kind,
                        deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                        scope: assertion_scope.clone(),
                        cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                        pass_action: pass_action_template.clone(),
                        fail_action: fail_action_template.clone(),
                    }];
                    success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                        lv: eda_core::ir::LValue {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                    }));
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::Block(success_statements)),
                        f: None,
                    });
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        let expired = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[ready_base + end - 1].clone()),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(suffix.clone()),
                            }),
                        };
                        let mut failure_statements = pending_nets
                            .iter()
                            .map(|&net| Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            })
                            .collect::<Vec<_>>();
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                        statements.push(Stmt::If {
                            c: expired,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }

                    for seen in (1..end).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[seen_base + seen - 1],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(references[seen_base + seen - 1].clone()),
                                b: Box::new(eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(references[seen_base + seen - 2].clone()),
                                    b: Box::new(repeated.clone()),
                                }),
                            },
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[seen_base],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(references[seen_base].clone()),
                            b: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seed_index].clone()),
                                b: Box::new(repeated.clone()),
                            }),
                        },
                    });
                    for seen in (1..=end).rev() {
                        let prior = if seen == 1 {
                            references[seed_index].clone()
                        } else {
                            references[seen_base + seen - 2].clone()
                        };
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[ready_base + seen - 1],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(prior),
                                b: Box::new(repeated.clone()),
                            },
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[seed_index],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: sequence_start,
                    });
                }
                ConcurrentDelay::NonconsecutiveRangeSequence {
                    prefix,
                    repeated,
                    suffix,
                    start,
                    end,
                } => {
                    let seed_index = 0_usize;
                    let seen_base = 1_usize;
                    let ready_base = end + 1;
                    for stage in 0..(2 * end + 1) {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(antecedent_reduction(&prefix)),
                    };
                    let repeated = antecedent_reduction(&repeated);
                    let suffix = antecedent_reduction(&suffix);
                    let success = (start..=end)
                        .map(|seen| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[ready_base + seen - 1].clone()),
                            b: Box::new(suffix.clone()),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .ok_or_else(|| {
                            "nonconsecutive range sequence has no boundaries".to_string()
                        })?;
                    let mut success_statements = vec![Stmt::ImmediateAssertion {
                        kind,
                        deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                        scope: assertion_scope.clone(),
                        cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                        pass_action: pass_action_template.clone(),
                        fail_action: fail_action_template.clone(),
                    }];
                    success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                        lv: eda_core::ir::LValue {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                    }));
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::Block(success_statements)),
                        f: None,
                    });
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        // Nonconsecutive repetition admits an unbounded number of
                        // trailing gaps. An attempt only exhausts a finite
                        // [start,end] count range when another repeated match
                        // exceeds the maximum boundary while the suffix is absent.
                        let expired = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[ready_base + end - 1].clone()),
                            b: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(repeated.clone()),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(suffix.clone()),
                                }),
                            }),
                        };
                        let mut failure_statements = pending_nets
                            .iter()
                            .map(|&net| Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net,
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                            })
                            .collect::<Vec<_>>();
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                        statements.push(Stmt::If {
                            c: expired,
                            t: Box::new(Stmt::Block(failure_statements)),
                            f: None,
                        });
                    }

                    for seen in (1..end).rev() {
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[seen_base + seen - 1],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(references[seen_base + seen - 1].clone()),
                                b: Box::new(eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(references[seen_base + seen - 2].clone()),
                                    b: Box::new(repeated.clone()),
                                }),
                            },
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[seen_base],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(references[seen_base].clone()),
                            b: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seed_index].clone()),
                                b: Box::new(repeated.clone()),
                            }),
                        },
                    });
                    for seen in (1..=end).rev() {
                        let prior = if seen == 1 {
                            references[seed_index].clone()
                        } else {
                            references[seen_base + seen - 2].clone()
                        };
                        let readiness = eda_core::ir::Expr::Cond {
                            c: Box::new(repeated.clone()),
                            t: Box::new(eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(prior),
                                b: Box::new(repeated.clone()),
                            }),
                            f: Box::new(references[ready_base + seen - 1].clone()),
                        };
                        statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[ready_base + seen - 1],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: readiness,
                        });
                    }
                    statements.push(Stmt::NBl {
                        lv: eda_core::ir::LValue {
                            net: pending_nets[seed_index],
                            decl_width: 1,
                            range: full_range(1),
                        },
                        e: sequence_start,
                    });
                }
                ConcurrentDelay::Consecutive(count) => {
                    if count == 1 {
                        statements.push(Stmt::If {
                            c: property.antecedent.clone(),
                            t: Box::new(assertion),
                            f: None,
                        });
                    } else {
                        for stage in 0..count - 1 {
                            pending_nets.push(self.declare_property_pending_net(
                                member,
                                property_index,
                                stage,
                            ));
                        }
                        let references = pending_nets
                            .iter()
                            .map(|&net| eda_core::ir::Expr::Ref {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let success = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                        }));
                        statements.push(Stmt::If {
                            c: success,
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                        if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                            let active_or_new = std::iter::once(property.antecedent.clone())
                                .chain(references.iter().cloned())
                                .reduce(|left, right| eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogOr,
                                    a: Box::new(left),
                                    b: Box::new(right),
                                })
                                .expect("consecutive repetition has an antecedent");
                            let broken = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(active_or_new),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            };
                            statements.push(Stmt::If {
                                c: broken,
                                t: Box::new(Stmt::ImmediateAssertion {
                                    kind,
                                    deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                                    scope: assertion_scope.clone(),
                                    cond: property.consequent.clone(),
                                    pass_action: pass_action_template.clone(),
                                    fail_action: fail_action_template.clone(),
                                }),
                                f: None,
                            });
                        }
                        for age in (2..count).rev() {
                            statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[age - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(references[age - 2].clone()),
                                    b: Box::new(property.consequent.clone()),
                                },
                            });
                        }
                        statements.push(Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(property.antecedent.clone()),
                                b: Box::new(property.consequent.clone()),
                            },
                        });
                    }
                }
                ConcurrentDelay::Goto(count) => {
                    if count == 1 {
                        statements.push(Stmt::If {
                            c: property.antecedent.clone(),
                            t: Box::new(assertion),
                            f: None,
                        });
                    } else {
                        for seen in 1..count {
                            pending_nets.push(self.declare_property_pending_net(
                                member,
                                property_index,
                                seen,
                            ));
                        }
                        let references = pending_nets
                            .iter()
                            .map(|&net| eda_core::ir::Expr::Ref {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let success = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                        }));
                        statements.push(Stmt::If {
                            c: success,
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                        for seen in (2..count).rev() {
                            let retain = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seen - 1].clone()),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            };
                            let advance = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seen - 2].clone()),
                                b: Box::new(property.consequent.clone()),
                            };
                            statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[seen - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogOr,
                                    a: Box::new(retain),
                                    b: Box::new(advance),
                                },
                            });
                        }
                        let retain_first = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[0].clone()),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(property.consequent.clone()),
                            }),
                        };
                        let start = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(property.antecedent.clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        statements.push(Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(retain_first),
                                b: Box::new(start),
                            },
                        });
                    }
                }
                ConcurrentDelay::Nonconsecutive(count) => {
                    if count == 1 {
                        statements.push(Stmt::If {
                            c: property.antecedent.clone(),
                            t: Box::new(assertion),
                            f: None,
                        });
                    } else {
                        for seen in 1..count {
                            pending_nets.push(self.declare_property_pending_net(
                                member,
                                property_index,
                                seen,
                            ));
                        }
                        let references = pending_nets
                            .iter()
                            .map(|&net| eda_core::ir::Expr::Ref {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                                signed: false,
                            })
                            .collect::<Vec<_>>();
                        let success = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[count - 2].clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        let mut success_statements = vec![Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: property.consequent.clone(),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }];
                        success_statements.extend(pending_nets.iter().map(|&net| Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                        }));
                        statements.push(Stmt::If {
                            c: success,
                            t: Box::new(Stmt::Block(success_statements)),
                            f: None,
                        });
                        for seen in (2..count).rev() {
                            let retain = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seen - 1].clone()),
                                b: Box::new(eda_core::ir::Expr::Un {
                                    op: eda_core::ir::UnOp::Not,
                                    a: Box::new(property.consequent.clone()),
                                }),
                            };
                            let advance = eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(references[seen - 2].clone()),
                                b: Box::new(property.consequent.clone()),
                            };
                            statements.push(Stmt::Bl {
                                lv: eda_core::ir::LValue {
                                    net: pending_nets[seen - 1],
                                    decl_width: 1,
                                    range: full_range(1),
                                },
                                e: eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogOr,
                                    a: Box::new(retain),
                                    b: Box::new(advance),
                                },
                            });
                        }
                        let retain_first = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(references[0].clone()),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(property.consequent.clone()),
                            }),
                        };
                        let start = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(property.antecedent.clone()),
                            b: Box::new(property.consequent.clone()),
                        };
                        statements.push(Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogOr,
                                a: Box::new(retain_first),
                                b: Box::new(start),
                            },
                        });
                    }
                }
                ConcurrentDelay::SequenceOrImmediate(expressions, delays, immediate) => {
                    let mut cumulative = Vec::with_capacity(expressions.len());
                    let mut total_delay = 0_usize;
                    for delay in &delays {
                        total_delay = total_delay.checked_add(*delay).ok_or_else(|| {
                            "concurrent sequence-or-immediate delay total overflows".to_string()
                        })?;
                        cumulative.push(total_delay);
                    }
                    if total_delay > 256 {
                        return Err(
                            "concurrent sequence-or-immediate total delay must stay within 0..=256"
                                .into(),
                        );
                    }
                    for stage in 0..total_delay {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let sequence_start = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new(eda_core::ir::Expr::Un {
                            op: eda_core::ir::UnOp::Not,
                            a: immediate.clone(),
                        }),
                    };
                    let immediate_success = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(antecedent_reduction(&property.antecedent)),
                        b: Box::new((*immediate).clone()),
                    };
                    statements.push(Stmt::If {
                        c: immediate_success,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    let candidate = |index: usize| {
                        let boundary = cumulative[index];
                        if boundary == 0 {
                            sequence_start.clone()
                        } else {
                            references[boundary - 1].clone()
                        }
                    };
                    let final_index = expressions.len() - 1;
                    let sequence_success = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(candidate(final_index)),
                        b: Box::new(expressions[final_index].clone()),
                    };
                    statements.push(Stmt::If {
                        c: sequence_success,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    let misses = expressions
                        .iter()
                        .enumerate()
                        .map(|(index, expression)| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(candidate(index)),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(expression.clone()),
                            }),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .expect("concurrent sequence-or-immediate has expressions");
                    let mut failure_statements = pending_nets
                        .iter()
                        .map(|&net| Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                        })
                        .collect::<Vec<_>>();
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                    }
                    let mut shift_statements = Vec::new();
                    for stage in (1..total_delay).rev() {
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: references[stage - 1].clone(),
                        });
                    }
                    if total_delay > 0 {
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: sequence_start.clone(),
                        });
                    }
                    statements.push(Stmt::If {
                        c: misses,
                        t: Box::new(Stmt::Block(failure_statements)),
                        f: Some(Box::new(Stmt::Block(shift_statements))),
                    });
                }
                ConcurrentDelay::Sequence(expressions, delays) => {
                    let mut cumulative = Vec::with_capacity(expressions.len());
                    let mut total_delay = 0_usize;
                    for delay in &delays {
                        total_delay = total_delay.checked_add(*delay).ok_or_else(|| {
                            "concurrent sequence delay total overflows".to_string()
                        })?;
                        cumulative.push(total_delay);
                    }
                    if total_delay > 256 {
                        return Err(
                            "concurrent sequence total delay must stay within 0..=256".to_string()
                        );
                    }
                    for stage in 0..total_delay {
                        pending_nets.push(self.declare_property_pending_net(
                            member,
                            property_index,
                            stage,
                        ));
                    }
                    let references = pending_nets
                        .iter()
                        .map(|&net| eda_core::ir::Expr::Ref {
                            net,
                            decl_width: 1,
                            range: full_range(1),
                            signed: false,
                        })
                        .collect::<Vec<_>>();
                    let candidate = |index: usize| {
                        let boundary = cumulative[index];
                        if boundary == 0 {
                            property.antecedent.clone()
                        } else {
                            references[boundary - 1].clone()
                        }
                    };
                    let final_index = expressions.len() - 1;
                    let success = eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::LogAnd,
                        a: Box::new(candidate(final_index)),
                        b: Box::new(expressions[final_index].clone()),
                    };
                    statements.push(Stmt::If {
                        c: success,
                        t: Box::new(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        }),
                        f: None,
                    });
                    let misses = expressions
                        .iter()
                        .enumerate()
                        .map(|(index, expression)| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogAnd,
                            a: Box::new(candidate(index)),
                            b: Box::new(eda_core::ir::Expr::Un {
                                op: eda_core::ir::UnOp::Not,
                                a: Box::new(expression.clone()),
                            }),
                        })
                        .reduce(|left, right| eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(left),
                            b: Box::new(right),
                        })
                        .expect("concurrent sequence has expressions");
                    let mut failure_statements = pending_nets
                        .iter()
                        .map(|&net| Stmt::Bl {
                            lv: eda_core::ir::LValue {
                                net,
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                        })
                        .collect::<Vec<_>>();
                    if !matches!(kind, eda_core::ir::ImmediateAssertionKind::Cover) {
                        failure_statements.push(Stmt::ImmediateAssertion {
                            kind,
                            deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                            scope: assertion_scope.clone(),
                            cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                            pass_action: pass_action_template.clone(),
                            fail_action: fail_action_template.clone(),
                        });
                    }
                    let mut shift_statements = Vec::new();
                    for stage in (1..total_delay).rev() {
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[stage],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: references[stage - 1].clone(),
                        });
                    }
                    if total_delay > 0 {
                        shift_statements.push(Stmt::NBl {
                            lv: eda_core::ir::LValue {
                                net: pending_nets[0],
                                decl_width: 1,
                                range: full_range(1),
                            },
                            e: antecedent_reduction(&property.antecedent),
                        });
                    }
                    statements.push(Stmt::If {
                        c: misses,
                        t: Box::new(Stmt::Block(failure_statements)),
                        f: Some(Box::new(Stmt::Block(shift_statements))),
                    });
                }
            }
        }
        for sample in &property.local_samples {
            pending_nets.extend(sample.nets.iter().copied());
            pending_nets.extend(
                sample
                    .histories
                    .iter()
                    .flat_map(|history| history.nets.iter().copied()),
            );
            statements.push(Stmt::Block(Self::local_sample_updates(sample)));
        }
        let property_body = Stmt::Block(statements);
        if !pending_nets.is_empty() {
            self.design
                .assertion_attempts
                .push(eda_core::ir::AssertionAttempt {
                    scope: assertion_scope,
                    nets: pending_nets.clone(),
                });
        }
        let body = if let Some(disable) = disable {
            let clear_states = pending_nets
                .into_iter()
                .map(|net| self.zero_property_net(net))
                .collect::<Vec<_>>();
            Stmt::If {
                c: disable,
                t: Box::new(Stmt::Block(clear_states)),
                f: Some(Box::new(property_body)),
            }
        } else {
            property_body
        };
        self.design.processes.push(eda_core::ir::Process {
            sens: sensitivity,
            body,
        });
        Ok(())
    }

    pub(super) fn collect_manual_covergroup(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        if member.get("event").is_some_and(|event| !event.is_null()) {
            return Ok(());
        }
        let group_name = string_field(member, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "covergroup has no name".to_string())?;
        let address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("covergroup '{group_name}' has no symbol address"))?;
        let members = member
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("covergroup '{group_name}' has no members"))?;
        let mut sample_names = Vec::new();
        for formal in members.iter().filter(|candidate| {
            string_field(candidate, "kind").as_deref() == Some("FormalArgument")
                && string_field(candidate, "flags").as_deref() == Some("formal_cov_sample")
        }) {
            let name = string_field(formal, "name")
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    format!("covergroup '{group_name}' has an unnamed sample argument")
                })?;
            if !sample_names.iter().all(|existing| existing != &name) {
                return Err(format!(
                    "covergroup '{group_name}' has duplicate sample argument '{name}'"
                ));
            }
            sample_names.push(name);
        }
        if sample_names.is_empty() {
            return Err(format!(
                "manual covergroup '{group_name}' has no with-function sample arguments"
            ));
        }
        let body = members
            .iter()
            .find(|candidate| string_field(candidate, "kind").as_deref() == Some("CovergroupBody"))
            .ok_or_else(|| format!("covergroup '{group_name}' must have one covergroup body"))?;
        let body_members = body
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("covergroup '{group_name}' body has no members"))?;
        let coverpoint_members = body_members
            .iter()
            .filter(|candidate| string_field(candidate, "kind").as_deref() == Some("Coverpoint"))
            .collect::<Vec<_>>();
        if coverpoint_members.len() != sample_names.len() {
            return Err(format!(
                "manual covergroup '{group_name}' must have one unlabeled coverpoint per sample argument"
            ));
        }
        let mut parsed_coverpoints = Vec::with_capacity(coverpoint_members.len());
        for coverpoint in coverpoint_members {
            let coverpoint_name = string_field(coverpoint, "name")
                .filter(|name| !name.is_empty())
                .ok_or_else(|| format!("covergroup '{group_name}' has an unnamed coverpoint"))?;
            if !sample_names.contains(&coverpoint_name) {
                return Err(format!(
                    "covergroup '{group_name}' supports only unlabeled coverpoints naming a sample argument"
                ));
            }
            if parsed_coverpoints
                .iter()
                .any(|parsed: &super::AstManualCoverpoint| parsed.sample_name == coverpoint_name)
            {
                return Err(format!(
                    "covergroup '{group_name}' has duplicate coverpoint '{coverpoint_name}'"
                ));
            }
            let bins = coverpoint
                .get("members")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    format!("coverpoint '{group_name}.{coverpoint_name}' has no coverage bins")
                })?;
            let mut parsed_bins = Vec::new();
            for bin in bins {
                match string_field(bin, "kind").as_deref() {
                    Some("ClassProperty") => {}
                    Some("CoverageBin") => {
                        if string_field(bin, "binsKind").as_deref() != Some("Bins")
                            || bool_field(bin, "isArray")
                            || bool_field(bin, "isWildcard")
                            || bool_field(bin, "isDefault")
                            || bool_field(bin, "isDefaultSequence")
                            || bin.get("iff").is_some()
                            || bin.get("numberOfBins").is_some()
                            || bin.get("setCoverage").is_some()
                            || bin.get("with").is_some()
                            || bin.get("crossSelect").is_some()
                            || bin.get("trans").is_some()
                        {
                            return Err(format!(
                                "coverpoint '{group_name}.{coverpoint_name}' supports only simple scalar-value bins"
                            ));
                        }
                        let bin_name = string_field(bin, "name")
                            .filter(|name| !name.is_empty())
                            .ok_or_else(|| {
                                format!(
                                    "coverpoint '{group_name}.{coverpoint_name}' has an unnamed bin"
                                )
                            })?;
                        let values = bin
                            .get("values")
                            .and_then(Value::as_array)
                            .filter(|values| !values.is_empty())
                            .ok_or_else(|| {
                                format!(
                                    "coverage bin '{group_name}.{coverpoint_name}' has no values"
                                )
                            })?;
                        let mut parsed_values = Vec::with_capacity(values.len());
                        for value in values {
                            match self.lower_expression(value)? {
                                eda_core::ir::Expr::Const(word) => parsed_values.push(word),
                                _ => {
                                    return Err(format!(
                                        "coverpoint '{group_name}.{coverpoint_name}' supports only constant scalar values"
                                    ));
                                }
                            }
                        }
                        parsed_bins.push(super::AstManualCovergroupBin {
                            name: bin_name,
                            values: parsed_values,
                        });
                    }
                    other => {
                        return Err(format!(
                            "coverpoint '{group_name}.{coverpoint_name}' has unsupported member kind {other:?}"
                        ))
                    }
                }
            }
            if parsed_bins.is_empty() {
                return Err(format!(
                    "coverpoint '{group_name}.{coverpoint_name}' has no simple coverage bins"
                ));
            }
            parsed_coverpoints.push(super::AstManualCoverpoint {
                sample_name: coverpoint_name,
                bins: parsed_bins,
            });
        }
        if parsed_coverpoints.is_empty() {
            return Err(format!(
                "manual covergroup '{group_name}' has no simple coverage bins"
            ));
        }
        self.manual_covergroups.insert(
            format!("{address} {group_name}"),
            super::AstManualCovergroup {
                name: group_name,
                sample_names,
                coverpoints: parsed_coverpoints,
            },
        );
        Ok(())
    }

    pub(super) fn lower_covergroup(&mut self, member: &serde_json::Value) -> Result<(), String> {
        let group_name = string_field(member, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "covergroup has no name".to_string())?;
        if member.get("event").is_none_or(|event| event.is_null()) {
            let address = member
                .get("addr")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("covergroup '{group_name}' has no symbol address"))?;
            let key = format!("{address} {group_name}");
            if self.manual_covergroups.contains_key(&key) {
                return Ok(());
            }
            return Err(format!(
                "covergroup '{group_name}' requires a sampling event or a supported with-function sample form"
            ));
        }
        let event = member
            .get("event")
            .ok_or_else(|| format!("covergroup '{group_name}' has no sampling event"))?;
        let sensitivity = self.lower_sensitivity(event)?;
        if !matches!(
            sensitivity,
            eda_core::ir::Sens::Posedge(_) | eda_core::ir::Sens::Negedge(_)
        ) {
            return Err(format!(
                "covergroup '{group_name}' requires a single posedge or negedge sampling event"
            ));
        }
        let bodies = member
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("covergroup '{group_name}' has no body"))?;
        if bodies.len() != 1
            || string_field(&bodies[0], "kind").as_deref() != Some("CovergroupBody")
        {
            return Err(format!(
                "covergroup '{group_name}' must have exactly one covergroup body"
            ));
        }
        let coverpoint_members = bodies[0]
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("covergroup '{group_name}' body has no members"))?;
        let mut statements = Vec::new();
        let mut seen_labels = std::collections::BTreeSet::new();
        for coverpoint_member in coverpoint_members {
            match string_field(coverpoint_member, "kind").as_deref() {
                Some("ClassProperty") => {}
                Some("Coverpoint") => {
                    let coverpoint_name = string_field(coverpoint_member, "name")
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| {
                            format!("covergroup '{group_name}' has an unnamed coverpoint")
                        })?;
                    let symbol = self
                        .symbols
                        .get(&coverpoint_name)
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "coverpoint '{group_name}.{coverpoint_name}' must name a simple declared symbol"
                            )
                        })?;
                    if symbol.memory.is_some() {
                        return Err(format!(
                            "coverpoint '{group_name}.{coverpoint_name}' must sample a scalar or packed net"
                        ));
                    }
                    let sampled = eda_core::ir::Expr::Ref {
                        net: symbol.base,
                        decl_width: symbol.width,
                        range: full_range(symbol.width),
                        signed: symbol.signed,
                    };
                    let bins = coverpoint_member
                        .get("members")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            format!(
                                "coverpoint '{group_name}.{coverpoint_name}' has no coverage bins"
                            )
                        })?;
                    for bin in bins {
                        match string_field(bin, "kind").as_deref() {
                            Some("ClassProperty") => {}
                            Some("CoverageBin") => {
                                if string_field(bin, "binsKind").as_deref() != Some("Bins")
                                    || bool_field(bin, "isArray")
                                    || bool_field(bin, "isWildcard")
                                    || bool_field(bin, "isDefault")
                                    || bool_field(bin, "isDefaultSequence")
                                    || bin.get("iff").is_some()
                                    || bin.get("numberOfBins").is_some()
                                    || bin.get("setCoverage").is_some()
                                    || bin.get("with").is_some()
                                    || bin.get("crossSelect").is_some()
                                    || bin.get("trans").is_some()
                                {
                                    return Err(format!(
                                        "coverpoint '{group_name}.{coverpoint_name}' supports only simple scalar-value bins"
                                    ));
                                }
                                let bin_name = string_field(bin, "name")
                                    .filter(|name| !name.is_empty())
                                    .ok_or_else(|| {
                                        format!(
                                            "coverpoint '{group_name}.{coverpoint_name}' has an unnamed bin"
                                        )
                                    })?;
                                let label = format!("{group_name}.{coverpoint_name}.{bin_name}");
                                if !seen_labels.insert(label.clone()) {
                                    return Err(format!(
                                        "covergroup '{group_name}' has duplicate coverage label '{label}'"
                                    ));
                                }
                                let values = bin
                                    .get("values")
                                    .and_then(Value::as_array)
                                    .filter(|values| !values.is_empty())
                                    .ok_or_else(|| {
                                        format!("coverage bin '{label}' has no scalar values")
                                    })?;
                                let mut matches = Vec::with_capacity(values.len());
                                for value in values {
                                    if !matches!(
                                        string_field(value, "kind").as_deref(),
                                        Some("Conversion") | Some("IntegerLiteral")
                                    ) {
                                        return Err(format!(
                                            "coverage bin '{label}' supports only constant scalar values"
                                        ));
                                    }
                                    let value = self.lower_expression(value)?;
                                    matches.push(eda_core::ir::Expr::Bin {
                                        op: eda_core::ir::BinOp::Eq,
                                        a: Box::new(sampled.clone()),
                                        b: Box::new(value),
                                    });
                                }
                                let condition = matches
                                    .into_iter()
                                    .reduce(|left, right| eda_core::ir::Expr::Bin {
                                        op: eda_core::ir::BinOp::LogOr,
                                        a: Box::new(left),
                                        b: Box::new(right),
                                    })
                                    .expect("coverage bin has a value");
                                statements.push(Stmt::Cover {
                                    label,
                                    cond: condition,
                                });
                            }
                            other => {
                                return Err(format!(
                                    "coverpoint '{group_name}.{coverpoint_name}' has unsupported member kind {other:?}"
                                ))
                            }
                        }
                    }
                }
                Some("CoverCross") => {
                    return Err(format!(
                        "covergroup '{group_name}' crosses are not yet supported"
                    ))
                }
                other => {
                    return Err(format!(
                        "covergroup '{group_name}' has unsupported member kind {other:?}"
                    ))
                }
            }
        }
        if statements.is_empty() {
            return Err(format!("covergroup '{group_name}' has no coverage bins"));
        }
        self.design.processes.push(eda_core::ir::Process {
            sens: sensitivity,
            body: Stmt::Block(statements),
        });
        Ok(())
    }

    pub(super) fn lower_checker_instance(
        &mut self,
        member: &serde_json::Value,
    ) -> Result<(), String> {
        let instance_name = string_field(member, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "checker instance has no name".to_string())?;
        let body = member
            .get("body")
            .ok_or_else(|| format!("checker instance '{instance_name}' has no body"))?;
        if string_field(body, "kind").as_deref() != Some("CheckerInstanceBody") {
            return Err(format!(
                "checker instance '{instance_name}' has an unexpected body kind"
            ));
        }
        let members = body
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("checker instance '{instance_name}' body has no members"))?;
        let connections = member
            .get("connections")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("checker instance '{instance_name}' has no connections"))?;
        let mut connection_symbols = Vec::with_capacity(connections.len());
        for connection in connections {
            let Some(formal) = string_field(connection, "formal") else {
                return Err(format!(
                    "checker instance '{instance_name}' has a connection without a formal symbol"
                ));
            };
            let actual = connection.get("actual").ok_or_else(|| {
                format!("checker instance '{instance_name}' connection '{formal}' has no actual")
            })?;
            let actual_symbol_value = if string_field(actual, "kind").as_deref()
                == Some("NamedValue")
            {
                actual
            } else if string_field(actual, "kind").as_deref() == Some("Assignment") {
                let left = actual.get("left").ok_or_else(|| {
                    format!("checker instance '{instance_name}' output connection has no left")
                })?;
                let right = actual.get("right").ok_or_else(|| {
                    format!("checker instance '{instance_name}' output connection has no right")
                })?;
                if string_field(right, "kind").as_deref() != Some("EmptyArgument")
                    || bool_field(actual, "isNonBlocking")
                {
                    return Err(format!(
                        "checker instance '{instance_name}' supports only simple named actual connections"
                    ));
                }
                left
            } else {
                return Err(format!(
                    "checker instance '{instance_name}' supports only simple named actual connections"
                ));
            };
            let symbol = self.named_symbol(actual_symbol_value)?;
            connection_symbols.push((formal, symbol));
        }
        for (formal, symbol) in &connection_symbols {
            self.external_symbols.insert(formal.clone(), symbol.clone());
        }
        let mut local_symbols = Vec::new();
        for checker_member in members {
            if string_field(checker_member, "kind").as_deref() != Some("Variable") {
                continue;
            }
            let local_name = string_field(checker_member, "name")
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    format!("checker instance '{instance_name}' has an unnamed local variable")
                })?;
            let address = checker_member
                .get("addr")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    format!(
                        "checker instance '{instance_name}' local '{local_name}' has no symbol address"
                    )
                })?;
            let unique_name = format!("__checker_{address}_{local_name}");
            let mut renamed = checker_member.clone();
            if let Some(fields) = renamed.as_object_mut() {
                fields.insert(
                    "name".into(),
                    serde_json::Value::String(unique_name.clone()),
                );
            }
            self.lower_declaration(&renamed).map_err(|error| {
                format!("checker instance '{instance_name}' local '{local_name}': {error}")
            })?;
            let Some(symbol) = self.symbols.get(&unique_name).cloned() else {
                return Err(format!(
                    "checker instance '{instance_name}' local '{local_name}' was not declared"
                ));
            };
            let formal = format!("{address} {local_name}");
            self.external_symbols.insert(formal.clone(), symbol);
            local_symbols.push((formal, unique_name));
        }
        for checker_member in members {
            match string_field(checker_member, "kind").as_deref() {
                Some("AssertionPort")
                | Some("FormalArgument")
                | Some("StatementBlock")
                | Some("Variable") => {}
                Some("ProceduralBlock") => {
                    let procedure_kind =
                        string_field(checker_member, "procedureKind").ok_or_else(|| {
                            format!(
                                "checker instance '{instance_name}' procedure has no procedure kind"
                            )
                        })?;
                    if procedure_kind == "Always" {
                        let statement = checker_member.get("body").ok_or_else(|| {
                            format!("checker instance '{instance_name}' procedure has no body")
                        })?;
                        self.lower_checker_assertion_statement(member, statement)?;
                    } else if matches!(procedure_kind.as_str(), "Initial" | "Final") {
                        self.lower_procedural_block(checker_member)
                            .map_err(|error| {
                                format!("checker instance '{instance_name}': {error}")
                            })?;
                    } else {
                        return Err(format!(
                            "checker instance '{instance_name}' supports only always concurrent assertions and initial/final assertion procedures"
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "checker instance '{instance_name}' has unsupported member kind {other:?}"
                    ))
                }
            }
        }
        for (formal, _) in connection_symbols {
            self.external_symbols.remove(&formal);
        }
        for (formal, unique_name) in local_symbols {
            self.external_symbols.remove(&formal);
            self.symbols.remove(&unique_name);
        }
        Ok(())
    }

    fn lower_checker_assertion_statement(
        &mut self,
        member: &serde_json::Value,
        statement: &serde_json::Value,
    ) -> Result<(), String> {
        match string_field(statement, "kind").as_deref() {
            Some("ConcurrentAssertion") => self.lower_concurrent_assertion(statement),
            Some("Block") => {
                let Some(body) = statement.get("body") else {
                    return Err("checker assertion block has no body".into());
                };
                self.lower_checker_assertion_statement(member, body)
            }
            Some("List") => {
                let items = statement
                    .get("list")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "checker assertion list has no items".to_string())?;
                for item in items {
                    self.lower_checker_assertion_statement(member, item)?;
                }
                Ok(())
            }
            _ => Err(format!(
                "checker instance '{}' supports only elaborated concurrent assertions",
                string_field(member, "name").unwrap_or_default()
            )),
        }
    }

    fn lower_simple_concurrent_property(
        &mut self,
        member: &serde_json::Value,
        property_index: usize,
        property_expr: &serde_json::Value,
    ) -> Result<ConcurrentProperty, String> {
        if string_field(property_expr, "kind").as_deref() == Some("AssertionInstance")
            && property_expr
                .get("localVars")
                .and_then(Value::as_array)
                .is_some_and(|locals| !locals.is_empty())
        {
            let normalized_property_instance;
            let mut property_implication = false;
            let local_sequence_instance = if string_field(
                property_expr
                    .get("body")
                    .unwrap_or(&serde_json::Value::Null),
                "kind",
            )
            .as_deref()
                == Some("SequenceConcat")
            {
                property_expr
            } else {
                normalized_property_instance =
                    Self::local_property_implication_as_sequence_instance(property_expr)?;
                property_implication = true;
                &normalized_property_instance
            };
            let parsed =
                self.lower_scalar_local_sequence(local_sequence_instance, member, property_index)?;
            let consequent =
                parsed.expressions.last().cloned().ok_or_else(|| {
                    "sequence-carried local sequence has no expressions".to_string()
                })?;
            let local_samples = parsed.local_samples;
            let antecedent = if property_implication {
                parsed.expressions.first().cloned().ok_or_else(|| {
                    "sequence-carried local property has no antecedent".to_string()
                })?
            } else {
                eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1))
            };
            let delay = ConcurrentDelay::Sequence(parsed.expressions, parsed.delays);
            return Ok(ConcurrentProperty {
                antecedent,
                antecedent_sequence: None,
                antecedent_goto: None,
                antecedent_consecutive_range: None,
                antecedent_goto_range: None,
                antecedent_nonconsecutive_range: None,
                antecedent_immediate: None,
                local_samples,
                consequent,
                delay,
            });
        }
        let property_expr = Self::inline_simple_assertion_instance(property_expr)?;
        match string_field(property_expr, "kind").as_deref() {
            Some("Simple") => {
                let expression = property_expr
                    .get("expr")
                    .ok_or_else(|| "simple concurrent property has no expression".to_string())?;
                Ok(ConcurrentProperty {
                    antecedent: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                    antecedent_sequence: None,
                    antecedent_goto: None,
                    antecedent_consecutive_range: None,
                    antecedent_goto_range: None,
                    antecedent_nonconsecutive_range: None,
                    antecedent_immediate: None,
                    local_samples: Vec::new(),
                    consequent: self.lower_expression(expression)?,
                    delay: ConcurrentDelay::SameCycle,
                })
            }
            Some("SequenceConcat") => {
                let (consequent, delay) = self.lower_fixed_sequence_consequent(property_expr)?;
                Ok(ConcurrentProperty {
                    antecedent: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                    antecedent_sequence: None,
                    antecedent_goto: None,
                    antecedent_consecutive_range: None,
                    antecedent_goto_range: None,
                    antecedent_nonconsecutive_range: None,
                    antecedent_immediate: None,
                    local_samples: Vec::new(),
                    consequent,
                    delay,
                })
            }
            Some("Binary")
                if string_field(property_expr, "op").as_deref() == Some("Iff") =>
            {
                Ok(ConcurrentProperty {
                    antecedent: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                    antecedent_sequence: None,
                    antecedent_goto: None,
                    antecedent_consecutive_range: None,
                    antecedent_goto_range: None,
                    antecedent_nonconsecutive_range: None,
                    antecedent_immediate: None,
                    local_samples: Vec::new(),
                    consequent: self.lower_same_cycle_boolean_property(property_expr)?,
                    delay: ConcurrentDelay::SameCycle,
                })
            }
            Some("Binary")
                if string_field(property_expr, "op").as_deref()
                    == Some("OverlappedImplication") =>
            {
                let raw_antecedent = property_expr
                    .get("left")
                    .ok_or_else(|| "overlapped implication has no antecedent".to_string())?;
                let has_local_sequence_instance =
                    Self::local_assertion_instance(raw_antecedent).is_some();
                let antecedent = if has_local_sequence_instance {
                    raw_antecedent
                } else {
                    Self::inline_simple_assertion_instance(raw_antecedent)?
                };
                let consequent = property_expr
                    .get("right")
                    .ok_or_else(|| "overlapped implication has no consequent".to_string())?;
                let mut antecedent_sequence =
                    if string_field(antecedent, "kind").as_deref() == Some("SequenceConcat") {
                        Some(self.lower_fixed_sequence_antecedent(antecedent)?)
                    } else {
                        None
                    };
                let mut antecedent_goto = None;
                let mut antecedent_consecutive_range = None;
                let mut antecedent_goto_range = None;
                let mut antecedent_nonconsecutive_range = None;
                let mut antecedent_immediate = None;
                let mut local_samples = Vec::new();
                let antecedent = match string_field(antecedent, "kind").as_deref() {
                    Some("Simple") => {
                        if antecedent.get("repetition").is_some() {
                            if string_field(
                                antecedent
                                    .get("repetition")
                                    .unwrap_or(&serde_json::Value::Null),
                                "kind",
                            )
                            .as_deref()
                                == Some("GoTo")
                            {
                                let repetition = antecedent
                                    .get("repetition")
                                    .ok_or("antecedent repetition is missing")?;
                                let min = repetition
                                    .get("min")
                                    .and_then(Value::as_u64)
                                    .ok_or("goto antecedent repetition has no minimum")?;
                                let max = repetition
                                    .get("max")
                                    .and_then(Value::as_u64)
                                    .ok_or("goto antecedent repetition has no maximum")?;
                                if min != max {
                                    let (expression, start, end) = self
                                        .lower_goto_antecedent_repetition_range(antecedent)?;
                                    antecedent_goto_range = Some((expression.clone(), start, end));
                                    expression
                                } else {
                                    let (expression, count) =
                                        self.lower_goto_antecedent_repetition(antecedent)?;
                                    antecedent_goto = Some((expression.clone(), count));
                                    expression
                                }
                            } else if string_field(
                                antecedent
                                    .get("repetition")
                                    .unwrap_or(&serde_json::Value::Null),
                                "kind",
                            )
                            .as_deref()
                                == Some("Nonconsecutive")
                            {
                                let (expression, start, end) = self
                                    .lower_nonconsecutive_antecedent_repetition(antecedent)?;
                                antecedent_nonconsecutive_range =
                                    Some((expression.clone(), start, end));
                                expression
                            } else {
                                let repetition = antecedent
                                    .get("repetition")
                                    .ok_or("antecedent repetition is missing")?;
                                let min = repetition
                                    .get("min")
                                    .and_then(Value::as_u64)
                                    .ok_or("antecedent repetition has no minimum")?;
                                let max = repetition
                                    .get("max")
                                    .and_then(Value::as_u64)
                                    .ok_or("antecedent repetition has no maximum")?;
                                if min != max {
                                    let (expression, start, end) = self
                                        .lower_consecutive_antecedent_repetition_range(
                                            antecedent,
                                        )?;
                                    antecedent_consecutive_range =
                                        Some((expression.clone(), start, end));
                                    expression
                                } else {
                                    let parsed = self
                                        .lower_consecutive_antecedent_repetition(antecedent)?;
                                    let start = parsed
                                        .expressions
                                        .first()
                                        .cloned()
                                        .ok_or("antecedent repetition has no expression")?;
                                    antecedent_sequence = Some(parsed);
                                    start
                                }
                            }
                        } else {
                            let expression = Self::simple_property_operand(antecedent, "left")?;
                            if let Some((matched, other)) =
                                Self::matched_sequence_disjunction(expression)
                            {
                                let instance = matched
                                    .get("arguments")
                                    .and_then(Value::as_array)
                                    .and_then(|arguments| arguments.first())
                                    .ok_or_else(|| {
                                        "sequence matched() has no argument".to_string()
                                    })?;
                                let parsed = if instance
                                    .get("localVars")
                                    .and_then(Value::as_array)
                                    .is_some_and(|locals| !locals.is_empty())
                                {
                                    self.lower_scalar_local_sequence(
                                        instance,
                                        member,
                                        property_index,
                                    )?
                                } else {
                                    let sequence =
                                        Self::inline_nonlocal_assertion_instance(instance)?;
                                    self.lower_fixed_sequence_antecedent(sequence)?
                                };
                                local_samples.extend(parsed.local_samples.clone());
                                antecedent_immediate =
                                    Some(antecedent_reduction(&self.lower_expression(other)?));
                                let start = parsed
                                    .expressions
                                    .first()
                                    .cloned()
                                    .ok_or_else(|| {
                                        "matched sequence has no expressions".to_string()
                                    })?;
                                antecedent_sequence = Some(parsed);
                                start
                            } else if let Some((left_matched, right_matched)) =
                                Self::matched_sequence_pair(expression)
                            {
                                let parsed = self.lower_matched_sequence_pair(
                                    left_matched,
                                    right_matched,
                                    member,
                                    property_index,
                                    false,
                                )?;
                                local_samples.extend(parsed.local_samples.clone());
                                let start = parsed.expressions.first().cloned().ok_or_else(|| {
                                    "matched sequence pair has no expressions".to_string()
                                })?;
                                antecedent_sequence = Some(parsed);
                                start
                            } else if let Some((left_matched, right_matched)) =
                                Self::matched_sequence_disjunction_pair(expression)
                            {
                                let parsed = self.lower_matched_sequence_pair(
                                    left_matched,
                                    right_matched,
                                    member,
                                    property_index,
                                    true,
                                )?;
                                local_samples.extend(parsed.local_samples.clone());
                                let start = parsed.expressions.first().cloned().ok_or_else(|| {
                                    "matched sequence OR pair has no expressions".to_string()
                                })?;
                                antecedent_sequence = Some(parsed);
                                start
                            } else if let Some((matched, other)) =
                                Self::matched_sequence_conjunction(expression)
                            {
                                let instance = matched
                                    .get("arguments")
                                    .and_then(Value::as_array)
                                    .and_then(|arguments| arguments.first())
                                    .ok_or_else(|| {
                                        "sequence matched() has no argument".to_string()
                                    })?;
                                let mut parsed = if instance
                                    .get("localVars")
                                    .and_then(Value::as_array)
                                    .is_some_and(|locals| !locals.is_empty())
                                {
                                    self.lower_scalar_local_sequence(
                                        instance,
                                        member,
                                        property_index,
                                    )?
                                } else {
                                    let sequence =
                                        Self::inline_nonlocal_assertion_instance(instance)?;
                                    self.lower_fixed_sequence_antecedent(sequence)?
                                };
                                local_samples.extend(parsed.local_samples.clone());
                                let endpoint = parsed
                                    .expressions
                                    .last_mut()
                                    .ok_or_else(|| {
                                        "matched sequence has no expressions".to_string()
                                    })?;
                                *endpoint = eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(antecedent_reduction(endpoint)),
                                    b: Box::new(antecedent_reduction(
                                        &self.lower_expression(other)?,
                                    )),
                                };
                                let start = parsed
                                    .expressions
                                    .first()
                                    .cloned()
                                    .ok_or_else(|| {
                                        "matched sequence has no expressions".to_string()
                                    })?;
                                antecedent_sequence = Some(parsed);
                                start
                            } else if string_field(expression, "kind").as_deref()
                                == Some("AssertionInstance")
                                && expression
                                    .get("localVars")
                                    .and_then(Value::as_array)
                                    .is_some_and(|locals| !locals.is_empty())
                            {
                                let parsed =
                                    self.lower_scalar_local_sequence(expression, member, property_index)?;
                                let start = parsed.expressions.first().cloned().ok_or_else(
                                    || {
                                        "constant-local sequence operand has no expressions"
                                            .to_string()
                                    },
                                )?;
                                antecedent_sequence = Some(parsed);
                                local_samples.extend(
                                    antecedent_sequence
                                        .as_ref()
                                        .map(|sequence| sequence.local_samples.clone())
                                        .unwrap_or_default(),
                                );
                                start
                            } else if Self::is_sequence_matched_call(expression) {
                                let instance = expression
                                    .get("arguments")
                                    .and_then(Value::as_array)
                                    .and_then(|arguments| arguments.first())
                                    .ok_or_else(|| "sequence matched() has no argument".to_string())?;
                                let parsed = if instance
                                    .get("localVars")
                                    .and_then(Value::as_array)
                                    .is_some_and(|locals| !locals.is_empty())
                                {
                                    self.lower_scalar_local_sequence(
                                        instance,
                                        member,
                                        property_index,
                                    )?
                                } else {
                                    let sequence =
                                        Self::inline_nonlocal_assertion_instance(instance)?;
                                    self.lower_fixed_sequence_antecedent(sequence)?
                                };
                                local_samples.extend(parsed.local_samples.clone());
                                let start = parsed
                                    .expressions
                                    .first()
                                    .cloned()
                                    .ok_or_else(|| "matched sequence has no expressions".to_string())?;
                                antecedent_sequence = Some(parsed);
                                start
                            } else {
                                self.lower_expression(expression)?
                            }
                        }
                    }
                    Some("SequenceConcat") => antecedent_sequence
                        .as_ref()
                        .and_then(|sequence| sequence.expressions.first())
                        .cloned()
                        .ok_or_else(|| "temporal antecedent sequence has no expressions".to_string())?,
                    _ => {
                        return Err(
                            "direct concurrent assertions support only simple or fixed-delay sequence antecedents"
                                .into(),
                        );
                    }
                };
                let unrolled_recursive_consequent =
                    self.unroll_self_recursive_tail_property(consequent)?;
                let unrolled_recursive_consequent_storage;
                let consequent = if let Some(unrolled) = unrolled_recursive_consequent {
                    unrolled_recursive_consequent_storage = unrolled;
                    &unrolled_recursive_consequent_storage
                } else {
                    consequent
                };
                let (consequent, delay) =
                    if string_field(consequent, "kind").as_deref() == Some("Simple")
                        && consequent.get("repetition").is_none()
                        && {
                            let expression = consequent.get("expr").ok_or_else(|| {
                                "matched consequent has no expression".to_string()
                            })?;
                            Self::is_sequence_matched_call(expression)
                                || Self::matched_sequence_conjunction(expression).is_some()
                                || Self::matched_sequence_pair(expression).is_some()
                                || Self::matched_sequence_disjunction(expression).is_some()
                                || Self::matched_sequence_disjunction_pair(expression).is_some()
                        }
                    {
                        let expression = consequent
                            .get("expr")
                            .ok_or_else(|| "matched consequent has no expression".to_string())?;
                        if let Some((matched, other)) =
                            Self::matched_sequence_disjunction(expression)
                        {
                            let instance = matched
                                .get("arguments")
                                .and_then(Value::as_array)
                                .and_then(|arguments| arguments.first())
                                .ok_or_else(|| {
                                    "sequence matched() has no argument".to_string()
                                })?;
                            let parsed = if instance
                                .get("localVars")
                                .and_then(Value::as_array)
                                .is_some_and(|locals| !locals.is_empty())
                            {
                                self.lower_scalar_local_sequence(
                                    instance,
                                    member,
                                    property_index,
                                )?
                            } else {
                                let sequence = Self::inline_nonlocal_assertion_instance(instance)?;
                                self.lower_fixed_sequence_antecedent(sequence)?
                            };
                            local_samples.extend(parsed.local_samples.clone());
                            let final_expression = parsed
                                .expressions
                                .last()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched sequence consequent has no expressions".to_string()
                                })?;
                            let immediate =
                                antecedent_reduction(&self.lower_expression(other)?);
                            let delay = ConcurrentDelay::SequenceOrImmediate(
                                parsed.expressions,
                                parsed.delays,
                                Box::new(immediate),
                            );
                            (final_expression, delay)
                        } else if let Some((left_matched, right_matched)) =
                            Self::matched_sequence_pair(expression)
                        {
                            let parsed = self.lower_matched_sequence_pair(
                                left_matched,
                                right_matched,
                                member,
                                property_index,
                                false,
                            )?;
                            local_samples.extend(parsed.local_samples.clone());
                            let final_expression = parsed
                                .expressions
                                .last()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched sequence pair consequent has no expressions"
                                        .to_string()
                                })?;
                            let delay = ConcurrentDelay::Sequence(parsed.expressions, parsed.delays);
                            (final_expression, delay)
                        } else if let Some((left_matched, right_matched)) =
                            Self::matched_sequence_disjunction_pair(expression)
                        {
                            let parsed = self.lower_matched_sequence_pair(
                                left_matched,
                                right_matched,
                                member,
                                property_index,
                                true,
                            )?;
                            local_samples.extend(parsed.local_samples.clone());
                            let final_expression = parsed
                                .expressions
                                .last()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched sequence OR pair consequent has no expressions"
                                        .to_string()
                                })?;
                            let delay = ConcurrentDelay::Sequence(parsed.expressions, parsed.delays);
                            (final_expression, delay)
                        } else {
                        let (matched, conjunction_other) = if let Some((matched, other)) =
                            Self::matched_sequence_conjunction(expression)
                        {
                            (matched, Some(other))
                        } else {
                            (expression, None)
                        };
                        let instance = matched
                            .get("arguments")
                            .and_then(Value::as_array)
                            .and_then(|arguments| arguments.first())
                            .ok_or_else(|| "sequence matched() has no argument".to_string())?;
                        if conjunction_other.is_none()
                            && instance
                                .get("localVars")
                                .and_then(Value::as_array)
                                .is_some_and(|locals| !locals.is_empty())
                        {
                            if let Some(nonterminal) = self
                                .lower_scalar_local_nonterminal_range_matched_consequent(
                                    instance,
                                    member,
                                    property_index,
                                    &antecedent,
                                )?
                            {
                                let ScalarLocalNonterminalRangeLowering {
                                    prefix,
                                    prefix_miss,
                                    range_start,
                                    range_end,
                                    suffix_delays,
                                    middle_boundaries,
                                    suffix_boundaries,
                                    final_boundaries,
                                    samples,
                                } = nonterminal;
                                let final_expression = final_boundaries
                                    .iter()
                                    .cloned()
                                    .reduce(|left, right| eda_core::ir::Expr::Bin {
                                        op: eda_core::ir::BinOp::LogOr,
                                        a: Box::new(left),
                                        b: Box::new(right),
                                    })
                                    .ok_or_else(|| {
                                        "scalar-local nonterminal matched range has no boundaries"
                                            .to_string()
                                    })?;
                                local_samples.extend(samples);
                                return Ok(ConcurrentProperty {
                                    antecedent: antecedent.clone(),
                                    antecedent_sequence,
                                    antecedent_goto,
                                    antecedent_consecutive_range,
                                    antecedent_goto_range,
                                    antecedent_nonconsecutive_range,
                                    antecedent_immediate,
                                    local_samples,
                                    consequent: final_expression,
                                    delay: ConcurrentDelay::LocalNonterminalPrefixRange {
                                        prefix: Box::new(prefix),
                                        prefix_miss: Box::new(prefix_miss),
                                        range_start,
                                        range_end,
                                        suffix_delays,
                                        middle_boundaries,
                                        suffix_boundaries,
                                        final_boundaries,
                                    },
                                });
                            }
                            if let Some(local_range) = self
                                .lower_scalar_local_range_matched_consequent(
                                    instance,
                                    member,
                                    property_index,
                                    &antecedent,
                                )?
                            {
                                let ScalarLocalRangeLowering {
                                    prefix,
                                    prefix_miss,
                                    start: range_start,
                                    end: range_end,
                                    boundaries,
                                    prefix_includes_antecedent,
                                    samples,
                                } = local_range;
                                local_samples.extend(samples);
                                let final_expression = boundaries
                                    .iter()
                                    .cloned()
                                    .reduce(|left, right| eda_core::ir::Expr::Bin {
                                        op: eda_core::ir::BinOp::LogOr,
                                        a: Box::new(left),
                                        b: Box::new(right),
                                    })
                                    .ok_or_else(|| {
                                        "scalar-local matched range has no boundaries".to_string()
                                    })?;
                                return Ok(ConcurrentProperty {
                                    antecedent: antecedent.clone(),
                                    antecedent_sequence,
                                    antecedent_goto,
                                    antecedent_consecutive_range,
                                    antecedent_goto_range,
                                    antecedent_nonconsecutive_range,
                                    antecedent_immediate,
                                    local_samples,
                                    consequent: final_expression,
                                    delay: ConcurrentDelay::PrefixRange(
                                        Box::new(prefix.clone()),
                                        Box::new(prefix_miss),
                                        range_start,
                                        range_end,
                                        boundaries,
                                        prefix_includes_antecedent,
                                    ),
                                });
                            }
                        }
                        let parsed = if instance
                            .get("localVars")
                            .and_then(Value::as_array)
                            .is_some_and(|locals| !locals.is_empty())
                        {
                            self.lower_scalar_local_sequence(
                                instance,
                                member,
                                property_index,
                            )?
                        } else {
                            let sequence = Self::inline_nonlocal_assertion_instance(instance)?;
                            self.lower_fixed_sequence_antecedent(sequence)?
                        };
                        if let Some((start, end)) = parsed.range {
                            let first = parsed
                                .expressions
                                .first()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched range consequent has no first expression".to_string()
                                })?;
                            let final_expression = parsed
                                .expressions
                                .last()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched range consequent has no final expression".to_string()
                                })?;
                            if conjunction_other.is_some() {
                                return Err(
                                    "matched range consequents cannot combine a delay range with another expression"
                                        .into(),
                                );
                            }
                            let suffix_count = parsed.expressions.len().saturating_sub(2);
                            if suffix_count > 0
                                && parsed.range_element == 1
                                && parsed.delays.len() == parsed.expressions.len()
                                && parsed.delays.first() == Some(&0)
                                && parsed.delays.get(1) == Some(&end)
                                && parsed.delays[2..].iter().all(|delay| *delay > 0)
                            {
                                let middle = parsed
                                    .expressions
                                    .get(1)
                                    .cloned()
                                    .ok_or_else(|| {
                                        "nonterminal matched range has no middle expression"
                                            .to_string()
                                    })?;
                                (
                                    final_expression.clone(),
                                    ConcurrentDelay::NonterminalPrefixRange {
                                        prefix: Box::new(first.clone()),
                                        prefix_miss: Box::new(eda_core::ir::Expr::Bin {
                                            op: eda_core::ir::BinOp::LogAnd,
                                            a: Box::new(antecedent_reduction(&antecedent)),
                                            b: Box::new(eda_core::ir::Expr::Un {
                                                op: eda_core::ir::UnOp::Not,
                                                a: Box::new(antecedent_reduction(&first)),
                                            }),
                                        }),
                                        range_start: start,
                                        range_end: end,
                                        suffix_delays: parsed.delays[2..].to_vec(),
                                        middle: Box::new(middle),
                                        suffix_expressions: parsed.expressions[2..].to_vec(),
                                    },
                                )
                            } else {
                                if parsed.expressions.len() != 2
                                    || parsed.delays != vec![0, end]
                                    || parsed.range_element != 1
                                {
                                    return Err(
                                        "matched range consequents support one terminal [a:b] range or first ##[a:b] second ##n third"
                                            .into(),
                                    );
                                }
                                (
                                    final_expression.clone(),
                                    ConcurrentDelay::PrefixRange(
                                        Box::new(first.clone()),
                                        Box::new(eda_core::ir::Expr::Bin {
                                            op: eda_core::ir::BinOp::LogAnd,
                                            a: Box::new(antecedent_reduction(&antecedent)),
                                            b: Box::new(eda_core::ir::Expr::Un {
                                                op: eda_core::ir::UnOp::Not,
                                                a: Box::new(antecedent_reduction(&first)),
                                            }),
                                        }),
                                        start,
                                        end,
                                        vec![final_expression.clone(); end - start + 1],
                                        false,
                                    ),
                                )
                            }
                        } else {
                            local_samples.extend(parsed.local_samples.clone());
                            let mut parsed = parsed;
                            if let Some(other) = conjunction_other {
                                let endpoint = parsed
                                    .expressions
                                    .last_mut()
                                    .ok_or_else(|| {
                                        "matched sequence consequent has no expressions"
                                            .to_string()
                                    })?;
                                *endpoint = eda_core::ir::Expr::Bin {
                                    op: eda_core::ir::BinOp::LogAnd,
                                    a: Box::new(antecedent_reduction(endpoint)),
                                    b: Box::new(antecedent_reduction(
                                        &self.lower_expression(other)?,
                                    )),
                                };
                            }
                            let final_expression = parsed
                                .expressions
                                .last()
                                .cloned()
                                .ok_or_else(|| {
                                    "matched sequence consequent has no expressions".to_string()
                                })?;
                            let delay =
                                ConcurrentDelay::Sequence(parsed.expressions, parsed.delays);
                            (final_expression, delay)
                        }
                        }
                    } else if let Some(instance) = Self::local_assertion_instance(consequent) {
                        let parsed = self.lower_scalar_local_sequence(instance, member, property_index)?;
                        local_samples.extend(parsed.local_samples.clone());
                        let final_expression = parsed
                            .expressions
                            .last()
                            .cloned()
                            .ok_or_else(|| {
                                "constant-local sequence consequent has no expressions".to_string()
                            })?;
                        let delay = ConcurrentDelay::Sequence(parsed.expressions, parsed.delays);
                        (final_expression, delay)
                    } else {
                        let consequent = Self::inline_simple_assertion_instance(consequent)?;
                        if string_field(consequent, "kind").as_deref() == Some("Simple") {
                            if consequent.get("repetition").is_some() {
                                self.lower_repetition_operand(consequent)?
                            } else {
                                (
                                    self.lower_expression(Self::simple_property_operand(
                                        consequent,
                                        "right",
                                    )?)?,
                                    ConcurrentDelay::SameCycle,
                                )
                            }
                        } else if matches!(
                            string_field(consequent, "op").as_deref(),
                            Some("And") | Some("Or") | Some("Iff")
                        ) && matches!(
                            string_field(consequent, "kind").as_deref(),
                            Some("Binary")
                        ) {
                            (
                                self.lower_same_cycle_boolean_property(consequent)?,
                                ConcurrentDelay::SameCycle,
                            )
                        } else {
                            self.lower_fixed_sequence_consequent(consequent)?
                        }
                    };
                let delay = if antecedent_sequence.is_some() {
                    match delay {
                        ConcurrentDelay::Consecutive(count) => {
                            let mut delays = Vec::with_capacity(count);
                            delays.push(0);
                            delays.resize(count, 1);
                            ConcurrentDelay::Sequence(vec![consequent.clone(); count], delays)
                        }
                        other => other,
                    }
                } else {
                    delay
                };
                if antecedent_sequence.is_some()
                    && !matches!(delay, ConcurrentDelay::SameCycle)
                    && !matches!(delay, ConcurrentDelay::Fixed(_))
                    && !matches!(delay, ConcurrentDelay::Range(_, _))
                    && !matches!(delay, ConcurrentDelay::Sequence(_, _))
                    && !matches!(delay, ConcurrentDelay::Goto(_))
                    && !matches!(delay, ConcurrentDelay::Nonconsecutive(_))
                {
                    return Err(
                        "temporal sequence antecedents support only same-cycle, fixed-delay, delay-range, fixed-delay sequence, fixed goto, or fixed nonconsecutive consequents"
                            .into(),
                    );
                }
                Ok(ConcurrentProperty {
                    antecedent,
                    antecedent_sequence,
                    antecedent_goto,
                    antecedent_consecutive_range,
                    antecedent_goto_range,
                    antecedent_nonconsecutive_range,
                    antecedent_immediate,
                    local_samples,
                    consequent,
                    delay,
                })
            }
            _ => Err(
                "direct concurrent assertion lowering supports only same-cycle Simple or overlapped-implication properties"
                    .into(),
            ),
        }
    }

    fn lower_optional_assertion_action(
        &self,
        action: Option<&serde_json::Value>,
    ) -> Result<Option<Box<Stmt>>, String> {
        let Some(action) = action else {
            return Ok(None);
        };
        if string_field(action, "kind").as_deref() == Some("Empty") {
            return Ok(None);
        }
        Ok(Some(Box::new(self.lower_statement(action)?)))
    }

    fn lower_same_cycle_boolean_property(
        &self,
        property_expr: &serde_json::Value,
    ) -> Result<eda_core::ir::Expr, String> {
        let property_expr = Self::inline_simple_assertion_instance(property_expr)?;
        match string_field(property_expr, "kind").as_deref() {
            Some("Simple") => {
                self.lower_expression(Self::simple_property_operand(property_expr, "composition")?)
            }
            Some("Binary")
                if matches!(
                    string_field(property_expr, "op").as_deref(),
                    Some("And") | Some("Or") | Some("Iff")
                ) =>
            {
                let left = property_expr
                    .get("left")
                    .ok_or_else(|| "property composition has no left operand".to_string())?;
                let right = property_expr
                    .get("right")
                    .ok_or_else(|| "property composition has no right operand".to_string())?;
                let left = self.lower_same_cycle_boolean_property(left)?;
                let right = self.lower_same_cycle_boolean_property(right)?;
                let op = match string_field(property_expr, "op").as_deref() {
                    Some("And") => eda_core::ir::BinOp::LogAnd,
                    Some("Iff") => eda_core::ir::BinOp::Eq,
                    _ => eda_core::ir::BinOp::LogOr,
                };
                let (left, right) = if string_field(property_expr, "op").as_deref() == Some("Iff") {
                    (antecedent_reduction(&left), antecedent_reduction(&right))
                } else {
                    (left, right)
                };
                Ok(eda_core::ir::Expr::Bin {
                    op,
                    a: Box::new(left),
                    b: Box::new(right),
                })
            }
            _ => Err(
                "direct same-cycle property composition supports only simple operands and AND/OR"
                    .into(),
            ),
        }
    }

    fn inline_nonlocal_assertion_instance(
        value: &serde_json::Value,
    ) -> Result<&serde_json::Value, String> {
        if string_field(value, "kind").as_deref() != Some("AssertionInstance") {
            return Ok(value);
        }
        let empty_local_vars = Vec::new();
        let local_vars = value
            .get("localVars")
            .and_then(Value::as_array)
            .unwrap_or(&empty_local_vars);
        if !local_vars.is_empty() {
            return Err(
                "direct concurrent assertions do not support assertion-instance local variables"
                    .into(),
            );
        }
        if bool_field(value, "isRecursiveProperty") {
            return Err(
                "direct concurrent assertions do not support recursive assertion instances".into(),
            );
        }
        value
            .get("body")
            .ok_or_else(|| "assertion instance has no elaborated body".to_string())
    }

    fn true_simple_property() -> Value {
        let mut expression = serde_json::Map::new();
        expression.insert("kind".into(), Value::String("IntegerLiteral".into()));
        expression.insert("type".into(), Value::String("bit".into()));
        expression.insert("value".into(), Value::String("1'b1".into()));
        expression.insert("constant".into(), Value::String("1'b1".into()));
        let mut simple = serde_json::Map::new();
        simple.insert("kind".into(), Value::String("Simple".into()));
        simple.insert("expr".into(), Value::Object(expression));
        Value::Object(simple)
    }

    fn unroll_self_recursive_tail_property(
        &self,
        property_expr: &serde_json::Value,
    ) -> Result<Option<Value>, String> {
        let template = if string_field(property_expr, "kind").as_deref() == Some("Simple") {
            property_expr
                .get("expr")
                .and_then(|expression| expression.get("body"))
        } else {
            property_expr.get("body")
        };
        let Some(template) = template else {
            return Ok(None);
        };
        let template = (|| {
            let right = template.get("right")?;
            let instance = right.get("expr")?;
            if string_field(instance, "kind").as_deref() != Some("AssertionInstance")
                || bool_field(instance, "isRecursiveProperty")
            {
                return None;
            }
            let body = instance.get("body")?;
            if string_field(body, "kind").as_deref() == Some("Binary")
                && string_field(body, "op").as_deref() == Some("NonOverlappedImplication")
            {
                Some(body)
            } else {
                None
            }
        })()
        .unwrap_or(template);
        if string_field(template, "kind").as_deref() != Some("Binary")
            || string_field(template, "op").as_deref() != Some("NonOverlappedImplication")
        {
            return Ok(None);
        }
        let left = template
            .get("left")
            .ok_or_else(|| "recursive tail property has no antecedent".to_string())?;
        let right = template
            .get("right")
            .ok_or_else(|| "recursive tail property has no recursive consequent".to_string())?;
        if string_field(left, "kind").as_deref() != Some("SequenceConcat") {
            return Ok(None);
        }
        let Some(instance) = right.get("expr").filter(|expression| {
            string_field(expression, "kind").as_deref() == Some("AssertionInstance")
        }) else {
            return Ok(None);
        };
        if !bool_field(instance, "isRecursiveProperty")
            || instance
                .get("body")
                .and_then(|body| string_field(body, "kind"))
                .as_deref()
                != Some("Invalid")
            || instance
                .get("localVars")
                .and_then(Value::as_array)
                .is_some_and(|locals| !locals.is_empty())
        {
            return Ok(None);
        }

        let source_elements = left
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "recursive tail antecedent has no elements".to_string())?;
        if source_elements.len() < 2
            || source_elements.first().is_some_and(|element| {
                element.get("min") != element.get("max")
                    || element.get("min").and_then(Value::as_u64) != Some(0)
            })
            || source_elements.iter().skip(1).any(|element| {
                element.get("min") != element.get("max")
                    || element.get("min").and_then(Value::as_u64) == Some(0)
            })
        {
            return Err(
                "recursive tail properties require positive fixed-delay antecedent elements".into(),
            );
        }

        let mut elements = Vec::new();
        let repetition_count = self.recursive_property_depth + 1;
        for repetition in 0..repetition_count {
            if repetition > 0 {
                let mut separator = serde_json::Map::new();
                separator.insert("sequence".into(), Self::true_simple_property());
                separator.insert("min".into(), Value::from(1));
                separator.insert("max".into(), Value::from(1));
                elements.push(Value::Object(separator));
            }
            elements.extend(source_elements.iter().cloned());
        }
        let mut terminal = serde_json::Map::new();
        terminal.insert("sequence".into(), Self::true_simple_property());
        terminal.insert("min".into(), Value::from(0));
        terminal.insert("max".into(), Value::from(0));
        elements.push(Value::Object(terminal));

        let mut unrolled = serde_json::Map::new();
        unrolled.insert("kind".into(), Value::String("SequenceConcat".into()));
        unrolled.insert("elements".into(), Value::Array(elements));
        Ok(Some(Value::Object(unrolled)))
    }

    fn local_assertion_instance(value: &serde_json::Value) -> Option<&serde_json::Value> {
        if string_field(value, "kind").as_deref() != Some("Simple")
            || value.get("repetition").is_some()
        {
            return None;
        }
        let expression = value.get("expr")?;
        if string_field(expression, "kind").as_deref() != Some("AssertionInstance") {
            return None;
        }
        expression
            .get("localVars")
            .and_then(Value::as_array)
            .is_some_and(|locals| !locals.is_empty())
            .then_some(expression)
    }

    fn local_property_implication_as_sequence_instance(
        instance: &serde_json::Value,
    ) -> Result<Value, String> {
        let body = instance
            .get("body")
            .ok_or_else(|| "local assertion instance has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("Binary")
            || string_field(body, "op").as_deref() != Some("OverlappedImplication")
        {
            return Err(
                "local assertion properties support SequenceConcat or fixed-delay overlapped implications"
                    .into(),
            );
        }
        let antecedent = body
            .get("left")
            .ok_or_else(|| "local property implication has no antecedent".to_string())?;
        if string_field(antecedent, "kind").as_deref() != Some("SequenceWithMatch") {
            return Err(
                "local property implications require a matching assignment antecedent".into(),
            );
        }
        let consequent = body
            .get("right")
            .ok_or_else(|| "local property implication has no consequent".to_string())?;
        if string_field(consequent, "kind").as_deref() != Some("SequenceConcat") {
            return Err(
                "local property implications require a fixed-delay sequence consequent".into(),
            );
        }
        let consequent_elements = consequent
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "local property consequent has no elements".to_string())?;
        if consequent_elements.is_empty() {
            return Err("local property consequent sequence is empty".into());
        }

        let mut first_element = serde_json::Map::new();
        first_element.insert("sequence".into(), antecedent.clone());
        first_element.insert("min".into(), Value::from(0));
        first_element.insert("max".into(), Value::from(0));
        let mut elements = vec![Value::Object(first_element)];
        elements.extend(consequent_elements.iter().cloned());
        let mut sequence_body = serde_json::Map::new();
        sequence_body.insert("kind".into(), Value::String("SequenceConcat".into()));
        sequence_body.insert("elements".into(), Value::Array(elements));

        let mut normalized = instance.clone();
        if let Value::Object(fields) = &mut normalized {
            fields.insert("body".into(), Value::Object(sequence_body));
        }
        Ok(normalized)
    }

    fn inline_simple_assertion_instance(
        value: &serde_json::Value,
    ) -> Result<&serde_json::Value, String> {
        let value = Self::inline_nonlocal_assertion_instance(value)?;
        if string_field(value, "kind").as_deref() == Some("Simple") {
            if let Some(expression) = value.get("expr") {
                if string_field(expression, "kind").as_deref() == Some("AssertionInstance") {
                    return Self::inline_nonlocal_assertion_instance(expression);
                }
            }
        }
        Ok(value)
    }

    fn is_sequence_matched_call(value: &serde_json::Value) -> bool {
        string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine").as_deref() == Some("matched")
            && value
                .get("arguments")
                .and_then(Value::as_array)
                .is_some_and(|arguments| {
                    arguments.len() == 1
                        && string_field(&arguments[0], "kind").as_deref()
                            == Some("AssertionInstance")
                })
    }

    fn matched_sequence_conjunction(
        value: &serde_json::Value,
    ) -> Option<(&serde_json::Value, &serde_json::Value)> {
        if string_field(value, "kind").as_deref() != Some("BinaryOp")
            || string_field(value, "op").as_deref() != Some("LogicalAnd")
        {
            return None;
        }
        let left = value.get("left")?;
        let right = value.get("right")?;
        if Self::is_sequence_matched_call(left) && !Self::is_sequence_matched_call(right) {
            Some((left, right))
        } else if Self::is_sequence_matched_call(right) && !Self::is_sequence_matched_call(left) {
            Some((right, left))
        } else {
            None
        }
    }

    fn matched_sequence_disjunction(
        value: &serde_json::Value,
    ) -> Option<(&serde_json::Value, &serde_json::Value)> {
        if string_field(value, "kind").as_deref() != Some("BinaryOp")
            || string_field(value, "op").as_deref() != Some("LogicalOr")
        {
            return None;
        }
        let left = value.get("left")?;
        let right = value.get("right")?;
        if Self::is_sequence_matched_call(left) && !Self::is_sequence_matched_call(right) {
            Some((left, right))
        } else if Self::is_sequence_matched_call(right) && !Self::is_sequence_matched_call(left) {
            Some((right, left))
        } else {
            None
        }
    }

    fn matched_sequence_pair(
        value: &serde_json::Value,
    ) -> Option<(&serde_json::Value, &serde_json::Value)> {
        if string_field(value, "kind").as_deref() != Some("BinaryOp")
            || string_field(value, "op").as_deref() != Some("LogicalAnd")
        {
            return None;
        }
        let left = value.get("left")?;
        let right = value.get("right")?;
        if Self::is_sequence_matched_call(left) && Self::is_sequence_matched_call(right) {
            Some((left, right))
        } else {
            None
        }
    }

    fn matched_sequence_disjunction_pair(
        value: &serde_json::Value,
    ) -> Option<(&serde_json::Value, &serde_json::Value)> {
        if string_field(value, "kind").as_deref() != Some("BinaryOp")
            || string_field(value, "op").as_deref() != Some("LogicalOr")
        {
            return None;
        }
        let left = value.get("left")?;
        let right = value.get("right")?;
        if Self::is_sequence_matched_call(left) && Self::is_sequence_matched_call(right) {
            Some((left, right))
        } else {
            None
        }
    }

    fn matched_call_instance(value: &serde_json::Value) -> Result<&serde_json::Value, String> {
        value
            .get("arguments")
            .and_then(Value::as_array)
            .and_then(|arguments| arguments.first())
            .ok_or_else(|| "sequence matched() has no argument".to_string())
    }

    /// Lower `(trigger, local = first) ##delay
    /// (update, local = second) ##[start:end] local-expression`.
    fn lower_repeated_scalar_local_range_matched_consequent(
        &mut self,
        instance: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        property_antecedent: &eda_core::ir::Expr,
    ) -> Result<Option<ScalarLocalRangeLowering>, String> {
        let body = instance
            .get("body")
            .ok_or_else(|| "repeated scalar-local matched range has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("SequenceConcat") {
            return Ok(None);
        }
        let elements = body
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "repeated scalar-local matched range has no elements".to_string())?;
        if elements.len() != 3 {
            return Ok(None);
        }
        let local_vars = instance
            .get("localVars")
            .and_then(Value::as_array)
            .ok_or_else(|| "repeated scalar-local matched range has no locals".to_string())?;
        if local_vars.len() != 1 {
            return Ok(None);
        }
        let local = &local_vars[0];
        let local_name = string_field(local, "name")
            .ok_or_else(|| "assertion-instance local variable has no name".to_string())?;
        let local_type = local
            .get("type")
            .ok_or_else(|| "assertion-instance local variable has no type".to_string())?;
        let local_type_name = local_type
            .as_str()
            .or_else(|| local_type.get("name").and_then(Value::as_str));
        if !matches!(local_type_name, Some("logic") | Some("bit")) {
            return Ok(None);
        }
        fn parse_assignment<'v>(
            element: &'v serde_json::Value,
            local_name: &str,
        ) -> Option<(&'v serde_json::Value, &'v serde_json::Value)> {
            let sequence = element.get("sequence")?;
            if string_field(sequence, "kind").as_deref() != Some("SequenceWithMatch") {
                return None;
            }
            let items = sequence.get("matchItems")?.as_array()?;
            if items.len() != 1 {
                return None;
            }
            let assignment = items.first()?;
            if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                return None;
            }
            let target = assignment.get("left")?;
            let symbol = expression_symbol(target);
            let is_local = symbol.is_some_and(|symbol| {
                symbol == local_name || symbol.ends_with(&format!(" {local_name}"))
            });
            if !is_local {
                return None;
            }
            Some((sequence, assignment))
        }

        let Some((first_sequence, first_assignment)) = parse_assignment(&elements[0], &local_name)
        else {
            return Ok(None);
        };
        if elements[0].get("min").and_then(Value::as_u64) != Some(0)
            || elements[0].get("max").and_then(Value::as_u64) != Some(0)
        {
            return Ok(None);
        }
        let Some((second_sequence, second_assignment)) =
            parse_assignment(&elements[1], &local_name)
        else {
            return Ok(None);
        };
        let update_delay = elements[1]
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "repeated scalar-local matched range has no update delay".to_string())?;
        let update_max = elements[1]
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                "repeated scalar-local matched range has no update maximum".to_string()
            })?;
        if update_delay == 0 || update_delay != update_max || update_delay > 256 {
            return Ok(None);
        }

        let first_rhs = first_assignment
            .get("right")
            .ok_or_else(|| "repeated scalar-local range assignment has no value".to_string())?;
        let second_rhs = second_assignment
            .get("right")
            .ok_or_else(|| "repeated scalar-local range assignment has no value".to_string())?;
        if json_references_local_symbol(first_rhs, &local_name) {
            return Err(
                "the first repeated scalar-local matched range assignment cannot read its local"
                    .into(),
            );
        }
        let trigger_source = first_sequence
            .get("expr")
            .ok_or_else(|| "repeated scalar-local matched range has no trigger".to_string())?;
        let trigger =
            self.lower_expression(Self::simple_property_operand(trigger_source, "left")?)?;
        let first_gate = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogAnd,
            a: Box::new(antecedent_reduction(property_antecedent)),
            b: Box::new(antecedent_reduction(&trigger)),
        };
        let update_source = second_sequence
            .get("expr")
            .ok_or_else(|| "repeated scalar-local matched range has no update".to_string())?;
        let update =
            self.lower_expression(Self::simple_property_operand(update_source, "left")?)?;
        let first_value_placeholder = self.lower_expression(first_rhs)?;
        let update_delay_age = usize::try_from(update_delay).map_err(|_| {
            "repeated scalar-local matched range update delay overflows".to_string()
        })?;
        let local_names_for_rhs = vec![local_name.clone()];
        let (rewritten_second_rhs, second_rhs_references) = rewrite_sequence_local_references(
            second_rhs,
            &local_names_for_rhs,
            property_index,
            update_delay_age,
        );
        let final_element = &elements[2];
        let final_sequence = final_element.get("sequence").ok_or_else(|| {
            "repeated scalar-local matched range has no final sequence".to_string()
        })?;
        let final_source = Self::simple_property_operand(final_sequence, "right")?;
        let start = final_element
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "repeated scalar-local matched range has no minimum".to_string())?;
        let end = final_element
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "repeated scalar-local matched range has no maximum".to_string())?;
        if start == 0 || start > end || end > 256 || update_delay + end > 256 {
            return Ok(None);
        }
        let start = start as usize;
        let end = end as usize;
        let update_delay = update_delay as usize;

        let mut first_sample_nets = Vec::with_capacity(update_delay);
        for stage in 0..update_delay {
            first_sample_nets.push(self.declare_property_local_sample_net(
                member,
                property_index,
                0,
                (0, 0, stage),
                1,
                false,
            ));
        }
        let second_rhs_key = format!("__eda_slseq_{property_index}_0_{update_delay}");
        if !second_rhs_references.is_empty() {
            self.symbols.insert(
                second_rhs_key.clone(),
                AstLoweredSymbol {
                    base: first_sample_nets[update_delay - 1],
                    width: 1,
                    signed: false,
                    bounds: None,
                    dimensions: Vec::new(),
                    memory: None,
                    struct_fields: std::collections::BTreeMap::new(),
                },
            );
        }
        let second_value = self.lower_expression(&rewritten_second_rhs)?;
        if !second_rhs_references.is_empty() {
            self.symbols.remove(&second_rhs_key);
        }
        let mut history_nets = Vec::with_capacity(update_delay);
        let mut attempt_history_nets = Vec::with_capacity(update_delay);
        for stage in 0..update_delay {
            history_nets.push(self.declare_property_local_history_net(
                member,
                property_index,
                0,
                0,
                1,
                (0, stage),
            ));
            attempt_history_nets.push(self.declare_property_local_history_net(
                member,
                property_index,
                0,
                0,
                1,
                (1, stage),
            ));
        }
        let history_last = eda_core::ir::Expr::Ref {
            net: history_nets[update_delay - 1],
            decl_width: 1,
            range: full_range(1),
            signed: false,
        };
        let attempt_history_last = eda_core::ir::Expr::Ref {
            net: attempt_history_nets[update_delay - 1],
            decl_width: 1,
            range: full_range(1),
            signed: false,
        };
        let prefix = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogAnd,
            a: Box::new(history_last.clone()),
            b: Box::new(antecedent_reduction(&update)),
        };
        let prefix_miss = eda_core::ir::Expr::Bin {
            op: eda_core::ir::BinOp::LogAnd,
            a: Box::new(attempt_history_last),
            b: Box::new(eda_core::ir::Expr::Un {
                op: eda_core::ir::UnOp::Not,
                a: Box::new(prefix.clone()),
            }),
        };

        let mut sample_nets = Vec::with_capacity(end);
        for stage in 0..end {
            sample_nets.push(self.declare_property_local_sample_net(
                member,
                property_index,
                0,
                (0, 1, stage),
                1,
                false,
            ));
        }
        let local_names = vec![local_name.clone()];
        let mut boundaries = Vec::with_capacity(end - start + 1);
        for delay in start..=end {
            let (rewritten, references) = rewrite_sequence_local_references(
                final_source,
                &local_names,
                property_index,
                delay,
            );
            if references.is_empty() {
                return Ok(None);
            }
            let net = sample_nets.get(delay - 1).copied().ok_or_else(|| {
                "repeated scalar-local matched range has no sample state".to_string()
            })?;
            let key = format!("__eda_slseq_{property_index}_0_{delay}");
            self.symbols.insert(
                key.clone(),
                AstLoweredSymbol {
                    base: net,
                    width: 1,
                    signed: false,
                    bounds: None,
                    dimensions: Vec::new(),
                    memory: None,
                    struct_fields: std::collections::BTreeMap::new(),
                },
            );
            let lowered = self.lower_expression(&rewritten)?;
            self.symbols.remove(&key);
            boundaries.push(antecedent_reduction(&lowered));
        }

        let sample = ConcurrentLocalSample {
            width: 1,
            signed: false,
            trigger: prefix.clone(),
            value: second_value,
            nets: sample_nets,
            histories: vec![
                ConcurrentLocalHistory {
                    trigger: antecedent_reduction(property_antecedent),
                    nets: attempt_history_nets,
                },
                ConcurrentLocalHistory {
                    trigger: first_gate.clone(),
                    nets: history_nets,
                },
            ],
        };
        Ok(Some(ScalarLocalRangeLowering {
            prefix,
            prefix_miss,
            start,
            end,
            boundaries,
            prefix_includes_antecedent: true,
            samples: vec![
                ConcurrentLocalSample {
                    width: 1,
                    signed: false,
                    trigger: first_gate,
                    value: first_value_placeholder,
                    nets: first_sample_nets,
                    histories: Vec::new(),
                },
                sample,
            ],
        }))
    }

    /// Lower one scalar-local expression against a first-assignment sample at
    /// the requested age.
    fn lower_local_expression_at_age(
        &mut self,
        source: &serde_json::Value,
        local_names: &[String],
        sample_nets: &[Vec<usize>],
        property_index: usize,
        age: usize,
    ) -> Result<Option<eda_core::ir::Expr>, String> {
        let (rewritten, references) =
            rewrite_sequence_local_references(source, local_names, property_index, age);
        if references.is_empty() {
            let lowered = self.lower_expression(source)?;
            return Ok(Some(antecedent_reduction(&lowered)));
        }
        let mut temporary_symbols = Vec::new();
        for local_index in references
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
        {
            let net = sample_nets
                .get(local_index)
                .and_then(|nets| nets.get(age.saturating_sub(1)))
                .copied()
                .ok_or_else(|| {
                    "scalar-local nonterminal matched range has no sample state".to_string()
                })?;
            let key = format!("__eda_slseq_{property_index}_{local_index}_{age}");
            self.symbols.insert(
                key.clone(),
                AstLoweredSymbol {
                    base: net,
                    width: 1,
                    signed: false,
                    bounds: None,
                    dimensions: Vec::new(),
                    memory: None,
                    struct_fields: std::collections::BTreeMap::new(),
                },
            );
            temporary_symbols.push(key);
        }
        let lowered = self.lower_expression(&rewritten)?;
        for key in temporary_symbols {
            self.symbols.remove(&key);
        }
        Ok(Some(antecedent_reduction(&lowered)))
    }

    /// Lower the local-bearing nonterminal-range subset for a matched
    /// consequent: `(trigger, locals = values...) ##[start:end] middle ##n ... last`.
    #[allow(clippy::type_complexity)]
    fn lower_scalar_local_nonterminal_range_matched_consequent(
        &mut self,
        instance: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        property_antecedent: &eda_core::ir::Expr,
    ) -> Result<Option<ScalarLocalNonterminalRangeLowering>, String> {
        let body = instance
            .get("body")
            .ok_or_else(|| "scalar-local nonterminal matched range has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("SequenceConcat") {
            return Ok(None);
        }
        let elements = body
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local nonterminal matched range has no elements".to_string())?;
        if elements.len() < 3 {
            return Ok(None);
        }

        let range_element = &elements[1];
        let range_start = range_element
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "scalar-local nonterminal matched range has no minimum".to_string())?;
        let range_end = range_element
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "scalar-local nonterminal matched range has no maximum".to_string())?;
        if range_start == range_end {
            return Ok(None);
        }
        if range_start == 0 || range_start > range_end || range_end > 256 {
            return Ok(None);
        }
        let range_start = range_start as usize;
        let range_end = range_end as usize;

        let mut suffix_delays = Vec::with_capacity(elements.len() - 2);
        for element in elements.iter().skip(2) {
            let minimum = element.get("min").and_then(Value::as_u64).ok_or_else(|| {
                "scalar-local nonterminal matched range suffix has no minimum".to_string()
            })?;
            let maximum = element.get("max").and_then(Value::as_u64).ok_or_else(|| {
                "scalar-local nonterminal matched range suffix has no maximum".to_string()
            })?;
            if minimum != maximum || minimum == 0 || maximum > 256 {
                return Ok(None);
            }
            suffix_delays.push(minimum as usize);
        }
        let suffix_total_delay = suffix_delays.iter().try_fold(0_usize, |total, delay| {
            total.checked_add(*delay).ok_or_else(|| {
                "scalar-local nonterminal matched suffix delay total overflows".to_string()
            })
        })?;
        let alternative_count = range_end - range_start + 1;
        let alternative_state_count = alternative_count
            .checked_mul(suffix_total_delay)
            .ok_or_else(|| {
                "scalar-local nonterminal matched alternative state overflows".to_string()
            })?;
        if alternative_state_count > 256 {
            return Err(
                "scalar-local nonterminal matched ranges support at most 256 alternative-delay states"
                    .into(),
            );
        }

        let local_vars = instance
            .get("localVars")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local nonterminal matched range has no locals".to_string())?;
        if local_vars.is_empty() || local_vars.len() > 64 {
            return Ok(None);
        }
        let mut local_names = Vec::with_capacity(local_vars.len());
        for local in local_vars {
            let name = string_field(local, "name")
                .ok_or_else(|| "assertion-instance local variable has no name".to_string())?;
            let local_type = local
                .get("type")
                .ok_or_else(|| "assertion-instance local variable has no type".to_string())?;
            let local_type_name = local_type
                .as_str()
                .or_else(|| local_type.get("name").and_then(Value::as_str));
            if !matches!(local_type_name, Some("logic") | Some("bit")) {
                return Ok(None);
            }
            local_names.push(name);
        }
        let local_symbol_index = |symbol: Option<&str>| {
            symbol.and_then(|symbol| {
                local_names
                    .iter()
                    .position(|name| symbol == *name || symbol.ends_with(&format!(" {name}")))
            })
        };

        let first = &elements[0];
        let first_sequence = first.get("sequence").ok_or_else(|| {
            "scalar-local nonterminal matched range has no first sequence".to_string()
        })?;
        if string_field(first_sequence, "kind").as_deref() != Some("SequenceWithMatch")
            || first.get("min").and_then(Value::as_u64) != Some(0)
            || first.get("max").and_then(Value::as_u64) != Some(0)
        {
            return Ok(None);
        }
        let match_items = first_sequence
            .get("matchItems")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "scalar-local nonterminal matched range has no match items".to_string()
            })?;
        if match_items.len() != local_vars.len() {
            return Ok(None);
        }
        let trigger_source = first_sequence
            .get("expr")
            .ok_or_else(|| "scalar-local matched range has no trigger".to_string())?;
        let trigger =
            self.lower_expression(Self::simple_property_operand(trigger_source, "left")?)?;
        let mut assigned_values = vec![None; local_vars.len()];
        let mut assigned_targets = vec![false; local_vars.len()];
        for assignment in match_items {
            if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                return Ok(None);
            }
            let assigned = assignment
                .get("left")
                .ok_or_else(|| "scalar-local range assignment has no target".to_string())?;
            let local_index = local_symbol_index(expression_symbol(assigned).as_deref())
                .ok_or_else(|| {
                    "scalar-local matched range assignments must target local variables".to_string()
                })?;
            if assigned_targets[local_index] {
                return Ok(None);
            }
            let assigned_source = assignment
                .get("right")
                .ok_or_else(|| "scalar-local range assignment has no value".to_string())?;
            if local_names
                .iter()
                .any(|name| json_references_local_symbol(assigned_source, name))
            {
                return Err(
                    "scalar-local matched range assignments cannot read local variables".into(),
                );
            }
            assigned_values[local_index] = Some(self.lower_expression(assigned_source)?);
            assigned_targets[local_index] = true;
        }
        if assigned_targets.iter().any(|assigned| !assigned) {
            return Ok(None);
        }

        let middle_sequence = range_element.get("sequence").ok_or_else(|| {
            "scalar-local nonterminal matched range has no middle sequence".to_string()
        })?;
        let (middle_source, update_rhs_by_local) =
            if string_field(middle_sequence, "kind").as_deref() == Some("SequenceWithMatch") {
                let items = middle_sequence
                    .get("matchItems")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        "scalar-local repeated nonterminal range has no match items".to_string()
                    })?;
                if items.len() != local_vars.len() {
                    return Ok(None);
                }
                let mut ordered_rhs = vec![None; local_vars.len()];
                for assignment in items {
                    if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                        return Ok(None);
                    }
                    let assigned = assignment.get("left").ok_or_else(|| {
                        "scalar-local repeated nonterminal assignment has no target".to_string()
                    })?;
                    let local_index = local_symbol_index(expression_symbol(assigned).as_deref())
                        .ok_or_else(|| {
                            "scalar-local repeated nonterminal assignments must target locals"
                                .to_string()
                        })?;
                    if ordered_rhs[local_index].is_some() {
                        return Ok(None);
                    }
                    ordered_rhs[local_index] = Some(assignment.get("right").ok_or_else(|| {
                        "scalar-local repeated nonterminal assignment has no value".to_string()
                    })?);
                }
                let ordered_rhs = ordered_rhs
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| {
                        "scalar-local repeated nonterminal assignments must cover every local"
                            .to_string()
                    })?;
                let expression = middle_sequence.get("expr").ok_or_else(|| {
                    "scalar-local repeated nonterminal range has no trigger".to_string()
                })?;
                let source = Self::simple_property_operand(expression, "left")?;
                (source, Some(ordered_rhs))
            } else {
                let source = Self::simple_property_operand(middle_sequence, "left")?;
                (source, None)
            };
        let middle_source = unwrap_conversion(middle_source);
        let mut chained_rhs_stages: Vec<Vec<&serde_json::Value>> = Vec::new();
        let suffix_source_count = elements.len().saturating_sub(2);
        let suffix_sources = elements
            .iter()
            .skip(2)
            .enumerate()
            .map(|(suffix_index, element)| {
                let sequence = element.get("sequence").ok_or_else(|| {
                    "scalar-local nonterminal matched range suffix has no sequence".to_string()
                })?;
                if string_field(sequence, "kind").as_deref() == Some("SequenceWithMatch") {
                    if suffix_index + 2 >= elements.len() || chained_rhs_stages.len() != suffix_index {
                        return Err(
                            "scalar-local chained nonterminal updates must occupy contiguous nonfinal suffix elements"
                                .into(),
                        );
                    }
                    let items = sequence
                        .get("matchItems")
                        .and_then(Value::as_array)
                        .ok_or_else(|| {
                            "scalar-local chained nonterminal update has no match items".to_string()
                        })?;
                    if items.len() != local_vars.len() {
                        return Err(
                            "scalar-local chained nonterminal updates must cover every local".into(),
                        );
                    }
                    let mut ordered_rhs = vec![None; local_vars.len()];
                    for assignment in items {
                        if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                            return Err(
                                "scalar-local chained nonterminal update must be an assignment".into(),
                            );
                        }
                        let assigned = assignment.get("left").ok_or_else(|| {
                            "scalar-local chained nonterminal update has no target".to_string()
                        })?;
                        let local_index =
                            local_symbol_index(expression_symbol(assigned).as_deref()).ok_or_else(
                                || {
                                    "scalar-local chained nonterminal updates must target locals"
                                        .to_string()
                                },
                            )?;
                        if ordered_rhs[local_index].is_some() {
                            return Err(
                                "scalar-local chained nonterminal updates must be distinct".into(),
                            );
                        }
                        ordered_rhs[local_index] =
                            Some(assignment.get("right").ok_or_else(|| {
                                "scalar-local chained nonterminal update has no value".to_string()
                            })?);
                    }
                    let ordered_rhs = ordered_rhs
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(|| {
                            "scalar-local chained nonterminal updates must cover every local"
                                .to_string()
                        })?;
                    chained_rhs_stages.push(ordered_rhs);
                    let expression = sequence.get("expr").ok_or_else(|| {
                        "scalar-local chained nonterminal update has no trigger".to_string()
                    })?;
                    let source = Self::simple_property_operand(expression, "right")?;
                    return Ok(unwrap_conversion(source));
                }
                let source = Self::simple_property_operand(sequence, "right")?;
                Ok(unwrap_conversion(source))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if !chained_rhs_stages.is_empty() {
            if chained_rhs_stages.len() + 1 != suffix_source_count {
                return Err(
                    "scalar-local chained nonterminal updates require one terminal suffix".into(),
                );
            }
            if chained_rhs_stages.len() > 64 {
                return Err(
                    "scalar-local nonterminal matched ranges support at most 64 chained updates"
                        .into(),
                );
            }
            if update_rhs_by_local.is_none() {
                return Ok(None);
            }
        }
        let maximum_age = range_end + suffix_total_delay;
        let mut sample_nets = vec![Vec::new(); local_vars.len()];
        let mut samples = Vec::with_capacity(local_vars.len());
        for (local_index, assigned_value) in assigned_values
            .iter()
            .enumerate()
            .map(|(index, value)| (index, value.as_ref().expect("assigned range local value")))
        {
            let mut nets = Vec::with_capacity(maximum_age);
            for age in 0..maximum_age {
                nets.push(self.declare_property_local_sample_net(
                    member,
                    property_index,
                    0,
                    (local_index, local_index, age),
                    1,
                    false,
                ));
            }
            sample_nets[local_index] = nets.clone();
            samples.push(ConcurrentLocalSample {
                width: 1,
                signed: false,
                trigger: trigger.clone(),
                value: assigned_value.clone(),
                nets,
                histories: Vec::new(),
            });
        }

        let mut middle_boundaries = Vec::with_capacity(alternative_count);
        let mut suffix_boundaries =
            vec![Vec::with_capacity(suffix_sources.len().saturating_sub(1)); alternative_count];
        let mut final_boundaries = Vec::with_capacity(alternative_count);
        let mut cumulative_boundaries = Vec::with_capacity(suffix_delays.len());
        let mut cumulative = 0_usize;
        for delay in &suffix_delays {
            cumulative += delay;
            cumulative_boundaries.push(cumulative);
        }
        for (alternative, alternative_suffix_boundaries) in suffix_boundaries.iter_mut().enumerate()
        {
            let range_delay = range_start + alternative;
            let Some(middle) = self.lower_local_expression_at_age(
                middle_source,
                &local_names,
                &sample_nets,
                property_index,
                range_delay,
            )?
            else {
                return Ok(None);
            };
            middle_boundaries.push(middle);

            let alternative_sample_nets = if let Some(update_rhs_by_local) =
                update_rhs_by_local.as_ref()
            {
                let mut nets_by_local = Vec::with_capacity(local_vars.len());
                for (local_index, update_rhs) in update_rhs_by_local.iter().enumerate() {
                    let update_value = self
                        .lower_local_expression_at_age(
                            update_rhs,
                            &local_names,
                            &sample_nets,
                            property_index,
                            range_delay,
                        )?
                        .ok_or_else(|| {
                            "scalar-local repeated nonterminal assignment has no value".to_string()
                        })?;
                    let mut history_nets = Vec::with_capacity(range_delay);
                    for stage in 0..range_delay {
                        history_nets.push(self.declare_property_local_history_net(
                            member,
                            property_index,
                            1,
                            local_index,
                            1,
                            (alternative, stage),
                        ));
                    }
                    let mut nets = Vec::with_capacity(suffix_total_delay);
                    for age in 0..suffix_total_delay {
                        nets.push(self.declare_property_local_sample_net(
                            member,
                            property_index,
                            1,
                            (local_index, 1, age),
                            1,
                            false,
                        ));
                    }
                    samples.push(ConcurrentLocalSample {
                        width: 1,
                        signed: false,
                        trigger: middle_boundaries[alternative].clone(),
                        value: update_value,
                        nets: nets.clone(),
                        histories: vec![ConcurrentLocalHistory {
                            trigger: eda_core::ir::Expr::Bin {
                                op: eda_core::ir::BinOp::LogAnd,
                                a: Box::new(antecedent_reduction(property_antecedent)),
                                b: Box::new(antecedent_reduction(&trigger)),
                            },
                            nets: history_nets,
                        }],
                    });
                    nets_by_local.push(nets);
                }
                nets_by_local
            } else {
                sample_nets.clone()
            };
            let mut final_sample_nets = alternative_sample_nets.clone();
            if chained_rhs_stages.is_empty() {
                for suffix_index in 0..suffix_sources.len().saturating_sub(1) {
                    let age = if update_rhs_by_local.is_some() {
                        cumulative_boundaries[suffix_index]
                    } else {
                        range_delay + cumulative_boundaries[suffix_index]
                    };
                    let Some(expression) = self.lower_local_expression_at_age(
                        suffix_sources[suffix_index],
                        &local_names,
                        &alternative_sample_nets,
                        property_index,
                        age,
                    )?
                    else {
                        return Ok(None);
                    };
                    alternative_suffix_boundaries.push(expression);
                }
            } else {
                for (stage_index, chained_rhs_by_local) in chained_rhs_stages.iter().enumerate() {
                    let delay_from_previous = suffix_delays[stage_index];
                    let Some(stage_trigger) = self.lower_local_expression_at_age(
                        suffix_sources[stage_index],
                        &local_names,
                        &final_sample_nets,
                        property_index,
                        delay_from_previous,
                    )?
                    else {
                        return Ok(None);
                    };
                    alternative_suffix_boundaries.push(stage_trigger);
                    let remaining_delay = suffix_delays[stage_index + 1..].iter().try_fold(
                        0_usize,
                        |total, delay| {
                            total.checked_add(*delay).ok_or_else(|| {
                                "scalar-local chained nonterminal sample delay total overflows"
                                    .to_string()
                            })
                        },
                    )?;
                    let previous_trigger = if stage_index == 0 {
                        middle_boundaries[alternative].clone()
                    } else {
                        alternative_suffix_boundaries[stage_index - 1].clone()
                    };
                    let mut stage_nets_by_local = Vec::with_capacity(local_vars.len());
                    for (local_index, chained_rhs) in chained_rhs_by_local.iter().enumerate() {
                        let update_value = self
                            .lower_local_expression_at_age(
                                chained_rhs,
                                &local_names,
                                &final_sample_nets,
                                property_index,
                                delay_from_previous,
                            )?
                            .ok_or_else(|| {
                                "scalar-local chained nonterminal update has no value".to_string()
                            })?;
                        let mut update_history_nets = Vec::with_capacity(delay_from_previous);
                        for stage in 0..delay_from_previous {
                            update_history_nets.push(self.declare_property_local_history_net(
                                member,
                                property_index,
                                stage_index + 2,
                                local_index,
                                stage_index + 2,
                                (1, stage),
                            ));
                        }
                        let mut nets = Vec::with_capacity(remaining_delay);
                        for age in 0..remaining_delay {
                            nets.push(self.declare_property_local_sample_net(
                                member,
                                property_index,
                                stage_index + 2,
                                (local_index, stage_index + 2, age),
                                1,
                                false,
                            ));
                        }
                        samples.push(ConcurrentLocalSample {
                            width: 1,
                            signed: false,
                            trigger: alternative_suffix_boundaries[stage_index].clone(),
                            value: update_value,
                            nets: nets.clone(),
                            histories: vec![ConcurrentLocalHistory {
                                trigger: previous_trigger.clone(),
                                nets: update_history_nets,
                            }],
                        });
                        stage_nets_by_local.push(nets);
                    }
                    final_sample_nets = stage_nets_by_local;
                }
            }
            let final_age = if update_rhs_by_local.is_some() {
                if chained_rhs_stages.is_empty() {
                    suffix_total_delay
                } else {
                    suffix_delays[chained_rhs_stages.len()]
                }
            } else {
                range_delay + suffix_total_delay
            };
            let Some(final_expression) = self.lower_local_expression_at_age(
                suffix_sources
                    .last()
                    .expect("nonterminal suffix is nonempty"),
                &local_names,
                &final_sample_nets,
                property_index,
                final_age,
            )?
            else {
                return Ok(None);
            };
            final_boundaries.push(final_expression);
        }

        Ok(Some(ScalarLocalNonterminalRangeLowering {
            prefix: trigger.clone(),
            prefix_miss: eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::LogAnd,
                a: Box::new(antecedent_reduction(property_antecedent)),
                b: Box::new(eda_core::ir::Expr::Un {
                    op: eda_core::ir::UnOp::Not,
                    a: Box::new(antecedent_reduction(&trigger)),
                }),
            },
            range_start,
            range_end,
            suffix_delays,
            middle_boundaries,
            suffix_boundaries,
            final_boundaries,
            samples,
        }))
    }

    /// Lower the local-bearing terminal-range subset for a matched consequent:
    /// `(trigger, locals = values...) ##[start:end] local-expression`.
    fn lower_scalar_local_range_matched_consequent(
        &mut self,
        instance: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        property_antecedent: &eda_core::ir::Expr,
    ) -> Result<Option<ScalarLocalRangeLowering>, String> {
        let body = instance
            .get("body")
            .ok_or_else(|| "scalar-local matched range has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("SequenceConcat") {
            return Ok(None);
        }
        let elements = body
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local matched range has no elements".to_string())?;
        if elements.len() == 3 {
            return self.lower_repeated_scalar_local_range_matched_consequent(
                instance,
                member,
                property_index,
                property_antecedent,
            );
        }
        if elements.len() != 2 {
            return Ok(None);
        }
        let local_vars = instance
            .get("localVars")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local matched range has no locals".to_string())?;
        if local_vars.is_empty() || local_vars.len() > 64 {
            return Ok(None);
        }

        let mut local_names = Vec::with_capacity(local_vars.len());
        for local in local_vars {
            let name = string_field(local, "name")
                .ok_or_else(|| "assertion-instance local variable has no name".to_string())?;
            let local_type = local
                .get("type")
                .ok_or_else(|| "assertion-instance local variable has no type".to_string())?;
            let local_type_name = local_type
                .as_str()
                .or_else(|| local_type.get("name").and_then(Value::as_str));
            if !matches!(local_type_name, Some("logic") | Some("bit")) {
                return Ok(None);
            }
            local_names.push(name);
        }
        let local_symbol_index = |symbol: Option<&str>| {
            symbol.and_then(|symbol| {
                local_names
                    .iter()
                    .position(|name| symbol == *name || symbol.ends_with(&format!(" {name}")))
            })
        };

        let first = &elements[0];
        let first_sequence = first
            .get("sequence")
            .ok_or_else(|| "scalar-local matched range has no first sequence".to_string())?;
        if string_field(first_sequence, "kind").as_deref() != Some("SequenceWithMatch") {
            return Ok(None);
        }
        if first.get("min").and_then(Value::as_u64) != Some(0)
            || first.get("max").and_then(Value::as_u64) != Some(0)
        {
            return Ok(None);
        }
        let match_items = first_sequence
            .get("matchItems")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local matched range has no match items".to_string())?;
        if match_items.len() != local_vars.len() {
            return Ok(None);
        }

        let trigger_source = first_sequence
            .get("expr")
            .ok_or_else(|| "scalar-local matched range has no trigger".to_string())?;
        let trigger =
            self.lower_expression(Self::simple_property_operand(trigger_source, "left")?)?;
        let mut assigned_values = vec![None; local_vars.len()];
        let mut assigned_targets = vec![false; local_vars.len()];
        for assignment in match_items {
            if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                return Ok(None);
            }
            let assigned = assignment
                .get("left")
                .ok_or_else(|| "scalar-local range assignment has no target".to_string())?;
            let local_index = local_symbol_index(expression_symbol(assigned).as_deref())
                .ok_or_else(|| {
                    "scalar-local matched range assignments must target local variables".to_string()
                })?;
            if assigned_targets[local_index] {
                return Ok(None);
            }
            let assigned_source = assignment
                .get("right")
                .ok_or_else(|| "scalar-local range assignment has no value".to_string())?;
            if local_names
                .iter()
                .any(|name| json_references_local_symbol(assigned_source, name))
            {
                return Err(
                    "scalar-local matched range assignments cannot read local variables".into(),
                );
            }
            assigned_values[local_index] = Some(self.lower_expression(assigned_source)?);
            assigned_targets[local_index] = true;
        }
        if assigned_targets.iter().any(|assigned| !assigned) {
            return Ok(None);
        }

        let second = &elements[1];
        let second_sequence = second
            .get("sequence")
            .ok_or_else(|| "scalar-local matched range has no final sequence".to_string())?;
        let final_source = Self::simple_property_operand(second_sequence, "right")?;
        let final_source = unwrap_conversion(final_source);
        let start = second
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "scalar-local matched range has no minimum".to_string())?;
        let end = second
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "scalar-local matched range has no maximum".to_string())?;
        if start == 0 || start > end || end > 256 {
            return Ok(None);
        }
        let start = start as usize;
        let end = end as usize;

        let mut sample_nets = vec![Vec::new(); local_vars.len()];
        let mut samples = Vec::with_capacity(local_vars.len());
        for (local_index, assigned_value) in assigned_values
            .iter()
            .enumerate()
            .map(|(index, value)| (index, value.as_ref().expect("assigned range local value")))
        {
            let mut nets = Vec::with_capacity(end);
            for stage in 0..end {
                nets.push(self.declare_property_local_sample_net(
                    member,
                    property_index,
                    0,
                    (local_index, local_index, stage),
                    1,
                    false,
                ));
            }
            sample_nets[local_index] = nets.clone();
            samples.push(ConcurrentLocalSample {
                width: 1,
                signed: false,
                trigger: trigger.clone(),
                value: assigned_value.clone(),
                nets,
                histories: Vec::new(),
            });
        }

        let mut boundaries = Vec::with_capacity(end - start + 1);
        for delay in start..=end {
            let (rewritten, references) = rewrite_sequence_local_references(
                final_source,
                &local_names,
                property_index,
                delay,
            );
            if references.is_empty() {
                return Ok(None);
            }
            let mut temporary_symbols = Vec::new();
            for local_index in references
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
            {
                let net = sample_nets[local_index]
                    .get(delay - 1)
                    .copied()
                    .ok_or_else(|| "scalar-local matched range has no sample state".to_string())?;
                let key = format!("__eda_slseq_{property_index}_{local_index}_{delay}");
                self.symbols.insert(
                    key.clone(),
                    AstLoweredSymbol {
                        base: net,
                        width: 1,
                        signed: false,
                        bounds: None,
                        dimensions: Vec::new(),
                        memory: None,
                        struct_fields: std::collections::BTreeMap::new(),
                    },
                );
                temporary_symbols.push(key);
            }
            let lowered = self.lower_expression(&rewritten)?;
            for key in temporary_symbols {
                self.symbols.remove(&key);
            }
            boundaries.push(antecedent_reduction(&lowered));
        }

        Ok(Some(ScalarLocalRangeLowering {
            prefix: trigger.clone(),
            prefix_miss: eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::LogAnd,
                a: Box::new(antecedent_reduction(property_antecedent)),
                b: Box::new(eda_core::ir::Expr::Un {
                    op: eda_core::ir::UnOp::Not,
                    a: Box::new(antecedent_reduction(&trigger)),
                }),
            },
            start,
            end,
            boundaries,
            prefix_includes_antecedent: false,
            samples,
        }))
    }

    fn lower_matched_sequence_for_pair(
        &mut self,
        matched: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        sequence_group: usize,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        let instance = Self::matched_call_instance(matched)?;
        if instance
            .get("localVars")
            .and_then(Value::as_array)
            .is_some_and(|locals| !locals.is_empty())
        {
            self.lower_scalar_local_sequence_in_group(
                instance,
                member,
                property_index,
                sequence_group,
            )
        } else {
            let sequence = Self::inline_nonlocal_assertion_instance(instance)?;
            self.lower_fixed_sequence_antecedent(sequence)
        }
    }

    fn lower_matched_sequence_pair(
        &mut self,
        left: &serde_json::Value,
        right: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        combine_as_or: bool,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        let left_sequence =
            self.lower_matched_sequence_for_pair(left, member, property_index, 1)?;
        let right_sequence =
            self.lower_matched_sequence_for_pair(right, member, property_index, 2)?;
        if left_sequence.range.is_some() || right_sequence.range.is_some() {
            return Err("matched endpoint pairs do not support delay ranges".into());
        }
        let mut left_cumulative = Vec::with_capacity(left_sequence.delays.len());
        let mut total_delay = 0_usize;
        for delay in &left_sequence.delays {
            total_delay = total_delay
                .checked_add(*delay)
                .ok_or_else(|| "matched endpoint pair delay total overflows".to_string())?;
            left_cumulative.push(total_delay);
        }
        let left_total_delay = total_delay;
        let mut right_cumulative = Vec::with_capacity(right_sequence.delays.len());
        let mut total_delay = 0_usize;
        for delay in &right_sequence.delays {
            total_delay = total_delay
                .checked_add(*delay)
                .ok_or_else(|| "matched endpoint pair delay total overflows".to_string())?;
            right_cumulative.push(total_delay);
        }
        if left_total_delay != total_delay {
            return Err(
                "matched endpoint pairs require identical fixed-delay schedules and equal total delay".into(),
            );
        }

        let mut boundaries = std::collections::BTreeSet::from([0_usize]);
        boundaries.extend(left_cumulative.iter().copied());
        boundaries.extend(right_cumulative.iter().copied());
        let mut expressions = Vec::with_capacity(boundaries.len());
        let mut delays = Vec::with_capacity(boundaries.len());
        let mut previous_boundary = None;
        for boundary in boundaries {
            let left_expression = left_cumulative
                .iter()
                .position(|cumulative| *cumulative == boundary)
                .and_then(|index| left_sequence.expressions.get(index))
                .cloned()
                .unwrap_or_else(|| eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)));
            let right_expression = right_cumulative
                .iter()
                .position(|cumulative| *cumulative == boundary)
                .and_then(|index| right_sequence.expressions.get(index))
                .cloned()
                .unwrap_or_else(|| eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)));
            expressions.push(eda_core::ir::Expr::Bin {
                op: if combine_as_or {
                    eda_core::ir::BinOp::LogOr
                } else {
                    eda_core::ir::BinOp::LogAnd
                },
                a: Box::new(antecedent_reduction(&left_expression)),
                b: Box::new(antecedent_reduction(&right_expression)),
            });
            if let Some(previous) = previous_boundary {
                delays.push(boundary - previous);
            } else {
                delays.push(0);
            }
            previous_boundary = Some(boundary);
        }
        Ok(ConcurrentAntecedentSequence {
            expressions,
            delays,
            range: None,
            range_element: 0,
            local_samples: left_sequence
                .local_samples
                .into_iter()
                .chain(right_sequence.local_samples)
                .collect(),
        })
    }

    fn lower_repetition_operand(
        &self,
        operand: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, ConcurrentDelay), String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "right sequence operand has no repetition".to_string())?;
        let repetition_kind = string_field(repetition, "kind").unwrap_or_default();
        if !matches!(
            repetition_kind.as_str(),
            "Consecutive" | "GoTo" | "Nonconsecutive"
        ) {
            return Err(
                "direct concurrent assertions support only fixed-count consecutive, goto, or nonconsecutive repetition"
                    .into(),
            );
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "consecutive repetition has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "consecutive repetition has no maximum".to_string())?;
        if min > max {
            return Err("concurrent-property repetition range start exceeds end".into());
        }
        if !(1..=256).contains(&min) || !(1..=256).contains(&max) {
            return Err("concurrent-property repetition range must stay within 1..=256".into());
        }
        let expression = operand
            .get("expr")
            .ok_or_else(|| "consecutive repetition operand has no expression".to_string())?;
        let delay = match repetition_kind.as_str() {
            "Consecutive" => ConcurrentDelay::Consecutive(min as usize),
            "GoTo" => ConcurrentDelay::Goto(min as usize),
            _ => ConcurrentDelay::Nonconsecutive(min as usize),
        };
        Ok((self.lower_expression(expression)?, delay))
    }

    fn lower_fixed_sequence_consequent(
        &self,
        consequent: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, ConcurrentDelay), String> {
        if string_field(consequent, "kind").as_deref() != Some("SequenceConcat") {
            return Err(
                "direct concurrent assertions support only Simple or fixed-delay sequence consequents"
                    .into(),
            );
        }
        let elements = consequent
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "sequence concatenation has no elements".to_string())?;
        if elements.is_empty() {
            return Err("sequence concatenation has no sequence elements".into());
        }
        if let Some((suffix, delay)) = self.lower_consecutive_range_sequence_consequent(elements)? {
            return Ok((suffix, delay));
        }
        if elements.len() > 64 {
            return Err(
                "direct concurrent sequence composition supports at most 64 elements".into(),
            );
        }
        let mut parsed_elements = Vec::with_capacity(elements.len());
        let mut total_delay = 0_usize;
        for element in elements {
            let sequence = element
                .get("sequence")
                .ok_or_else(|| "sequence element has no sequence".to_string())?;
            let expression =
                self.lower_expression(Self::simple_property_operand(sequence, "right")?)?;
            let min = element
                .get("min")
                .and_then(Value::as_u64)
                .ok_or_else(|| "sequence element has no minimum".to_string())?;
            let max = element
                .get("max")
                .and_then(Value::as_u64)
                .ok_or_else(|| "sequence element has no maximum".to_string())?;
            if min != max && elements.len() > 1 {
                return Err(
                    "direct concurrent sequence composition supports fixed delays only".into(),
                );
            }
            if min > 256 {
                return Err("concurrent sequence element delay must stay within 0..=256".into());
            }
            total_delay = total_delay
                .checked_add(min as usize)
                .ok_or_else(|| "concurrent sequence delay total overflows".to_string())?;
            parsed_elements.push((expression, min as usize));
        }
        if total_delay > 256 {
            return Err("concurrent sequence total delay must stay within 0..=256".into());
        }
        if parsed_elements.len() > 1 {
            let (expressions, delays): (Vec<eda_core::ir::Expr>, Vec<usize>) =
                parsed_elements.into_iter().unzip();
            let final_expression = expressions
                .last()
                .cloned()
                .ok_or_else(|| "concurrent sequence has no expressions".to_string())?;
            return Ok((
                final_expression,
                ConcurrentDelay::Sequence(expressions, delays),
            ));
        }
        let element = &elements[0];
        let sequence = element
            .get("sequence")
            .ok_or_else(|| "fixed-delay sequence element has no sequence".to_string())?;
        let expression =
            self.lower_expression(Self::simple_property_operand(sequence, "right")?)?;
        let min = element
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "fixed-delay sequence element has no minimum".to_string())?;
        let max = element
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "fixed-delay sequence element has no maximum".to_string())?;
        if min > max {
            return Err("concurrent-property delay range start exceeds end".into());
        }
        if !(0..=256).contains(&min) || !(0..=256).contains(&max) {
            return Err("concurrent-property delay range must stay within 0..=256".into());
        }
        let delay = if min == max {
            if min == 0 {
                ConcurrentDelay::SameCycle
            } else {
                ConcurrentDelay::Fixed(min as usize)
            }
        } else {
            ConcurrentDelay::Range(min as usize, max as usize)
        };
        Ok((expression, delay))
    }

    fn lower_consecutive_range_sequence_consequent(
        &self,
        elements: &[serde_json::Value],
    ) -> Result<Option<(eda_core::ir::Expr, ConcurrentDelay)>, String> {
        if elements.len() != 3 {
            return Ok(None);
        }
        let first_delay = elements[0]
            .get("min")
            .and_then(Value::as_u64)
            .zip(elements[0].get("max").and_then(Value::as_u64));
        let middle_delay = elements[1]
            .get("min")
            .and_then(Value::as_u64)
            .zip(elements[1].get("max").and_then(Value::as_u64));
        let last_delay = elements[2]
            .get("min")
            .and_then(Value::as_u64)
            .zip(elements[2].get("max").and_then(Value::as_u64));
        if first_delay != Some((0, 0)) || middle_delay != Some((1, 1)) || last_delay != Some((1, 1))
        {
            return Ok(None);
        }
        let first_sequence = elements[0]
            .get("sequence")
            .ok_or_else(|| "sequence element has no sequence".to_string())?;
        let repeated_sequence = elements[1]
            .get("sequence")
            .ok_or_else(|| "sequence element has no sequence".to_string())?;
        let last_sequence = elements[2]
            .get("sequence")
            .ok_or_else(|| "sequence element has no sequence".to_string())?;
        let Some(repetition) = repeated_sequence.get("repetition") else {
            return Ok(None);
        };
        let repetition_kind = string_field(repetition, "kind");
        if !matches!(
            repetition_kind.as_deref(),
            Some("Consecutive") | Some("GoTo") | Some("Nonconsecutive")
        ) {
            return Ok(None);
        }
        if first_sequence.get("repetition").is_some() || last_sequence.get("repetition").is_some() {
            return Ok(None);
        }
        let start = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "consecutive repetition has no minimum".to_string())?;
        let end = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "consecutive repetition has no maximum".to_string())?;
        if start == 0 || start > end || end > 256 {
            return Ok(None);
        }
        let prefix = self.lower_expression(Self::simple_property_operand(
            first_sequence,
            "sequence prefix",
        )?)?;
        let repeated = self
            .lower_expression(
                repeated_sequence
                    .get("expr")
                    .ok_or_else(|| "repeated sequence has no expression".to_string())?,
            )
            .map_err(|error| format!("repeated sequence expression: {error}"))?;
        let suffix = self.lower_expression(Self::simple_property_operand(
            last_sequence,
            "sequence suffix",
        )?)?;
        let consequent = suffix.clone();
        let delay = if repetition_kind.as_deref() == Some("GoTo") {
            ConcurrentDelay::GotoRangeSequence {
                prefix: Box::new(prefix),
                repeated: Box::new(repeated),
                suffix: Box::new(suffix),
                start: start as usize,
                end: end as usize,
            }
        } else if repetition_kind.as_deref() == Some("Nonconsecutive") {
            ConcurrentDelay::NonconsecutiveRangeSequence {
                prefix: Box::new(prefix),
                repeated: Box::new(repeated),
                suffix: Box::new(suffix),
                start: start as usize,
                end: end as usize,
            }
        } else {
            ConcurrentDelay::ConsecutiveRangeSequence {
                prefix: Box::new(prefix),
                repeated: Box::new(repeated),
                suffix: Box::new(suffix),
                start: start as usize,
                end: end as usize,
            }
        };
        Ok(Some((consequent, delay)))
    }

    fn lower_fixed_sequence_antecedent(
        &self,
        antecedent: &serde_json::Value,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        if string_field(antecedent, "kind").as_deref() != Some("SequenceConcat") {
            return Err(
                "direct concurrent assertions support only fixed-delay sequence antecedents".into(),
            );
        }
        let elements = antecedent
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "antecedent sequence concatenation has no elements".to_string())?;
        if elements.is_empty() {
            return Err("antecedent sequence concatenation is empty".into());
        }
        if elements.len() > 64 {
            return Err(
                "direct concurrent antecedent sequence supports at most 64 elements".into(),
            );
        }
        let first_delay = elements[0]
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "antecedent sequence element has no minimum".to_string())?;
        if first_delay != 0 {
            return Err(
                "direct concurrent antecedent sequences must begin at the current cycle".into(),
            );
        }
        let mut parsed = Vec::with_capacity(elements.len());
        let mut range = None;
        let mut range_element = 0;
        let mut range_count = 0_usize;
        let mut total_delay = 0_usize;
        for (element_index, element) in elements.iter().enumerate() {
            let sequence = element
                .get("sequence")
                .ok_or_else(|| "antecedent sequence element has no sequence".to_string())?;
            let expression =
                self.lower_expression(Self::simple_property_operand(sequence, "left")?)?;
            let min = element
                .get("min")
                .and_then(Value::as_u64)
                .ok_or_else(|| "antecedent sequence element has no minimum".to_string())?;
            let max = element
                .get("max")
                .and_then(Value::as_u64)
                .ok_or_else(|| "antecedent sequence element has no maximum".to_string())?;
            if min > max || max > 256 {
                return Err(
                    "concurrent antecedent sequence delay range must stay within 0..=256".into(),
                );
            }
            let element_delay = if min == max {
                min as usize
            } else {
                if element_index == 0 || range_count != 0 {
                    return Err(
                        "direct concurrent antecedent ranges support one non-initial range".into(),
                    );
                }
                range = Some((min as usize, max as usize));
                range_element = element_index;
                range_count += 1;
                max as usize
            };
            total_delay = total_delay.checked_add(element_delay).ok_or_else(|| {
                "concurrent antecedent sequence delay total overflows".to_string()
            })?;
            parsed.push((expression, element_delay));
        }
        if total_delay > 256 {
            return Err(
                "concurrent antecedent sequence total delay must stay within 0..=256".into(),
            );
        }
        if total_delay == 0 && parsed.len() != 1 {
            return Err("multi-element antecedent sequences require a nonzero fixed delay".into());
        }
        if range.is_some() {
            let range_index = range_element;
            if range_index + 1 < parsed.len()
                && parsed
                    .iter()
                    .skip(range_index + 1)
                    .any(|(_, delay)| *delay == 0)
            {
                return Err("non-terminal antecedent ranges require positive suffix delays".into());
            }
        }
        let (expressions, delays): (Vec<_>, Vec<_>) = parsed.into_iter().unzip();
        Ok(ConcurrentAntecedentSequence {
            expressions,
            delays,
            range,
            range_element,
            local_samples: Vec::new(),
        })
    }

    /// Lower scalar sequence-local variables with one assignment each. Locals
    /// may be assigned in different fixed-delay match elements; each receives
    /// prefix history and an assignment-relative sampled-value pipeline.
    fn lower_scalar_local_sequence(
        &mut self,
        instance: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        self.lower_scalar_local_sequence_in_group(instance, member, property_index, 0)
    }

    fn lower_scalar_local_sequence_in_group(
        &mut self,
        instance: &serde_json::Value,
        member: &serde_json::Value,
        property_index: usize,
        sequence_group: usize,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        if string_field(instance, "kind").as_deref() != Some("AssertionInstance") {
            return Err("scalar-local sequence operand is not an assertion instance".into());
        }
        let local_vars = instance
            .get("localVars")
            .and_then(Value::as_array)
            .ok_or_else(|| "assertion instance has no local variables".to_string())?;
        if local_vars.is_empty() || local_vars.len() > 64 {
            return Err(
                "scalar-local sequences support one to sixty-four assertion-instance local variables"
                    .into(),
            );
        }
        let mut local_names = Vec::with_capacity(local_vars.len());
        let mut local_shapes = Vec::with_capacity(local_vars.len());
        for local in local_vars {
            let name = string_field(local, "name")
                .ok_or_else(|| "assertion-instance local variable has no name".to_string())?;
            let local_type = local
                .get("type")
                .ok_or_else(|| "assertion-instance local variable has no type".to_string())?;
            let local_type_name = local_type
                .as_str()
                .or_else(|| local_type.get("name").and_then(Value::as_str));
            let Some(local_type_name) = local_type_name else {
                return Err("assertion-local variables require a printable integral type".into());
            };
            let base_type = local_type_name.split('[').next().unwrap_or_default();
            let base_tokens = base_type.split_whitespace().collect::<Vec<_>>();
            let integral_scalar = base_tokens
                .iter()
                .any(|token| matches!(*token, "logic" | "reg" | "bit"));
            let integral_predefined = base_tokens.iter().any(|token| {
                matches!(
                    *token,
                    "byte" | "shortint" | "int" | "integer" | "longint" | "time"
                )
            });
            if !integral_scalar && !integral_predefined {
                return Err(
                    "assertion-local variables support only integral logic/bit/reg or predefined integer storage"
                        .into(),
                );
            }
            let shape = simple_type_shape(local_type_name).ok_or_else(|| {
                "assertion-local variables require a fixed-width integral type".to_string()
            })?;
            let signed = simple_type_signed(local_type_name).ok_or_else(|| {
                "assertion-local variables require a fixed-width integral type".to_string()
            })?;
            if shape.width == 0 || shape.width > 4096 {
                return Err(
                    "assertion-local packed integral width must stay within 1..=4096 bits".into(),
                );
            }
            local_names.push(name);
            local_shapes.push((shape.width, signed));
        }
        let local_symbol_index = |symbol: Option<&str>| {
            symbol.and_then(|symbol| {
                local_names
                    .iter()
                    .position(|name| symbol == *name || symbol.ends_with(&format!(" {name}")))
            })
        };

        let body = instance
            .get("body")
            .ok_or_else(|| "scalar-local sequence instance has no body".to_string())?;
        if string_field(body, "kind").as_deref() != Some("SequenceConcat") {
            return Err(
                "scalar-local sequences support only fixed-delay sequence concatenation".into(),
            );
        }
        let elements = body
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "scalar-local sequence concatenation has no elements".to_string())?;
        if elements.len() < 2 || elements.len() > 64 {
            return Err(
                "scalar-local sequences require two to sixty-four fixed-delay elements".into(),
            );
        }

        let mut assignment_elements = Vec::new();
        for (element_index, element) in elements.iter().enumerate() {
            let sequence = element
                .get("sequence")
                .ok_or_else(|| "scalar-local sequence element has no sequence".to_string())?;
            if string_field(sequence, "kind").as_deref() != Some("SequenceWithMatch") {
                continue;
            }
            let match_items = sequence
                .get("matchItems")
                .and_then(Value::as_array)
                .ok_or_else(|| "scalar-local sequence has no match items".to_string())?;
            if !match_items.is_empty() {
                assignment_elements.push(element_index);
            }
        }
        if assignment_elements.is_empty() {
            return Err("scalar-local sequence has no assignment element".into());
        }

        let mut raw_elements = Vec::with_capacity(elements.len());
        let mut cumulative = Vec::with_capacity(elements.len());
        let mut total_delay = 0_usize;
        for (element_index, element) in elements.iter().enumerate() {
            let sequence = element
                .get("sequence")
                .ok_or_else(|| "scalar-local sequence element has no sequence".to_string())?;
            let expression =
                if string_field(sequence, "kind").as_deref() == Some("SequenceWithMatch") {
                    sequence
                        .get("expr")
                        .ok_or_else(|| "scalar-local match element has no expression".to_string())?
                } else {
                    sequence
                };
            let expression = Self::simple_property_operand(expression, "left")?;
            let min = element
                .get("min")
                .and_then(Value::as_u64)
                .ok_or_else(|| "scalar-local sequence element has no minimum".to_string())?;
            let max = element
                .get("max")
                .and_then(Value::as_u64)
                .ok_or_else(|| "scalar-local sequence element has no maximum".to_string())?;
            if element_index == 0 {
                if min != 0 || max != 0 {
                    return Err("scalar-local sequences must begin in the current cycle".into());
                }
            } else if min != max || min == 0 || max > 256 {
                return Err(
                    "scalar-local sequence suffixes require fixed positive delays through 256"
                        .into(),
                );
            }
            let delay = min as usize;
            total_delay = total_delay
                .checked_add(delay)
                .ok_or_else(|| "scalar-local sequence delay total overflows".to_string())?;
            cumulative.push(total_delay);
            raw_elements.push((expression, delay));
        }
        if total_delay > 256 {
            return Err("scalar-local sequence total delay must stay within 0..=256".into());
        }

        let mut assignments = Vec::new();
        for assignment_element in &assignment_elements {
            let assignment_sequence = elements[*assignment_element]
                .get("sequence")
                .ok_or_else(|| "scalar-local assignment element has no sequence".to_string())?;
            let match_items = assignment_sequence
                .get("matchItems")
                .and_then(Value::as_array)
                .ok_or_else(|| "scalar-local assignment element has no match items".to_string())?;
            if match_items.len() > local_vars.len() {
                return Err(
                    "scalar-local assignment elements cannot assign more targets than locals"
                        .into(),
                );
            }
            for assignment in match_items {
                if string_field(assignment, "kind").as_deref() != Some("Assignment") {
                    return Err("scalar-local sequence match items must be assignments".into());
                }
                let assigned = assignment
                    .get("left")
                    .ok_or_else(|| "scalar-local sequence assignment has no target".to_string())?;
                let local_index = local_symbol_index(expression_symbol(assigned).as_deref())
                    .ok_or_else(|| {
                        "scalar-local sequence assignment must target one of its local variables"
                            .to_string()
                    })?;
                let assigned_source = assignment
                    .get("right")
                    .ok_or_else(|| "scalar-local sequence assignment has no value".to_string())?;
                let reads_local = local_names
                    .iter()
                    .any(|name| json_references_local_symbol(assigned_source, name));
                let value = if reads_local {
                    ScalarLocalAssignmentValue::Pending(assigned_source.clone())
                } else {
                    ScalarLocalAssignmentValue::Ready(self.lower_expression(assigned_source)?)
                };
                assignments.push(ScalarLocalAssignment {
                    local_index,
                    element: *assignment_element,
                    age: cumulative[*assignment_element],
                    value,
                });
            }
        }
        if assignments.len() > 64 {
            return Err("scalar-local sequences support at most sixty-four assignments".into());
        }
        for local_index in 0..local_vars.len() {
            if !assignments
                .iter()
                .any(|assignment| assignment.local_index == local_index)
            {
                return Err(
                    "every scalar-local sequence variable must be assigned at least once".into(),
                );
            }
        }

        let mut referenced = vec![false; local_vars.len()];
        let mut element_plans = Vec::with_capacity(elements.len());
        for element_index in 0..elements.len() {
            let (expression, delay) = &raw_elements[element_index];
            let age = cumulative[element_index];
            let direct_local = if string_field(expression, "kind").as_deref() == Some("NamedValue")
            {
                local_symbol_index(expression_symbol(expression).as_deref())
            } else {
                None
            };
            let (rewritten, references) =
                rewrite_sequence_local_references(expression, &local_names, property_index, age);
            for &local_index in &references {
                if !assignments
                    .iter()
                    .any(|assignment| assignment.local_index == local_index && assignment.age < age)
                {
                    return Err(
                        "sequence-local references must follow their assignment element".into(),
                    );
                }
                referenced[local_index] = true;
            }
            let plain = if references.is_empty() {
                Some(self.lower_expression(expression)?)
            } else {
                None
            };
            element_plans.push((direct_local, rewritten, references, plain, age, *delay));
        }
        if referenced.iter().any(|seen| !seen) {
            return Err(
                "every scalar-local sequence assignment must be referenced by a suffix".into(),
            );
        }

        fn latest_assignment_index(
            assignments: &[ScalarLocalAssignment],
            local_index: usize,
            age: usize,
        ) -> Option<usize> {
            assignments
                .iter()
                .enumerate()
                .filter(|(_, assignment)| {
                    assignment.local_index == local_index && assignment.age < age
                })
                .max_by_key(|(_, assignment)| assignment.age)
                .map(|(index, _)| index)
        }

        let mut required_sample_locals = vec![false; local_vars.len()];
        for assignment in &assignments {
            match &assignment.value {
                ScalarLocalAssignmentValue::Ready(value) => {
                    if !matches!(value, eda_core::ir::Expr::Const(_)) {
                        required_sample_locals[assignment.local_index] = true;
                    }
                }
                ScalarLocalAssignmentValue::Pending(source) => {
                    for (local_index, name) in local_names.iter().enumerate() {
                        if json_references_local_symbol(source, name) {
                            required_sample_locals[local_index] = true;
                        }
                    }
                }
            }
        }
        for (element_index, (direct_local, _, references, _, _, _)) in
            element_plans.iter().enumerate()
        {
            let has_later_assignment = assignments
                .iter()
                .any(|assignment| assignment.element > element_index);
            if direct_local.is_none() || has_later_assignment {
                for &local_index in references {
                    required_sample_locals[local_index] = true;
                }
            }
        }

        let mut sample_nets_by_assignment = vec![None; assignments.len()];
        let mut sample_references = vec![Vec::new(); assignments.len()];
        for (assignment_index, assignment) in assignments.iter().enumerate() {
            if !required_sample_locals[assignment.local_index] {
                continue;
            }
            let sample_stage_count = total_delay
                .checked_sub(assignment.age)
                .ok_or_else(|| "scalar-local assignment age exceeds sequence delay".to_string())?;
            if sample_stage_count == 0 {
                return Err("scalar-local assignments require a later reference".into());
            }
            let mut nets = Vec::with_capacity(sample_stage_count);
            for stage in 0..sample_stage_count {
                let (width, signed) = local_shapes[assignment.local_index];
                nets.push(self.declare_property_local_sample_net(
                    member,
                    property_index,
                    sequence_group,
                    (assignment.local_index, assignment_index, stage),
                    width,
                    signed,
                ));
            }
            sample_references[assignment_index] = nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: local_shapes[assignment.local_index].0,
                    range: full_range(local_shapes[assignment.local_index].0),
                    signed: local_shapes[assignment.local_index].1,
                })
                .collect();
            sample_nets_by_assignment[assignment_index] = Some(nets);
        }

        for assignment_index in 0..assignments.len() {
            let source = match assignments[assignment_index].value.clone() {
                ScalarLocalAssignmentValue::Pending(source) => source,
                ScalarLocalAssignmentValue::Ready(_) => continue,
            };
            let assignment_age = assignments[assignment_index].age;
            let (rewritten, references) = rewrite_sequence_local_references(
                &source,
                &local_names,
                property_index,
                assignment_age,
            );
            let mut temporary_symbols = Vec::new();
            for local_index in references
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
            {
                let prior_index =
                    latest_assignment_index(&assignments, local_index, assignment_age).ok_or_else(
                        || {
                            "sequence-local assignment RHS references an unassigned local"
                                .to_string()
                        },
                    )?;
                let prior_age = assignments[prior_index].age;
                let stage = assignment_age
                    .checked_sub(prior_age)
                    .and_then(|relative| relative.checked_sub(1))
                    .ok_or_else(|| {
                        "sequence-local assignment RHS has no prior sample state".to_string()
                    })?;
                let reference = sample_references
                    .get(prior_index)
                    .and_then(|references| references.get(stage))
                    .ok_or_else(|| {
                        "sequence-local assignment RHS has no prior sample state".to_string()
                    })?;
                let net = match reference {
                    eda_core::ir::Expr::Ref { net, .. } => *net,
                    _ => return Err("scalar-local sample reference is malformed".into()),
                };
                let key = format!("__eda_slseq_{property_index}_{local_index}_{assignment_age}");
                self.symbols.insert(
                    key.clone(),
                    AstLoweredSymbol {
                        base: net,
                        width: 1,
                        signed: false,
                        bounds: None,
                        dimensions: Vec::new(),
                        memory: None,
                        struct_fields: std::collections::BTreeMap::new(),
                    },
                );
                temporary_symbols.push(key);
            }
            let lowered = self.lower_expression(&rewritten)?;
            for key in temporary_symbols {
                self.symbols.remove(&key);
            }
            assignments[assignment_index].value = ScalarLocalAssignmentValue::Ready(lowered);
        }

        let local_reference = |local_index: usize, age: usize| -> Result<(usize, usize), String> {
            let assignment_index = latest_assignment_index(&assignments, local_index, age)
                .ok_or_else(|| "sequence-local reference has no prior assignment".to_string())?;
            let assignment_age = assignments[assignment_index].age;
            let stage = age
                .checked_sub(assignment_age)
                .and_then(|relative| relative.checked_sub(1))
                .ok_or_else(|| "scalar-local reference age has no sample state".to_string())?;
            if sample_references
                .get(assignment_index)
                .is_some_and(|references| stage < references.len())
            {
                Ok((assignment_index, stage))
            } else {
                Err("scalar-local reference age has no sample state".into())
            }
        };

        let mut resolved_elements = Vec::with_capacity(elements.len());
        for (direct_local, rewritten, references, plain, age, delay) in element_plans {
            let expression = if let Some(plain_expression) = plain {
                plain_expression
            } else if let Some(local_index) = direct_local {
                let assignment_index = latest_assignment_index(&assignments, local_index, age)
                    .ok_or_else(|| {
                        "sequence-local reference has no prior assignment".to_string()
                    })?;
                if let ScalarLocalAssignmentValue::Ready(eda_core::ir::Expr::Const(constant)) =
                    &assignments[assignment_index].value
                {
                    eda_core::ir::Expr::Const(constant.clone())
                } else {
                    let (assignment_index, stage) = local_reference(local_index, age)?;
                    sample_references[assignment_index][stage].clone()
                }
            } else {
                let mut temporary_symbols = Vec::new();
                for local_index in references
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    let (assignment_index, stage) = local_reference(local_index, age)?;
                    let reference = &sample_references[assignment_index][stage];
                    let net = match reference {
                        eda_core::ir::Expr::Ref { net, .. } => *net,
                        _ => return Err("scalar-local sample reference is malformed".into()),
                    };
                    let key = format!("__eda_slseq_{property_index}_{local_index}_{age}");
                    self.symbols.insert(
                        key.clone(),
                        AstLoweredSymbol {
                            base: net,
                            width: local_shapes[local_index].0,
                            signed: local_shapes[local_index].1,
                            bounds: None,
                            dimensions: Vec::new(),
                            memory: None,
                            struct_fields: std::collections::BTreeMap::new(),
                        },
                    );
                    temporary_symbols.push(key);
                }
                let lowered = self.lower_expression(&rewritten)?;
                for key in temporary_symbols {
                    self.symbols.remove(&key);
                }
                antecedent_reduction(&lowered)
            };
            resolved_elements.push((expression, delay));
        }

        let mut history_specs_by_element = vec![Vec::new(); elements.len()];
        for assignment_element in &assignment_elements {
            let assignment_age = cumulative[*assignment_element];
            let mut history_specs = Vec::new();
            for prior_index in 0..*assignment_element {
                history_specs.push((
                    resolved_elements[prior_index].0.clone(),
                    assignment_age - cumulative[prior_index],
                ));
            }
            history_specs_by_element[*assignment_element] = history_specs;
        }

        let mut local_samples = Vec::new();
        for (assignment_index, assignment) in assignments.iter().enumerate() {
            let Some(nets) = sample_nets_by_assignment[assignment_index].take() else {
                continue;
            };
            let mut histories = Vec::new();
            for (history_index, (history_trigger, history_depth)) in history_specs_by_element
                [assignment.element]
                .iter()
                .enumerate()
            {
                let mut history_nets = Vec::with_capacity(*history_depth);
                for stage in 0..*history_depth {
                    history_nets.push(self.declare_property_local_history_net(
                        member,
                        property_index,
                        sequence_group,
                        assignment.local_index,
                        assignment_index,
                        (history_index, stage),
                    ));
                }
                histories.push(ConcurrentLocalHistory {
                    trigger: history_trigger.clone(),
                    nets: history_nets,
                });
            }
            let value = match &assignment.value {
                ScalarLocalAssignmentValue::Ready(value) => value.clone(),
                ScalarLocalAssignmentValue::Pending(_) => {
                    return Err("scalar-local assignment RHS was not resolved".into())
                }
            };
            local_samples.push(ConcurrentLocalSample {
                width: local_shapes[assignment.local_index].0,
                signed: local_shapes[assignment.local_index].1,
                trigger: antecedent_reduction(&resolved_elements[assignment.element].0),
                value,
                nets,
                histories,
            });
        }

        let (expressions, delays): (Vec<_>, Vec<_>) = resolved_elements.into_iter().unzip();
        Ok(ConcurrentAntecedentSequence {
            expressions,
            delays,
            range: None,
            range_element: 0,
            local_samples,
        })
    }

    fn lower_consecutive_antecedent_repetition(
        &self,
        operand: &serde_json::Value,
    ) -> Result<ConcurrentAntecedentSequence, String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "antecedent repetition is missing".to_string())?;
        if string_field(repetition, "kind").as_deref() != Some("Consecutive") {
            return Err(
                "direct concurrent antecedents support only fixed-count consecutive repetition"
                    .into(),
            );
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "antecedent repetition has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "antecedent repetition has no maximum".to_string())?;
        if min != max {
            return Err("direct concurrent antecedent repetition ranges are not supported".into());
        }
        if !(1..=256).contains(&min) {
            return Err(
                "direct concurrent antecedent repetition count must stay within 1..=256".into(),
            );
        }
        let expression_source = operand
            .get("expr")
            .ok_or_else(|| "antecedent repetition has no expression".to_string())?;
        let expression = self.lower_expression(expression_source)?;
        let count = min as usize;
        let expressions = vec![expression; count];
        let mut delays = Vec::with_capacity(count);
        delays.push(0);
        delays.resize(count, 1);
        Ok(ConcurrentAntecedentSequence {
            expressions,
            delays,
            range: None,
            range_element: 0,
            local_samples: Vec::new(),
        })
    }

    fn lower_goto_antecedent_repetition(
        &self,
        operand: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize), String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "antecedent repetition is missing".to_string())?;
        if string_field(repetition, "kind").as_deref() != Some("GoTo") {
            return Err("direct concurrent antecedent goto repetition is malformed".into());
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "goto antecedent repetition has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "goto antecedent repetition has no maximum".to_string())?;
        if min != max {
            return Err(
                "direct concurrent goto antecedent repetition ranges are not supported".into(),
            );
        }
        if !(1..=256).contains(&min) {
            return Err(
                "direct concurrent goto antecedent repetition count must stay within 1..=256"
                    .into(),
            );
        }
        let expression_source = operand
            .get("expr")
            .ok_or_else(|| "goto antecedent repetition has no expression".to_string())?;
        Ok((self.lower_expression(expression_source)?, min as usize))
    }

    fn lower_consecutive_antecedent_repetition_range(
        &self,
        operand: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "antecedent repetition is missing".to_string())?;
        if string_field(repetition, "kind").as_deref() != Some("Consecutive") {
            return Err("direct consecutive antecedent repetition ranges are malformed".into());
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "antecedent repetition range has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "antecedent repetition range has no maximum".to_string())?;
        if min >= max {
            return Err("antecedent repetition range must have distinct ordered bounds".into());
        }
        if !(1..=256).contains(&min) || !(1..=256).contains(&max) {
            return Err(
                "direct consecutive antecedent repetition ranges must stay within 1..=256".into(),
            );
        }
        let expression_source = operand
            .get("expr")
            .ok_or_else(|| "antecedent repetition range has no expression".to_string())?;
        Ok((
            self.lower_expression(expression_source)?,
            min as usize,
            max as usize,
        ))
    }

    fn lower_nonconsecutive_antecedent_repetition(
        &self,
        operand: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "antecedent repetition is missing".to_string())?;
        if string_field(repetition, "kind").as_deref() != Some("Nonconsecutive") {
            return Err("direct nonconsecutive antecedent repetition is malformed".into());
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "nonconsecutive antecedent repetition has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "nonconsecutive antecedent repetition has no maximum".to_string())?;
        if min > max {
            return Err("nonconsecutive antecedent repetition range start exceeds end".into());
        }
        if !(1..=256).contains(&min) || !(1..=256).contains(&max) {
            return Err(
                "direct nonconsecutive antecedent repetition must stay within 1..=256".into(),
            );
        }
        let expression_source = operand
            .get("expr")
            .ok_or_else(|| "nonconsecutive antecedent repetition has no expression".to_string())?;
        Ok((
            self.lower_expression(expression_source)?,
            min as usize,
            max as usize,
        ))
    }

    fn lower_goto_antecedent_repetition_range(
        &self,
        operand: &serde_json::Value,
    ) -> Result<(eda_core::ir::Expr, usize, usize), String> {
        let repetition = operand
            .get("repetition")
            .ok_or_else(|| "antecedent repetition is missing".to_string())?;
        if string_field(repetition, "kind").as_deref() != Some("GoTo") {
            return Err("direct goto antecedent repetition ranges are malformed".into());
        }
        let min = repetition
            .get("min")
            .and_then(Value::as_u64)
            .ok_or_else(|| "goto antecedent repetition range has no minimum".to_string())?;
        let max = repetition
            .get("max")
            .and_then(Value::as_u64)
            .ok_or_else(|| "goto antecedent repetition range has no maximum".to_string())?;
        if min >= max {
            return Err(
                "goto antecedent repetition range must have distinct ordered bounds".into(),
            );
        }
        if !(1..=256).contains(&min) || !(1..=256).contains(&max) {
            return Err("direct goto antecedent repetition ranges must stay within 1..=256".into());
        }
        let expression_source = operand
            .get("expr")
            .ok_or_else(|| "goto antecedent repetition range has no expression".to_string())?;
        Ok((
            self.lower_expression(expression_source)?,
            min as usize,
            max as usize,
        ))
    }

    fn declare_property_pending_net(
        &mut self,
        member: &serde_json::Value,
        property_index: usize,
        stage: usize,
    ) -> usize {
        let address = member
            .get("addr")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| u64::try_from(property_index).unwrap_or(u64::MAX));
        let base = self.design.n_nets;
        self.design.n_nets += 1;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: format!("__property_pending_{address}_{stage}"),
            width: 1,
            is_reg: true,
            is_signed: false,
            base,
        });
        base
    }

    fn declare_property_local_sample_net(
        &mut self,
        member: &serde_json::Value,
        property_index: usize,
        sequence_group: usize,
        (local_index, assignment_index, age): (usize, usize, usize),
        width: usize,
        signed: bool,
    ) -> usize {
        let address = member
            .get("addr")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| u64::try_from(property_index).unwrap_or(u64::MAX));
        let base = self.design.n_nets;
        self.design.n_nets += width;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: format!(
                "__property_local_sample_{address}_{sequence_group}_{local_index}_{assignment_index}_{age}"
            ),
            width,
            is_reg: true,
            is_signed: signed,
            base,
        });
        base
    }

    fn zero_property_net(&self, net: usize) -> Stmt {
        let width = self
            .design
            .net_decls
            .iter()
            .find(|declaration| declaration.base == net)
            .map(|declaration| declaration.width)
            .unwrap_or(1);
        Stmt::Bl {
            lv: eda_core::ir::LValue {
                net,
                decl_width: width,
                range: full_range(width),
            },
            e: eda_core::ir::Expr::Const(LogicWord::zero(width)),
        }
    }

    fn declare_property_local_history_net(
        &mut self,
        member: &serde_json::Value,
        property_index: usize,
        sequence_group: usize,
        local_index: usize,
        assignment_index: usize,
        (history_index, stage): (usize, usize),
    ) -> usize {
        let address = member
            .get("addr")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| u64::try_from(property_index).unwrap_or(u64::MAX));
        let base = self.design.n_nets;
        self.design.n_nets += 1;
        self.design.net_decls.push(eda_core::ir::NetDecl {
            name: format!(
                "__property_local_history_{address}_{sequence_group}_{local_index}_{assignment_index}_{history_index}_{stage}"
            ),
            width: 1,
            is_reg: true,
            is_signed: false,
            base,
        });
        base
    }

    fn local_sample_updates(sample: &ConcurrentLocalSample) -> Vec<Stmt> {
        let history_net_count = sample
            .histories
            .iter()
            .map(|history| history.nets.len())
            .sum::<usize>();
        let mut updates = Vec::with_capacity(sample.nets.len() + history_net_count);
        let references = sample
            .nets
            .iter()
            .map(|&net| eda_core::ir::Expr::Ref {
                net,
                decl_width: sample.width,
                range: full_range(sample.width),
                signed: sample.signed,
            })
            .collect::<Vec<_>>();
        let mut assignment_match = antecedent_reduction(&sample.trigger);
        for history in &sample.histories {
            let history_references = history
                .nets
                .iter()
                .map(|&net| eda_core::ir::Expr::Ref {
                    net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                })
                .collect::<Vec<_>>();
            updates.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: history.nets[0],
                    decl_width: 1,
                    range: full_range(1),
                },
                e: antecedent_reduction(&history.trigger),
            });
            for stage in 1..history.nets.len() {
                updates.push(Stmt::NBl {
                    lv: eda_core::ir::LValue {
                        net: history.nets[stage],
                        decl_width: 1,
                        range: full_range(1),
                    },
                    e: history_references[stage - 1].clone(),
                });
            }
            let history_net = history.nets[history.nets.len() - 1];
            assignment_match = eda_core::ir::Expr::Bin {
                op: eda_core::ir::BinOp::LogAnd,
                a: Box::new(assignment_match),
                b: Box::new(eda_core::ir::Expr::Ref {
                    net: history_net,
                    decl_width: 1,
                    range: full_range(1),
                    signed: false,
                }),
            };
        }
        let sampled_value = eda_core::ir::Expr::Cond {
            c: Box::new(assignment_match),
            t: Box::new(sample.value.clone()),
            f: Box::new(eda_core::ir::Expr::Const(LogicWord::zero(sample.width))),
        };
        updates.push(Stmt::NBl {
            lv: eda_core::ir::LValue {
                net: sample.nets[0],
                decl_width: sample.width,
                range: full_range(sample.width),
            },
            e: sampled_value,
        });
        for age in 1..sample.nets.len() {
            updates.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: sample.nets[age],
                    decl_width: sample.width,
                    range: full_range(sample.width),
                },
                e: references[age - 1].clone(),
            });
        }
        updates
    }

    fn simple_property_operand<'v>(
        value: &'v serde_json::Value,
        side: &'static str,
    ) -> Result<&'v serde_json::Value, String> {
        if string_field(value, "kind").as_deref() != Some("Simple") {
            return Err(format!(
                "direct concurrent assertion lowering supports only simple {side} expressions"
            ));
        }
        if value.get("repetition").is_some() {
            return Err(format!(
                "direct concurrent assertion lowering does not support {side} sequence repetition"
            ));
        }
        value
            .get("expr")
            .ok_or_else(|| format!("simple concurrent {side} expression has no value"))
    }

    fn sensitivity_has_clock_edge(sensitivity: &eda_core::ir::Sens) -> bool {
        match sensitivity {
            eda_core::ir::Sens::Posedge(_) | eda_core::ir::Sens::Negedge(_) => true,
            eda_core::ir::Sens::List(events) => events.iter().any(Self::sensitivity_has_clock_edge),
            eda_core::ir::Sens::Comb | eda_core::ir::Sens::Timed | eda_core::ir::Sens::Any(_) => {
                false
            }
        }
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
        if string_field(statement, "kind").as_deref() == Some("ConcurrentAssertion")
            && string_field(statement, "assertionKind").as_deref() == Some("Expect")
        {
            return self
                .lower_procedural_expect_statement(statement)
                .map_err(|error| with_source_span(statement, error));
        }
        self.lower_statement_inner(statement)
            .map_err(|error| with_source_span(statement, error))
    }

    fn lower_procedural_expect_statement(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        let property_spec = statement
            .get("propertySpec")
            .ok_or_else(|| "expect statement has no property spec".to_string())?;
        let clocking = match string_field(property_spec, "kind").as_deref() {
            Some("Clocking") => property_spec
                .get("clocking")
                .ok_or_else(|| "expect property has no clocking event".to_string())?,
            _ => {
                return Err(
                    "procedural expect statements require an explicit clocking event".into(),
                )
            }
        };
        let sensitivity = self.lower_sensitivity(clocking)?;
        let watch = match sensitivity {
            eda_core::ir::Sens::Posedge(net) => eda_core::ir::EventWatch {
                net,
                edge: eda_core::ir::EventEdge::Posedge,
            },
            eda_core::ir::Sens::Negedge(net) => eda_core::ir::EventWatch {
                net,
                edge: eda_core::ir::EventEdge::Negedge,
            },
            _ => {
                return Err(
                    "procedural expect statements require a single posedge or negedge clock".into(),
                )
            }
        };
        let property_expr = property_spec
            .get("expr")
            .ok_or_else(|| "expect property has no expression".to_string())?;
        if string_field(property_expr, "kind").as_deref() != Some("SequenceConcat") {
            return Err(
                "procedural expect statements support only fixed-delay sequence concatenation"
                    .into(),
            );
        }
        let elements = property_expr
            .get("elements")
            .and_then(Value::as_array)
            .ok_or_else(|| "expect sequence has no elements".to_string())?;
        if elements.is_empty() || elements.len() > 64 {
            return Err("procedural expect sequences support one to sixty-four elements".into());
        }

        let mut expressions = Vec::with_capacity(elements.len());
        let mut delays = Vec::with_capacity(elements.len());
        let mut total_delay = 0_usize;
        for (index, element) in elements.iter().enumerate() {
            let sequence = element
                .get("sequence")
                .ok_or_else(|| "expect sequence element has no sequence".to_string())?;
            let expression =
                self.lower_expression(Self::simple_property_operand(sequence, "expect sequence")?)?;
            let min = element
                .get("min")
                .and_then(Value::as_u64)
                .ok_or_else(|| "expect sequence element has no minimum delay".to_string())?;
            let max = element
                .get("max")
                .and_then(Value::as_u64)
                .ok_or_else(|| "expect sequence element has no maximum delay".to_string())?;
            if min != max {
                return Err("procedural expect sequences support fixed delays only".into());
            }
            let delay = usize::try_from(min)
                .map_err(|_| "expect sequence delay is too large".to_string())?;
            if index == 0 {
                if delay != 0 {
                    return Err(
                        "the first procedural expect element must start at the clock edge".into(),
                    );
                }
            } else if delay == 0 || delay > 256 {
                return Err("later procedural expect elements require 1..=256 clock delays".into());
            }
            total_delay = total_delay
                .checked_add(delay)
                .ok_or_else(|| "procedural expect sequence delay overflows".to_string())?;
            expressions.push(expression);
            delays.push(delay);
        }
        if total_delay > 256 {
            return Err("procedural expect sequence delay must stay within 0..=256".into());
        }
        // The leading property clock is itself the first sampling edge. Slang
        // records the first sequence element with delay zero, but a procedural
        // expect must first suspend until that clock edge before evaluating it.
        delays[0] = 1;

        let pass_action = self.lower_optional_assertion_action(statement.get("ifTrue"))?;
        let fail_action = self.lower_optional_assertion_action(statement.get("ifFalse"))?;
        let pass_statement = pass_action.unwrap_or_else(|| {
            Box::new(Stmt::ImmediateAssertion {
                kind: eda_core::ir::ImmediateAssertionKind::Assert,
                deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                scope: self.current_assertion_scope(),
                cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 1)),
                pass_action: None,
                fail_action: None,
            })
        });
        let failure_statement = |fail_action: &Option<Box<Stmt>>| {
            fail_action.clone().unwrap_or_else(|| {
                Box::new(Stmt::ImmediateAssertion {
                    kind: eda_core::ir::ImmediateAssertionKind::Assert,
                    deferred: eda_core::ir::ImmediateAssertionDeferred::None,
                    scope: self.current_assertion_scope(),
                    cond: eda_core::ir::Expr::Const(LogicWord::from_u64(1, 0)),
                    pass_action: None,
                    fail_action: None,
                })
            })
        };

        fn expect_sequence(
            expressions: &[eda_core::ir::Expr],
            delays: &[usize],
            watch: eda_core::ir::EventWatch,
            pass_statement: &Stmt,
            fail_statement: &Stmt,
        ) -> Result<Stmt, String> {
            let Some((first_expression, rest_expressions)) = expressions.split_first() else {
                return Err("procedural expect sequence has no expressions".into());
            };
            let Some((first_delay, rest_delays)) = delays.split_first() else {
                return Err("procedural expect sequence has no delays".into());
            };
            let success = if rest_expressions.is_empty() {
                pass_statement.clone()
            } else {
                expect_sequence(
                    rest_expressions,
                    rest_delays,
                    watch,
                    pass_statement,
                    fail_statement,
                )?
            };
            let mut body = Stmt::If {
                c: antecedent_reduction(first_expression),
                t: Box::new(success),
                f: Some(Box::new(fail_statement.clone())),
            };
            for _ in 0..*first_delay {
                body = Stmt::EventWait {
                    watches: vec![watch],
                    body: Box::new(body),
                };
            }
            Ok(body)
        }

        let fail_statement = failure_statement(&fail_action);
        expect_sequence(
            &expressions,
            &delays,
            watch,
            &pass_statement,
            &fail_statement,
        )
    }

    fn active_named_block_ids(&self) -> Vec<String> {
        self.active_named_blocks.borrow().clone()
    }

    fn current_assertion_scope(&self) -> Option<String> {
        self.active_assertion_scopes.borrow().last().cloned()
    }

    fn antecedent_possible_boundaries(
        index: usize,
        cumulative: &[usize],
        range: Option<(usize, usize)>,
        range_element: usize,
    ) -> Result<Vec<usize>, String> {
        let Some((start, end)) = range else {
            return Ok(vec![cumulative[index]]);
        };
        if index < range_element {
            return Ok(vec![cumulative[index]]);
        }
        let anchor = if range_element == 0 {
            0
        } else {
            cumulative[range_element - 1]
        };
        let mut boundaries: Vec<usize> = (anchor + start..=anchor + end).collect();
        for element in range_element + 1..=index {
            let delay = cumulative[element] - cumulative[element - 1];
            boundaries = boundaries
                .into_iter()
                .map(|boundary| boundary.saturating_add(delay))
                .collect();
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        if boundaries.is_empty() || boundaries.contains(&0) {
            return Err("antecedent range has no positive candidate boundaries".into());
        }
        Ok(boundaries)
    }

    fn antecedent_candidate_at_boundary(
        boundary: usize,
        first_expression: &eda_core::ir::Expr,
        references: &[eda_core::ir::Expr],
    ) -> Result<eda_core::ir::Expr, String> {
        if boundary == 0 {
            return Ok(first_expression.clone());
        }
        references
            .get(boundary - 1)
            .cloned()
            .ok_or_else(|| "antecedent range candidate is outside the pending pipeline".to_string())
    }

    fn antecedent_sequence_shifts(
        first_expression: &eda_core::ir::Expr,
        expressions: &[eda_core::ir::Expr],
        cumulative: &[usize],
        range: Option<(usize, usize)>,
        range_element: usize,
        references: &[eda_core::ir::Expr],
        pending_nets: &[usize],
    ) -> Result<Vec<Stmt>, String> {
        let mut stage_conditions = std::collections::BTreeMap::<usize, eda_core::ir::Expr>::new();
        for (index, _) in expressions.iter().enumerate() {
            if index + 1 == expressions.len() {
                break;
            }
            for boundary in
                Self::antecedent_possible_boundaries(index, cumulative, range, range_element)?
            {
                let candidate =
                    Self::antecedent_candidate_at_boundary(boundary, first_expression, references)?;
                let condition = eda_core::ir::Expr::Bin {
                    op: eda_core::ir::BinOp::LogAnd,
                    a: Box::new(candidate),
                    b: Box::new(expressions[index].clone()),
                };
                stage_conditions
                    .entry(boundary)
                    .and_modify(|existing| {
                        *existing = eda_core::ir::Expr::Bin {
                            op: eda_core::ir::BinOp::LogOr,
                            a: Box::new(existing.clone()),
                            b: Box::new(condition.clone()),
                        };
                    })
                    .or_insert(condition);
            }
        }

        let mut statements = Vec::with_capacity(pending_nets.len());
        for (stage, net) in pending_nets.iter().enumerate() {
            let shifted = if stage == 0 {
                None
            } else {
                Some(references[stage - 1].clone())
            };
            let condition = match stage_conditions.remove(&stage) {
                Some(seed) => match shifted {
                    Some(_) => seed,
                    None => seed,
                },
                None => shifted.unwrap_or_else(|| eda_core::ir::Expr::Const(LogicWord::zero(1))),
            };
            statements.push(Stmt::NBl {
                lv: eda_core::ir::LValue {
                    net: *net,
                    decl_width: 1,
                    range: full_range(1),
                },
                e: condition,
            });
        }
        if !stage_conditions.is_empty() {
            return Err("antecedent shift state is outside the pending pipeline".into());
        }
        Ok(statements)
    }

    fn lower_covergroup_construction(
        &self,
        assignment: &serde_json::Value,
    ) -> Result<Option<Stmt>, String> {
        let right = assignment
            .get("right")
            .ok_or_else(|| "assignment has no right side".to_string())?;
        if string_field(right, "kind").as_deref() != Some("NewCovergroup") {
            return Ok(None);
        }
        let left = assignment
            .get("left")
            .ok_or_else(|| "covergroup construction has no target".to_string())?;
        let instance = string_field(left, "symbol")
            .filter(|symbol| !symbol.is_empty())
            .ok_or_else(|| "covergroup construction target has no symbol".to_string())?;
        let constructed_type = string_field(right, "type")
            .filter(|type_name| !type_name.is_empty())
            .ok_or_else(|| "covergroup construction has no type".to_string())?;
        let declared_type = self
            .covergroup_instances
            .get(&instance)
            .ok_or_else(|| format!("covergroup instance '{instance}' was not declared"))?;
        if declared_type != &constructed_type {
            return Err(format!(
                "covergroup instance '{instance}' cannot be constructed as '{constructed_type}'"
            ));
        }
        self.constructed_covergroups.borrow_mut().insert(instance);
        Ok(Some(Stmt::Block(Vec::new())))
    }

    fn validate_manual_covergroup_lifetime(
        &self,
        statement: &serde_json::Value,
    ) -> Result<(), String> {
        if self.covergroup_instances.is_empty() {
            return Ok(());
        }
        let mut constructed = std::collections::BTreeSet::new();
        self.visit_sequential_covergroup_lifetime(statement, &mut constructed)
    }

    fn visit_sequential_covergroup_lifetime(
        &self,
        statement: &serde_json::Value,
        constructed: &mut std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        match string_field(statement, "kind").as_deref() {
            Some("List") => {
                let empty_items = Vec::new();
                let items = statement
                    .get("list")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_items);
                for item in items {
                    self.visit_sequential_covergroup_lifetime(item, constructed)?;
                }
                Ok(())
            }
            Some("Block")
                if matches!(
                    string_field(statement, "blockKind").as_deref(),
                    None | Some("Sequential")
                ) =>
            {
                let Some(body) = statement.get("body") else {
                    return Ok(());
                };
                self.visit_sequential_covergroup_lifetime(body, constructed)
            }
            Some("ExpressionStatement") => {
                let Some(expression) = statement.get("expr") else {
                    return Ok(());
                };
                if string_field(expression, "kind").as_deref() == Some("Assignment")
                    && expression.get("right").is_some_and(|right| {
                        string_field(right, "kind").as_deref() == Some("NewCovergroup")
                    })
                {
                    let instance = expression
                        .get("left")
                        .and_then(|left| string_field(left, "symbol"))
                        .filter(|symbol| !symbol.is_empty())
                        .ok_or_else(|| {
                            "covergroup construction target has no symbol".to_string()
                        })?;
                    constructed.insert(instance);
                    return Ok(());
                }
                self.visit_nested_covergroup_lifetime(expression, constructed)
            }
            Some("Assignment") => {
                if statement.get("right").is_some_and(|right| {
                    string_field(right, "kind").as_deref() == Some("NewCovergroup")
                }) {
                    let instance = statement
                        .get("left")
                        .and_then(|left| string_field(left, "symbol"))
                        .filter(|symbol| !symbol.is_empty())
                        .ok_or_else(|| {
                            "covergroup construction target has no symbol".to_string()
                        })?;
                    constructed.insert(instance);
                    return Ok(());
                }
                self.visit_nested_covergroup_lifetime(statement, constructed)
            }
            _ => self.visit_nested_covergroup_lifetime(statement, constructed),
        }
    }

    fn visit_nested_covergroup_lifetime(
        &self,
        value: &serde_json::Value,
        constructed: &std::collections::BTreeSet<String>,
    ) -> Result<(), String> {
        match string_field(value, "kind").as_deref() {
            Some("NewCovergroup") => {
                return Err(
                    "manual covergroup construction is supported only in straight-line procedural order"
                        .into(),
                );
            }
            Some("Call")
                if value
                    .get("subroutine")
                    .and_then(Value::as_str)
                    .is_some_and(|subroutine| {
                        subroutine.split_whitespace().next_back() == Some("sample")
                    }) =>
            {
                let instance = value
                    .get("thisClass")
                    .and_then(|receiver| string_field(receiver, "symbol"))
                    .filter(|symbol| !symbol.is_empty());
                if let Some(instance) = instance {
                    if self.covergroup_instances.contains_key(&instance)
                        && !constructed.contains(&instance)
                    {
                        return Err(format!(
                            "covergroup instance '{instance}' must be constructed before sample()"
                        ));
                    }
                }
            }
            _ => {}
        }
        match value {
            Value::Array(items) => {
                for item in items {
                    self.visit_nested_covergroup_lifetime(item, constructed)?;
                }
            }
            Value::Object(fields) => {
                for field in fields.values() {
                    self.visit_nested_covergroup_lifetime(field, constructed)?;
                }
            }
            _ => {}
        }
        Ok(())
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
        Ok(eda_core::ir::Stmt::DisableNamedBlock {
            name: named_id,
            flag: net,
        })
    }

    fn lower_foreach_loop(&self, statement: &serde_json::Value) -> Result<Stmt, String> {
        let array_reference = statement
            .get("arrayRef")
            .ok_or_else(|| "foreach loop has no array reference".to_string())?;
        let array_symbol = self
            .named_symbol(array_reference)
            .map_err(|error| format!("foreach array reference: {error}"))?;
        let Some(memory_index) = array_symbol.memory else {
            return Err("foreach arrays require fixed unpacked-array storage".into());
        };
        let memory = &self.design.memories[memory_index];
        if memory.dynamic || memory.queue || memory.associative {
            return Err("foreach loops support only fixed unpacked arrays".into());
        }
        if memory.dimensions.len() != 1 {
            return Err("foreach loops currently support one-dimensional fixed arrays".into());
        }
        let loop_dims = statement
            .get("loopDims")
            .and_then(Value::as_array)
            .ok_or_else(|| "foreach loop has no dimensions".to_string())?;
        if loop_dims.len() != 1 {
            return Err("foreach loops currently support one iteration dimension".into());
        }
        let variable = loop_dims[0]
            .get("var")
            .ok_or_else(|| "foreach dimension has no iterator".to_string())?;
        let iterator_name = string_field(variable, "name")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| "foreach iterator has no name".to_string())?;
        let iterator_address = variable
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| "foreach iterator has no address".to_string())?;
        let iterator_id = format!("{iterator_address} {iterator_name}");
        let iterator = self
            .symbols
            .get(&iterator_id)
            .cloned()
            .ok_or_else(|| "foreach iterator was not prepared".to_string())?;

        let dimension = memory.dimensions[0];
        if dimension.step != 1 && dimension.step != -1 {
            return Err("foreach loops support unit-stride fixed-array dimensions".into());
        }
        let count = i64::try_from(dimension.length)
            .map_err(|_| "foreach array dimension length is too large".to_string())?;
        let last = dimension
            .base
            .checked_add(
                dimension.step
                    * count.checked_sub(1).ok_or_else(|| {
                        "foreach array dimension must contain at least one element".to_string()
                    })?,
            )
            .ok_or_else(|| "foreach array endpoint overflows".to_string())?;
        if dimension.base < 0 || last < 0 {
            return Err("foreach loops currently support nonnegative array bounds".into());
        }

        let iterator_reference = eda_core::ir::Expr::Ref {
            net: iterator.base,
            decl_width: iterator.width,
            range: full_range(iterator.width),
            signed: iterator.signed,
        };
        let iterator_lvalue = eda_core::ir::LValue {
            net: iterator.base,
            decl_width: iterator.width,
            range: full_range(iterator.width),
        };
        let word = |value: i64| {
            eda_core::ir::Expr::Const(LogicWord::from_u64(
                iterator.width,
                u64::try_from(value).unwrap_or(u64::MAX),
            ))
        };
        let op = if dimension.step == 1 {
            eda_core::ir::BinOp::Le
        } else {
            eda_core::ir::BinOp::Ge
        };
        let step = if dimension.step == 1 { 1_i64 } else { -1_i64 };
        let body = statement
            .get("body")
            .ok_or_else(|| "foreach loop has no body".to_string())?;
        Ok(Stmt::Block(vec![
            Stmt::Bl {
                lv: iterator_lvalue.clone(),
                e: word(dimension.base),
            },
            Stmt::While {
                cond: eda_core::ir::Expr::Bin {
                    op,
                    a: Box::new(iterator_reference.clone()),
                    b: Box::new(word(last)),
                },
                body: Box::new(self.lower_loop_body(body)?),
                steps: vec![Stmt::Bl {
                    lv: iterator_lvalue,
                    e: eda_core::ir::Expr::Bin {
                        op: eda_core::ir::BinOp::Add,
                        a: Box::new(iterator_reference),
                        b: Box::new(word(step)),
                    },
                }],
            },
        ]))
    }

    fn lower_procedural_concatenation_assign(
        &self,
        left: &Value,
        right: &Value,
        is_force: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::{Expr, Stmt};

        let empty_operands = Vec::new();
        let operands = left
            .get("operands")
            .and_then(Value::as_array)
            .unwrap_or(&empty_operands);
        let targets = operands
            .iter()
            .map(|operand| {
                let lv = self.lower_lvalue(operand)?;
                validate_static_procedural_assignment_lvalue(operand, &lv)?;
                Ok((operand, lv))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let total_width = targets
            .iter()
            .map(|(_, lv)| lv.range.width())
            .sum::<usize>();
        let rhs = self.lower_expression(right)?;
        if rhs.width() != total_width {
            return Err(format!(
                "procedural concatenation assignment width {} does not match RHS width {}",
                total_width,
                rhs.width()
            ));
        }

        let mut statements = Vec::new();
        let mut remaining = total_width;
        for (operand, lv) in targets {
            let width = lv.range.width();
            let lsb = remaining
                .checked_sub(width)
                .ok_or_else(|| "procedural concatenation assignment width underflow".to_string())?;
            let msb = remaining - 1;
            let e = if width == remaining {
                rhs.clone()
            } else {
                Expr::Index {
                    e: Box::new(rhs.clone()),
                    msb,
                    lsb,
                }
            };
            if !is_force {
                validate_procedural_driver_rhs(&e)?;
            }
            let _ = operand;
            if is_force {
                statements.push(Stmt::Force { lv, e });
            } else {
                statements.push(Stmt::ProceduralAssign { lv, e });
            }
            remaining = lsb;
        }
        if remaining != 0 {
            return Err("procedural concatenation assignment did not cover RHS".into());
        }
        Ok(Stmt::Block(statements))
    }

    fn lower_procedural_concatenation_deassign(
        &self,
        left: &Value,
        is_release: bool,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;

        let empty_operands = Vec::new();
        let operands = left
            .get("operands")
            .and_then(Value::as_array)
            .unwrap_or(&empty_operands);
        let statements = operands
            .iter()
            .map(|operand| {
                let lv = self.lower_lvalue(operand)?;
                validate_static_procedural_assignment_lvalue(operand, &lv)?;
                if is_release {
                    Ok(Stmt::Release { lv })
                } else {
                    Ok(Stmt::ProceduralDeassign { lv })
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Stmt::Block(statements))
    }

    fn lower_statement_inner(
        &self,
        statement: &serde_json::Value,
    ) -> Result<eda_core::ir::Stmt, String> {
        use eda_core::ir::Stmt;
        let kind = string_field(statement, "kind").unwrap_or_default();
        match kind.as_str() {
            "Block" => {
                let named_id = string_field(statement, "block")
                    .filter(|symbol| !symbol.trim().is_empty());
                match string_field(statement, "blockKind").as_deref() {
                    None | Some("Sequential") => {}
                    Some(block_kind @ ("JoinAll" | "JoinAny" | "JoinNone")) => {
                        let body = statement
                            .get("body")
                            .ok_or_else(|| "fork block has no body".to_string())?;
                        if let Some(named_id) = &named_id {
                            self.active_named_blocks.borrow_mut().push(named_id.clone());
                        }
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
                        if named_id.is_some() {
                            self.active_named_blocks.borrow_mut().pop();
                        }
                        let fork = Stmt::Fork { branches, mode };
                        if let Some(named_id) = named_id {
                            let net = self
                                .named_block_flags
                                .get(&named_id)
                                .copied()
                                .ok_or_else(|| "named block has no disable flag".to_string())?;
                            return Ok(Stmt::NamedBlock {
                                name: named_id,
                                body: Box::new(Stmt::Block(vec![
                                    Stmt::Bl {
                                        lv: eda_core::ir::LValue {
                                            net,
                                            decl_width: 1,
                                            range: full_range(1),
                                        },
                                        e: eda_core::ir::Expr::Const(LogicWord::zero(1)),
                                    },
                                    fork,
                                ])),
                            });
                        }
                        return Ok(fork);
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
                if let Some(named_id) = &named_id {
                    self.active_named_blocks.borrow_mut().push(named_id.clone());
                    self.active_assertion_scopes
                        .borrow_mut()
                        .push(named_id.clone());
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
                                .cloned()
                                .flatten();
                            let symbol = self
                                .local_symbols
                                .get(symbol_id)
                                .ok_or_else(|| "procedure-local symbol was not collected".to_string())?;
                            if let Some(initializer) = initializer {
                                let rhs = self.lower_expression(&initializer)?;
                                statements.push(Stmt::Bl {
                                    lv: eda_core::ir::LValue {
                                        net: symbol.base,
                                        decl_width: symbol.width,
                                        range: full_range(symbol.width),
                                    },
                                    e: rhs,
                                });
                            }
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
                            self.active_assertion_scopes.borrow_mut().pop();
                        }
                        if let Some(named_id) = named_id {
                            return Ok(Stmt::NamedBlock {
                                name: named_id,
                                body: Box::new(Stmt::Block(statements)),
                            });
                        }
                        return Ok(Stmt::Block(statements));
                    }
                    let nested = self.lower_statement(nested)?;
                    let nested = self.guard_named_block_statement(nested, &active_ids);
                    if named_id.is_some() {
                        self.active_named_blocks.borrow_mut().pop();
                        self.active_assertion_scopes.borrow_mut().pop();
                    }
                    if let Some(named_id) = named_id {
                        return Ok(Stmt::NamedBlock {
                            name: named_id,
                            body: Box::new(Stmt::Block(vec![nested])),
                        });
                    }
                    return Ok(Stmt::Block(vec![nested]));
                }
                if named_id.is_some() {
                    self.active_named_blocks.borrow_mut().pop();
                    self.active_assertion_scopes.borrow_mut().pop();
                }
                if let Some(named_id) = named_id {
                    return Ok(Stmt::NamedBlock {
                        name: named_id,
                        body: Box::new(Stmt::Block(Vec::new())),
                    });
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
            "ImmediateAssertion" => {
                let condition = statement
                    .get("cond")
                    .ok_or_else(|| "immediate assertion has no condition".to_string())?;
                let condition = self.lower_expression(condition)?;
                let lower_action = |action: Option<&serde_json::Value>| -> Result<
                    Option<Box<Stmt>>,
                    String,
                > {
                    let Some(action) = action else {
                        return Ok(None);
                    };
                    if string_field(action, "kind").as_deref() == Some("Empty") {
                        return Ok(None);
                    }
                    Ok(Some(Box::new(self.lower_statement(action)?)))
                };
                let pass_action = lower_action(statement.get("ifTrue"))?;
                let fail_action = lower_action(statement.get("ifFalse"))?;
                let deferred = if bool_field(statement, "isFinal") {
                    eda_core::ir::ImmediateAssertionDeferred::Final
                } else if bool_field(statement, "isDeferred") {
                    eda_core::ir::ImmediateAssertionDeferred::Observed
                } else {
                    eda_core::ir::ImmediateAssertionDeferred::None
                };
                match string_field(statement, "assertionKind").as_deref() {
                    Some("Assert") => Ok(Stmt::ImmediateAssertion {
                        kind: eda_core::ir::ImmediateAssertionKind::Assert,
                        deferred,
                        scope: self.current_assertion_scope(),
                        cond: condition,
                        pass_action,
                        fail_action,
                    }),
                    Some("Assume") => Ok(Stmt::ImmediateAssertion {
                        kind: eda_core::ir::ImmediateAssertionKind::Assume,
                        deferred,
                        scope: self.current_assertion_scope(),
                        cond: condition,
                        pass_action,
                        fail_action,
                    }),
                    Some("CoverProperty") => Ok(Stmt::ImmediateAssertion {
                        kind: eda_core::ir::ImmediateAssertionKind::Cover,
                        deferred,
                        scope: self.current_assertion_scope(),
                        cond: condition,
                        pass_action,
                        fail_action,
                    }),
                    other => Err(format!(
                        "unsupported immediate assertion kind {other:?}"
                    )),
                }
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
                if let Some(statement) = self.lower_covergroup_construction(expression)? {
                    return Ok(statement);
                }
                self.lower_assignment(expression)
            }
            "ProceduralAssign" => {
                let assignment = statement
                    .get("assignment")
                    .ok_or_else(|| "procedural assignment statement has no assignment".to_string())?;
                let left = assignment
                    .get("left")
                    .ok_or_else(|| "procedural assignment statement has no left side".to_string())?;
                let right = assignment
                    .get("right")
                    .ok_or_else(|| "procedural assignment statement has no right side".to_string())?;
                let is_force = bool_field(statement, "isForce");
                if string_field(left, "kind").as_deref() == Some("Concatenation")
                {
                    return self.lower_procedural_concatenation_assign(left, right, is_force);
                }
                let lv = self.lower_lvalue(left)?;
                let e = self.lower_expression(right)?;
                if is_force {
                    Ok(Stmt::Force { lv, e })
                } else {
                    validate_static_procedural_assignment_lvalue(left, &lv)?;
                    validate_procedural_driver_rhs(&e)?;
                    Ok(Stmt::ProceduralAssign { lv, e })
                }
            }
            "ProceduralDeassign" => {
                let left = statement
                    .get("lvalue")
                    .ok_or_else(|| "procedural deassignment statement has no lvalue".to_string())?;
                let is_release = bool_field(statement, "isRelease");
                if string_field(left, "kind").as_deref() == Some("Concatenation")
                {
                    return self.lower_procedural_concatenation_deassign(left, is_release);
                }
                let lv = self.lower_lvalue(left)?;
                if is_release {
                    Ok(Stmt::Release { lv })
                } else {
                    validate_static_procedural_assignment_lvalue(left, &lv)?;
                    Ok(Stmt::ProceduralDeassign { lv })
                }
            }
            "Assignment" => {
                if let Some(statement) = self.lower_covergroup_construction(statement)? {
                    return Ok(statement);
                }
                self.lower_assignment(statement)
            }
            "VariableDeclaration" => Err(
                "direct slang-AST procedure locals are not supported; declare loop variables in module scope"
                    .into(),
            ),
            "Disable" => self.lower_named_disable(statement),
            "DisableFork" => Ok(Stmt::DisableFork),
            "WaitFork" => Ok(Stmt::WaitFork),
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
            "ForeachLoop" => self.lower_foreach_loop(statement),
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
        if string_field(expression, "kind").as_deref() == Some("RealLiteral") {
            let real = expression
                .get("value")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| "fractional delay control has no numeric value".to_string())?;
            return self.scale_delay_constant(super::DelayConstant::Real(real));
        }
        if string_field(expression, "kind").as_deref() == Some("UnaryOp")
            && string_field(expression, "op").as_deref() == Some("Minus")
            && expression
                .get("operand")
                .and_then(|operand| operand.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("RealLiteral")
        {
            let real = expression
                .get("operand")
                .and_then(|operand| operand.get("value"))
                .and_then(serde_json::Value::as_f64)
                .map(|value| -value)
                .ok_or_else(|| "fractional delay control has no numeric value".to_string())?;
            return self.scale_delay_constant(super::DelayConstant::Real(real));
        }
        let delay = slang_constant_u64(expression)
            .map_err(|_| "delay control must be an integer or finite real constant".to_string())?;
        self.scale_delay_constant(super::DelayConstant::Integer(delay))
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
        if let Some(statement) = self.lower_file_error_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_file_read_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_file_scan_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_file_gets_assignment(assignment, left, right)? {
            return Ok(statement);
        }
        if let Some(statement) = self.lower_seeded_random_assignment(assignment, left, right)? {
            return Ok(statement);
        }
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
            let expression = if string_field(left, "type")
                .as_deref()
                .is_some_and(super::expressions::type_is_realtime_compatible)
            {
                self.lower_real_assignment_expression(left, right)?
            } else {
                self.lower_expression(right)?
            };
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
        let expression = self.lower_real_assignment_expression(left, right)?;
        if bool_field(assignment, "isNonBlocking") {
            Ok(Stmt::NBl { lv, e: expression })
        } else {
            Ok(Stmt::Bl { lv, e: expression })
        }
    }

    fn lower_file_gets_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        mut right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        while string_field(right, "kind").as_deref() == Some("Conversion") {
            right = right
                .get("operand")
                .ok_or_else(|| "conversion has no operand".to_string())?;
        }
        if string_field(right, "kind").as_deref() != Some("Call")
            || string_field(right, "subroutine").as_deref() != Some("$fgets")
        {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("nonblocking $fgets assignment is not supported".into());
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Err("$fgets requires one string output and one descriptor".into());
        }
        let Some(output) = arguments.first() else {
            return Err("$fgets has no string output".into());
        };
        if string_field(output, "kind").as_deref() != Some("Assignment")
            || output
                .get("right")
                .and_then(|value| string_field(value, "kind"))
                .as_deref()
                != Some("EmptyArgument")
        {
            return Err("$fgets first argument must be its string output".into());
        }
        let Some(string_source) = output.get("left") else {
            return Err("$fgets string output has no target".into());
        };
        let Some(descriptor_source) = arguments.get(1) else {
            return Err("$fgets has no descriptor".into());
        };
        let string = self.lower_lvalue(string_source)?;
        if string.decl_width != crate::types::STRING_WIDTH {
            return Err("$fgets requires a 512-bit string target".into());
        }
        let status = self.lower_lvalue(left)?;
        if status.decl_width > 32 {
            return Err("$fgets status target exceeds 32 bits".into());
        }
        let fd = self.lower_expression(descriptor_source)?;
        Ok(Some(eda_core::ir::Stmt::FileGets { fd, string, status }))
    }

    fn lower_file_error_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let mut right = right;
        while string_field(right, "kind").as_deref() == Some("Conversion") {
            right = right
                .get("operand")
                .ok_or_else(|| "conversion has no operand".to_string())?;
        }
        if string_field(right, "kind").as_deref() != Some("Call")
            || string_field(right, "subroutine").as_deref() != Some("$ferror")
        {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("nonblocking $ferror assignment is not supported".into());
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Err("$ferror requires a descriptor and string output".into());
        }
        let Some(descriptor_source) = arguments.first() else {
            return Err("$ferror has no descriptor".into());
        };
        let Some(output) = arguments.get(1) else {
            return Err("$ferror has no string output".into());
        };
        if string_field(output, "kind").as_deref() != Some("Assignment")
            || output
                .get("right")
                .and_then(|value| string_field(value, "kind"))
                .as_deref()
                != Some("EmptyArgument")
        {
            return Err("$ferror second argument must be its string output".into());
        }
        let Some(message_source) = output.get("left") else {
            return Err("$ferror string output has no target".into());
        };
        let message = self.lower_lvalue(message_source)?;
        if message.decl_width != crate::types::STRING_WIDTH {
            return Err("$ferror requires a 512-bit string target".into());
        }
        let status = self.lower_lvalue(left)?;
        if status.decl_width > 32 {
            return Err("$ferror status target exceeds 32 bits".into());
        }
        let fd = self.lower_expression(descriptor_source)?;
        Ok(Some(eda_core::ir::Stmt::FileError {
            fd,
            message,
            status,
        }))
    }

    fn lower_file_read_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let mut right = right;
        while string_field(right, "kind").as_deref() == Some("Conversion") {
            right = right
                .get("operand")
                .ok_or_else(|| "conversion has no operand".to_string())?;
        }
        if string_field(right, "kind").as_deref() != Some("Call")
            || string_field(right, "subroutine").as_deref() != Some("$fread")
        {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("nonblocking $fread assignment is not supported".into());
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() < 2 || arguments.len() > 4 {
            return Err(
                "$fread requires a target, descriptor, and optional start/count".to_string(),
            );
        }
        let Some(output) = arguments.first() else {
            return Err("$fread has no target".into());
        };
        if string_field(output, "kind").as_deref() != Some("Assignment")
            || output
                .get("right")
                .and_then(|value| string_field(value, "kind"))
                .as_deref()
                != Some("EmptyArgument")
        {
            return Err("$fread first argument must be its output target".into());
        }
        let Some(target_source) = output.get("left") else {
            return Err("$fread output has no target".into());
        };
        let Some(descriptor_source) = arguments.get(1) else {
            return Err("$fread has no descriptor".into());
        };
        let memory_symbol = self.named_symbol(target_source)?;
        if let Some(memory) = memory_symbol.memory {
            let declaration = &self.design.memories[memory];
            if declaration.dynamic {
                return Err("dynamic-array $fread targets are not supported".into());
            }
            if declaration.queue {
                return Err("queue $fread targets are not supported".into());
            }
            if declaration.associative {
                return Err("associative-array $fread targets are not supported".into());
            }
            if declaration.dimensions.len() != 1 {
                return Err(
                    "$fread memory targets currently require one unpacked dimension".into(),
                );
            }
            if declaration.element_width == 0 || !declaration.element_width.is_multiple_of(8) {
                return Err("$fread memory elements must contain whole bytes".into());
            }

            let start = arguments
                .get(2)
                .map(|source| self.lower_expression(source))
                .transpose()?;
            let count = arguments
                .get(3)
                .map(|source| self.lower_expression(source))
                .transpose()?;
            let status = self.lower_lvalue(left)?;
            if status.decl_width > 32 {
                return Err("$fread status target exceeds 32 bits".into());
            }
            let fd = self.lower_expression(descriptor_source)?;
            return Ok(Some(eda_core::ir::Stmt::FileReadMemory {
                fd,
                memory,
                start,
                count,
                status,
            }));
        }

        if arguments.len() > 2 {
            return Err("optional $fread start/count arguments require a memory target".into());
        }

        let target = self.lower_lvalue(target_source)?;
        if target.decl_width == 0 || target.decl_width % 8 != 0 {
            return Err("scalar $fread requires a whole-byte integral target".into());
        }
        if target.decl_width > 512 {
            return Err("scalar $fread targets currently support at most 512 bits".into());
        }
        let status = self.lower_lvalue(left)?;
        if status.decl_width > 32 {
            return Err("$fread status target exceeds 32 bits".into());
        }
        let fd = self.lower_expression(descriptor_source)?;
        Ok(Some(eda_core::ir::Stmt::FileRead { fd, target, status }))
    }

    fn lower_seeded_random_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        mut right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        while string_field(right, "kind").as_deref() == Some("Conversion") {
            right = right
                .get("operand")
                .ok_or_else(|| "conversion has no operand".to_string())?;
        }
        if string_field(right, "kind").as_deref() != Some("Call")
            || string_field(right, "subroutine").as_deref() != Some("$random")
        {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err("nonblocking seeded $random assignment is not supported".into());
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 1 {
            return Err("seeded $random requires exactly one inout seed".into());
        }
        let Some(seed_source) = arguments.first() else {
            return Err("seeded $random has no inout seed".into());
        };
        let seed = self.lower_lvalue(seed_source)?;
        if seed.decl_width != 32 || seed.range.width() != 32 {
            return Err("seeded $random requires a whole 32-bit seed lvalue".into());
        }
        let destination = self.lower_lvalue(left)?;
        if destination.decl_width > 32 {
            return Err("seeded $random destination exceeds 32 bits".into());
        }
        if destination.net == seed.net && destination.range == seed.range {
            return Err("seeded $random destination and inout seed must be distinct".into());
        }
        Ok(Some(eda_core::ir::Stmt::RandomSeeded { seed, destination }))
    }

    fn lower_file_scan_assignment(
        &self,
        assignment: &serde_json::Value,
        left: &serde_json::Value,
        right: &serde_json::Value,
    ) -> Result<Option<eda_core::ir::Stmt>, String> {
        let mut right = right;
        while string_field(right, "kind").as_deref() == Some("Conversion") {
            right = right
                .get("operand")
                .ok_or_else(|| "conversion has no operand".to_string())?;
        }
        if string_field(right, "kind").as_deref() != Some("Call") {
            return Ok(None);
        }
        let Some(task_name) = string_field(right, "subroutine") else {
            return Ok(None);
        };
        if !matches!(task_name.as_str(), "$fscanf" | "$sscanf") {
            return Ok(None);
        }
        if bool_field(assignment, "isNonBlocking") {
            return Err(format!(
                "nonblocking {task_name} assignment is not supported"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(serde_json::Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() < 2 {
            return Err(format!("{task_name} requires input and a format string"));
        }
        let Some(input_source) = arguments.first() else {
            return Err(format!("{task_name} has no input expression"));
        };
        let format_source = arguments
            .get(1)
            .ok_or_else(|| format!("{task_name} has no format string"))?;
        let format = string_field(format_source, "literal")
            .ok_or_else(|| format!("{task_name} format must be a string literal"))?;
        let specifiers = Self::file_scan_specifiers(&format, &task_name)?;
        let conversion_count = specifiers.len();
        if arguments.len() != conversion_count + 2 {
            return Err(format!(
                "{task_name} format has {conversion_count} conversions but {} output arguments",
                arguments.len().saturating_sub(2)
            ));
        }
        let mut targets = Vec::with_capacity(conversion_count);
        for (output, expected_specifier) in arguments.iter().skip(2).zip(specifiers.iter()) {
            if string_field(output, "kind").as_deref() != Some("Assignment")
                || output
                    .get("right")
                    .and_then(|value| string_field(value, "kind"))
                    .as_deref()
                    != Some("EmptyArgument")
            {
                return Err(format!(
                    "{task_name} output arguments must be lvalue outputs"
                ));
            }
            let Some(target_source) = output.get("left") else {
                return Err(format!("{task_name} output has no target"));
            };
            let target = self.lower_lvalue(target_source)?;
            let target_is_string = string_field(output, "type").as_deref() == Some("string");
            if (expected_specifier.0 == 's') != target_is_string {
                return Err(format!(
                    "{task_name} conversion '%{}' does not match its target type",
                    expected_specifier.0
                ));
            }
            if target.range != full_range(target.decl_width)
                || (!target_is_string && target.decl_width > 64)
                || (target_is_string && target.decl_width != crate::types::STRING_WIDTH)
            {
                return Err(
                    format!("{task_name} targets must be whole integer declarations up to 64 bits or bounded strings"),
                );
            }
            targets.push(target);
        }
        let status = self.lower_lvalue(left)?;
        if status.decl_width > 32 {
            return Err(format!("{task_name} status target exceeds 32 bits"));
        }
        if task_name == "$fscanf" {
            let fd = self.lower_expression(input_source)?;
            return Ok(Some(eda_core::ir::Stmt::FileScan {
                fd,
                format,
                targets,
                status,
            }));
        }

        let source = self.lower_expression(input_source)?;
        if source.width() != crate::types::STRING_WIDTH {
            return Err("$sscanf input must be a 512-bit string expression".into());
        }
        Ok(Some(eda_core::ir::Stmt::StringScan {
            source,
            format,
            targets,
            status,
        }))
    }

    fn file_scan_specifiers(
        format: &str,
        task_name: &str,
    ) -> Result<Vec<(char, Option<usize>)>, String> {
        let mut conversions = Vec::new();
        let mut characters = format.chars();
        while let Some(character) = characters.next() {
            if character != '%' {
                continue;
            }
            let mut width = String::new();
            while matches!(characters.clone().next(), Some(c) if c.is_ascii_digit()) {
                width.push(characters.next().expect("peeked width digit"));
            }
            let field_width = if width.is_empty() {
                None
            } else {
                let parsed = width
                    .parse::<usize>()
                    .map_err(|_| format!("{task_name} width is too large"))?;
                if parsed == 0 || parsed > 64 {
                    return Err(format!("{task_name} width must be 1..=64"));
                }
                Some(parsed)
            };
            match characters.next() {
                Some(specifier @ ('d' | 'h' | 'o' | 'b' | 's')) => {
                    conversions.push((specifier, field_width))
                }
                Some('%') if field_width.is_none() => {}
                Some('%') => {
                    return Err(format!("{task_name} literal % cannot take a width"));
                }
                Some(other) => {
                    return Err(format!(
                        "{task_name} conversion '%{other}' is unsupported; only %d/%h/%o/%b/%s, optional width 1..=64, and %% execute"
                ))
                }
                None => return Err("$fscanf format ends with an incomplete conversion".into()),
            }
        }
        Ok(conversions)
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

        let value = self.lower_real_assignment_expression(left, right)?;
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
        Expr::Const(_)
        | Expr::RealConst(_)
        | Expr::RealRef { .. }
        | Expr::RealBinary { .. }
        | Expr::RealCompare { .. }
        | Expr::RealNeg(_)
        | Expr::RealToInt { .. }
        | Expr::RealTruncToInt { .. }
        | Expr::IntToReal { .. }
        | Expr::RealTime { .. }
        | Expr::ShortRealConst(_)
        | Expr::ShortRealRef { .. }
        | Expr::ShortRealBinary { .. }
        | Expr::ShortRealNeg(_)
        | Expr::ShortRealToReal(_)
        | Expr::RealToShortReal(_)
        | Expr::BitsToReal(_)
        | Expr::BitsToShortReal(_)
        | Expr::ProcessSelf
        | Expr::EventTriggered { .. }
        | Expr::SimulationTime { .. } => {}
        Expr::Past { value, gating, .. } => {
            collect_expression_references(value, references);
            if let Some(gating) = gating {
                collect_expression_references(gating, references);
            }
        }
        Expr::Sampled(value) => collect_expression_references(value, references),
        Expr::SampledValue { value, .. } => collect_expression_references(value, references),
        Expr::Ref { .. } => references.push(expression.clone()),
        Expr::ProcessStatus(handle) => collect_expression_references(handle, references),
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
        Expr::StringFormat { args, .. } => args
            .iter()
            .for_each(|argument| collect_expression_references(argument, references)),
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
        Expr::MemoryPattern { .. } => {}
        Expr::EnumPattern { value, .. } => collect_expression_references(value, references),
        Expr::StructPattern { value, .. } => collect_expression_references(value, references),
        Expr::Random { args, .. } => args
            .iter()
            .for_each(|argument| collect_expression_references(argument, references)),
        Expr::CountBits { value, .. }
        | Expr::IsUnknown(value)
        | Expr::OneHot(value)
        | Expr::OneHot0(value) => collect_expression_references(value, references),
        Expr::FileOpen { .. } => {}
        Expr::FileOpenDynamic { path, mode } => {
            collect_expression_references(path, references);
            collect_expression_references(mode, references);
        }
        Expr::FileGetc { fd } => collect_expression_references(fd, references),
        Expr::FileUngetc { value, fd } => {
            collect_expression_references(value, references);
            collect_expression_references(fd, references);
        }
        Expr::FileEof { fd } => collect_expression_references(fd, references),
        Expr::FileTell { fd } => collect_expression_references(fd, references),
        Expr::FileSeek { fd, offset, whence } => {
            collect_expression_references(fd, references);
            collect_expression_references(offset, references);
            collect_expression_references(whence, references);
        }
    }
}

fn definite_true(expression: &eda_core::ir::Expr) -> eda_core::ir::Expr {
    eda_core::ir::Expr::Bin {
        op: eda_core::ir::BinOp::CaseEq,
        a: Box::new(expression.clone()),
        b: Box::new(eda_core::ir::Expr::Const(LogicWord::ones(1))),
    }
}

fn validate_static_procedural_assignment_lvalue(
    source: &serde_json::Value,
    lv: &eda_core::ir::LValue,
) -> Result<(), String> {
    if !matches!(
        string_field(source, "kind").as_deref(),
        Some("NamedValue" | "HierarchicalValue")
    ) {
        return Err(
            "direct procedural continuous assignment supports only whole packed-variable lvalues"
                .into(),
        );
    }
    if lv.range.width() != lv.decl_width {
        return Err(
            "direct procedural continuous assignment does not support part-select/member lvalues"
                .into(),
        );
    }
    Ok(())
}

fn validate_procedural_driver_rhs(expression: &eda_core::ir::Expr) -> Result<(), String> {
    if matches!(
        expression,
        eda_core::ir::Expr::EventTriggered { .. }
            | eda_core::ir::Expr::SimulationTime { .. }
            | eda_core::ir::Expr::RealTime { .. }
    ) {
        return Ok(());
    }
    if procedural_rhs_has_untracked_dependency(expression) {
        return Err(
            "procedural continuous assignment RHS has runtime state not tracked by the driver model"
                .into(),
        );
    }
    Ok(())
}

fn procedural_rhs_has_untracked_dependency(expression: &eda_core::ir::Expr) -> bool {
    use eda_core::ir::Expr;
    match expression {
        Expr::EventTriggered { .. }
        | Expr::SimulationTime { .. }
        | Expr::Random { .. }
        | Expr::FileOpen { .. }
        | Expr::FileOpenDynamic { .. }
        | Expr::FileGetc { .. }
        | Expr::FileUngetc { .. }
        | Expr::FileEof { .. }
        | Expr::FileTell { .. }
        | Expr::FileSeek { .. }
        | Expr::RealTime { .. } => true,
        Expr::Un { a, .. }
        | Expr::Signed { e: a, .. }
        | Expr::StringLength { e: a }
        | Expr::StringCase { e: a, .. }
        | Expr::StringToRadix { e: a, .. }
        | Expr::RealNeg(a)
        | Expr::ShortRealNeg(a)
        | Expr::ShortRealToReal(a)
        | Expr::RealToShortReal(a)
        | Expr::BitsToReal(a)
        | Expr::BitsToShortReal(a)
        | Expr::RealToInt { value: a, .. }
        | Expr::RealTruncToInt { value: a, .. }
        | Expr::IntToReal { value: a, .. }
        | Expr::StringFromRadix { value: a, .. }
        | Expr::EnumPattern { value: a, .. }
        | Expr::StructPattern { value: a, .. } => procedural_rhs_has_untracked_dependency(a),
        Expr::Bin { a, b, .. }
        | Expr::StringConcat { left: a, right: b }
        | Expr::StringCompare {
            left: a, right: b, ..
        }
        | Expr::RealBinary {
            left: a, right: b, ..
        }
        | Expr::RealCompare {
            left: a, right: b, ..
        }
        | Expr::ShortRealBinary {
            left: a, right: b, ..
        } => {
            procedural_rhs_has_untracked_dependency(a) || procedural_rhs_has_untracked_dependency(b)
        }
        Expr::Cond { c, t, f } => {
            procedural_rhs_has_untracked_dependency(c)
                || procedural_rhs_has_untracked_dependency(t)
                || procedural_rhs_has_untracked_dependency(f)
        }
        Expr::Concat(parts) => parts.iter().any(procedural_rhs_has_untracked_dependency),
        Expr::Repeat { e, .. } => procedural_rhs_has_untracked_dependency(e),
        Expr::Index { e, .. } => procedural_rhs_has_untracked_dependency(e),
        Expr::StringGetByte { e, index } => {
            procedural_rhs_has_untracked_dependency(e)
                || procedural_rhs_has_untracked_dependency(index)
        }
        Expr::StringPutc { e, index, value } => {
            procedural_rhs_has_untracked_dependency(e)
                || procedural_rhs_has_untracked_dependency(index)
                || procedural_rhs_has_untracked_dependency(value)
        }
        Expr::StringSubstr { e, left, right } => {
            procedural_rhs_has_untracked_dependency(e)
                || procedural_rhs_has_untracked_dependency(left)
                || procedural_rhs_has_untracked_dependency(right)
        }
        Expr::StringFormat { args, .. } => args.iter().any(procedural_rhs_has_untracked_dependency),
        _ => false,
    }
}
