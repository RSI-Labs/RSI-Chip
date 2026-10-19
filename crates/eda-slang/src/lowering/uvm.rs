//! Minimal direct-lowering bootstrap for official-UVM `run_test`.

use super::classes::{class_expr, class_lvalue, method_return_type};
use super::SlangAstLowerer;
use crate::ast_util::{bool_field, slang_constant_u64, string_field, with_source_span};
use eda_core::ir::{BinOp, Expr, Stmt};
use eda_core::logic::LogicWord;
use serde_json::{json, Value};
use std::collections::BTreeMap;

const UVM_BOOTSTRAP_PHASES: [&str; 9] = [
    "build_phase",
    "connect_phase",
    "end_of_elaboration_phase",
    "start_of_simulation_phase",
    "run_phase",
    "extract_phase",
    "check_phase",
    "report_phase",
    "final_phase",
];
const UVM_BOOTSTRAP_OBJECTION_TIMEOUT: u64 = 1_000;

impl SlangAstLowerer {
    pub(super) fn prepare_run_test_objects(&mut self, body: &Value) -> Result<(), String> {
        let mut calls = Vec::new();
        collect_run_test_calls(body, &mut calls);
        let mut type_alias_calls = Vec::new();
        collect_uvm_factory_type_alias_calls(body, &mut type_alias_calls);
        let mut instance_alias_calls = Vec::new();
        collect_uvm_factory_instance_alias_calls(body, &mut instance_alias_calls);
        let mut override_calls = Vec::new();
        collect_uvm_factory_override_calls(body, &mut override_calls);
        let mut instance_override_calls = Vec::new();
        collect_uvm_factory_instance_override_calls(body, &mut instance_override_calls);
        let mut timeout_calls = Vec::new();
        collect_uvm_timeout_calls(body, &mut timeout_calls);
        for alias_call in &type_alias_calls {
            let (alias_name, original_name) = uvm_factory_type_alias_names(alias_call)?;
            let run_test_location = calls
                .first()
                .and_then(|call| source_location(call))
                .ok_or_else(|| {
                    with_source_span(
                        alias_call,
                        "UVM type alias requires a following run_test call".into(),
                    )
                })?;
            let alias_location = source_location(alias_call).ok_or_else(|| {
                with_source_span(alias_call, "UVM type alias has no source location".into())
            })?;
            if alias_location >= run_test_location {
                return Err(with_source_span(
                    alias_call,
                    "UVM type alias must be registered before run_test".into(),
                ));
            }
            let original_name = self.resolve_uvm_type_alias(&original_name)?;
            if let Some(existing) = self
                .uvm_factory_type_aliases
                .iter()
                .find(|existing| existing.alias_name == alias_name)
            {
                if existing.original_name != original_name {
                    return Err(with_source_span(
                        alias_call,
                        format!("conflicting UVM type alias '{alias_name}'"),
                    ));
                }
            } else {
                self.uvm_factory_type_aliases.push(super::AstUvmTypeAlias {
                    alias_name,
                    original_name,
                });
            }
        }
        for alias_call in &instance_alias_calls {
            let (alias_name, original_name, pattern) =
                uvm_factory_instance_alias_names(alias_call)?;
            let run_test_location = calls
                .first()
                .and_then(|call| source_location(call))
                .ok_or_else(|| {
                    with_source_span(
                        alias_call,
                        "UVM instance alias requires a following run_test call".into(),
                    )
                })?;
            let alias_location = source_location(alias_call).ok_or_else(|| {
                with_source_span(
                    alias_call,
                    "UVM instance alias has no source location".into(),
                )
            })?;
            if alias_location >= run_test_location {
                return Err(with_source_span(
                    alias_call,
                    "UVM instance alias must be registered before run_test".into(),
                ));
            }
            let original_name = self.resolve_uvm_type_alias(&original_name)?;
            if let Some(existing) = self
                .uvm_factory_instance_aliases
                .iter()
                .find(|existing| existing.alias_name == alias_name && existing.pattern == pattern)
            {
                if existing.original_name != original_name {
                    return Err(with_source_span(
                        alias_call,
                        format!("conflicting UVM instance alias '{alias_name}'"),
                    ));
                }
            } else {
                self.uvm_factory_instance_aliases
                    .push(super::AstUvmInstanceAlias {
                        alias_name,
                        original_name,
                        pattern,
                    });
            }
        }
        for override_call in &override_calls {
            let (original, override_name, replace) = uvm_factory_override_names(override_call)?;
            let run_test_location = calls
                .first()
                .and_then(|call| source_location(call))
                .ok_or_else(|| {
                    with_source_span(
                        override_call,
                        "UVM factory override requires a following run_test call".into(),
                    )
                })?;
            let override_location = source_location(override_call).ok_or_else(|| {
                with_source_span(
                    override_call,
                    "UVM factory override has no source location".into(),
                )
            })?;
            if override_location >= run_test_location {
                return Err(with_source_span(
                    override_call,
                    "UVM factory override must be registered before run_test".into(),
                ));
            }
            if original == override_name {
                continue;
            }
            let original = self.resolve_uvm_type_alias(&original)?;
            let override_name = self.resolve_uvm_type_alias(&override_name)?;
            if let Some(existing) = self
                .uvm_factory_overrides
                .iter_mut()
                .find(|existing| existing.original == original)
            {
                if replace {
                    existing.override_name = override_name;
                    existing.replace = true;
                }
            } else {
                self.uvm_factory_overrides.insert(
                    0,
                    super::AstUvmTypeOverride {
                        original,
                        override_name,
                        replace,
                    },
                );
            }
        }
        for override_call in &instance_override_calls {
            let (original, override_name, instance_path) =
                uvm_factory_instance_override_names(override_call)?;
            let run_test_location = calls
                .first()
                .and_then(|call| source_location(call))
                .ok_or_else(|| {
                    with_source_span(
                        override_call,
                        "UVM instance override requires a following run_test call".into(),
                    )
                })?;
            let override_location = source_location(override_call).ok_or_else(|| {
                with_source_span(
                    override_call,
                    "UVM instance override has no source location".into(),
                )
            })?;
            if override_location >= run_test_location {
                return Err(with_source_span(
                    override_call,
                    "UVM instance override must be registered before run_test".into(),
                ));
            }
            let original = self.resolve_uvm_instance_alias(&original, &instance_path)?;
            let already_registered = self.uvm_factory_instance_overrides.iter().any(|existing| {
                existing.pattern == instance_path
                    && existing.original == original
                    && existing.override_name == override_name
            });
            if !already_registered {
                self.uvm_factory_instance_overrides
                    .push(super::AstUvmInstanceOverride {
                        pattern: instance_path.clone(),
                        original: original.clone(),
                        override_name: override_name.clone(),
                    });
            }
        }
        let mut objection_timeout = UVM_BOOTSTRAP_OBJECTION_TIMEOUT;
        for timeout_call in &timeout_calls {
            let run_test_location = calls
                .first()
                .and_then(|call| source_location(call))
                .ok_or_else(|| {
                    with_source_span(
                        timeout_call,
                        "UVM timeout configuration requires a following run_test call".into(),
                    )
                })?;
            let timeout_location = source_location(timeout_call).ok_or_else(|| {
                with_source_span(
                    timeout_call,
                    "UVM timeout configuration has no source location".into(),
                )
            })?;
            if timeout_location >= run_test_location {
                return Err(with_source_span(
                    timeout_call,
                    "UVM timeout must be configured before run_test".into(),
                ));
            }
            objection_timeout = uvm_timeout_value(timeout_call)?;
        }
        for call in calls {
            if !self.run_test_objects.is_empty() {
                return Err(with_source_span(
                    call,
                    "only one direct-lowering run_test call is supported per module".into(),
                ));
            }
            let requested_class_name = run_test_class_name(call)?;
            let requested_class_name = self.resolve_uvm_type_alias(&requested_class_name)?;
            let class_name = self.resolve_global_uvm_factory_class(&requested_class_name)?;
            let requested_class_key = self.class_type_key(&requested_class_name);
            let class_key = self.class_type_key(&class_name);
            if !self.class_definitions.contains_key(&class_key) {
                return Err(with_source_span(
                    call,
                    format!("run_test class '{class_name}' is unknown"),
                ));
            }
            let chain = self
                .class_chain(&class_key)
                .map_err(|error| with_source_span(call, error))?;
            if class_name != requested_class_name
                && !chain.iter().any(|(owner, _)| {
                    owner == &requested_class_key
                        || owner.split('@').next() == requested_class_key.split('@').next()
                })
            {
                return Err(with_source_span(
                    call,
                    format!(
                        "UVM factory override '{class_name}' is not derived from '{requested_class_name}'"
                    ),
                ));
            }
            let Some(official_index) = official_uvm_test_index(&chain) else {
                return Err(with_source_span(
                    call,
                    format!("run_test class '{class_name}' must derive from official uvm_test"),
                ));
            };
            let Some(run_phase) = user_phase_method(&chain, official_index, "run_phase") else {
                return Err(with_source_span(
                    call,
                    format!("run_test class '{class_name}' has no run_phase"),
                ));
            };
            if let Err(error) = validate_bootstrap_phase_method(run_phase, "run_phase") {
                return Err(with_source_span(
                    call,
                    format!("run_test class '{class_name}' run_phase is invalid: {error}"),
                ));
            }
            let constructor = chain.iter().find_map(|(_, class)| {
                let empty_members = Vec::new();
                class
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members)
                    .iter()
                    .find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some("new")
                    })
                    .cloned()
            });
            let Some(constructor) = constructor else {
                return Err(with_source_span(
                    call,
                    format!("run_test class '{class_name}' has no explicit constructor"),
                ));
            };
            let empty_arguments = Vec::new();
            let formals = constructor
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if formals
                .iter()
                .any(|formal| formal.get("defaultValue").is_none())
            {
                return Err(with_source_span(
                    call,
                    format!(
                        "run_test class '{class_name}' constructor arguments must have defaults"
                    ),
                ));
            }
            let root_name = formals
                .first()
                .and_then(|formal| formal.get("defaultValue").cloned())
                .and_then(|default| string_field(unwrap_conversion(&default), "literal"))
                .ok_or_else(|| {
                    with_source_span(
                        call,
                        format!(
                            "run_test class '{class_name}' constructor requires a literal name default"
                        ),
                    )
                })?;

            let handle_address = self.next_synthetic_class_handle;
            self.next_synthetic_class_handle = self
                .next_synthetic_class_handle
                .checked_sub(1)
                .ok_or_else(|| "synthetic class-handle address space exhausted".to_string())?;
            let object_name = format!(
                "__uvm_run_test_{}",
                class_name
                    .chars()
                    .map(|character| {
                        if character.is_ascii_alphanumeric() {
                            character
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>()
            );
            let synthetic_member = json!({
                "name": object_name,
                "kind": "Variable",
                "type": class_key,
                "lifetime": "Static",
                "addr": handle_address,
            });
            if !self.declare_class_object(&synthetic_member, &object_name)? {
                return Err(with_source_span(
                    call,
                    format!("run_test cannot allocate class '{class_name}'"),
                ));
            }
            let object_key = format!("{handle_address} {object_name}");
            let phase_key = format!("{handle_address} __uvm_phase");
            let objection_count = self.declare_class_internal_net(
                &format!("__uvm_run_test_objections_{handle_address}"),
                32,
            );
            let objection_drained = self.declare_class_internal_net(
                &format!("__uvm_objections_drained_{handle_address}"),
                1,
            );
            let owner_keys = self.uvm_objection_owner_keys(&object_key);
            let mut objection_owner_counts = BTreeMap::new();
            for (owner_index, owner_key) in owner_keys.into_iter().enumerate() {
                let owner_address = owner_key
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .replace(['[', ']'], "_");
                let count = self.declare_class_internal_net(
                    &format!("__uvm_objection_owner_{owner_address}_{owner_index}"),
                    32,
                );
                objection_owner_counts.insert(owner_key, count);
            }
            self.initialized_class_objects.insert(object_key.clone());
            self.run_test_objects.insert(
                requested_class_name,
                super::AstRunTestObject {
                    class_name: class_key,
                    root_name,
                    object_key,
                    phase_key,
                    objection_count,
                    objection_drained,
                    objection_timeout,
                    objection_owner_counts,
                },
            );
        }
        Ok(())
    }

    pub(super) fn lower_run_test_call(&self, value: &Value) -> Result<Option<Stmt>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        if subroutine.split_whitespace().next_back() != Some("run_test") {
            return Ok(None);
        }
        let class_name = run_test_class_name(value)?;
        let Some(run_object) = self.run_test_objects.get(&class_name) else {
            return Err(format!(
                "run_test object for class '{class_name}' was not prepared"
            ));
        };
        let chain = self.class_chain(&run_object.class_name)?;
        let constructor = chain
            .iter()
            .find_map(|(_, class)| {
                let empty_members = Vec::new();
                class
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members)
                    .iter()
                    .find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some("new")
                    })
                    .cloned()
            })
            .ok_or_else(|| format!("run_test class '{class_name}' has no explicit constructor"))?;
        let empty_arguments = Vec::new();
        let formals = constructor
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let actuals = formals
            .iter()
            .map(|formal| {
                formal.get("defaultValue").cloned().ok_or_else(|| {
                    format!("run_test class '{class_name}' constructor argument has no default")
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let constructor_address = constructor
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("run_test class '{class_name}' constructor has no address"))?;
        let construction = json!({
            "kind": "Assignment",
            "left": {
                "kind": "NamedValue",
                "type": run_object.class_name,
                "symbol": run_object.object_key,
            },
            "right": {
                "kind": "NewClass",
                "type": run_object.class_name,
                "isSuperClass": false,
                "constructorCall": {
                    "kind": "Call",
                    "subroutine": format!("{constructor_address} new"),
                    "arguments": actuals,
                },
            },
            "isNonBlocking": false,
        });
        let Some(construction) = self.lower_class_constructor(&construction)? else {
            return Err(format!("run_test failed to construct class '{class_name}'"));
        };

        let chain = self.class_chain(&run_object.class_name)?;
        let official_index = official_uvm_test_index(&chain)
            .ok_or_else(|| format!("run_test class '{class_name}' is not official UVM"))?;
        let mut statements = vec![construction];
        statements.push(Stmt::Bl {
            lv: class_lvalue(&run_object.objection_count),
            e: Expr::Const(LogicWord::zero(32)),
        });
        statements.push(Stmt::Bl {
            lv: class_lvalue(&run_object.objection_drained),
            e: Expr::Const(LogicWord::zero(1)),
        });
        for owner_count in run_object.objection_owner_counts.values() {
            statements.push(Stmt::Bl {
                lv: class_lvalue(owner_count),
                e: Expr::Const(LogicWord::zero(32)),
            });
        }
        for phase_name in UVM_BOOTSTRAP_PHASES {
            let Some(method) = user_phase_method(&chain, official_index, phase_name) else {
                if phase_name == "run_phase" {
                    return Err(format!("run_test class '{class_name}' has no run_phase"));
                }
                continue;
            };
            validate_bootstrap_phase_method(method, phase_name).map_err(|error| {
                format!("run_test class '{class_name}' {phase_name} is invalid: {error}")
            })?;
            let method_address = method.get("addr").and_then(Value::as_u64).ok_or_else(|| {
                format!("run_test class '{class_name}' {phase_name} has no address")
            })?;
            let phase_call = json!({
                "kind": "Call",
                "subroutine": format!("{method_address} {phase_name}"),
                "thisClass": {
                    "kind": "NamedValue",
                    "type": run_object.class_name,
                    "symbol": run_object.object_key,
                },
                "arguments": [
                    {
                        "kind": "NamedValue",
                        "type": "uvm_phase",
                        "symbol": run_object.phase_key,
                    },
                ],
            });
            let Some(phase_statement) = self.lower_class_task_call(&phase_call)? else {
                return Err(format!(
                    "run_test failed to execute {phase_name} for class '{class_name}'"
                ));
            };
            statements.push(phase_statement);
            if phase_name == "run_phase" {
                let mut drain_checks = vec![Stmt::Bl {
                    lv: class_lvalue(&run_object.objection_drained),
                    e: Expr::Const(LogicWord::from_u64(1, 1)),
                }];
                for owner_count in run_object.objection_owner_counts.values() {
                    drain_checks.push(Stmt::Assert {
                        cond: Expr::Bin {
                            op: BinOp::Eq,
                            a: Box::new(class_expr(owner_count)),
                            b: Box::new(Expr::Const(LogicWord::zero(32))),
                        },
                        message: Some(
                            "run_phase completed with a per-owner UVM objection imbalance".into(),
                        ),
                    });
                }
                let timeout_check = Stmt::If {
                    c: Expr::Bin {
                        op: BinOp::Eq,
                        a: Box::new(class_expr(&run_object.objection_drained)),
                        b: Box::new(Expr::Const(LogicWord::zero(1))),
                    },
                    t: Box::new(Stmt::Assert {
                        cond: Expr::Const(LogicWord::zero(1)),
                        message: Some(
                            "run_phase objections did not drain before the UVM bootstrap timeout"
                                .into(),
                        ),
                    }),
                    f: None,
                };
                statements.push(Stmt::Fork {
                    branches: vec![Stmt::Delay {
                        ns: run_object.objection_timeout,
                        body: Box::new(timeout_check),
                    }],
                    mode: eda_core::ir::ForkMode::JoinNone,
                });
                statements.push(Stmt::Delay {
                    ns: 0,
                    body: Box::new(Stmt::Wait {
                        cond: Expr::Bin {
                            op: BinOp::Eq,
                            a: Box::new(class_expr(&run_object.objection_count)),
                            b: Box::new(Expr::Const(LogicWord::zero(32))),
                        },
                        body: Box::new(Stmt::Block(drain_checks)),
                    }),
                });
            }
        }
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_factory_override_call(
        &self,
        value: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        let method = subroutine
            .split_whitespace()
            .next_back()
            .unwrap_or_default();
        if !matches!(
            method,
            "set_type_override_by_name"
                | "set_type_override_by_type"
                | "set_inst_override_by_name"
                | "set_inst_override_by_type"
        ) {
            return Ok(None);
        }
        if matches!(
            method,
            "set_inst_override_by_name" | "set_inst_override_by_type"
        ) {
            let (original, override_name, instance_path) =
                uvm_factory_instance_override_names(value)?;
            let original = self.resolve_uvm_instance_alias(&original, &instance_path)?;
            if !self.uvm_factory_instance_overrides.iter().any(|existing| {
                existing.pattern == instance_path
                    && existing.original == original
                    && existing.override_name == override_name
            }) {
                return Err(format!(
                    "UVM instance override '{original}' -> '{override_name}' at '{instance_path}' was not prepared"
                ));
            }
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        uvm_factory_override_names(value)?;
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_factory_type_alias_call(
        &self,
        value: &Value,
    ) -> Result<Option<Stmt>, String> {
        if !is_uvm_factory_type_alias_call(value) {
            return Ok(None);
        }
        let (alias_name, original_name) = uvm_factory_type_alias_names(value)?;
        let original_name = self.resolve_uvm_type_alias(&original_name)?;
        if !self.uvm_factory_type_aliases.iter().any(|existing| {
            existing.alias_name == alias_name && existing.original_name == original_name
        }) {
            return Err(format!(
                "UVM type alias '{alias_name}' -> '{original_name}' was not prepared"
            ));
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_factory_instance_alias_call(
        &self,
        value: &Value,
    ) -> Result<Option<Stmt>, String> {
        if !is_uvm_factory_instance_alias_call(value) {
            return Ok(None);
        }
        let (alias_name, original_name, pattern) = uvm_factory_instance_alias_names(value)?;
        let original_name = self.resolve_uvm_type_alias(&original_name)?;
        if !self.uvm_factory_instance_aliases.iter().any(|existing| {
            existing.alias_name == alias_name
                && existing.original_name == original_name
                && existing.pattern == pattern
        }) {
            return Err(format!(
                "UVM instance alias '{alias_name}' -> '{original_name}' at '{pattern}' was not prepared"
            ));
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    fn resolve_uvm_type_alias(&self, type_name: &str) -> Result<String, String> {
        let mut current = type_name.to_string();
        let mut visited = std::collections::BTreeSet::new();
        while let Some(alias) = self
            .uvm_factory_type_aliases
            .iter()
            .find(|alias| alias.alias_name == current)
        {
            if !visited.insert(current.clone()) {
                return Err(format!("UVM type alias cycle at '{current}'"));
            }
            current = alias.original_name.clone();
        }
        Ok(current)
    }

    fn resolve_uvm_instance_alias(
        &self,
        type_name: &str,
        instance_path: &str,
    ) -> Result<String, String> {
        let mut current = type_name.to_string();
        let mut visited = std::collections::BTreeSet::new();
        while let Some(alias) = self.uvm_factory_instance_aliases.iter().find(|alias| {
            alias.alias_name == current && wildcard_matches(&alias.pattern, instance_path)
        }) {
            if !visited.insert(current.clone()) {
                return Err(format!("UVM instance alias cycle at '{current}'"));
            }
            current = alias.original_name.clone();
        }
        self.resolve_uvm_type_alias(&current)
    }

    pub(super) fn lower_uvm_timeout_call(&self, value: &Value) -> Result<Option<Stmt>, String> {
        if !is_uvm_timeout_call(value) {
            return Ok(None);
        }
        uvm_timeout_value(value)?;
        Ok(Some(Stmt::Block(Vec::new())))
    }

    fn resolve_global_uvm_factory_class(&self, requested_name: &str) -> Result<String, String> {
        let mut selected_name = requested_name.to_string();
        let mut visited = std::collections::BTreeSet::new();
        loop {
            let Some(next_name) = self
                .uvm_factory_overrides
                .iter()
                .find(|existing| existing.original == selected_name)
                .map(|existing| existing.override_name.clone())
            else {
                return Ok(selected_name);
            };
            if !visited.insert(selected_name.clone()) {
                return Err(format!("UVM factory override cycle at '{selected_name}'"));
            }
            selected_name = next_name;
        }
    }

    fn uvm_factory_instance_path(
        &self,
        name: &str,
        parent: Option<&Value>,
        context: Option<&Value>,
        use_parent_context: bool,
    ) -> Result<String, String> {
        if let Some(context) = context {
            let context = string_literal_allow_empty(unwrap_conversion(context))
                .ok_or_else(|| "UVM factory context must be a string literal".to_string())?;
            if !context.is_empty() {
                return Ok(format!("{context}.{name}"));
            }
        }
        if !use_parent_context {
            return Ok(name.to_string());
        }
        let parent_key = parent
            .map(|parent| {
                let parent = unwrap_conversion(parent);
                if string_field(parent, "kind").as_deref() == Some("NullLiteral") {
                    return Ok(None);
                }
                self.class_object_key_for_value(parent)?
                    .or_else(|| {
                        is_this_reference(parent)
                            .then(|| self.active_class_objects.borrow().last().cloned())
                            .flatten()
                    })
                    .map(Some)
                    .ok_or_else(|| "UVM factory parent must be a prepared class object".to_string())
            })
            .transpose()?
            .flatten();
        if let Some(parent_key) = parent_key {
            let parent_full_name = self
                .class_object_declared_full_names
                .borrow()
                .get(&parent_key)
                .cloned()
                .ok_or_else(|| {
                    format!("UVM factory parent '{parent_key}' has no declared full name")
                })?;
            Ok(format!("{parent_full_name}.{name}"))
        } else {
            Ok(name.to_string())
        }
    }

    fn select_uvm_factory_class(
        &self,
        requested_name: &str,
        instance_path: &str,
    ) -> Result<String, String> {
        let selected_instance_override = self
            .uvm_factory_instance_overrides
            .iter()
            .find(|existing| wildcard_matches(&existing.pattern, instance_path))
            .map(|existing| {
                (
                    existing.pattern.clone(),
                    existing.original.clone(),
                    existing.override_name.clone(),
                )
            });
        if let Some((pattern, original, override_name)) = selected_instance_override {
            let requested_key = self.class_type_key(requested_name);
            let original_key = self.class_type_key(&original);
            let applies = requested_key == original_key
                || self.class_chain(&requested_key)?.iter().any(|(owner, _)| {
                    owner == &original_key
                        || owner.split('@').next()
                            == Some(original_key.split('@').next().unwrap_or(&original_key))
                });
            if !applies {
                return Err(format!(
                    "UVM instance override '{pattern}' expects '{original}', not '{requested_name}'"
                ));
            }
            return Ok(override_name);
        }
        self.resolve_global_uvm_factory_class(requested_name)
    }

    pub(super) fn lower_uvm_factory_create_assignment(
        &self,
        assignment: &Value,
        left: &Value,
        right: &Value,
    ) -> Result<Option<Stmt>, String> {
        let direct_factory_kind = uvm_factory_direct_create_kind(right);
        let registry_kind = uvm_registry_create_kind(right);
        if direct_factory_kind.is_none() && registry_kind.is_none() {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = right
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let (
            requested_name,
            name_argument,
            constructor_arguments,
            direct_parent,
            use_parent_context,
        ) = match direct_factory_kind {
            Some("component_by_type") => {
                if arguments.len() != 4 {
                    return Err(
                        "UVM factory create_component_by_type requires four arguments".into(),
                    );
                }
                string_literal(unwrap_conversion(&arguments[1])).ok_or_else(|| {
                    "UVM factory parent path must be a string literal".to_string()
                })?;
                (
                    wrapper_type_name(&arguments[0]).ok_or_else(|| {
                        "UVM factory requested component type must be Class::get_type()".to_string()
                    })?,
                    &arguments[2],
                    vec![arguments[2].clone(), arguments[3].clone()],
                    Some(&arguments[3]),
                    true,
                )
            }
            Some("component_by_name") => {
                if arguments.len() != 4 {
                    return Err(
                        "UVM factory create_component_by_name requires four arguments".into(),
                    );
                }
                (
                    string_literal(unwrap_conversion(&arguments[0])).ok_or_else(|| {
                        "UVM factory requested component name must be a string literal".to_string()
                    })?,
                    &arguments[2],
                    vec![arguments[2].clone(), arguments[3].clone()],
                    Some(&arguments[3]),
                    true,
                )
            }
            Some("object_by_name") => {
                if arguments.len() != 3 {
                    return Err("UVM factory create_object_by_name requires three arguments".into());
                }
                (
                    string_literal(unwrap_conversion(&arguments[0])).ok_or_else(|| {
                        "UVM factory requested object name must be a string literal".to_string()
                    })?,
                    &arguments[2],
                    vec![arguments[2].clone()],
                    None,
                    false,
                )
            }
            Some(_) => return Ok(None),
            None => {
                let registry_kind =
                    registry_kind.expect("UVM registry create kind was just checked");
                if registry_kind == "component" {
                    if arguments.len() < 2 || arguments.len() > 3 {
                        return Err(
                            "UVM component registry create requires two or three arguments".into(),
                        );
                    }
                    if let Some(context) = arguments.get(2) {
                        string_literal_allow_empty(unwrap_conversion(context)).ok_or_else(
                            || {
                                "UVM component registry context must be a string literal"
                                    .to_string()
                            },
                        )?;
                    }
                } else if arguments.is_empty() || arguments.len() > 3 {
                    return Err("UVM object registry create requires one to three arguments".into());
                } else if let Some(context) = arguments.get(2) {
                    string_literal_allow_empty(unwrap_conversion(context)).ok_or_else(|| {
                        "UVM object registry context must be a string literal".to_string()
                    })?;
                }
                let requested_name =
                    registry_create_class_name(right, &self.class_definitions, registry_kind)
                        .ok_or_else(|| {
                            "UVM registry create could not resolve its requested class".to_string()
                        })?;
                if registry_kind == "component" {
                    (
                        requested_name,
                        &arguments[0],
                        vec![arguments[0].clone(), arguments[1].clone()],
                        None,
                        true,
                    )
                } else {
                    (
                        requested_name,
                        &arguments[0],
                        vec![arguments[0].clone()],
                        None,
                        true,
                    )
                }
            }
        };
        let component_name = string_literal(unwrap_conversion(name_argument))
            .ok_or_else(|| "UVM factory component name must be a string literal".to_string())?;
        let instance_path = if direct_factory_kind.is_some() {
            self.uvm_factory_instance_path(
                &component_name,
                direct_parent,
                Some(&arguments[1]),
                use_parent_context,
            )?
        } else {
            self.uvm_factory_instance_path(
                &component_name,
                arguments.get(1),
                arguments.get(2),
                true,
            )?
        };
        let requested_name = self.resolve_uvm_instance_alias(&requested_name, &instance_path)?;
        let selected_name = self.select_uvm_factory_class(&requested_name, &instance_path)?;
        let selected_key = self.class_type_key(&selected_name);
        let Some(target_key) = self.class_object_key_for_value(left)? else {
            return Err(with_source_span(
                assignment,
                "UVM factory component assignment target is not a prepared class object".into(),
            ));
        };
        let target_class = self
            .class_object_types
            .get(&target_key)
            .cloned()
            .ok_or_else(|| format!("unknown UVM factory component target '{target_key}'"))?;
        let selected_compatible = self.class_chain(&selected_key)?.iter().any(|(owner, _)| {
            owner == &target_class
                || owner.split('@').next()
                    == Some(target_class.split('@').next().unwrap_or(&target_class))
        });
        if selected_key != target_class && !selected_compatible {
            return Err(with_source_span(
                assignment,
                format!(
                    "UVM factory create selected '{selected_name}', which does not match prepared target type '{target_class}'"
                ),
            ));
        }
        if selected_key != target_class {
            self.record_runtime_dynamic_source(&target_key, &target_key, &selected_key)?;
        }
        let constructor = self
            .class_chain(&selected_key)?
            .iter()
            .find_map(|(_, class)| {
                let empty_members = Vec::new();
                class
                    .get("members")
                    .and_then(Value::as_array)
                    .unwrap_or(&empty_members)
                    .iter()
                    .find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some("new")
                    })
                    .cloned()
            })
            .ok_or_else(|| format!("UVM factory class '{selected_name}' has no constructor"))?;
        let empty_formals = Vec::new();
        let formals = constructor
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_formals);
        let expected_constructor_arguments = constructor_arguments.len();
        if formals.len() != expected_constructor_arguments {
            return Err(format!(
                "UVM factory class '{selected_name}' must have {expected_constructor_arguments} constructor arguments"
            ));
        }
        let constructor_address =
            constructor
                .get("addr")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    format!("UVM factory class '{selected_name}' constructor has no address")
                })?;
        let construction = json!({
            "kind": "Assignment",
            "left": left,
            "right": {
                "kind": "NewClass",
                "type": selected_key,
                "isSuperClass": false,
                "constructorCall": {
                    "kind": "Call",
                    "subroutine": format!("{constructor_address} new"),
                    "arguments": constructor_arguments,
                },
            },
            "isNonBlocking": bool_field(assignment, "isNonBlocking"),
        });
        self.lower_class_constructor(&construction)?
            .map(Some)
            .ok_or_else(|| format!("UVM factory failed to construct '{selected_name}'"))
    }

    pub(super) fn lower_uvm_factory_cast_conditional(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(condition) = statement
            .get("conditions")
            .and_then(Value::as_array)
            .and_then(|conditions| conditions.first())
            .and_then(|condition| condition.get("expr"))
        else {
            return Ok(None);
        };
        let (cast_call, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
            && string_field(condition, "op").as_deref() == Some("LogicalNot")
        {
            (
                condition
                    .get("operand")
                    .ok_or_else(|| "logical-not expression has no operand".to_string())?,
                true,
            )
        } else {
            (condition, false)
        };
        if string_field(cast_call, "subroutine").as_deref() != Some("$cast") {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = cast_call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Ok(None);
        }
        let target = arguments[0]
            .get("left")
            .ok_or_else(|| "UVM factory cast destination must be assignable".to_string())?;
        let source = unwrap_conversion(&arguments[1]);
        if uvm_factory_direct_create_kind(source).is_none() {
            return Ok(None);
        }
        let construction = self
            .lower_uvm_factory_create_assignment(
                &json!({
                    "kind": "Assignment",
                    "left": target,
                    "right": source,
                    "isNonBlocking": false,
                }),
                target,
                source,
            )?
            .ok_or_else(|| "UVM factory cast source did not construct a class".to_string())?;
        let branch = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let branch = match branch {
            Some(branch) => Some(self.lower_statement(branch)?),
            None => None,
        };
        let mut statements = vec![construction];
        statements.extend(branch);
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_phase_objection_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        let receiver = unwrap_conversion(receiver);
        let Some(phase_key) = string_field(receiver, "symbol") else {
            return Ok(None);
        };
        let Some(run_object) = self
            .run_test_objects
            .values()
            .find(|object| object.phase_key == phase_key)
        else {
            return Ok(None);
        };
        let method_name = string_field(call, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if !matches!(method_name.as_str(), "raise_objection" | "drop_objection") {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() || arguments.len() > 3 {
            return Err(format!(
                "UVM bootstrap {method_name} takes one to three arguments, found {}",
                arguments.len()
            ));
        }

        let owner_argument = unwrap_conversion(&arguments[0]);
        let owner_key = match self.class_object_key_for_value(owner_argument)? {
            Some(owner_key) => owner_key,
            None if is_this_reference(owner_argument) => run_object.object_key.clone(),
            None => {
                return Err(
                    "UVM bootstrap objection owner must be a known class object or this".into(),
                )
            }
        };
        let owner_id = self
            .class_object_ids
            .get(&owner_key)
            .cloned()
            .ok_or_else(|| {
                format!("UVM bootstrap objection owner '{owner_key}' has no object identity")
            })?;
        let registry = run_object
            .objection_owner_counts
            .iter()
            .filter_map(|(registry_key, count)| {
                self.class_object_ids
                    .get(registry_key)
                    .map(|identity| (registry_key.clone(), count.clone(), identity.clone()))
            })
            .collect::<Vec<_>>();
        if registry.is_empty() {
            return Err(format!(
                "UVM bootstrap objection owner registry for '{}' is empty",
                run_object.class_name
            ));
        }

        let delta_value = match arguments.get(2) {
            Some(count) => slang_constant_u64(count)?,
            None => 1,
        };
        if delta_value == 0 || delta_value > u64::from(u32::MAX) {
            return Err(format!(
                "UVM bootstrap {method_name} count must be between 1 and {}",
                u32::MAX
            ));
        }
        let delta_value = u32::try_from(delta_value).expect("checked UVM objection count");
        let delta = Expr::Const(LogicWord::from_u64(32, u64::from(delta_value)));
        let add = method_name == "raise_objection";
        let total_current = class_expr(&run_object.objection_count);
        let owner_id_current = class_expr(&owner_id);

        let mut matched = None;
        let mut selected_owner_current = Some(Expr::Const(LogicWord::zero(32)));
        for (_, count, identity) in &registry {
            let slot_match = Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(owner_id_current.clone()),
                b: Box::new(class_expr(identity)),
            };
            matched = Some(match matched {
                Some(previous) => Expr::Bin {
                    op: BinOp::LogOr,
                    a: Box::new(previous),
                    b: Box::new(slot_match.clone()),
                },
                None => slot_match.clone(),
            });
            selected_owner_current = Some(Expr::Cond {
                c: Box::new(slot_match),
                t: Box::new(class_expr(count)),
                f: Box::new(selected_owner_current.take().expect("owner current exists")),
            });
        }
        let matched = matched.expect("objection registry is nonempty");
        let selected_owner_current =
            selected_owner_current.expect("objection registry is nonempty");

        let next_total = Expr::Bin {
            op: if add { BinOp::Add } else { BinOp::Sub },
            a: Box::new(total_current.clone()),
            b: Box::new(delta.clone()),
        };
        let (boundary_op, boundary) = if add {
            (BinOp::Gt, u32::MAX - delta_value)
        } else {
            (BinOp::Lt, delta_value)
        };
        let boundary = Expr::Const(LogicWord::from_u64(32, u64::from(boundary)));
        let owner_invalid = Expr::Bin {
            op: boundary_op,
            a: Box::new(selected_owner_current),
            b: Box::new(boundary.clone()),
        };
        let total_invalid = Expr::Bin {
            op: boundary_op,
            a: Box::new(total_current),
            b: Box::new(boundary),
        };
        let unknown_owner = Expr::Un {
            op: eda_core::ir::UnOp::Not,
            a: Box::new(matched),
        };
        let invalid = Expr::Bin {
            op: BinOp::LogOr,
            a: Box::new(Expr::Bin {
                op: BinOp::LogOr,
                a: Box::new(unknown_owner),
                b: Box::new(owner_invalid),
            }),
            b: Box::new(total_invalid),
        };

        let mut owner_updates = Stmt::Assert {
            cond: Expr::Const(LogicWord::zero(1)),
            message: Some("UVM bootstrap objection owner identity is not registered".into()),
        };
        for (_, count, identity) in &registry {
            let next_owner = Expr::Bin {
                op: if add { BinOp::Add } else { BinOp::Sub },
                a: Box::new(class_expr(count)),
                b: Box::new(delta.clone()),
            };
            owner_updates = Stmt::If {
                c: Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(owner_id_current.clone()),
                    b: Box::new(class_expr(identity)),
                },
                t: Box::new(Stmt::Bl {
                    lv: class_lvalue(count),
                    e: next_owner,
                }),
                f: Some(Box::new(owner_updates)),
            };
        }

        let message = if add {
            "UVM bootstrap objection counter overflowed or owner is unknown".to_string()
        } else {
            "UVM bootstrap owner dropped an objection it did not own".to_string()
        };
        Ok(Some(Stmt::If {
            c: invalid,
            t: Box::new(Stmt::Assert {
                cond: Expr::Const(LogicWord::zero(1)),
                message: Some(message),
            }),
            f: Some(Box::new(Stmt::Block(vec![
                Stmt::Bl {
                    lv: class_lvalue(&run_object.objection_count),
                    e: next_total,
                },
                owner_updates,
            ]))),
        }))
    }

    fn uvm_objection_owner_keys(&self, root: &str) -> Vec<String> {
        let mut pending = vec![root.to_string()];
        let mut visited = std::collections::BTreeSet::new();
        while let Some(object_key) = pending.pop() {
            if !visited.insert(object_key.clone()) {
                continue;
            }
            if let Some(elements) = self.class_object_arrays.get(&object_key) {
                pending.extend(elements.values().cloned());
            }
            pending.extend(
                self.class_composition_children
                    .iter()
                    .filter(|((parent, _), _)| parent == &object_key)
                    .map(|(_, child)| child.clone()),
            );
        }
        visited.into_iter().collect()
    }
}

fn unwrap_conversion(mut value: &Value) -> &Value {
    while string_field(value, "kind").as_deref() == Some("Conversion") {
        let Some(operand) = value.get("operand") else {
            break;
        };
        value = operand;
    }
    value
}

fn is_this_reference(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("NamedValue")
        && string_field(value, "symbol")
            .and_then(|symbol| symbol.split_whitespace().next_back().map(str::to_string))
            .as_deref()
            == Some("this")
}

fn official_uvm_test_index(chain: &[(String, Value)]) -> Option<usize> {
    chain.iter().position(|(owner, class)| {
        owner.split('@').next() == Some("uvm_test")
            && string_field(class, "source_file")
                .and_then(|file| file.split('/').next_back().map(str::to_string))
                .as_deref()
                == Some("uvm_test.svh")
    })
}

fn user_phase_method<'chain>(
    chain: &'chain [(String, Value)],
    official_index: usize,
    phase_name: &str,
) -> Option<&'chain Value> {
    chain.iter().take(official_index).find_map(|(_, class)| {
        class
            .get("members")
            .and_then(Value::as_array)
            .and_then(|members| {
                members.iter().find(|member| {
                    string_field(member, "kind").as_deref() == Some("Subroutine")
                        && string_field(member, "name").as_deref() == Some(phase_name)
                })
            })
    })
}

fn validate_bootstrap_phase_method(method: &Value, phase_name: &str) -> Result<(), String> {
    let expected_kind = if phase_name == "run_phase" {
        "Task"
    } else {
        "Function"
    };
    if string_field(method, "subroutineKind").as_deref() != Some(expected_kind) {
        return Err(format!("must be a void {expected_kind}").to_lowercase());
    }
    if method_return_type(method) != "void" {
        return Err("must return void".into());
    }
    let empty_arguments = Vec::new();
    let arguments = method
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() != 1 {
        return Err("must take one uvm_phase argument".into());
    }
    Ok(())
}

fn collect_run_test_calls<'value>(value: &'value Value, calls: &mut Vec<&'value Value>) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && value.get("thisClass").is_none()
            && string_field(value, "subroutine")
                .and_then(|subroutine| {
                    subroutine
                        .split_whitespace()
                        .next_back()
                        .map(str::to_string)
                })
                .as_deref()
                == Some("run_test")
        {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_run_test_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_run_test_calls(nested, calls);
        }
    }
}

fn collect_uvm_factory_override_calls<'value>(
    value: &'value Value,
    calls: &mut Vec<&'value Value>,
) {
    if let Some(fields) = value.as_object() {
        if is_uvm_factory_override_call(value) {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_factory_override_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_factory_override_calls(nested, calls);
        }
    }
}

fn collect_uvm_factory_type_alias_calls<'value>(
    value: &'value Value,
    calls: &mut Vec<&'value Value>,
) {
    if let Some(fields) = value.as_object() {
        if is_uvm_factory_type_alias_call(value) {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_factory_type_alias_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_factory_type_alias_calls(nested, calls);
        }
    }
}

fn collect_uvm_factory_instance_alias_calls<'value>(
    value: &'value Value,
    calls: &mut Vec<&'value Value>,
) {
    if let Some(fields) = value.as_object() {
        if is_uvm_factory_instance_alias_call(value) {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_factory_instance_alias_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_factory_instance_alias_calls(nested, calls);
        }
    }
}

fn collect_uvm_factory_instance_override_calls<'value>(
    value: &'value Value,
    calls: &mut Vec<&'value Value>,
) {
    if let Some(fields) = value.as_object() {
        if is_uvm_factory_instance_override_call(value) {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_factory_instance_override_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_factory_instance_override_calls(nested, calls);
        }
    }
}

fn collect_uvm_timeout_calls<'value>(value: &'value Value, calls: &mut Vec<&'value Value>) {
    if let Some(fields) = value.as_object() {
        if is_uvm_timeout_call(value) {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_timeout_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_timeout_calls(nested, calls);
        }
    }
}

fn is_uvm_timeout_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("set_timeout")
        && value.get("thisClass").is_some_and(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_root"))
                    .unwrap_or(false)
        })
}

fn uvm_timeout_value(call: &Value) -> Result<u64, String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.is_empty() || arguments.len() > 2 {
        return Err("UVM set_timeout requires one or two arguments".into());
    }
    let timeout = unwrap_conversion(&arguments[0]);
    if string_field(timeout, "kind").as_deref() != Some("TimeLiteral") {
        return Err("UVM timeout must be a time literal".into());
    }
    let raw_value = timeout
        .get("value")
        .and_then(Value::as_u64)
        .or_else(|| string_field(timeout, "value").and_then(|raw| raw.trim().parse::<u64>().ok()))
        .ok_or_else(|| "UVM timeout literal has no valid value".to_string())?;
    if raw_value == 0 {
        return Err("UVM timeout must be greater than zero".into());
    }
    Ok(raw_value)
}

fn is_uvm_factory_override_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            .is_some_and(|method| {
                matches!(
                    method,
                    "set_type_override_by_name" | "set_type_override_by_type"
                )
            })
        && value.get("thisClass").is_some_and(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_factory"))
                    .unwrap_or(false)
        })
}

fn is_uvm_factory_instance_alias_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("set_inst_alias")
        && value.get("thisClass").is_some_and(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_factory"))
                    .unwrap_or(false)
        })
        && value
            .get("arguments")
            .and_then(Value::as_array)
            .is_some_and(|arguments| arguments.len() == 3)
}

fn is_uvm_factory_type_alias_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("set_type_alias")
        && value.get("thisClass").is_some_and(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_factory"))
                    .unwrap_or(false)
        })
        && value
            .get("arguments")
            .and_then(Value::as_array)
            .is_some_and(|arguments| arguments.len() == 2)
}

fn is_uvm_factory_instance_override_call(value: &Value) -> bool {
    string_field(value, "kind").as_deref() == Some("Call")
        && string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            .is_some_and(|method| {
                matches!(
                    method,
                    "set_inst_override_by_name" | "set_inst_override_by_type"
                )
            })
        && value.get("thisClass").is_some_and(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_factory"))
                    .unwrap_or(false)
        })
}

fn uvm_factory_direct_create_kind(value: &Value) -> Option<&'static str> {
    if string_field(value, "kind").as_deref() != Some("Call") {
        return None;
    }
    let method = string_field(value, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_default();
    let method = match method.as_str() {
        "create_component_by_type" => "component_by_type",
        "create_component_by_name" => "component_by_name",
        "create_object_by_name" => "object_by_name",
        _ => return None,
    };
    value
        .get("thisClass")
        .filter(|receiver| {
            string_field(receiver, "kind").as_deref() == Some("Call")
                && string_field(receiver, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("get")
                && string_field(receiver, "type")
                    .map(|type_name| type_name.contains("uvm_factory"))
                    .unwrap_or(false)
        })
        .map(|_| method)
}

fn uvm_registry_create_kind(value: &Value) -> Option<&'static str> {
    if string_field(value, "kind").as_deref() != Some("Call") {
        return None;
    }
    let subroutine = string_field(value, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_default();
    if subroutine != "create" {
        return None;
    }
    let type_name = string_field(value, "type")?;
    if type_name.contains("uvm_component_registry::T") {
        Some("component")
    } else if type_name.contains("uvm_object_registry::T") {
        Some("object")
    } else {
        None
    }
}

fn registry_create_class_name(
    call: &Value,
    class_definitions: &BTreeMap<String, Value>,
    registry_kind: &str,
) -> Option<String> {
    let address = string_field(call, "subroutine")?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    for class in class_definitions.values() {
        let generic_class = string_field(class, "genericClass").unwrap_or_default();
        let expected_registry = match registry_kind {
            "component" => "uvm_component_registry",
            _ => "uvm_object_registry",
        };
        if generic_class
            .split_whitespace()
            .next_back()
            .is_none_or(|name| name != expected_registry)
        {
            continue;
        }
        let empty_members = Vec::new();
        let members = class
            .get("members")
            .and_then(Value::as_array)
            .unwrap_or(&empty_members);
        let has_create = members.iter().any(|member| {
            string_field(member, "kind").as_deref() == Some("Subroutine")
                && string_field(member, "name").as_deref() == Some("create")
                && member.get("addr").and_then(Value::as_u64) == Some(address)
        });
        if !has_create {
            continue;
        }
        return members
            .iter()
            .find(|member| {
                string_field(member, "kind").as_deref() == Some("TypeParameter")
                    && string_field(member, "name").as_deref() == Some("T")
            })
            .and_then(|parameter| string_field(parameter, "type"))
            .and_then(|type_name| type_name.split_whitespace().next_back().map(str::to_string));
    }
    None
}

fn uvm_factory_override_names(call: &Value) -> Result<(String, String, bool), String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() < 2 || arguments.len() > 3 {
        return Err("UVM factory set_type_override_by_name requires two or three arguments".into());
    }
    let replace = match arguments.get(2) {
        Some(replace) => slang_constant_u64(replace)? != 0,
        None => true,
    };
    let method = string_field(call, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_default();
    if method == "set_type_override_by_name" {
        let original = string_literal(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM factory original type must be a string literal".to_string())?;
        let override_name = string_literal(unwrap_conversion(&arguments[1]))
            .ok_or_else(|| "UVM factory override type must be a string literal".to_string())?;
        return Ok((original, override_name, replace));
    }
    if method == "set_type_override_by_type" {
        let original = wrapper_type_name(&arguments[0])
            .ok_or_else(|| "UVM factory original wrapper must be Class::get_type()".to_string())?;
        let override_name = wrapper_type_name(&arguments[1])
            .ok_or_else(|| "UVM factory override wrapper must be Class::get_type()".to_string())?;
        return Ok((original, override_name, replace));
    }
    Err("unsupported UVM factory override method".into())
}

fn uvm_factory_type_alias_names(call: &Value) -> Result<(String, String), String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() != 2 {
        return Err("UVM type alias requires two arguments".into());
    }
    let alias_name = string_literal(unwrap_conversion(&arguments[0]))
        .ok_or_else(|| "UVM type alias name must be a string literal".to_string())?;
    let original_name = wrapper_type_name(&arguments[1])
        .ok_or_else(|| "UVM type alias original must be Class::get_type()".to_string())?;
    Ok((alias_name, original_name))
}

fn uvm_factory_instance_alias_names(call: &Value) -> Result<(String, String, String), String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() != 3 {
        return Err("UVM instance alias requires three arguments".into());
    }
    let alias_name = string_literal(unwrap_conversion(&arguments[0]))
        .ok_or_else(|| "UVM instance alias name must be a string literal".to_string())?;
    let original_name = wrapper_type_name(&arguments[1])
        .ok_or_else(|| "UVM instance alias original must be Class::get_type()".to_string())?;
    let pattern = string_literal(unwrap_conversion(&arguments[2]))
        .ok_or_else(|| "UVM instance alias path must be a string literal".to_string())?;
    Ok((alias_name, original_name, pattern))
}

fn uvm_factory_instance_override_names(call: &Value) -> Result<(String, String, String), String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() != 3 {
        return Err("UVM factory instance overrides require three arguments".into());
    }
    let method = string_field(call, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_default();
    let (original, override_name) = if method == "set_inst_override_by_name" {
        (
            string_literal(unwrap_conversion(&arguments[0])).ok_or_else(|| {
                "UVM factory original instance type must be a string literal".to_string()
            })?,
            string_literal(unwrap_conversion(&arguments[1])).ok_or_else(|| {
                "UVM factory override instance type must be a string literal".to_string()
            })?,
        )
    } else if method == "set_inst_override_by_type" {
        (
            wrapper_type_name(&arguments[0]).ok_or_else(|| {
                "UVM factory original instance wrapper must be Class::get_type()".to_string()
            })?,
            wrapper_type_name(&arguments[1]).ok_or_else(|| {
                "UVM factory override instance wrapper must be Class::get_type()".to_string()
            })?,
        )
    } else {
        return Err("unsupported UVM factory instance override method".into());
    };
    let instance_path = string_literal(unwrap_conversion(&arguments[2]))
        .ok_or_else(|| "UVM factory instance path must be a string literal".to_string())?;
    if instance_path.starts_with('.')
        || instance_path.ends_with('.')
        || instance_path.contains("..")
    {
        return Err(format!(
            "UVM factory instance path '{instance_path}' must be a well-formed path"
        ));
    }
    Ok((original, override_name, instance_path))
}

fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let mut previous = vec![false; text.len() + 1];
    previous[0] = true;
    if let Some(&first) = pattern.first() {
        for index in 1..=text.len() {
            previous[index] = first == '*' && previous[index - 1];
        }
    }
    for &pattern_character in &pattern {
        let mut current = vec![false; text.len() + 1];
        current[0] = previous[0] && pattern_character == '*';
        for index in 1..=text.len() {
            current[index] = if pattern_character == '*' {
                previous[index] || current[index - 1]
            } else {
                (pattern_character == '?' || pattern_character == text[index - 1])
                    && previous[index - 1]
            };
        }
        previous = current;
    }
    previous[text.len()]
}

fn wrapper_type_name(value: &Value) -> Option<String> {
    let wrapper = unwrap_conversion(value);
    if string_field(wrapper, "kind").as_deref() != Some("Call") {
        return None;
    }
    let subroutine = string_field(wrapper, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_default();
    if subroutine != "get_type" {
        return None;
    }
    let wrapper_type = string_field(wrapper, "type")?;
    let (class_type, wrapper_name) = wrapper_type.rsplit_once("::")?;
    if wrapper_name != "type_id" {
        return None;
    }
    class_type
        .split_whitespace()
        .next_back()
        .map(str::to_string)
}

fn string_literal(value: &Value) -> Option<String> {
    string_field(value, "literal").filter(|literal| !literal.is_empty())
}

fn string_literal_allow_empty(value: &Value) -> Option<String> {
    string_field(value, "literal")
}

fn source_location(value: &Value) -> Option<(String, u64, u64)> {
    Some((
        string_field(value, "source_file_start")?,
        value.get("source_line_start").and_then(Value::as_u64)?,
        value.get("source_column_start").and_then(Value::as_u64)?,
    ))
}

fn run_test_class_name(call: &Value) -> Result<String, String> {
    let invalid = || {
        with_source_span(
            call,
            "direct-lowering run_test requires one compile-time string literal argument".into(),
        )
    };
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if arguments.len() != 1 {
        return Err(invalid());
    }
    let mut argument = &arguments[0];
    while string_field(argument, "kind").as_deref() == Some("Conversion") {
        let Some(operand) = argument.get("operand") else {
            break;
        };
        argument = operand;
    }
    let class_name = string_field(argument, "literal")
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid)?;
    Ok(class_name.to_string())
}
