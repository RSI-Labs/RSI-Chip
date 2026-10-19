//! Minimal direct-lowering bootstrap for official-UVM `run_test`.

use super::classes::{class_expr, class_lvalue, method_return_type};
use super::SlangAstLowerer;
use crate::ast_util::{slang_constant_u64, string_field, with_source_span};
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
        let mut override_calls = Vec::new();
        collect_uvm_factory_override_calls(body, &mut override_calls);
        for override_call in &override_calls {
            let (original, override_name) = uvm_factory_override_names(override_call)?;
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
            if let Some(previous) = self
                .uvm_factory_overrides
                .insert(original.clone(), override_name.clone())
            {
                if previous != override_name {
                    return Err(with_source_span(
                        override_call,
                        format!("conflicting UVM factory overrides for '{original}'"),
                    ));
                }
            }
        }
        for call in calls {
            if !self.run_test_objects.is_empty() {
                return Err(with_source_span(
                    call,
                    "only one direct-lowering run_test call is supported per module".into(),
                ));
            }
            let requested_class_name = run_test_class_name(call)?;
            let mut class_name = requested_class_name.clone();
            let mut visited = std::collections::BTreeSet::new();
            while let Some(override_name) = self.uvm_factory_overrides.get(&class_name).cloned() {
                if !visited.insert(class_name.clone()) {
                    return Err(with_source_span(
                        call,
                        format!("UVM factory override cycle at '{class_name}'"),
                    ));
                }
                class_name = override_name;
            }
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
                    object_key,
                    phase_key,
                    objection_count,
                    objection_drained,
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
                        ns: UVM_BOOTSTRAP_OBJECTION_TIMEOUT,
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
        if subroutine.split_whitespace().next_back() != Some("set_type_override_by_name") {
            return Ok(None);
        }
        let (original, override_name) = uvm_factory_override_names(value)?;
        if self.uvm_factory_overrides.get(&original) != Some(&override_name) {
            return Err(format!(
                "UVM factory override '{original}' -> '{override_name}' was not prepared"
            ));
        }
        Ok(Some(Stmt::Block(Vec::new())))
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
            == Some("set_type_override_by_name")
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

fn uvm_factory_override_names(call: &Value) -> Result<(String, String), String> {
    let empty_arguments = Vec::new();
    let arguments = call
        .get("arguments")
        .and_then(Value::as_array)
        .unwrap_or(&empty_arguments);
    if arguments.len() < 2 || arguments.len() > 3 {
        return Err("UVM factory set_type_override_by_name requires two or three arguments".into());
    }
    if let Some(replace) = arguments.get(2) {
        if slang_constant_u64(replace)? == 0 {
            return Err("non-replacing UVM factory overrides are not supported".into());
        }
    }
    let original = string_literal(unwrap_conversion(&arguments[0]))
        .ok_or_else(|| "UVM factory original type must be a string literal".to_string())?;
    let override_name = string_literal(unwrap_conversion(&arguments[1]))
        .ok_or_else(|| "UVM factory override type must be a string literal".to_string())?;
    Ok((original, override_name))
}

fn string_literal(value: &Value) -> Option<String> {
    string_field(value, "literal").filter(|literal| !literal.is_empty())
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
