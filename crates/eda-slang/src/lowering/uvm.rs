//! Minimal direct-lowering bootstrap for official-UVM `run_test`.

use super::classes::{class_expr, class_lvalue, method_return_type};
use super::SlangAstLowerer;
use crate::ast_util::{bool_field, slang_constant_u64, string_field, with_source_span};
use eda_core::ir::{BinOp, EventEdge, EventWatch, Expr, Stmt};
use eda_core::logic::LogicWord;
use regex::Regex;
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
const UVM_CONFIG_DB_DEFAULT_PRECEDENCE: u32 = 1_000;

struct AstUvmConfigScope {
    pattern: String,
    depth: u64,
}

impl SlangAstLowerer {
    pub(super) fn declare_uvm_tlm_fifo(
        &mut self,
        member: &Value,
        name: &str,
    ) -> Result<(), String> {
        let handle_address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("UVM TLM FIFO '{name}' has no symbol address"))?;
        let object_key = format!("{handle_address} {name}");
        let sanitized_name = object_key
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let count =
            self.declare_class_internal_net(&format!("__uvm_tlm_count_{sanitized_name}"), 32);
        self.design.initials.push(Stmt::Bl {
            lv: class_lvalue(&count),
            e: Expr::Const(LogicWord::zero(count.width)),
        });
        let mut slots = Vec::new();
        for slot in 0..4_usize {
            let symbol = self
                .declare_class_internal_net(&format!("__uvm_tlm_slot_{slot}_{sanitized_name}"), 32);
            self.design.initials.push(Stmt::Bl {
                lv: class_lvalue(&symbol),
                e: Expr::Const(LogicWord::zero(symbol.width)),
            });
            slots.push(symbol);
        }
        self.class_objects
            .insert(object_key.clone(), BTreeMap::new());
        self.class_object_types
            .insert(object_key.clone(), "uvm_tlm_fifo".to_string());
        self.uvm_tlm_fifos.borrow_mut().insert(
            object_key,
            super::AstUvmTlmFifo {
                capacity: 4,
                count,
                slots,
            },
        );
        Ok(())
    }

    pub(super) fn declare_uvm_tlm_port(
        &mut self,
        member: &Value,
        name: &str,
    ) -> Result<(), String> {
        let handle_address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("UVM TLM port '{name}' has no symbol address"))?;
        let object_key = format!("{handle_address} {name}");
        let endpoint_class = if string_field(member, "type")
            .is_some_and(|type_name| type_name.contains("uvm_get_peek"))
        {
            if string_field(member, "type")
                .is_some_and(|type_name| type_name.contains("uvm_get_peek_export"))
            {
                "uvm_get_peek_export"
            } else {
                "uvm_get_peek_port"
            }
        } else if string_field(member, "type")
            .is_some_and(|type_name| type_name.contains("uvm_put_export"))
        {
            "uvm_put_export"
        } else {
            "uvm_put_port"
        };
        self.class_objects
            .insert(object_key.clone(), BTreeMap::new());
        self.class_object_types
            .insert(object_key, endpoint_class.to_string());
        Ok(())
    }

    pub(super) fn declare_uvm_sequencer(
        &mut self,
        member: &Value,
        name: &str,
    ) -> Result<(), String> {
        let handle_address = member
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("UVM sequencer '{name}' has no symbol address"))?;
        let object_key = format!("{handle_address} {name}");
        let sanitized_name = object_key
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let request_ready = self.declare_class_internal_net(
            &format!("__uvm_sequence_request_ready_{sanitized_name}"),
            1,
        );
        let request_done =
            self.declare_class_internal_net(&format!("__uvm_sequence_done_{sanitized_name}"), 1);
        let response_head = self.declare_class_internal_net(
            &format!("__uvm_sequence_response_head_{sanitized_name}"),
            32,
        );
        let response_count = self.declare_class_internal_net(
            &format!("__uvm_sequence_response_count_{sanitized_name}"),
            32,
        );
        self.design.initials.extend([
            Stmt::Bl {
                lv: class_lvalue(&response_head),
                e: Expr::Const(LogicWord::zero(response_head.width)),
            },
            Stmt::Bl {
                lv: class_lvalue(&response_count),
                e: Expr::Const(LogicWord::zero(response_count.width)),
            },
        ]);
        let mut response_entries = Vec::new();
        for entry_index in 0..8_usize {
            let transaction_id = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_transaction_id_{entry_index}_{sanitized_name}"),
                32,
            );
            let tag = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_tag_{entry_index}_{sanitized_name}"),
                32,
            );
            let id = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_id_{entry_index}_{sanitized_name}"),
                64,
            );
            let name = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_name_{entry_index}_{sanitized_name}"),
                crate::types::STRING_WIDTH,
            );
            let full_name = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_full_name_{entry_index}_{sanitized_name}"),
                crate::types::STRING_WIDTH,
            );
            let parent_id = self.declare_class_internal_net(
                &format!("__uvm_sequence_response_parent_id_{entry_index}_{sanitized_name}"),
                64,
            );
            let mut slots = Vec::new();
            for slot_index in 0..4_usize {
                let slot = self.declare_class_internal_net(
                    &format!(
                        "__uvm_sequence_response_slot_{entry_index}_{slot_index}_{sanitized_name}"
                    ),
                    crate::types::STRING_WIDTH,
                );
                self.design.initials.push(Stmt::Bl {
                    lv: class_lvalue(&slot),
                    e: Expr::Const(LogicWord::zero(slot.width)),
                });
                slots.push(slot);
            }
            for symbol in [&transaction_id, &tag, &id, &name, &full_name, &parent_id] {
                self.design.initials.push(Stmt::Bl {
                    lv: class_lvalue(symbol),
                    e: Expr::Const(LogicWord::zero(symbol.width)),
                });
            }
            response_entries.push(super::AstUvmSequencerResponseEntry {
                transaction_id,
                tag,
                id,
                name,
                full_name,
                parent_id,
                slots,
            });
        }
        let sequencer_tag = self.declare_class_internal_net(
            &format!("__uvm_sequence_sequencer_tag_{sanitized_name}"),
            32,
        );
        let sequencer_id = self.declare_class_internal_net(
            &format!("__uvm_sequence_sequencer_id_{sanitized_name}"),
            64,
        );
        let sequencer_name = self.declare_class_internal_net(
            &format!("__uvm_sequence_sequencer_name_{sanitized_name}"),
            crate::types::STRING_WIDTH,
        );
        let sequencer_full_name = self.declare_class_internal_net(
            &format!("__uvm_sequence_sequencer_full_name_{sanitized_name}"),
            crate::types::STRING_WIDTH,
        );
        let sequencer_parent_id = self.declare_class_internal_net(
            &format!("__uvm_sequence_sequencer_parent_id_{sanitized_name}"),
            64,
        );
        for symbol in [
            &sequencer_tag,
            &sequencer_id,
            &sequencer_name,
            &sequencer_full_name,
            &sequencer_parent_id,
        ] {
            self.design.initials.push(Stmt::Bl {
                lv: class_lvalue(symbol),
                e: Expr::Const(LogicWord::zero(symbol.width)),
            });
        }
        self.class_objects
            .insert(object_key.clone(), BTreeMap::new());
        self.class_object_types
            .insert(object_key.clone(), "uvm_sequencer".to_string());
        self.class_object_tags
            .insert(object_key.clone(), sequencer_tag);
        self.class_object_ids
            .insert(object_key.clone(), sequencer_id);
        self.class_object_names
            .insert(object_key.clone(), sequencer_name);
        self.class_object_full_names
            .insert(object_key.clone(), sequencer_full_name);
        self.class_object_parent_ids
            .insert(object_key.clone(), sequencer_parent_id);
        self.uvm_sequencers.borrow_mut().insert(
            object_key,
            super::AstUvmSequencer {
                request_ready,
                request_done,
                response_head,
                response_count,
                response_entries,
            },
        );
        Ok(())
    }

    pub(super) fn declare_uvm_sequence_transaction_id(&mut self, object_key: &str) {
        if self
            .uvm_sequence_transaction_ids
            .borrow()
            .contains_key(object_key)
        {
            return;
        }
        let sanitized_name = object_key
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let symbol = self.declare_class_internal_net(
            &format!("__uvm_sequence_transaction_id_{sanitized_name}"),
            32,
        );
        self.design.initials.push(Stmt::Bl {
            lv: class_lvalue(&symbol),
            e: Expr::Const(LogicWord::from_u64(32, u32::MAX as u64)),
        });
        self.uvm_sequence_transaction_ids
            .borrow_mut()
            .insert(object_key.to_string(), symbol);
    }

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
            formals
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
            // Official uvm_root::run_test creates the selected test component
            // under the fixed instance name "uvm_test_top"; the constructor's
            // name default is only a validation contract for this bootstrap.
            let root_name = "uvm_test_top".to_string();

            let root_handle_address = self.next_synthetic_class_handle;
            self.next_synthetic_class_handle = self
                .next_synthetic_class_handle
                .checked_sub(1)
                .ok_or_else(|| "synthetic class-handle address space exhausted".to_string())?;
            let root_object_key = format!("{root_handle_address} __uvm_root");
            self.ensure_class_object_serial()?;
            let root_name_symbol = self.declare_class_internal_net(
                &format!("__uvm_root_name_{root_handle_address}"),
                crate::types::STRING_WIDTH,
            );
            let root_full_name_symbol = self.declare_class_internal_net(
                &format!("__uvm_root_full_name_{root_handle_address}"),
                crate::types::STRING_WIDTH,
            );
            let root_tag = self
                .declare_class_internal_net(&format!("__uvm_root_tag_{root_handle_address}"), 32);
            let root_id = self
                .declare_class_internal_net(&format!("__uvm_root_id_{root_handle_address}"), 64);
            self.design.initials.extend([
                Stmt::Bl {
                    lv: class_lvalue(&root_name_symbol),
                    e: Expr::Const(LogicWord::zero(crate::types::STRING_WIDTH)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&root_full_name_symbol),
                    e: Expr::Const(LogicWord::zero(crate::types::STRING_WIDTH)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&root_tag),
                    e: Expr::Const(LogicWord::zero(32)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&root_id),
                    e: Expr::Const(LogicWord::zero(64)),
                },
            ]);
            self.class_objects
                .insert(root_object_key.clone(), BTreeMap::new());
            self.class_object_types
                .insert(root_object_key.clone(), "uvm_root".to_string());
            self.class_object_names
                .insert(root_object_key.clone(), root_name_symbol);
            self.class_object_full_names
                .insert(root_object_key.clone(), root_full_name_symbol);
            self.class_object_tags
                .insert(root_object_key.clone(), root_tag);
            self.class_object_ids
                .insert(root_object_key.clone(), root_id.clone());
            self.class_object_parent_ids
                .insert(root_object_key.clone(), root_id);
            self.class_object_declared_names
                .borrow_mut()
                .insert(root_object_key.clone(), "__top__".to_string());
            self.class_object_declared_full_names
                .borrow_mut()
                .insert(root_object_key.clone(), String::new());
            self.initialized_class_objects
                .insert(root_object_key.clone());

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
            self.class_object_parent_keys
                .borrow_mut()
                .insert(object_key.clone(), root_object_key.clone());
            self.class_object_declared_names
                .borrow_mut()
                .insert(object_key.clone(), root_name.clone());
            self.class_object_declared_full_names
                .borrow_mut()
                .insert(object_key.clone(), root_name.clone());
            self.class_object_children
                .borrow_mut()
                .insert(root_object_key.clone(), vec![object_key.clone()]);
            self.prepare_uvm_config_waits(&chain, &object_key, handle_address)?;
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
                    root_object_key,
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

    fn prepare_uvm_config_waits(
        &mut self,
        chain: &[(String, Value)],
        object_key: &str,
        handle_address: u64,
    ) -> Result<(), String> {
        let mut calls = Vec::new();
        for (_, class) in chain {
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            for member in members {
                let mut nested_calls = Vec::new();
                collect_uvm_config_wait_calls(member, &mut nested_calls);
                calls.extend(nested_calls.into_iter().cloned());
            }
        }
        for (index, call) in calls.iter().enumerate() {
            let Some(type_name) = self.uvm_config_db_type_name(call, "wait_modified") else {
                continue;
            };
            if !matches!(type_name.as_str(), "int" | "string") {
                return Err(with_source_span(
                    call,
                    format!(
                        "UVM config DB type '{type_name}' is unsupported; expected int or string"
                    ),
                ));
            }
            let empty_arguments = Vec::new();
            let arguments = call
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 3 {
                return Err(with_source_span(
                    call,
                    "UVM config DB wait_modified requires three arguments".into(),
                ));
            }
            let context = unwrap_conversion(&arguments[0]);
            if string_field(context, "kind").as_deref() != Some("NullLiteral")
                && !is_this_reference(context)
            {
                return Err(with_source_span(
                    call,
                    "UVM config DB wait_modified supports only null or this contexts".into(),
                ));
            }
            let field_name = string_literal(unwrap_conversion(&arguments[2])).ok_or_else(|| {
                with_source_span(
                    call,
                    "UVM config DB field names must be string literals".into(),
                )
            })?;
            let previous_active = std::mem::take(&mut *self.active_class_objects.borrow_mut());
            self.active_class_objects
                .borrow_mut()
                .push(object_key.to_string());
            let scope = self.uvm_config_db_scope(
                unwrap_conversion(&arguments[0]),
                unwrap_conversion(&arguments[1]),
            );
            *self.active_class_objects.borrow_mut() = previous_active;
            let scope = scope.map_err(|error| with_source_span(call, error))?;
            let location = source_location(call).ok_or_else(|| {
                with_source_span(
                    call,
                    "UVM config DB wait call has no source location".into(),
                )
            })?;
            let event = self.declare_class_internal_net(
                &format!("__uvm_config_wait_{handle_address}_{index}"),
                1,
            );
            self.design.initials.push(Stmt::Bl {
                lv: class_lvalue(&event),
                e: Expr::Const(LogicWord::zero(1)),
            });
            self.uvm_config_waits.borrow_mut().insert(
                location,
                super::AstUvmConfigWait {
                    type_name,
                    scope: scope.pattern,
                    field_name,
                    net: event.base,
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
        let mut actuals = actuals;
        if let Some(root_actual) = actuals.first_mut() {
            *root_actual = json!({
                "kind": "StringLiteral",
                "type": formals
                    .first()
                    .and_then(|formal| string_field(formal, "type"))
                    .unwrap_or_else(|| "string".into()),
                "literal": "uvm_test_top",
            });
        }
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
            *self.active_uvm_phase.borrow_mut() = Some(phase_name.to_string());
            let phase_statement = self.lower_class_task_call(&phase_call);
            self.active_uvm_phase.borrow_mut().take();
            let Some(phase_statement) = phase_statement? else {
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

    pub(super) fn lower_uvm_config_wait_call(&self, value: &Value) -> Result<Option<Stmt>, String> {
        let Some(type_name) = self.uvm_config_db_type_name(value, "wait_modified") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM config DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 3 {
            return Err("UVM config DB wait_modified requires three arguments".into());
        }
        let context = unwrap_conversion(&arguments[0]);
        if string_field(context, "kind").as_deref() != Some("NullLiteral")
            && !is_this_reference(context)
        {
            return Err("UVM config DB wait_modified supports only null or this contexts".into());
        }
        let scope = self.uvm_config_db_scope(
            unwrap_conversion(&arguments[0]),
            unwrap_conversion(&arguments[1]),
        )?;
        let field_name = string_literal(unwrap_conversion(&arguments[2]))
            .ok_or_else(|| "UVM config DB field names must be string literals".to_string())?;
        let location = source_location(value)
            .ok_or_else(|| "UVM config DB wait call has no source location".to_string())?;
        let wait = self.uvm_config_waits.borrow().get(&location).cloned();
        let Some(wait) = wait else {
            return Err(with_source_span(
                value,
                "UVM config DB wait_modified was not prepared by run_test".into(),
            ));
        };
        if wait.type_name != type_name
            || wait.scope != scope.pattern
            || wait.field_name != field_name
        {
            return Err(with_source_span(
                value,
                "UVM config DB wait_modified changed after run_test preparation".into(),
            ));
        }
        Ok(Some(Stmt::EventWait {
            watches: vec![EventWatch {
                net: wait.net,
                edge: EventEdge::Any,
            }],
            body: Box::new(Stmt::Block(Vec::new())),
        }))
    }

    pub(super) fn lower_uvm_config_set_call(&self, value: &Value) -> Result<Option<Stmt>, String> {
        let Some(type_name) = self.uvm_config_db_type_name(value, "set") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM config DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 4 {
            return Err("UVM config DB set requires four arguments".into());
        }
        let scope = self.uvm_config_db_scope(
            unwrap_conversion(&arguments[0]),
            unwrap_conversion(&arguments[1]),
        )?;
        let precedence = self.uvm_config_db_precedence(scope.depth)?;
        Self::uvm_config_scope_match(&scope.pattern, "")?;
        let field_name = string_literal(unwrap_conversion(&arguments[2]))
            .ok_or_else(|| "UVM config DB field names must be string literals".to_string())?;
        let config_value = unwrap_conversion(&arguments[3]).clone();
        if !matches!(
            string_field(&config_value, "kind").as_deref(),
            Some("IntegerLiteral" | "StringLiteral")
        ) {
            return Err("UVM config DB bootstrap values must be integer or string literals".into());
        }
        let mut triggers = Vec::new();
        for wait in self.uvm_config_waits.borrow().values() {
            if wait.type_name == type_name
                && wait.field_name == field_name
                && Self::uvm_config_scope_match(&scope.pattern, &wait.scope)?
            {
                triggers.push(Self::uvm_config_event_trigger(wait.net));
            }
        }
        let mut entries = self.uvm_config_entries.borrow_mut();
        entries.retain(|entry| {
            !(entry.type_name == type_name
                && entry.scope == scope.pattern
                && entry.field_name == field_name)
        });
        entries.push(super::AstUvmConfigEntry {
            type_name,
            scope: scope.pattern,
            field_name,
            precedence,
            name_priority: true,
            type_priority: false,
            value: config_value,
        });
        Ok(Some(Stmt::Block(triggers)))
    }

    pub(super) fn lower_uvm_resource_set_call(
        &self,
        value: &Value,
    ) -> Result<Option<Stmt>, String> {
        let method_name = string_field(value, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .ok_or_else(|| "UVM resource DB call has no subroutine name".to_string())?;
        if !matches!(
            method_name.as_str(),
            "set"
                | "set_default"
                | "set_anonymous"
                | "set_override"
                | "set_override_type"
                | "set_override_name"
        ) {
            return Ok(None);
        }
        let Some(type_name) = self.uvm_resource_db_type_name(value, &method_name) else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM resource DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if method_name == "set_default" {
            if arguments.len() != 2 {
                return Err("UVM resource DB set_default requires two arguments".into());
            }
            let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
                .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
            Self::uvm_config_scope_match(&scope, "")?;
            let field_name = string_literal(unwrap_conversion(&arguments[1]))
                .ok_or_else(|| "UVM resource DB names must be string literals".to_string())?;
            self.uvm_config_entries
                .borrow_mut()
                .push(super::AstUvmConfigEntry {
                    type_name: type_name.clone(),
                    scope,
                    field_name,
                    precedence: UVM_CONFIG_DB_DEFAULT_PRECEDENCE,
                    name_priority: false,
                    type_priority: false,
                    value: Self::uvm_resource_default(&type_name),
                });
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        if method_name == "set_anonymous" {
            if arguments.len() != 2 && arguments.len() != 3 {
                return Err(
                    "UVM resource DB set_anonymous requires two arguments plus optional accessor"
                        .into(),
                );
            }
            if arguments.len() == 3
                && string_field(unwrap_conversion(&arguments[2]), "kind").as_deref()
                    != Some("NullLiteral")
            {
                return Err("UVM resource DB accessors are not yet supported".into());
            }
            let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
                .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
            Self::uvm_config_scope_match(&scope, "")?;
            let resource_value = unwrap_conversion(&arguments[1]).clone();
            if !matches!(
                string_field(&resource_value, "kind").as_deref(),
                Some("IntegerLiteral" | "StringLiteral")
            ) {
                return Err(
                    "UVM resource DB bootstrap values must be integer or string literals".into(),
                );
            }
            self.uvm_config_entries
                .borrow_mut()
                .push(super::AstUvmConfigEntry {
                    type_name,
                    scope,
                    field_name: String::new(),
                    precedence: UVM_CONFIG_DB_DEFAULT_PRECEDENCE,
                    name_priority: false,
                    type_priority: false,
                    value: resource_value,
                });
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        if arguments.len() != 3 && arguments.len() != 4 {
            return Err(format!(
                "UVM resource DB {method_name} requires three arguments plus optional accessor"
            ));
        }
        if arguments.len() == 4
            && string_field(unwrap_conversion(&arguments[3]), "kind").as_deref()
                != Some("NullLiteral")
        {
            return Err("UVM resource DB accessors are not yet supported".into());
        }
        let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
        Self::uvm_config_scope_match(&scope, "")?;
        let field_name = string_literal(unwrap_conversion(&arguments[1]))
            .ok_or_else(|| "UVM resource DB names must be string literals".to_string())?;
        let resource_value = unwrap_conversion(&arguments[2]).clone();
        if !matches!(
            string_field(&resource_value, "kind").as_deref(),
            Some("IntegerLiteral" | "StringLiteral")
        ) {
            return Err(
                "UVM resource DB bootstrap values must be integer or string literals".into(),
            );
        }
        let mut entries = self.uvm_config_entries.borrow_mut();
        if method_name == "set" {
            entries.retain(|entry| {
                !(entry.type_name == type_name
                    && entry.scope == scope
                    && entry.field_name == field_name
                    && !entry.name_priority
                    && !entry.type_priority)
            });
        }
        let (name_priority, type_priority) = match method_name.as_str() {
            "set" => (false, false),
            "set_override" => (true, true),
            "set_override_type" => (false, true),
            _ => (true, false),
        };
        entries.push(super::AstUvmConfigEntry {
            type_name,
            scope,
            field_name,
            precedence: UVM_CONFIG_DB_DEFAULT_PRECEDENCE,
            name_priority,
            type_priority,
            value: resource_value,
        });
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_config_get_conditional(
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
        let (call, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
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
        let Some(type_name) = self.uvm_config_db_type_name(call, "get") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM config DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 4 {
            return Err("UVM config DB get requires four arguments".into());
        }
        let scope = self.uvm_config_db_scope(
            unwrap_conversion(&arguments[0]),
            unwrap_conversion(&arguments[1]),
        )?;
        let field_name = string_literal(unwrap_conversion(&arguments[2]))
            .ok_or_else(|| "UVM config DB field names must be string literals".to_string())?;
        let target = arguments[3]
            .get("left")
            .ok_or_else(|| "UVM config DB get output must be assignable".to_string())?;
        let mut selected_entry: Option<&super::AstUvmConfigEntry> = None;
        let entries = self.uvm_config_entries.borrow();
        for entry in entries.iter() {
            if entry.type_name == type_name
                && entry.field_name == field_name
                && Self::uvm_config_scope_match(&entry.scope, &scope.pattern)?
                && selected_entry
                    .as_ref()
                    .is_none_or(|selected| Self::uvm_name_priority_ge(entry, selected))
            {
                selected_entry = Some(entry);
            }
        }
        let entry = selected_entry.map(|entry| entry.value.clone());
        let Some(entry) = entry else {
            let failure = if negated {
                statement.get("ifTrue")
            } else {
                statement.get("ifFalse")
            };
            return failure
                .map(|failure| self.lower_statement(failure).map(Some))
                .transpose()
                .map(|statement| statement.unwrap_or_else(|| Some(Stmt::Block(Vec::new()))));
        };
        let assignment = Stmt::Bl {
            lv: self.lower_lvalue(target)?,
            e: self.lower_expression(&entry)?,
        };
        let success = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let success = success
            .map(|success| self.lower_statement(success))
            .transpose()?;
        let mut statements = vec![assignment];
        statements.extend(success);
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_resource_read_conditional(
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
        let (call, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
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
        let Some(type_name) = self.uvm_resource_db_type_name(call, "read_by_name") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM resource DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 3 && arguments.len() != 4 {
            return Err(
                "UVM resource DB read_by_name requires three arguments plus optional accessor"
                    .into(),
            );
        }
        if arguments.len() == 4
            && string_field(unwrap_conversion(&arguments[3]), "kind").as_deref()
                != Some("NullLiteral")
        {
            return Err("UVM resource DB accessors are not yet supported".into());
        }
        let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
        let field_name = string_literal(unwrap_conversion(&arguments[1]))
            .ok_or_else(|| "UVM resource DB names must be string literals".to_string())?;
        let target = arguments[2]
            .get("left")
            .ok_or_else(|| "UVM resource DB read output must be assignable".to_string())?;
        let mut selected_entry: Option<&super::AstUvmConfigEntry> = None;
        let entries = self.uvm_config_entries.borrow();
        for entry in entries.iter() {
            if entry.type_name == type_name
                && entry.field_name == field_name
                && Self::uvm_config_scope_match(&entry.scope, &scope)?
                && selected_entry
                    .as_ref()
                    .is_none_or(|selected| Self::uvm_name_priority_ge(entry, selected))
            {
                selected_entry = Some(entry);
            }
        }
        let entry = selected_entry.map(|entry| entry.value.clone());
        let Some(entry) = entry else {
            let failure = if negated {
                statement.get("ifTrue")
            } else {
                statement.get("ifFalse")
            };
            return failure
                .map(|failure| self.lower_statement(failure).map(Some))
                .transpose()
                .map(|statement| statement.unwrap_or_else(|| Some(Stmt::Block(Vec::new()))));
        };
        let assignment = Stmt::Bl {
            lv: self.lower_lvalue(target)?,
            e: self.lower_expression(&entry)?,
        };
        let success = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let success = success
            .map(|success| self.lower_statement(success))
            .transpose()?;
        let mut statements = vec![assignment];
        statements.extend(success);
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_resource_write_conditional(
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
        let (call, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
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
        let Some(type_name) = self.uvm_resource_db_type_name(call, "write_by_name") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM resource DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 3 && arguments.len() != 4 {
            return Err(
                "UVM resource DB write_by_name requires three arguments plus optional accessor"
                    .into(),
            );
        }
        if arguments.len() == 4
            && string_field(unwrap_conversion(&arguments[3]), "kind").as_deref()
                != Some("NullLiteral")
        {
            return Err("UVM resource DB accessors are not yet supported".into());
        }
        let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
        let field_name = string_literal(unwrap_conversion(&arguments[1]))
            .ok_or_else(|| "UVM resource DB names must be string literals".to_string())?;
        let write_value = unwrap_conversion(&arguments[2]).clone();
        if !matches!(
            string_field(&write_value, "kind").as_deref(),
            Some("IntegerLiteral" | "StringLiteral")
        ) {
            return Err(
                "UVM resource DB bootstrap values must be integer or string literals".into(),
            );
        }

        let mut selected_index: Option<usize> = None;
        {
            let entries = self.uvm_config_entries.borrow();
            for (index, entry) in entries.iter().enumerate() {
                if entry.type_name == type_name
                    && entry.field_name == field_name
                    && Self::uvm_config_scope_match(&entry.scope, &scope)?
                    && selected_index.is_none_or(|selected| {
                        Self::uvm_name_priority_ge(entry, &entries[selected])
                    })
                {
                    selected_index = Some(index);
                }
            }
        }
        let Some(selected_index) = selected_index else {
            let failure = if negated {
                statement.get("ifTrue")
            } else {
                statement.get("ifFalse")
            };
            return failure
                .map(|failure| self.lower_statement(failure).map(Some))
                .transpose()
                .map(|statement| statement.unwrap_or_else(|| Some(Stmt::Block(Vec::new()))));
        };
        self.uvm_config_entries.borrow_mut()[selected_index].value = write_value;
        let success = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let success = success
            .map(|success| self.lower_statement(success))
            .transpose()?;
        Ok(Some(Stmt::Block(success.into_iter().collect())))
    }

    pub(super) fn lower_uvm_resource_read_type_conditional(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some((call, negated)) = uvm_conditional_call(statement)? else {
            return Ok(None);
        };
        let Some(type_name) = self.uvm_resource_db_type_name(call, "read_by_type") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM resource DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 && arguments.len() != 3 {
            return Err(
                "UVM resource DB read_by_type requires two arguments plus optional accessor".into(),
            );
        }
        if arguments.len() == 3
            && string_field(unwrap_conversion(&arguments[2]), "kind").as_deref()
                != Some("NullLiteral")
        {
            return Err("UVM resource DB accessors are not yet supported".into());
        }
        let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
        let target = arguments[1]
            .get("left")
            .ok_or_else(|| "UVM resource DB read output must be assignable".to_string())?;

        let mut selected_entry: Option<&super::AstUvmConfigEntry> = None;
        let entries = self.uvm_config_entries.borrow();
        for entry in entries.iter() {
            if entry.type_name == type_name
                && Self::uvm_config_scope_match(&entry.scope, &scope)?
                && selected_entry
                    .as_ref()
                    .is_none_or(|selected| Self::uvm_type_priority_ge(entry, selected))
            {
                selected_entry = Some(entry);
            }
        }
        let entry = selected_entry.map(|entry| entry.value.clone());
        let Some(entry) = entry else {
            let failure = if negated {
                statement.get("ifTrue")
            } else {
                statement.get("ifFalse")
            };
            return failure
                .map(|failure| self.lower_statement(failure).map(Some))
                .transpose()
                .map(|statement| statement.unwrap_or_else(|| Some(Stmt::Block(Vec::new()))));
        };
        let assignment = Stmt::Bl {
            lv: self.lower_lvalue(target)?,
            e: self.lower_expression(&entry)?,
        };
        let success = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let success = success
            .map(|success| self.lower_statement(success))
            .transpose()?;
        let mut statements = vec![assignment];
        statements.extend(success);
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_resource_write_type_conditional(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some((call, negated)) = uvm_conditional_call(statement)? else {
            return Ok(None);
        };
        let Some(type_name) = self.uvm_resource_db_type_name(call, "write_by_type") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM resource DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 && arguments.len() != 3 {
            return Err(
                "UVM resource DB write_by_type requires two arguments plus optional accessor"
                    .into(),
            );
        }
        if arguments.len() == 3
            && string_field(unwrap_conversion(&arguments[2]), "kind").as_deref()
                != Some("NullLiteral")
        {
            return Err("UVM resource DB accessors are not yet supported".into());
        }
        let scope = string_literal_allow_empty(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM resource DB scopes must be string literals".to_string())?;
        let write_value = unwrap_conversion(&arguments[1]).clone();
        if !matches!(
            string_field(&write_value, "kind").as_deref(),
            Some("IntegerLiteral" | "StringLiteral")
        ) {
            return Err(
                "UVM resource DB bootstrap values must be integer or string literals".into(),
            );
        }

        let mut selected_index: Option<usize> = None;
        {
            let entries = self.uvm_config_entries.borrow();
            for (index, entry) in entries.iter().enumerate() {
                if entry.type_name == type_name
                    && Self::uvm_config_scope_match(&entry.scope, &scope)?
                    && selected_index.is_none_or(|selected| {
                        Self::uvm_type_priority_ge(entry, &entries[selected])
                    })
                {
                    selected_index = Some(index);
                }
            }
        }
        let Some(selected_index) = selected_index else {
            let failure = if negated {
                statement.get("ifTrue")
            } else {
                statement.get("ifFalse")
            };
            return failure
                .map(|failure| self.lower_statement(failure).map(Some))
                .transpose()
                .map(|statement| statement.unwrap_or_else(|| Some(Stmt::Block(Vec::new()))));
        };
        self.uvm_config_entries.borrow_mut()[selected_index].value = write_value;
        let success = if negated {
            statement.get("ifFalse")
        } else {
            statement.get("ifTrue")
        };
        let success = success
            .map(|success| self.lower_statement(success))
            .transpose()?;
        Ok(Some(Stmt::Block(success.into_iter().collect())))
    }

    pub(super) fn lower_uvm_config_exists_conditional(
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
        let (call, negated) = if string_field(condition, "kind").as_deref() == Some("UnaryOp")
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
        let Some(type_name) = self.uvm_config_db_type_name(call, "exists") else {
            return Ok(None);
        };
        if !matches!(type_name.as_str(), "int" | "string") {
            return Err(format!(
                "UVM config DB type '{type_name}' is unsupported; expected int or string"
            ));
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() < 3 || arguments.len() > 4 {
            return Err(
                "UVM config DB exists requires three arguments plus an optional spell check".into(),
            );
        }
        let mut spell_check = false;
        if arguments.len() == 4 {
            match (
                string_field(unwrap_conversion(&arguments[3]), "kind").as_deref(),
                string_field(unwrap_conversion(&arguments[3]), "value").as_deref(),
            ) {
                (Some("IntegerLiteral"), Some("0")) => {}
                (Some("IntegerLiteral"), Some("1")) => spell_check = true,
                _ => return Err("UVM config DB spell check must be a literal 0 or 1".into()),
            }
        }
        let scope = self.uvm_config_db_scope(
            unwrap_conversion(&arguments[0]),
            unwrap_conversion(&arguments[1]),
        )?;
        let field_name = string_literal(unwrap_conversion(&arguments[2]))
            .ok_or_else(|| "UVM config DB field names must be string literals".to_string())?;
        let mut exists = false;
        for entry in self.uvm_config_entries.borrow().iter() {
            if entry.type_name == type_name
                && entry.field_name == field_name
                && Self::uvm_config_scope_match(&entry.scope, &scope.pattern)?
            {
                exists = true;
                break;
            }
        }
        let selects_if_true = exists != negated;
        let selected = if selects_if_true {
            statement.get("ifTrue")
        } else {
            statement.get("ifFalse")
        };
        let selected = selected
            .map(|selected| self.lower_statement(selected))
            .transpose()?;
        let mut statements = Vec::new();
        if !exists && spell_check {
            let alternatives = self.uvm_config_spell_alternatives(&field_name);
            let suggestion = if alternatives.is_empty() {
                format!("{field_name} not located, no alternatives to suggest")
            } else {
                format!(
                    "{field_name} not located, did you mean {}",
                    alternatives.join("|")
                )
            };
            statements.push(Stmt::Display {
                parts: vec![eda_core::ir::DispPart::Str(format!(
                    "[UVM_INFO][UVM/CONFIGDB/SPELLCHK][UVM_NONE] {suggestion}\n"
                ))],
                args: Vec::new(),
                monitor: false,
            });
        }
        if let Some(selected) = selected {
            statements.push(selected);
        }
        Ok(Some(Stmt::Block(statements)))
    }

    fn uvm_config_spell_alternatives(&self, field_name: &str) -> Vec<String> {
        let mut minimum = usize::MAX;
        let mut alternatives = Vec::new();
        for entry in self.uvm_config_entries.borrow().iter() {
            let distance = Self::levenshtein_distance(&entry.field_name, field_name);
            if distance == minimum {
                if !alternatives.contains(&entry.field_name) {
                    alternatives.push(entry.field_name.clone());
                }
            } else if distance < minimum {
                minimum = distance;
                alternatives.clear();
                alternatives.push(entry.field_name.clone());
            }
        }
        alternatives
    }

    fn uvm_config_db_scope(
        &self,
        context: &Value,
        inst_name: &Value,
    ) -> Result<AstUvmConfigScope, String> {
        let inst_name = string_literal_allow_empty(inst_name)
            .ok_or_else(|| "UVM config DB instance names must be string literals".to_string())?;
        let context_key = if string_field(context, "kind").as_deref() == Some("NullLiteral") {
            None
        } else if is_this_reference(context) {
            self.active_class_objects
                .borrow()
                .last()
                .cloned()
                .map(Ok::<String, String>)
        } else {
            self.class_object_key_for_value(context)?.map(Ok)
        }
        .transpose()?;
        let depth = if let Some(context_key) = &context_key {
            self.class_object_depth(context_key)?
        } else {
            0
        };
        let base = if let Some(context_key) = context_key {
            self.class_object_declared_full_names
                .borrow()
                .get(&context_key)
                .cloned()
                .ok_or_else(|| format!("UVM config DB context '{context_key}' has no full name"))?
        } else {
            String::new()
        };
        let regex_body =
            if inst_name.len() > 2 && inst_name.starts_with('/') && inst_name.ends_with('/') {
                Some(inst_name[1..inst_name.len() - 1].to_string())
            } else {
                None
            };
        let pattern = if let Some(regex_body) = regex_body {
            if base.is_empty() {
                format!("/{regex_body}/")
            } else {
                format!("/{base}\\.({regex_body})/")
            }
        } else if base.is_empty() {
            inst_name
        } else if inst_name.is_empty() {
            base
        } else {
            format!("{base}.{inst_name}")
        };
        Ok(AstUvmConfigScope { pattern, depth })
    }

    fn uvm_config_db_precedence(&self, context_depth: u64) -> Result<u32, String> {
        if self.active_uvm_phase.borrow().as_deref() != Some("build_phase") {
            return Ok(UVM_CONFIG_DB_DEFAULT_PRECEDENCE);
        }
        let context_depth = u32::try_from(context_depth)
            .map_err(|_| "UVM component depth overflows".to_string())?;
        UVM_CONFIG_DB_DEFAULT_PRECEDENCE
            .checked_sub(context_depth)
            .ok_or_else(|| {
                format!("UVM component depth {context_depth} exceeds config DB default precedence")
            })
    }

    fn uvm_glob_match(pattern: &str, text: &str) -> bool {
        fn match_bytes(pattern: &[u8], text: &[u8]) -> bool {
            match pattern.first() {
                None => true,
                Some(b'*') => {
                    match_bytes(&pattern[1..], text)
                        || (!text.is_empty() && match_bytes(pattern, &text[1..]))
                }
                Some(b'?') => !text.is_empty() && match_bytes(&pattern[1..], &text[1..]),
                Some(byte) => text.first() == Some(byte) && match_bytes(&pattern[1..], &text[1..]),
            }
        }
        match_bytes(pattern.as_bytes(), text.as_bytes())
    }

    fn uvm_config_scope_match(pattern: &str, text: &str) -> Result<bool, String> {
        let Some(regex_body) =
            (pattern.len() > 2 && pattern.starts_with('/') && pattern.ends_with('/'))
                .then(|| &pattern[1..pattern.len() - 1])
        else {
            return Ok(Self::uvm_glob_match(pattern, text));
        };
        let regex = Regex::new(&format!("^(?:{regex_body})$"))
            .map_err(|error| format!("invalid UVM config DB regex scope: {error}"))?;
        Ok(regex.is_match(text))
    }

    fn uvm_name_priority_ge(
        candidate: &super::AstUvmConfigEntry,
        selected: &super::AstUvmConfigEntry,
    ) -> bool {
        if candidate.name_priority != selected.name_priority {
            return candidate.name_priority;
        }
        candidate.precedence >= selected.precedence
    }

    fn uvm_type_priority_ge(
        candidate: &super::AstUvmConfigEntry,
        selected: &super::AstUvmConfigEntry,
    ) -> bool {
        if candidate.type_priority != selected.type_priority {
            return candidate.type_priority;
        }
        candidate.precedence >= selected.precedence
    }

    fn uvm_resource_default(type_name: &str) -> Value {
        if type_name == "string" {
            json!({
                "kind": "StringLiteral",
                "type": "string",
                "literal": "",
            })
        } else {
            json!({
                "kind": "IntegerLiteral",
                "type": "int",
                "value": "0",
            })
        }
    }

    fn levenshtein_distance(left: &str, right: &str) -> usize {
        let left = left.as_bytes();
        let right = right.as_bytes();
        let mut previous = (0..=right.len()).collect::<Vec<_>>();
        for (left_index, left_byte) in left.iter().enumerate() {
            let mut current = vec![left_index + 1];
            for (right_index, right_byte) in right.iter().enumerate() {
                let substitution = previous[right_index] + usize::from(left_byte != right_byte);
                current.push(
                    previous[right_index + 1]
                        .saturating_add(1)
                        .min(current[right_index].saturating_add(1))
                        .min(substitution),
                );
            }
            previous = current;
        }
        previous[right.len()]
    }

    fn uvm_config_event_trigger(net: usize) -> Stmt {
        let range = super::hierarchy::full_range(1);
        let current = Expr::Ref {
            net,
            decl_width: 1,
            range: range.clone(),
            signed: false,
        };
        let next = Expr::Cond {
            c: Box::new(Expr::Bin {
                op: BinOp::CaseEq,
                a: Box::new(current.clone()),
                b: Box::new(Expr::Const(LogicWord::new_const(
                    1,
                    eda_core::logic::LogicValue::X,
                ))),
            }),
            t: Box::new(Expr::Const(LogicWord::zero(1))),
            f: Box::new(Expr::Un {
                op: eda_core::ir::UnOp::Not,
                a: Box::new(current),
            }),
        };
        Stmt::Bl {
            lv: eda_core::ir::LValue {
                net,
                decl_width: 1,
                range,
            },
            e: next,
        }
    }

    pub(super) fn uvm_config_db_type_name(
        &self,
        call: &Value,
        method_name: &str,
    ) -> Option<String> {
        self.uvm_db_type_name(call, method_name, "uvm_config_db")
    }

    pub(super) fn uvm_resource_db_type_name(
        &self,
        call: &Value,
        method_name: &str,
    ) -> Option<String> {
        let type_name = self.uvm_db_type_name(call, method_name, "uvm_resource_db")?;
        if matches!(type_name.as_str(), "int" | "string") {
            return Some(type_name);
        }

        let argument_index = match method_name {
            "set" | "set_override" | "set_override_type" | "set_override_name" => Some(2),
            "set_anonymous" => Some(1),
            "read_by_name" => Some(2),
            "write_by_name" => Some(2),
            "read_by_type" => Some(1),
            "write_by_type" => Some(1),
            _ => None,
        };
        if let Some(index) = argument_index {
            let argument = call.get("arguments")?.get(index)?;
            let type_name = if string_field(argument, "kind").as_deref() == Some("Assignment") {
                string_field(argument, "type")?
            } else {
                match string_field(unwrap_conversion(argument), "kind").as_deref() {
                    Some("IntegerLiteral") => "int".to_string(),
                    Some("StringLiteral") => "string".to_string(),
                    _ => string_field(argument, "type")?,
                }
            };
            if matches!(type_name.as_str(), "int" | "string") {
                return Some(type_name);
            }
        }
        if method_name == "set_default" {
            if let Some(type_name) = self.uvm_resource_return_type(call) {
                return Some(type_name);
            }
        }
        Some(type_name)
    }

    fn uvm_resource_return_type(&self, call: &Value) -> Option<String> {
        let return_type_address = string_field(call, "type")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        for class in self.class_definitions.values() {
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            let Some(alias) = members.iter().find(|member| {
                string_field(member, "kind").as_deref() == Some("TypeAlias")
                    && member.get("addr").and_then(Value::as_u64) == Some(return_type_address)
            }) else {
                continue;
            };
            let target = string_field(alias, "target")?;
            if target.contains("uvm_resource#(int)") {
                return Some("int".to_string());
            }
            if target.contains("uvm_resource#(string)") {
                return Some("string".to_string());
            }
        }
        None
    }

    fn uvm_db_type_name(
        &self,
        call: &Value,
        method_name: &str,
        generic_class_name: &str,
    ) -> Option<String> {
        let address = string_field(call, "subroutine")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        for class in self.class_definitions.values() {
            if string_field(class, "genericClass")
                .unwrap_or_default()
                .split_whitespace()
                .next_back()
                != Some(generic_class_name)
            {
                continue;
            }
            let empty_members = Vec::new();
            let members = class
                .get("members")
                .and_then(Value::as_array)
                .unwrap_or(&empty_members);
            if !members.iter().any(|member| {
                string_field(member, "kind").as_deref() == Some("Subroutine")
                    && string_field(member, "name").as_deref() == Some(method_name)
                    && member.get("addr").and_then(Value::as_u64) == Some(address)
            }) {
                continue;
            }
            let parameter = members.iter().find(|member| {
                string_field(member, "kind").as_deref() == Some("TypeParameter")
                    && string_field(member, "name").as_deref() == Some("T")
            })?;
            let parameter_type = parameter.get("type")?;
            return string_field(parameter_type, "name")
                .or_else(|| string_field(parameter_type, "type"))
                .or_else(|| Some(parameter_type.to_string()))
                .map(|type_name| {
                    type_name
                        .split_whitespace()
                        .next_back()
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string()
                });
        }
        None
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

    pub(super) fn lower_uvm_report_call(&self, value: &Value) -> Result<Option<Stmt>, String> {
        self.lower_uvm_report_call_with(value, None, None, None, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_uvm_report_call_with(
        &self,
        value: &Value,
        severity_override: Option<&str>,
        message_override: Option<&str>,
        action_override: Option<u64>,
        apply_verbosity_filter: bool,
    ) -> Result<Option<Stmt>, String> {
        let Some(subroutine) = string_field(value, "subroutine") else {
            return Ok(None);
        };
        let original_severity = match subroutine.split_whitespace().next_back() {
            Some("uvm_report_info") => "INFO",
            Some("uvm_report_warning") => "WARNING",
            Some("uvm_report_error") => "ERROR",
            Some("uvm_report_fatal") => "FATAL",
            _ => return Ok(None),
        };
        let severity = severity_override.unwrap_or(original_severity);
        if value.get("thisClass").is_some() {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() < 2 || arguments.len() > 7 {
            return Err(format!(
                "UVM {severity} report requires two arguments plus optional elaborated defaults"
            ));
        }
        let id = string_literal(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM report IDs must be string literals".to_string())?;
        let message = if let Some(message) = message_override {
            message.to_string()
        } else {
            string_literal_allow_empty(unwrap_conversion(&arguments[1]))
                .ok_or_else(|| "UVM report messages must be string literals".to_string())?
        };
        let default_verbosity = if original_severity == "INFO" {
            "UVM_MEDIUM"
        } else {
            "UVM_NONE"
        };
        let mut verbosity = default_verbosity.to_string();
        if arguments.len() > 2 {
            let verbosity_argument = unwrap_conversion(&arguments[2]);
            if let Some(symbol) = string_field(verbosity_argument, "symbol") {
                let candidate = symbol.split_whitespace().next_back().unwrap_or_default();
                if !matches!(
                    candidate,
                    "UVM_NONE" | "UVM_LOW" | "UVM_MEDIUM" | "UVM_HIGH" | "UVM_FULL"
                ) {
                    return Err(format!("unsupported UVM report verbosity '{candidate}'"));
                }
                verbosity = candidate.to_string();
            } else if string_field(verbosity_argument, "kind").as_deref() == Some("IntegerLiteral")
            {
                let value = string_field(verbosity_argument, "value")
                    .ok_or_else(|| "UVM report verbosity has no value".to_string())?;
                let verbosity_value = match value.as_str() {
                    "0" => "UVM_NONE",
                    "100" => "UVM_LOW",
                    "200" => "UVM_MEDIUM",
                    "300" => "UVM_HIGH",
                    "400" => "UVM_FULL",
                    other => return Err(format!("unsupported UVM report verbosity '{other}'")),
                };
                verbosity = verbosity_value.to_string();
            } else {
                return Err("UVM report verbosity must be a literal".into());
            }
        }
        if arguments.len() > 3
            && string_field(unwrap_conversion(&arguments[3]), "literal")
                .as_deref()
                .is_none()
        {
            return Err("UVM report filenames must be string literals".into());
        }
        if arguments.len() > 4
            && string_field(unwrap_conversion(&arguments[4]), "value")
                .as_deref()
                .is_none()
        {
            return Err("UVM report line numbers must be integer literals".into());
        }
        if arguments.len() > 5
            && string_field(unwrap_conversion(&arguments[5]), "literal")
                .as_deref()
                .is_none()
        {
            return Err("UVM report contexts must be string literals".into());
        }
        if arguments.len() > 6
            && !matches!(
                string_field(unwrap_conversion(&arguments[6]), "value").as_deref(),
                Some("0" | "1")
            )
        {
            return Err("UVM report action must be a literal 0 or 1".into());
        }
        let report_verbosity = uvm_report_verbosity_value(&verbosity)
            .ok_or_else(|| format!("unsupported UVM report verbosity '{verbosity}'"))?;
        if apply_verbosity_filter
            && report_verbosity > self.uvm_report_verbosity_threshold(severity, &id)
        {
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        let report_action =
            action_override.unwrap_or_else(|| self.uvm_report_action(severity, &id));
        let mut finish_after_report = false;
        if report_action != 0 {
            let mut controls = self.uvm_report_controls.borrow_mut();
            *controls
                .severity_counts
                .entry(severity.to_string())
                .or_insert(0) += 1;
            *controls.id_counts.entry(id.clone()).or_insert(0) += 1;
            if report_action & 4 != 0 && controls.max_quit_count != 0 {
                controls.quit_count = controls.quit_count.saturating_add(1);
                finish_after_report = controls.quit_count >= controls.max_quit_count;
            }
        }
        if report_action == 0 {
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        if severity == "FATAL" {
            let mut fatal_statements = Vec::new();
            if report_action & 1 != 0 {
                fatal_statements.push(Stmt::Display {
                    parts: vec![eda_core::ir::DispPart::Str(format!(
                        "[UVM_FATAL][{id}][{verbosity}] {message}\n"
                    ))],
                    args: Vec::new(),
                    monitor: false,
                });
            }
            if report_action & 8 != 0 || finish_after_report {
                fatal_statements.push(Stmt::Finish);
            }
            return Ok(Some(Stmt::Block(fatal_statements)));
        }
        let display = if report_action & 1 == 0 {
            Stmt::Block(Vec::new())
        } else {
            Stmt::Display {
                parts: vec![eda_core::ir::DispPart::Str(format!(
                    "[UVM_{severity}][{id}][{verbosity}] {message}\n"
                ))],
                args: Vec::new(),
                monitor: false,
            }
        };
        if finish_after_report {
            return Ok(Some(Stmt::Block(vec![display, Stmt::Finish])));
        }
        Ok(Some(display))
    }

    pub(super) fn lower_uvm_macro_report_list(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let statement_kind = string_field(statement, "kind");
        let container = match statement_kind.as_deref() {
            Some("Block") => {
                let Some(body) = statement.get("body") else {
                    return Ok(None);
                };
                body
            }
            Some("List") => statement,
            _ => return Ok(None),
        };
        let Some(items) = container.get("list").and_then(Value::as_array) else {
            return Ok(None);
        };
        if items.len() != 3
            || string_field(&items[0], "kind").as_deref() != Some("VariableDeclaration")
            || !string_field(&items[0], "symbol")
                .is_some_and(|symbol| symbol.ends_with("_local_report_object_"))
        {
            return Ok(None);
        }
        let Some(assignment) = items[1].as_object().and_then(|fields| {
            fields
                .get("expr")
                .filter(|expr| string_field(expr, "kind").as_deref() == Some("Assignment"))
        }) else {
            return Ok(None);
        };
        let Some(report_object_call) = assignment
            .get("right")
            .filter(|call| string_field(call, "kind").as_deref() == Some("Call"))
            .filter(|call| {
                string_field(call, "subroutine")
                    .and_then(|subroutine| {
                        subroutine
                            .split_whitespace()
                            .next_back()
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some("uvm_get_report_object")
            })
        else {
            return Ok(None);
        };
        if report_object_call.get("thisClass").is_none() {
            return Ok(None);
        }
        let Some(conditional) = items.get(2).filter(|conditional| {
            string_field(conditional, "kind").as_deref() == Some("Conditional")
        }) else {
            return Ok(None);
        };
        let Some(condition) = conditional
            .get("conditions")
            .and_then(Value::as_array)
            .and_then(|conditions| conditions.first())
            .and_then(|condition| condition.get("expr"))
        else {
            return Ok(None);
        };
        if !contains_subroutine_call(condition, "get_report_verbosity_level")
            || !contains_subroutine_call(condition, "get_report_action")
        {
            return Ok(None);
        }
        let report_body = conditional
            .get("ifTrue")
            .and_then(|block| block.get("body"))
            .ok_or_else(|| "UVM macro report branch has no body".to_string())?;
        let report_statement = if string_field(report_body, "kind").as_deref() == Some("List") {
            report_body
                .get("list")
                .and_then(Value::as_array)
                .and_then(|statements| statements.first())
                .ok_or_else(|| "UVM macro report branch has no statement".to_string())?
        } else {
            report_body
        };
        let Some(report_call) = report_statement
            .get("expr")
            .filter(|call| string_field(call, "kind").as_deref() == Some("Call"))
        else {
            return Ok(None);
        };
        self.lower_uvm_report_dispatch(report_call)
    }

    pub(super) fn lower_uvm_report_dispatch(&self, call: &Value) -> Result<Option<Stmt>, String> {
        let Some(subroutine) = string_field(call, "subroutine") else {
            return Ok(None);
        };
        let severity = match subroutine.split_whitespace().next_back() {
            Some("uvm_report_info") => "INFO",
            Some("uvm_report_warning") => "WARNING",
            Some("uvm_report_error") => "ERROR",
            Some("uvm_report_fatal") => "FATAL",
            _ => return Ok(None),
        };
        let Some(active_object) = self.active_class_objects.borrow().last().cloned() else {
            return self.lower_uvm_report_call(call);
        };
        let Some(active_class) = self.class_object_types.get(&active_object).cloned() else {
            return self.lower_uvm_report_call(call);
        };
        let chain = self.class_chain(&active_class)?;
        let Some(official_index) = official_uvm_test_index(&chain) else {
            return self.lower_uvm_report_call(call);
        };

        let id = uvm_report_call_id(call)?;
        let verbosity = uvm_report_call_verbosity(call)?;
        if verbosity > self.uvm_report_verbosity_threshold(severity, &id) {
            return Ok(Some(Stmt::Block(Vec::new())));
        }
        let report_action = self.uvm_report_action(severity, &id);
        let call_hooks = report_action & UVM_CALL_HOOK_ACTION != 0;
        let mut hook_statements = Vec::new();
        let mut hooks_allow = true;
        if call_hooks {
            for method_name in [
                "report_hook",
                match severity {
                    "INFO" => "report_info_hook",
                    "WARNING" => "report_warning_hook",
                    "ERROR" => "report_error_hook",
                    _ => "report_fatal_hook",
                },
            ] {
                let Some((owner, method)) =
                    user_uvm_class_method(&chain, official_index, method_name)
                else {
                    continue;
                };
                let hook_call =
                    self.uvm_report_hook_call(&active_object, &active_class, &method, call)?;
                let body =
                    self.specialized_method_body(&active_object, &owner, &method, &hook_call)?;
                let statements = body
                    .get("list")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("UVM report hook '{method_name}' has no body"))?;
                if statements.is_empty() {
                    return Err(format!("UVM report hook '{method_name}' has no return"));
                }
                self.active_class_objects
                    .borrow_mut()
                    .push(active_object.clone());
                let lowered: Result<Vec<Stmt>, String> = (|| {
                    let mut lowered = Vec::new();
                    for statement in &statements[..statements.len() - 1] {
                        lowered.push(self.lower_statement(statement)?);
                    }
                    Ok(lowered)
                })();
                self.active_class_objects.borrow_mut().pop();
                hook_statements.extend(lowered?);

                let return_statement = &statements[statements.len() - 1];
                if string_field(return_statement, "kind").as_deref() != Some("Return") {
                    return Err(format!(
                        "UVM report hook '{method_name}' must end in a literal return"
                    ));
                }
                let return_value = return_statement.get("expr").ok_or_else(|| {
                    format!("UVM report hook '{method_name}' return has no value")
                })?;
                let return_value = string_field(unwrap_conversion(return_value), "value")
                    .and_then(|value| match value.as_str() {
                        "0" => Some(false),
                        "1" => Some(true),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!("UVM report hook '{method_name}' must return literal 0 or 1")
                    })?;
                hooks_allow &= return_value;
            }
        }
        if !hooks_allow {
            return Ok(Some(Stmt::Block(hook_statements)));
        }
        let (catcher_statements, caught, modification) =
            self.lower_uvm_report_catcher(Some(&active_object))?;
        hook_statements.extend(catcher_statements);
        if caught {
            return Ok(Some(Stmt::Block(hook_statements)));
        }
        let modified_severity = modification
            .severity
            .unwrap_or_else(|| severity.to_string());
        let action_override = modification.action.or_else(|| {
            if modified_severity != severity
                && report_action == self.uvm_report_action(severity, &id)
            {
                Some(self.uvm_report_action(&modified_severity, &id))
            } else {
                None
            }
        });
        let Some(report_statement) = self.lower_uvm_report_call_with(
            call,
            Some(&modified_severity),
            modification.message.as_deref(),
            action_override,
            false,
        )?
        else {
            return Ok(Some(Stmt::Block(hook_statements)));
        };
        hook_statements.push(report_statement);
        Ok(Some(Stmt::Block(hook_statements)))
    }

    pub(super) fn lower_uvm_report_catcher_registration(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "add" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 && arguments.len() != 3 {
            return Ok(None);
        }
        let context_argument = unwrap_conversion(&arguments[0]);
        let context_key =
            if string_field(context_argument, "kind").as_deref() == Some("NullLiteral") {
                None
            } else {
                let context_key = if is_this_reference(context_argument) {
                    self.active_class_objects
                        .borrow()
                        .last()
                        .cloned()
                        .ok_or_else(|| {
                            "UVM report catcher registration has no active object".to_string()
                        })?
                } else {
                    self.class_object_key_for_value(context_argument)?
                        .ok_or_else(|| {
                            "UVM report catcher registration requires a report object handle"
                                .to_string()
                        })?
                };
                self.require_initialized_class_object(&context_key)?;
                Some(context_key)
            };
        if let Some(ordering) = arguments.get(2) {
            let ordering_name = string_field(unwrap_conversion(ordering), "symbol")
                .map(|symbol| {
                    symbol
                        .split_whitespace()
                        .next_back()
                        .unwrap_or_default()
                        .to_string()
                })
                .ok_or_else(|| "UVM report catcher ordering must be literal".to_string())?;
            if !matches!(ordering_name.as_str(), "UVM_APPEND" | "UVM_PREPEND") {
                return Err(format!(
                    "unsupported UVM report catcher ordering '{ordering_name}'"
                ));
            }
        }
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(&arguments[1]))?
            .ok_or_else(|| {
                "UVM report catcher registration requires a catcher handle".to_string()
            })?;
        self.require_initialized_class_object(&object_key)?;
        let class_name = self
            .class_object_types
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no type"))?;
        let chain = self.class_chain(&class_name)?;
        let Some(official_index) = official_uvm_report_catcher_index(&chain) else {
            return Ok(None);
        };
        let Some((_, method)) = user_uvm_class_method(&chain, official_index, "catch") else {
            return Err(format!(
                "UVM report catcher class '{class_name}' has no catch method"
            ));
        };
        if self
            .uvm_report_catcher
            .borrow()
            .iter()
            .any(|existing| existing.object_key == object_key)
        {
            return Err("duplicate direct-lowering UVM report catcher registration".into());
        }
        let catcher = super::AstUvmReportCatcher {
            context_key,
            object_key,
            class_name,
            method,
            enabled: true,
        };
        let prepend = arguments
            .get(2)
            .map(|ordering| {
                string_field(unwrap_conversion(ordering), "symbol")
                    .map(|symbol| symbol.ends_with("UVM_PREPEND"))
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if prepend {
            self.uvm_report_catcher.borrow_mut().insert(0, catcher);
        } else {
            self.uvm_report_catcher.borrow_mut().push(catcher);
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_callback_registration(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "add" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 && arguments.len() != 3 {
            return Ok(None);
        }

        let mut prepend = false;
        if let Some(ordering) = arguments.get(2) {
            let ordering_name = string_field(unwrap_conversion(ordering), "symbol")
                .map(|symbol| {
                    symbol
                        .split_whitespace()
                        .next_back()
                        .unwrap_or_default()
                        .to_string()
                })
                .ok_or_else(|| "UVM callback ordering must be literal".to_string())?;
            match ordering_name.as_str() {
                "UVM_APPEND" => prepend = false,
                "UVM_PREPEND" => prepend = true,
                other => {
                    return Err(format!("unsupported UVM callback ordering '{other}'"));
                }
            }
        }

        let object_key = self
            .class_object_key_for_value(unwrap_conversion(&arguments[1]))?
            .ok_or_else(|| "UVM callback registration requires a callback handle".to_string())?;
        self.require_initialized_class_object(&object_key)?;
        let class_name = self
            .class_object_types
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no type"))?;
        let chain = self.class_chain(&class_name)?;
        if official_uvm_callback_index(&chain).is_none() {
            return Ok(None);
        }
        let (target_type, callback_type) =
            uvm_callback_specialization(call, &self.class_definitions, "add").ok_or_else(|| {
                "UVM callback registration has no specialized T/CB pair".to_string()
            })?;

        let context_argument = unwrap_conversion(&arguments[0]);
        let context_key = if string_field(context_argument, "kind").as_deref()
            == Some("NullLiteral")
        {
            None
        } else if is_this_reference(context_argument) {
            Some(
                self.active_class_objects
                    .borrow()
                    .last()
                    .cloned()
                    .ok_or_else(|| "UVM callback registration has no active object".to_string())?,
            )
        } else {
            Some(
                self.class_object_key_for_value(context_argument)?
                    .ok_or_else(|| {
                        "UVM callback registration requires an initialized object handle"
                            .to_string()
                    })?,
            )
        };
        if let Some(context_key) = &context_key {
            self.require_initialized_class_object(context_key)?;
        }

        let mut callbacks = self.uvm_callbacks.borrow_mut();
        if callbacks.iter().any(|existing| {
            existing.context_key == context_key && existing.object_key == object_key
        }) {
            return Err("duplicate direct-lowering UVM callback registration".into());
        }
        let callback = super::AstUvmCallback {
            context_key,
            target_type,
            callback_type,
            object_key,
            class_name,
            enabled: true,
        };
        if prepend {
            callbacks.insert(0, callback);
        } else {
            callbacks.push(callback);
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_callback_macro(
        &self,
        statement: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_call) = uvm_callback_macro_method(statement) else {
            return Ok(None);
        };
        let Some(receiver) = method_call.get("thisClass") else {
            return Ok(None);
        };
        let requested_type = string_field(receiver, "type")
            .and_then(|type_name| type_name.split_whitespace().next_back().map(str::to_string))
            .ok_or_else(|| "UVM callback macro has no callback type".to_string())?;
        let active_object = self
            .active_class_objects
            .borrow()
            .last()
            .cloned()
            .ok_or_else(|| "UVM callback macro has no active object".to_string())?;
        let active_class = self
            .class_object_types
            .get(&active_object)
            .cloned()
            .ok_or_else(|| format!("class object '{active_object}' has no type"))?;
        let active_chain = self.class_chain(&active_class)?;

        let callbacks = self
            .uvm_callbacks
            .borrow()
            .iter()
            .filter(|callback| {
                callback
                    .context_key
                    .as_ref()
                    .is_none_or(|context| *context == active_object)
                    && callback.callback_type == requested_type
                    && callback.enabled
                    && active_chain.iter().any(|(owner, _)| {
                        owner.split('@').next() == Some(callback.target_type.as_str())
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut statements = Vec::new();
        for callback in callbacks {
            let chain = self.class_chain(&callback.class_name)?;
            if !chain
                .iter()
                .any(|(owner, _)| owner.split('@').next() == Some(requested_type.as_str()))
            {
                continue;
            }
            let mut specialized = method_call.clone();
            if let Some(fields) = specialized.as_object_mut() {
                fields.insert(
                    "thisClass".into(),
                    serde_json::json!({
                        "kind": "NamedValue",
                        "type": callback.class_name,
                        "symbol": callback.object_key,
                    }),
                );
            }
            let Some(statement) = self.lower_class_task_call(&specialized)? else {
                return Err(format!(
                    "UVM callback class '{requested_type}' has no executable callback method"
                ));
            };
            statements.push(statement);
        }
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_callback_mode_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "callback_mode" {
            return Ok(None);
        }
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(receiver))?
            .ok_or_else(|| "UVM callback mode requires a callback handle".to_string())?;
        if !self
            .uvm_callbacks
            .borrow()
            .iter()
            .any(|callback| callback.object_key == object_key)
        {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 1 {
            return Err("UVM callback mode requires one explicit argument".into());
        }
        let mode_argument = unwrap_conversion(&arguments[0]);
        let mode = uvm_callback_mode_value(mode_argument)?;
        if !matches!(mode, -1..=1) {
            return Err(format!("unsupported UVM callback mode '{mode}'"));
        }
        if mode != -1 {
            for callback in self
                .uvm_callbacks
                .borrow_mut()
                .iter_mut()
                .filter(|callback| callback.object_key == object_key)
            {
                callback.enabled = mode == 1;
            }
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_analysis_port_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if !matches!(method_name.as_str(), "connect" | "write") {
            return Ok(None);
        }
        let Some(port_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if self
            .class_object_types
            .get(&port_key)
            .is_none_or(|class_name| {
                !matches!(
                    class_name.as_str(),
                    "uvm_analysis_port" | "uvm_analysis_export"
                )
            })
        {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);

        if method_name == "connect" {
            if arguments.len() != 1 {
                return Err("UVM analysis endpoint connect requires one provider".into());
            }
            let provider = unwrap_conversion(&arguments[0]);
            let (subscriber_key, subscriber_class) = if string_field(provider, "kind").as_deref()
                == Some("NamedValue")
            {
                let export_key = self.class_object_key_for_value(provider)?.ok_or_else(|| {
                    "UVM analysis connect requires an initialized export".to_string()
                })?;
                let export_class = self
                    .class_object_types
                    .get(&export_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{export_key}' has no type"))?;
                if export_class != "uvm_analysis_export" {
                    return Err(
                        "direct UVM analysis connect providers must be exports or subscribers"
                            .into(),
                    );
                }
                self.require_initialized_class_object(&export_key)?;
                (export_key, export_class)
            } else {
                if string_field(provider, "kind").as_deref() != Some("MemberAccess")
                    || string_field(provider, "member")
                        .and_then(|member| {
                            member.split_whitespace().next_back().map(str::to_string)
                        })
                        .as_deref()
                        != Some("analysis_export")
                {
                    return Err(
                        "UVM analysis connect requires an export or subscriber analysis_export"
                            .into(),
                    );
                }
                let subscriber_receiver = provider
                    .get("value")
                    .ok_or_else(|| "UVM analysis export has no subscriber receiver".to_string())?;
                let subscriber_key = self
                    .class_object_key_for_value(subscriber_receiver)?
                    .ok_or_else(|| {
                        "UVM analysis connect requires an initialized subscriber".to_string()
                    })?;
                let subscriber_class = self
                    .class_object_types
                    .get(&subscriber_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{subscriber_key}' has no type"))?;
                let chain = self.class_chain(&subscriber_class)?;
                let official_index = official_uvm_subscriber_index(&chain).ok_or_else(|| {
                    "UVM analysis provider is not derived from uvm_subscriber".to_string()
                })?;
                user_uvm_class_method(&chain, official_index, "write").ok_or_else(|| {
                    "UVM analysis subscriber must define a literal write method".to_string()
                })?;
                (subscriber_key, subscriber_class)
            };

            let mut connections = self.uvm_analysis_connections.borrow_mut();
            if connections.iter().any(|existing| {
                existing.port_key == port_key && existing.subscriber_key == subscriber_key
            }) {
                return Err("duplicate direct-lowering UVM analysis connection".into());
            }
            connections.push(super::AstUvmAnalysisConnection {
                port_key,
                subscriber_key,
                subscriber_class,
            });
            return Ok(Some(Stmt::Block(Vec::new())));
        }

        if arguments.len() != 1 {
            return Err("UVM analysis port write requires one transaction".into());
        }
        let mut visited = std::collections::BTreeSet::new();
        let statements = self.lower_uvm_analysis_writes(&port_key, arguments, &mut visited)?;
        Ok(Some(Stmt::Block(statements)))
    }

    fn lower_uvm_analysis_writes(
        &self,
        endpoint_key: &str,
        arguments: &[Value],
        visited: &mut std::collections::BTreeSet<String>,
    ) -> Result<Vec<Stmt>, String> {
        if !visited.insert(endpoint_key.to_string()) {
            return Err(format!("UVM analysis connection cycle at '{endpoint_key}'"));
        }
        let connections = self
            .uvm_analysis_connections
            .borrow()
            .iter()
            .filter(|connection| connection.port_key == endpoint_key)
            .cloned()
            .collect::<Vec<_>>();
        if connections.is_empty() {
            return Err(format!(
                "UVM analysis endpoint '{endpoint_key}' has no connected subscriber"
            ));
        }
        let mut statements = Vec::new();
        for connection in connections {
            if connection.subscriber_class == "uvm_analysis_export" {
                statements.extend(self.lower_uvm_analysis_writes(
                    &connection.subscriber_key,
                    arguments,
                    visited,
                )?);
                continue;
            }
            let chain = self.class_chain(&connection.subscriber_class)?;
            let official_index = official_uvm_subscriber_index(&chain)
                .ok_or_else(|| "UVM analysis subscriber is no longer official".to_string())?;
            let Some((_, method)) = user_uvm_class_method(&chain, official_index, "write") else {
                return Err("UVM analysis subscriber has no executable write method".to_string());
            };
            let address = method
                .get("addr")
                .and_then(Value::as_u64)
                .ok_or_else(|| "UVM analysis write method has no address".to_string())?;
            let subscriber_call = json!({
                "kind": "Call",
                "type": "void",
                "subroutine": format!("{address} write"),
                "thisClass": {
                    "kind": "NamedValue",
                    "type": connection.subscriber_class,
                    "symbol": connection.subscriber_key,
                },
                "arguments": arguments,
            });
            let Some(statement) = self.lower_class_task_call(&subscriber_call)? else {
                return Err("UVM analysis subscriber write method could not be lowered".to_string());
            };
            statements.push(statement);
        }
        Ok(statements)
    }

    pub(super) fn lower_uvm_tlm_fifo_constructor(
        &self,
        construction: &Value,
        object_key: &str,
    ) -> Result<Option<Stmt>, String> {
        let constructor = construction
            .get("constructorCall")
            .ok_or_else(|| "UVM TLM FIFO construction has no constructor call".to_string())?;
        let empty_arguments = Vec::new();
        let arguments = constructor
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 3 {
            return Err(
                "direct UVM TLM FIFO construction requires an explicit bounded size".into(),
            );
        }
        let size = string_field(unwrap_conversion(&arguments[2]), "value")
            .or_else(|| string_field(unwrap_conversion(&arguments[2]), "constant"))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| "UVM TLM FIFO size must be an integer literal".to_string())?;
        if !matches!(size, 1..=4) {
            return Err(format!(
                "direct UVM TLM FIFO size must be between 1 and 4, found {size}"
            ));
        }
        let fifo = self
            .uvm_tlm_fifos
            .borrow()
            .get(object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no TLM FIFO state"))?;
        let mut fifo = fifo;
        fifo.capacity = size;
        self.uvm_tlm_fifos
            .borrow_mut()
            .get_mut(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no TLM FIFO state"))?
            .capacity = size;
        let mut statements = vec![Stmt::Bl {
            lv: class_lvalue(&fifo.count),
            e: Expr::Const(LogicWord::zero(fifo.count.width)),
        }];
        for slot in &fifo.slots {
            statements.push(Stmt::Bl {
                lv: class_lvalue(slot),
                e: Expr::Const(LogicWord::zero(slot.width)),
            });
        }
        Ok(Some(Stmt::Block(statements)))
    }

    pub(super) fn lower_uvm_tlm_fifo_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if !matches!(
            method_name.as_str(),
            "try_put" | "try_get" | "try_peek" | "flush" | "put" | "get" | "peek"
        ) {
            return Ok(None);
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if self
            .class_object_types
            .get(&object_key)
            .is_none_or(|class_name| class_name != "uvm_tlm_fifo")
        {
            return Ok(None);
        }
        let fifo = self
            .uvm_tlm_fifos
            .borrow()
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no TLM FIFO state"))?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);

        if method_name == "flush" {
            if !arguments.is_empty() {
                return Err("UVM TLM FIFO flush requires no arguments".into());
            }
            let mut statements = vec![Stmt::Bl {
                lv: class_lvalue(&fifo.count),
                e: Expr::Const(LogicWord::zero(fifo.count.width)),
            }];
            for slot in &fifo.slots {
                statements.push(Stmt::Bl {
                    lv: class_lvalue(slot),
                    e: Expr::Const(LogicWord::zero(slot.width)),
                });
            }
            return Ok(Some(Stmt::Block(statements)));
        }

        if arguments.len() != 1 {
            return Err(format!(
                "UVM TLM FIFO {method_name} requires one transaction argument"
            ));
        }
        match method_name.as_str() {
            "try_put" | "put" => {
                let value = self.lower_expression(&arguments[0])?;
                let mut slot_assignments = Vec::new();
                for (index, slot) in fifo.slots.iter().enumerate() {
                    slot_assignments.push(Stmt::If {
                        c: fifo_count_equals(&fifo, index),
                        t: Box::new(Stmt::Bl {
                            lv: class_lvalue(slot),
                            e: value.clone(),
                        }),
                        f: None,
                    });
                }
                slot_assignments.push(Stmt::Bl {
                    lv: class_lvalue(&fifo.count),
                    e: Expr::Bin {
                        op: BinOp::Add,
                        a: Box::new(class_expr(&fifo.count)),
                        b: Box::new(Expr::Const(LogicWord::from_u64(fifo.count.width, 1))),
                    },
                });
                let body = Stmt::Block(slot_assignments);
                Ok(Some(if method_name == "put" {
                    Stmt::Wait {
                        cond: fifo_count_less(&fifo, fifo.capacity),
                        body: Box::new(body),
                    }
                } else {
                    Stmt::If {
                        c: fifo_count_less(&fifo, fifo.capacity),
                        t: Box::new(body),
                        f: None,
                    }
                }))
            }
            "try_get" | "get" | "try_peek" | "peek" => {
                let output = arguments[0].get("left").ok_or_else(|| {
                    format!("UVM TLM FIFO {method_name} requires an output argument")
                })?;
                let output = self.lower_lvalue(output)?;
                let mut statements = vec![Stmt::Bl {
                    lv: output,
                    e: class_expr(&fifo.slots[0]),
                }];
                if matches!(method_name.as_str(), "try_get" | "get") {
                    for (index, (destination, source)) in
                        fifo.slots.iter().zip(fifo.slots.iter().skip(1)).enumerate()
                    {
                        statements.push(Stmt::If {
                            c: fifo_count_greater(&fifo, index + 1),
                            t: Box::new(Stmt::Bl {
                                lv: class_lvalue(destination),
                                e: class_expr(source),
                            }),
                            f: None,
                        });
                    }
                    statements.push(Stmt::Bl {
                        lv: class_lvalue(&fifo.count),
                        e: Expr::Bin {
                            op: BinOp::Sub,
                            a: Box::new(class_expr(&fifo.count)),
                            b: Box::new(Expr::Const(LogicWord::from_u64(fifo.count.width, 1))),
                        },
                    });
                }
                let body = Stmt::Block(statements);
                Ok(Some(if matches!(method_name.as_str(), "get" | "peek") {
                    Stmt::Wait {
                        cond: fifo_count_greater(&fifo, 0),
                        body: Box::new(body),
                    }
                } else {
                    Stmt::If {
                        c: fifo_count_greater(&fifo, 0),
                        t: Box::new(body),
                        f: None,
                    }
                }))
            }
            _ => unreachable!("UVM TLM FIFO method was checked"),
        }
    }

    pub(super) fn lower_uvm_tlm_fifo_query(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if matches!(method_name.as_str(), "try_put" | "try_get" | "try_peek") {
            return Err(format!(
                "side-effecting UVM TLM FIFO {method_name} is unsupported in an expression"
            ));
        }
        if !matches!(
            method_name.as_str(),
            "size" | "used" | "is_empty" | "is_full" | "can_put" | "can_get" | "can_peek"
        ) {
            return Ok(None);
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if self
            .class_object_types
            .get(&object_key)
            .is_none_or(|class_name| class_name != "uvm_tlm_fifo")
        {
            return Ok(None);
        }
        let fifo = self
            .uvm_tlm_fifos
            .borrow()
            .get(&object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no TLM FIFO state"))?;
        let expression = match method_name.as_str() {
            "size" => Expr::Const(LogicWord::from_u64(32, fifo.capacity as u64)),
            "used" => class_expr(&fifo.count),
            "is_empty" => fifo_count_equals(&fifo, 0),
            "is_full" => fifo_count_equals(&fifo, fifo.capacity),
            "can_put" => fifo_count_less(&fifo, fifo.capacity),
            "can_get" | "can_peek" => fifo_count_greater(&fifo, 0),
            _ => unreachable!("UVM TLM FIFO query was checked"),
        };
        Ok(Some(expression))
    }

    pub(super) fn lower_uvm_tlm_port_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if !matches!(
            method_name.as_str(),
            "connect" | "put" | "get" | "peek" | "try_put" | "try_get" | "try_peek"
        ) {
            return Ok(None);
        }
        let Some(source_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        let Some(source_class) = self.class_object_types.get(&source_key).cloned() else {
            return Ok(None);
        };
        if !is_uvm_tlm_endpoint_class(&source_class) {
            return Ok(None);
        }
        self.require_initialized_class_object(&source_key)?;

        if method_name == "connect" {
            let empty_arguments = Vec::new();
            let arguments = call
                .get("arguments")
                .and_then(Value::as_array)
                .unwrap_or(&empty_arguments);
            if arguments.len() != 1 {
                return Err("UVM TLM endpoint connect requires one provider".into());
            }
            let provider = unwrap_conversion(&arguments[0]);
            let expected_export = if source_class.starts_with("uvm_put") {
                "put_export"
            } else {
                "get_peek_export"
            };
            let expected_export_class = if source_class.starts_with("uvm_put") {
                "uvm_put_export"
            } else {
                "uvm_get_peek_export"
            };
            let (target_key, target_class) = if string_field(provider, "kind").as_deref()
                == Some("NamedValue")
            {
                let target_key = self
                    .class_object_key_for_value(provider)?
                    .ok_or_else(|| "UVM TLM connect requires an initialized export".to_string())?;
                let target_class = self
                    .class_object_types
                    .get(&target_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{target_key}' has no type"))?;
                if target_class != expected_export_class {
                    return Err(format!(
                        "UVM TLM endpoint requires '{expected_export_class}', found '{target_class}'"
                    ));
                }
                self.require_initialized_class_object(&target_key)?;
                (target_key, target_class)
            } else {
                if string_field(provider, "kind").as_deref() != Some("MemberAccess") {
                    return Err(format!(
                        "direct UVM TLM endpoints connect only to '{expected_export}' or a matching export"
                    ));
                }
                let export_name = string_field(provider, "member")
                    .and_then(|member| member.split_whitespace().next_back().map(str::to_string))
                    .ok_or_else(|| "UVM TLM export has no name".to_string())?;
                if export_name != expected_export {
                    return Err(format!(
                        "UVM TLM endpoint requires '{expected_export}', found '{export_name}'"
                    ));
                }
                let fifo_receiver = provider
                    .get("value")
                    .ok_or_else(|| "UVM TLM export has no FIFO receiver".to_string())?;
                let target_key =
                    self.class_object_key_for_value(fifo_receiver)?
                        .ok_or_else(|| {
                            "UVM TLM connect requires an initialized bounded FIFO".to_string()
                        })?;
                let target_class = self
                    .class_object_types
                    .get(&target_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{target_key}' has no type"))?;
                if target_class != "uvm_tlm_fifo" {
                    return Err("UVM TLM endpoint provider is not a bounded uvm_tlm_fifo".into());
                }
                self.require_initialized_class_object(&target_key)?;
                (target_key, target_class)
            };
            let _ = target_class;
            let mut connections = self.uvm_tlm_endpoint_connections.borrow_mut();
            if connections
                .iter()
                .any(|existing| existing.source_key == source_key)
            {
                return Err("UVM TLM endpoint is already connected".into());
            }
            connections.push(super::AstUvmTlmEndpointConnection {
                source_key,
                target_key,
            });
            return Ok(Some(Stmt::Block(Vec::new())));
        }

        let mut visited = std::collections::BTreeSet::new();
        let fifo_key = self.resolve_uvm_tlm_fifo(&source_key, &mut visited)?;
        let fifo_receiver = json!({
            "kind": "NamedValue",
            "type": "uvm_tlm_fifo",
            "symbol": fifo_key,
        });
        self.lower_uvm_tlm_fifo_call(call, &fifo_receiver)
    }

    fn resolve_uvm_tlm_fifo(
        &self,
        endpoint_key: &str,
        visited: &mut std::collections::BTreeSet<String>,
    ) -> Result<String, String> {
        if !visited.insert(endpoint_key.to_string()) {
            return Err(format!(
                "UVM TLM endpoint connection cycle at '{endpoint_key}'"
            ));
        }
        let endpoint_class = self
            .class_object_types
            .get(endpoint_key)
            .cloned()
            .ok_or_else(|| format!("class object '{endpoint_key}' has no type"))?;
        if endpoint_class == "uvm_tlm_fifo" {
            return Ok(endpoint_key.to_string());
        }
        if !is_uvm_tlm_endpoint_class(&endpoint_class) {
            return Err(format!(
                "UVM TLM endpoint '{endpoint_key}' has unsupported class '{endpoint_class}'"
            ));
        }
        let target_key = self
            .uvm_tlm_endpoint_connections
            .borrow()
            .iter()
            .find(|connection| connection.source_key == endpoint_key)
            .map(|connection| connection.target_key.clone())
            .ok_or_else(|| {
                format!("UVM TLM endpoint '{endpoint_key}' has no connected bounded FIFO")
            })?;
        self.resolve_uvm_tlm_fifo(&target_key, visited)
    }
    pub(super) fn lower_uvm_tlm_port_query(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        let Some(source_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        let Some(source_class) = self.class_object_types.get(&source_key).cloned() else {
            return Ok(None);
        };
        if !is_uvm_tlm_endpoint_class(&source_class) {
            return Ok(None);
        }
        if matches!(method_name.as_str(), "try_put" | "try_get" | "try_peek") {
            return Err(format!(
                "side-effecting UVM TLM port {method_name} is unsupported in an expression"
            ));
        }
        if !matches!(
            method_name.as_str(),
            "size" | "used" | "is_empty" | "is_full" | "can_put" | "can_get" | "can_peek"
        ) {
            return Ok(None);
        }
        let mut visited = std::collections::BTreeSet::new();
        let fifo_key = self.resolve_uvm_tlm_fifo(&source_key, &mut visited)?;
        let fifo_receiver = json!({
            "kind": "NamedValue",
            "type": "uvm_tlm_fifo",
            "symbol": fifo_key,
        });
        self.lower_uvm_tlm_fifo_query(call, &fifo_receiver)
    }

    pub(super) fn lower_uvm_sequencer_constructor(
        &self,
        construction: &Value,
        object_key: &str,
    ) -> Result<Option<Stmt>, String> {
        let Some(constructor) = construction.get("constructorCall") else {
            return Err("UVM sequencer construction has no constructor call".to_string());
        };
        let empty_arguments = Vec::new();
        let arguments = constructor
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() > 2 {
            return Err("direct UVM sequencer construction requires at most two arguments".into());
        }
        let sequencer = self
            .uvm_sequencers
            .borrow()
            .get(object_key)
            .cloned()
            .ok_or_else(|| format!("class object '{object_key}' has no sequencer state"))?;
        self.uvm_sequence_items.borrow_mut().remove(object_key);
        self.uvm_sequence_response_types
            .borrow_mut()
            .remove(object_key);
        Ok(Some(Stmt::Block(vec![
            Stmt::Bl {
                lv: class_lvalue(&sequencer.request_ready),
                e: Expr::Const(LogicWord::zero(sequencer.request_ready.width)),
            },
            Stmt::Bl {
                lv: class_lvalue(&sequencer.request_done),
                e: Expr::Const(LogicWord::zero(sequencer.request_done.width)),
            },
            Stmt::Bl {
                lv: class_lvalue(&sequencer.response_head),
                e: Expr::Const(LogicWord::zero(sequencer.response_head.width)),
            },
            Stmt::Bl {
                lv: class_lvalue(&sequencer.response_count),
                e: Expr::Const(LogicWord::zero(sequencer.response_count.width)),
            },
        ])))
    }

    pub(super) fn lower_uvm_sequence_start_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "start" {
            return Ok(None);
        }
        let Some(sequence_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        let sequence_class = self
            .class_object_types
            .get(&sequence_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequence_key}' has no type"))?;
        let chain = self.class_chain(&sequence_class)?;
        let Some(official_index) = official_uvm_sequence_index(&chain) else {
            return Ok(None);
        };
        let Some((owner, body)) = user_uvm_class_method(&chain, official_index, "body") else {
            return Err(format!(
                "UVM sequence class '{sequence_class}' has no user body task"
            ));
        };
        if string_field(&body, "subroutineKind").as_deref() != Some("Task") {
            return Err(format!(
                "UVM sequence class '{sequence_class}' body must be a void task"
            ));
        }

        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() || arguments.len() > 4 {
            return Err(
                "direct UVM sequence start requires a sequencer as its first argument".into(),
            );
        }
        let sequencer_argument = unwrap_conversion(&arguments[0]);
        let sequencer_key = self
            .class_object_key_for_value(sequencer_argument)?
            .ok_or_else(|| "UVM sequence start requires a sequencer handle".to_string())?;
        if self
            .class_object_types
            .get(&sequencer_key)
            .is_none_or(|class_name| class_name != "uvm_sequencer")
        {
            return Err("UVM sequence start requires a direct uvm_sequencer object".into());
        }
        self.require_initialized_class_object(&sequence_key)?;
        self.require_initialized_class_object(&sequencer_key)?;
        self.uvm_sequence_contexts
            .borrow_mut()
            .insert(sequence_key.clone(), sequencer_key);

        let address = body
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| "UVM sequence body task has no address".to_string())?;
        let body_call = json!({
            "kind": "Call",
            "type": "void",
            "subroutine": format!("{address} body"),
            "thisClass": {
                "kind": "NamedValue",
                "type": sequence_class,
                "symbol": sequence_key,
            },
            "arguments": [],
        });
        let _ = owner;
        self.lower_class_task_call(&body_call)
    }

    pub(super) fn lower_uvm_sequence_item_call(
        &self,
        call: &Value,
        method_name: &str,
    ) -> Result<Option<Stmt>, String> {
        if !matches!(method_name, "start_item" | "finish_item") {
            return Ok(None);
        }
        let Some(sequence_key) = self.active_class_objects.borrow().last().cloned() else {
            return Ok(None);
        };
        let sequence_class = self
            .class_object_types
            .get(&sequence_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequence_key}' has no type"))?;
        let chain = self.class_chain(&sequence_class)?;
        if official_uvm_sequence_index(&chain).is_none() {
            return Ok(None);
        }
        let sequencer_key = self
            .uvm_sequence_contexts
            .borrow()
            .get(&sequence_key)
            .cloned()
            .ok_or_else(|| format!("UVM sequence '{sequence_key}' has no sequencer context"))?;
        let sequencer = self
            .uvm_sequencers
            .borrow()
            .get(&sequencer_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequencer_key}' has no sequencer state"))?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() || arguments.len() > 3 {
            return Err(format!(
                "UVM sequence {method_name} requires one transaction item"
            ));
        }
        let item_key = self
            .class_object_key_for_value(unwrap_conversion(&arguments[0]))?
            .ok_or_else(|| format!("UVM sequence {method_name} requires an item handle"))?;
        self.require_initialized_class_object(&item_key)?;

        if method_name == "start_item" {
            self.uvm_sequence_items
                .borrow_mut()
                .insert(sequencer_key, item_key);
            return Ok(Some(Stmt::Block(Vec::new())));
        }

        let selected_item = self
            .uvm_sequence_items
            .borrow()
            .get(&sequencer_key)
            .cloned()
            .ok_or_else(|| "UVM sequence finish_item has no start_item".to_string())?;
        if selected_item != item_key {
            return Err("UVM sequence finish_item transaction differs from its start_item".into());
        }
        let finish_body = Stmt::Block(vec![
            Stmt::Bl {
                lv: class_lvalue(&sequencer.request_ready),
                e: Expr::Const(LogicWord::from_u64(sequencer.request_ready.width, 1)),
            },
            Stmt::Wait {
                cond: Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(class_expr(&sequencer.request_done)),
                    b: Box::new(Expr::Const(LogicWord::from_u64(
                        sequencer.request_done.width,
                        1,
                    ))),
                },
                body: Box::new(Stmt::Block(vec![
                    Stmt::Bl {
                        lv: class_lvalue(&sequencer.request_ready),
                        e: Expr::Const(LogicWord::zero(sequencer.request_ready.width)),
                    },
                    Stmt::Bl {
                        lv: class_lvalue(&sequencer.request_done),
                        e: Expr::Const(LogicWord::zero(sequencer.request_done.width)),
                    },
                ])),
            },
        ]);
        Ok(Some(finish_body))
    }

    pub(super) fn lower_uvm_sequencer_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if !matches!(method_name.as_str(), "get_next_item" | "item_done") {
            return Ok(None);
        }
        let Some(sequencer_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        if self
            .class_object_types
            .get(&sequencer_key)
            .is_none_or(|class_name| class_name != "uvm_sequencer")
        {
            return Ok(None);
        }
        self.require_initialized_class_object(&sequencer_key)?;
        let sequencer = self
            .uvm_sequencers
            .borrow()
            .get(&sequencer_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequencer_key}' has no sequencer state"))?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);

        if method_name == "item_done" {
            if arguments.len() > 1 {
                return Err("UVM sequencer item_done requires at most one response".into());
            }
            let mut completion = vec![
                Stmt::Bl {
                    lv: class_lvalue(&sequencer.request_ready),
                    e: Expr::Const(LogicWord::zero(sequencer.request_ready.width)),
                },
                Stmt::Bl {
                    lv: class_lvalue(&sequencer.request_done),
                    e: Expr::Const(LogicWord::from_u64(sequencer.request_done.width, 1)),
                },
            ];
            if arguments.first().is_some_and(|response| {
                string_field(unwrap_conversion(response), "kind").as_deref() != Some("NullLiteral")
            }) {
                if arguments.len() != 1 {
                    return Err("UVM sequencer item_done requires at most one response".into());
                }
                let response_key = self
                    .class_object_key_for_value(unwrap_conversion(&arguments[0]))?
                    .ok_or_else(|| {
                        "UVM sequencer item_done requires a prepared response handle".to_string()
                    })?;
                self.require_initialized_class_object(&response_key)?;
                let response_class = self
                    .class_object_types
                    .get(&response_key)
                    .cloned()
                    .ok_or_else(|| format!("class object '{response_key}' has no type"))?;
                let expected_response = self
                    .uvm_sequence_response_types
                    .borrow()
                    .get(&sequencer_key)
                    .cloned();
                if expected_response
                    .as_ref()
                    .is_some_and(|expected| *expected != response_class)
                {
                    return Err(format!(
                        "UVM sequencer response type '{response_class}' does not match sequence response '{expected_response:?}'"
                    ));
                }
                let capture = self.response_capture_statements(&sequencer, &response_key)?;
                completion.extend(capture);
            }
            let ready = class_expr(&sequencer.request_ready);
            return Ok(Some(Stmt::If {
                c: Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(ready),
                    b: Box::new(Expr::Const(LogicWord::from_u64(
                        sequencer.request_ready.width,
                        1,
                    ))),
                },
                t: Box::new(Stmt::Block(completion)),
                f: Some(Box::new(Stmt::Assert {
                    cond: Expr::Const(LogicWord::zero(1)),
                    message: Some(
                        "UVM sequencer item_done has no outstanding get_next_item".into(),
                    ),
                })),
            }));
        }

        if arguments.len() != 1 {
            return Err("UVM sequencer get_next_item requires one output item".into());
        }
        let output = arguments[0]
            .get("left")
            .ok_or_else(|| "UVM sequencer get_next_item requires an output argument".to_string())?;
        let output_key = self.class_object_key_for_value(output)?.ok_or_else(|| {
            "UVM sequencer get_next_item requires a prepared output item".to_string()
        })?;
        let item_key = self
            .uvm_sequence_items
            .borrow()
            .get(&sequencer_key)
            .cloned()
            .ok_or_else(|| {
                "UVM sequencer get_next_item was lowered before a sequence start_item".to_string()
            })?;
        let mut copies = self.class_object_copy_statements(&output_key, &item_key)?;
        let transaction_ids = self.uvm_sequence_transaction_ids.borrow();
        match (
            transaction_ids.get(&output_key),
            transaction_ids.get(&item_key),
        ) {
            (Some(target), Some(source)) => copies.push(Stmt::Bl {
                lv: class_lvalue(target),
                e: class_expr(source),
            }),
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(
                    "UVM sequencer get_next_item has inconsistent transaction-id metadata".into(),
                );
            }
        }
        Ok(Some(Stmt::Wait {
            cond: Expr::Bin {
                op: BinOp::Eq,
                a: Box::new(class_expr(&sequencer.request_ready)),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    sequencer.request_ready.width,
                    1,
                ))),
            },
            body: Box::new(Stmt::Block(copies)),
        }))
    }

    pub(super) fn lower_uvm_sequence_response_call(
        &self,
        call: &Value,
        method_name: &str,
    ) -> Result<Option<Stmt>, String> {
        if method_name != "get_response" {
            return Ok(None);
        }
        let Some(sequence_key) = self.active_class_objects.borrow().last().cloned() else {
            return Ok(None);
        };
        let sequence_class = self
            .class_object_types
            .get(&sequence_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequence_key}' has no type"))?;
        let chain = self.class_chain(&sequence_class)?;
        if official_uvm_sequence_index(&chain).is_none() {
            return Ok(None);
        }
        let sequencer_key = self
            .uvm_sequence_contexts
            .borrow()
            .get(&sequence_key)
            .cloned()
            .ok_or_else(|| format!("UVM sequence '{sequence_key}' has no sequencer context"))?;
        let sequencer = self
            .uvm_sequencers
            .borrow()
            .get(&sequencer_key)
            .cloned()
            .ok_or_else(|| format!("class object '{sequencer_key}' has no sequencer state"))?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(
                "UVM sequence get_response requires an output and optional transaction id".into(),
            );
        }
        let transaction_selector = match arguments.get(1) {
            None => None,
            Some(argument) => {
                let argument = unwrap_conversion(argument);
                let literal = match string_field(argument, "kind").as_deref() {
                    Some("UnaryOp") if string_field(argument, "op").as_deref() == Some("Minus") => {
                        argument
                            .get("operand")
                            .and_then(|operand| string_field(operand, "value"))
                            .and_then(|value| value.parse::<i64>().ok())
                            .map(|value| -value)
                    }
                    _ => string_field(argument, "value")
                        .or_else(|| string_field(argument, "constant"))
                        .and_then(|value| value.parse::<i64>().ok()),
                };
                if literal == Some(-1) {
                    None
                } else {
                    Some(self.lower_expression(argument)?)
                }
            }
        };
        let output = arguments[0]
            .get("left")
            .ok_or_else(|| "UVM sequence get_response requires an output argument".to_string())?;
        let output_key = self.class_object_key_for_value(output)?.ok_or_else(|| {
            "UVM sequence get_response requires a prepared response object".to_string()
        })?;
        self.require_initialized_class_object(&output_key)?;
        let output_class = self
            .class_object_types
            .get(&output_key)
            .cloned()
            .ok_or_else(|| format!("class object '{output_key}' has no type"))?;
        self.uvm_sequence_response_types
            .borrow_mut()
            .insert(sequencer_key.clone(), output_class);
        let (condition, body) = self.response_restore_statements(
            &sequencer,
            &output_key,
            transaction_selector.as_ref(),
        )?;
        Ok(Some(Stmt::Wait {
            cond: condition,
            body: Box::new(Stmt::Block(body)),
        }))
    }

    pub(super) fn lower_uvm_sequence_transaction_id_task(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "set_transaction_id" {
            return Ok(None);
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        let transaction_id = self
            .uvm_sequence_transaction_ids
            .borrow()
            .get(&object_key)
            .cloned();
        let Some(transaction_id) = transaction_id else {
            return Ok(None);
        };
        self.require_initialized_class_object(&object_key)?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 1 {
            return Err("UVM set_transaction_id requires one transaction id".into());
        }
        let value = self.lower_expression(&arguments[0])?;
        Ok(Some(Stmt::Bl {
            lv: class_lvalue(&transaction_id),
            e: value,
        }))
    }

    pub(super) fn lower_uvm_sequence_transaction_id_query(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "get_transaction_id" {
            return Ok(None);
        }
        let Some(object_key) = self.class_object_key_for_value(receiver)? else {
            return Ok(None);
        };
        let transaction_id = self
            .uvm_sequence_transaction_ids
            .borrow()
            .get(&object_key)
            .cloned();
        let Some(transaction_id) = transaction_id else {
            return Ok(None);
        };
        self.require_initialized_class_object(&object_key)?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if !arguments.is_empty() {
            return Err("UVM get_transaction_id requires no arguments".into());
        }
        Ok(Some(class_expr(&transaction_id)))
    }

    fn response_capture_statements(
        &self,
        sequencer: &super::AstUvmSequencer,
        source_key: &str,
    ) -> Result<Vec<Stmt>, String> {
        let fields = self.ordered_response_fields(source_key)?;
        let head = class_expr(&sequencer.response_head);
        let count = class_expr(&sequencer.response_count);
        let tail = Expr::Bin {
            op: BinOp::Mod,
            a: Box::new(Expr::Bin {
                op: BinOp::Add,
                a: Box::new(head),
                b: Box::new(count),
            }),
            b: Box::new(Expr::Const(LogicWord::from_u64(
                sequencer.response_head.width,
                8,
            ))),
        };
        let mut entry_captures = Vec::new();
        for (entry_index, entry) in sequencer.response_entries.iter().enumerate() {
            let mut captures = self.response_metadata_statements(
                source_key,
                [
                    &entry.tag,
                    &entry.id,
                    &entry.name,
                    &entry.full_name,
                    &entry.parent_id,
                ],
                true,
            );
            if let Some(transaction_id) = self
                .uvm_sequence_transaction_ids
                .borrow()
                .get(source_key)
                .cloned()
            {
                captures.insert(
                    0,
                    Stmt::Bl {
                        lv: class_lvalue(&entry.transaction_id),
                        e: class_expr(&transaction_id),
                    },
                );
            }
            for ((_, source), slot) in fields.iter().zip(&entry.slots) {
                captures.push(Stmt::Bl {
                    lv: class_lvalue(slot),
                    e: class_expr(source),
                });
            }
            entry_captures.push(Stmt::If {
                c: Expr::Bin {
                    op: BinOp::Eq,
                    a: Box::new(tail.clone()),
                    b: Box::new(Expr::Const(LogicWord::from_u64(
                        sequencer.response_head.width,
                        entry_index as u64,
                    ))),
                },
                t: Box::new(Stmt::Block(captures)),
                f: None,
            });
        }
        entry_captures.push(Stmt::Bl {
            lv: class_lvalue(&sequencer.response_count),
            e: Expr::Bin {
                op: BinOp::Add,
                a: Box::new(class_expr(&sequencer.response_count)),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    sequencer.response_count.width,
                    1,
                ))),
            },
        });
        Ok(vec![Stmt::If {
            c: Expr::Bin {
                op: BinOp::Lt,
                a: Box::new(class_expr(&sequencer.response_count)),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    sequencer.response_count.width,
                    8,
                ))),
            },
            t: Box::new(Stmt::Block(entry_captures)),
            f: Some(Box::new(Stmt::Assert {
                cond: Expr::Const(LogicWord::zero(1)),
                message: Some("UVM sequence response queue overflowed depth 8".into()),
            })),
        }])
    }

    fn response_restore_statements(
        &self,
        sequencer: &super::AstUvmSequencer,
        target_key: &str,
        transaction_selector: Option<&Expr>,
    ) -> Result<(Expr, Vec<Stmt>), String> {
        let fields = self.ordered_response_fields(target_key)?;
        let head_width = sequencer.response_head.width;
        let head = class_expr(&sequencer.response_head);
        let count = class_expr(&sequencer.response_count);
        let ring = |offset: usize| {
            if offset == 0 {
                head.clone()
            } else {
                Expr::Bin {
                    op: BinOp::Mod,
                    a: Box::new(Expr::Bin {
                        op: BinOp::Add,
                        a: Box::new(head.clone()),
                        b: Box::new(Expr::Const(LogicWord::from_u64(head_width, offset as u64))),
                    }),
                    b: Box::new(Expr::Const(LogicWord::from_u64(head_width, 8))),
                }
            }
        };
        let at = |offset: usize, entry_index: usize| Expr::Bin {
            op: BinOp::Eq,
            a: Box::new(ring(offset)),
            b: Box::new(Expr::Const(LogicWord::from_u64(
                head_width,
                entry_index as u64,
            ))),
        };
        let entry_transaction_id = |offset: usize| {
            let mut selected =
                class_expr(&sequencer.response_entries.last().unwrap().transaction_id);
            for (entry_index, entry) in sequencer.response_entries.iter().enumerate().rev() {
                if entry_index + 1 == sequencer.response_entries.len() {
                    continue;
                }
                selected = Expr::Cond {
                    c: Box::new(at(offset, entry_index)),
                    t: Box::new(class_expr(&entry.transaction_id)),
                    f: Box::new(selected),
                };
            }
            selected
        };

        let mut statements = Vec::new();
        let mut selective_availability = None;
        match transaction_selector {
            None => {
                for (entry_index, entry) in sequencer.response_entries.iter().enumerate() {
                    let mut restores = self.response_metadata_statements(
                        target_key,
                        [
                            &entry.tag,
                            &entry.id,
                            &entry.name,
                            &entry.full_name,
                            &entry.parent_id,
                        ],
                        false,
                    );
                    if let Some(transaction_id) = self
                        .uvm_sequence_transaction_ids
                        .borrow()
                        .get(target_key)
                        .cloned()
                    {
                        restores.push(Stmt::Bl {
                            lv: class_lvalue(&transaction_id),
                            e: class_expr(&entry.transaction_id),
                        });
                    }
                    for ((_, target), slot) in fields.iter().zip(&entry.slots) {
                        restores.push(Stmt::Bl {
                            lv: class_lvalue(target),
                            e: class_expr(slot),
                        });
                    }
                    statements.push(Stmt::If {
                        c: at(0, entry_index),
                        t: Box::new(Stmt::Block(restores)),
                        f: None,
                    });
                }
                statements.push(Stmt::If {
                    c: Expr::Bin {
                        op: BinOp::Lt,
                        a: Box::new(class_expr(&sequencer.response_head)),
                        b: Box::new(Expr::Const(LogicWord::from_u64(head_width, 7))),
                    },
                    t: Box::new(Stmt::Bl {
                        lv: class_lvalue(&sequencer.response_head),
                        e: Expr::Bin {
                            op: BinOp::Add,
                            a: Box::new(class_expr(&sequencer.response_head)),
                            b: Box::new(Expr::Const(LogicWord::from_u64(head_width, 1))),
                        },
                    }),
                    f: Some(Box::new(Stmt::Bl {
                        lv: class_lvalue(&sequencer.response_head),
                        e: Expr::Const(LogicWord::zero(head_width)),
                    })),
                });
                statements.push(Self::response_count_decrement(sequencer));
            }
            Some(selector) => {
                for selected_offset in 0..8_usize {
                    let selected_matches = Expr::Bin {
                        op: BinOp::LogAnd,
                        a: Box::new(Expr::Bin {
                            op: BinOp::Gt,
                            a: Box::new(count.clone()),
                            b: Box::new(Expr::Const(LogicWord::from_u64(
                                sequencer.response_count.width,
                                selected_offset as u64,
                            ))),
                        }),
                        b: Box::new(Expr::Bin {
                            op: BinOp::Eq,
                            a: Box::new(entry_transaction_id(selected_offset)),
                            b: Box::new(selector.clone()),
                        }),
                    };
                    selective_availability = Some(match selective_availability {
                        Some(previous) => Expr::Bin {
                            op: BinOp::LogOr,
                            a: Box::new(previous),
                            b: Box::new(selected_matches.clone()),
                        },
                        None => selected_matches.clone(),
                    });
                    let mut selected_statements = Vec::new();
                    for (entry_index, entry) in sequencer.response_entries.iter().enumerate() {
                        let mut restores = self.response_metadata_statements(
                            target_key,
                            [
                                &entry.tag,
                                &entry.id,
                                &entry.name,
                                &entry.full_name,
                                &entry.parent_id,
                            ],
                            false,
                        );
                        if let Some(transaction_id) = self
                            .uvm_sequence_transaction_ids
                            .borrow()
                            .get(target_key)
                            .cloned()
                        {
                            restores.push(Stmt::Bl {
                                lv: class_lvalue(&transaction_id),
                                e: class_expr(&entry.transaction_id),
                            });
                        }
                        for ((_, target), slot) in fields.iter().zip(&entry.slots) {
                            restores.push(Stmt::Bl {
                                lv: class_lvalue(target),
                                e: class_expr(slot),
                            });
                        }
                        selected_statements.push(Stmt::If {
                            c: at(selected_offset, entry_index),
                            t: Box::new(Stmt::Block(restores)),
                            f: None,
                        });
                    }
                    for shift_offset in (selected_offset + 1)..8_usize {
                        let mut shift_statements = Vec::new();
                        for (source_index, source_entry) in
                            sequencer.response_entries.iter().enumerate()
                        {
                            for (target_index, target_entry) in
                                sequencer.response_entries.iter().enumerate()
                            {
                                shift_statements.push(Stmt::If {
                                    c: Expr::Bin {
                                        op: BinOp::LogAnd,
                                        a: Box::new(at(shift_offset, source_index)),
                                        b: Box::new(at(shift_offset - 1, target_index)),
                                    },
                                    t: Box::new(Stmt::Block(Self::response_entry_copy_statements(
                                        target_entry,
                                        source_entry,
                                    ))),
                                    f: None,
                                });
                            }
                        }
                        selected_statements.push(Stmt::If {
                            c: Expr::Bin {
                                op: BinOp::Gt,
                                a: Box::new(class_expr(&sequencer.response_count)),
                                b: Box::new(Expr::Const(LogicWord::from_u64(
                                    sequencer.response_count.width,
                                    shift_offset as u64,
                                ))),
                            },
                            t: Box::new(Stmt::Block(shift_statements)),
                            f: None,
                        });
                    }
                    selected_statements.push(Self::response_count_decrement(sequencer));
                    statements.push(Stmt::If {
                        c: selected_matches,
                        t: Box::new(Stmt::Block(selected_statements)),
                        f: None,
                    });
                }
            }
        }
        let condition = match transaction_selector {
            None => Expr::Bin {
                op: BinOp::Gt,
                a: Box::new(count),
                b: Box::new(Expr::Const(LogicWord::zero(sequencer.response_count.width))),
            },
            Some(_) => selective_availability.unwrap_or_else(|| Expr::Const(LogicWord::zero(1))),
        };

        Ok((condition, statements))
    }

    fn response_count_decrement(sequencer: &super::AstUvmSequencer) -> Stmt {
        Stmt::Bl {
            lv: class_lvalue(&sequencer.response_count),
            e: Expr::Bin {
                op: BinOp::Sub,
                a: Box::new(class_expr(&sequencer.response_count)),
                b: Box::new(Expr::Const(LogicWord::from_u64(
                    sequencer.response_count.width,
                    1,
                ))),
            },
        }
    }

    fn response_entry_copy_statements(
        target: &super::AstUvmSequencerResponseEntry,
        source: &super::AstUvmSequencerResponseEntry,
    ) -> Vec<Stmt> {
        let assignments = [
            (&target.transaction_id, &source.transaction_id),
            (&target.tag, &source.tag),
            (&target.id, &source.id),
            (&target.name, &source.name),
            (&target.full_name, &source.full_name),
            (&target.parent_id, &source.parent_id),
        ];
        let mut statements = assignments
            .into_iter()
            .map(|(target_symbol, source_symbol)| Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            })
            .collect::<Vec<_>>();
        statements.extend(target.slots.iter().zip(&source.slots).map(
            |(target_symbol, source_symbol)| Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            },
        ));
        statements
    }

    fn response_metadata_statements(
        &self,
        object_key: &str,
        response_symbols: [&super::AstLoweredSymbol; 5],
        capture: bool,
    ) -> Vec<Stmt> {
        let object_symbols = [
            &self.class_object_tags[object_key],
            &self.class_object_ids[object_key],
            &self.class_object_names[object_key],
            &self.class_object_full_names[object_key],
            &self.class_object_parent_ids[object_key],
        ];
        object_symbols
            .into_iter()
            .zip(response_symbols)
            .map(|(object_symbol, response_symbol)| {
                let (target, source) = if capture {
                    (response_symbol, object_symbol)
                } else {
                    (object_symbol, response_symbol)
                };
                Stmt::Bl {
                    lv: class_lvalue(target),
                    e: class_expr(source),
                }
            })
            .collect()
    }

    fn ordered_response_fields(
        &self,
        object_key: &str,
    ) -> Result<Vec<(String, super::AstLoweredSymbol)>, String> {
        if self
            .class_composition_children
            .keys()
            .any(|(key, _)| key == object_key)
        {
            return Err(
                "direct UVM sequence responses with class-valued fields are unsupported".into(),
            );
        }
        let fields = self
            .class_objects
            .get(object_key)
            .ok_or_else(|| format!("class object '{object_key}' has no fields"))?;
        if fields.len() > 4 {
            return Err("direct UVM sequence responses support at most four scalar fields".into());
        }
        fields
            .iter()
            .map(|(name, symbol)| {
                if symbol.width > crate::types::STRING_WIDTH {
                    return Err(format!(
                        "UVM sequence response field '{name}' is wider than the bounded response storage"
                    ));
                }
                Ok((name.clone(), symbol.clone()))
            })
            .collect()
    }

    fn class_object_copy_statements(
        &self,
        target_key: &str,
        source_key: &str,
    ) -> Result<Vec<Stmt>, String> {
        if self
            .class_composition_children
            .keys()
            .any(|(object_key, _)| object_key == source_key)
        {
            return Err(
                "direct UVM sequence items with class-valued fields are unsupported".into(),
            );
        }
        let source_fields = self
            .class_objects
            .get(source_key)
            .ok_or_else(|| format!("class object '{source_key}' has no fields"))?;
        let target_fields = self
            .class_objects
            .get(target_key)
            .ok_or_else(|| format!("class object '{target_key}' has no fields"))?;
        let mut statements = Vec::new();
        for (source_symbol, target_symbol) in [
            (
                &self.class_object_tags[source_key],
                &self.class_object_tags[target_key],
            ),
            (
                &self.class_object_ids[source_key],
                &self.class_object_ids[target_key],
            ),
            (
                &self.class_object_names[source_key],
                &self.class_object_names[target_key],
            ),
            (
                &self.class_object_full_names[source_key],
                &self.class_object_full_names[target_key],
            ),
            (
                &self.class_object_parent_ids[source_key],
                &self.class_object_parent_ids[target_key],
            ),
        ] {
            statements.push(Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            });
        }
        for (field_name, source_symbol) in source_fields {
            let Some(target_symbol) = target_fields.get(field_name) else {
                return Err(format!(
                    "UVM sequence item output is missing field '{field_name}'"
                ));
            };
            statements.push(Stmt::Bl {
                lv: class_lvalue(target_symbol),
                e: class_expr(source_symbol),
            });
        }
        Ok(statements)
    }

    pub(super) fn lower_uvm_callback_mode_query(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "callback_mode" {
            return Ok(None);
        }
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(receiver))?
            .ok_or_else(|| "UVM callback mode query requires a callback handle".to_string())?;
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() > 1 {
            return Err("UVM callback mode query requires at most one argument".into());
        }
        if let Some(argument) = arguments.first() {
            let mode_argument = unwrap_conversion(argument);
            let mode = uvm_callback_mode_value(mode_argument)?;
            if mode != -1 {
                return Err(
                    "callback_mode(0/1) as a value-returning expression is unsupported".into(),
                );
            }
        }
        let callbacks = self.uvm_callbacks.borrow();
        let mut modes = callbacks
            .iter()
            .filter(|callback| callback.object_key == object_key)
            .map(|callback| callback.enabled);
        let Some(mode) = modes.next() else {
            return Ok(None);
        };
        if modes.any(|other| other != mode) {
            return Err("UVM callback object has inconsistent callback modes".into());
        }
        Ok(Some(Expr::Const(LogicWord::from_u64(1, u64::from(mode)))))
    }

    pub(super) fn lower_uvm_callback_delete_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "delete" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Ok(None);
        }
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(&arguments[1]))?
            .ok_or_else(|| "UVM callback deletion requires a callback handle".to_string())?;
        if !self
            .uvm_callbacks
            .borrow()
            .iter()
            .any(|callback| callback.object_key == object_key)
        {
            return Ok(None);
        }
        let Some((target_type, callback_type)) =
            uvm_callback_specialization(call, &self.class_definitions, "delete")
        else {
            return Err("UVM callback deletion has no specialized T/CB pair".into());
        };
        let context_argument = unwrap_conversion(&arguments[0]);
        let context_key = if string_field(context_argument, "kind").as_deref()
            == Some("NullLiteral")
        {
            None
        } else if is_this_reference(context_argument) {
            Some(
                self.active_class_objects
                    .borrow()
                    .last()
                    .cloned()
                    .ok_or_else(|| "UVM callback deletion has no active object".to_string())?,
            )
        } else {
            Some(
                self.class_object_key_for_value(context_argument)?
                    .ok_or_else(|| {
                        "UVM callback deletion requires an initialized object handle".to_string()
                    })?,
            )
        };
        let mut callbacks = self.uvm_callbacks.borrow_mut();
        let before = callbacks.len();
        callbacks.retain(|callback| {
            !(callback.object_key == object_key
                && callback.context_key == context_key
                && callback.target_type == target_type
                && callback.callback_type == callback_type)
        });
        if callbacks.len() == before {
            return Err("UVM callback is not registered with the requested context".into());
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_callback_delete_by_name_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "delete_by_name" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 3 {
            return Ok(None);
        }
        if string_field(unwrap_conversion(&arguments[2]), "kind").as_deref() != Some("NullLiteral")
        {
            return Err("non-null UVM callback delete_by_name roots are unsupported".into());
        }
        let component_name = string_literal(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM callback names must be string literals".to_string())?;
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(&arguments[1]))?
            .ok_or_else(|| "UVM callback delete_by_name requires a callback handle".to_string())?;
        if !self
            .uvm_callbacks
            .borrow()
            .iter()
            .any(|callback| callback.object_key == object_key)
        {
            return Ok(None);
        }
        let Some((target_type, callback_type)) =
            uvm_callback_specialization(call, &self.class_definitions, "delete_by_name")
        else {
            return Err("UVM callback delete_by_name has no specialized T/CB pair".into());
        };
        let matching_contexts = self
            .class_object_declared_full_names
            .borrow()
            .iter()
            .filter(|(_, full_name)| wildcard_matches(&component_name, full_name))
            .map(|(context_key, _)| context_key.clone())
            .collect::<Vec<_>>();
        if matching_contexts.is_empty() {
            return Err(format!(
                "UVM callback name '{component_name}' matches no component"
            ));
        }

        let mut callbacks = self.uvm_callbacks.borrow_mut();
        let before = callbacks.len();
        callbacks.retain(|callback| {
            !(callback.object_key == object_key
                && callback.target_type == target_type
                && callback.callback_type == callback_type
                && callback
                    .context_key
                    .as_ref()
                    .is_some_and(|context| matching_contexts.contains(context)))
        });
        if callbacks.len() == before {
            return Err("UVM callback is not registered with a matching component".into());
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_report_catcher_mode_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(receiver) = call.get("thisClass") else {
            return Ok(None);
        };
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "callback_mode" {
            return Ok(None);
        }
        let object_key = self
            .class_object_key_for_value(unwrap_conversion(receiver))?
            .ok_or_else(|| "UVM report catcher mode requires a catcher handle".to_string())?;
        if !self
            .uvm_report_catcher
            .borrow()
            .iter()
            .any(|catcher| catcher.object_key == object_key)
        {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.is_empty() || arguments.len() > 2 {
            return Err(
                "UVM report catcher callback_mode requires at most one explicit argument".into(),
            );
        }
        let mode_argument = unwrap_conversion(&arguments[0]);
        let mode = string_field(mode_argument, "value")
            .or_else(|| string_field(mode_argument, "constant"))
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| "UVM report catcher mode must be -1, 0, or 1".to_string())?;
        if !matches!(mode, -1..=1) {
            return Err(format!("unsupported UVM report catcher mode '{mode}'"));
        }
        if mode != -1 {
            if let Some(catcher) = self
                .uvm_report_catcher
                .borrow_mut()
                .iter_mut()
                .find(|catcher| catcher.object_key == object_key)
            {
                catcher.enabled = mode == 1;
            }
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_report_catcher_delete_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "delete" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 {
            return Ok(None);
        }
        let context_argument = unwrap_conversion(&arguments[0]);
        let context_key = if string_field(context_argument, "kind").as_deref()
            == Some("NullLiteral")
        {
            None
        } else if is_this_reference(context_argument) {
            Some(
                self.active_class_objects
                    .borrow()
                    .last()
                    .cloned()
                    .ok_or_else(|| {
                        "UVM report catcher deletion has no active object".to_string()
                    })?,
            )
        } else {
            Some(
                self.class_object_key_for_value(context_argument)?
                    .ok_or_else(|| {
                        "UVM report catcher deletion requires a report object handle".to_string()
                    })?,
            )
        };
        let Some(catcher_key) =
            self.class_object_key_for_value(unwrap_conversion(&arguments[1]))?
        else {
            return Ok(None);
        };
        let mut catchers = self.uvm_report_catcher.borrow_mut();
        if !catchers
            .iter()
            .any(|catcher| catcher.object_key == catcher_key)
        {
            return Ok(None);
        }
        let Some(index) = catchers.iter().position(|catcher| {
            catcher.object_key == catcher_key && catcher.context_key == context_key
        }) else {
            return Err("UVM report catcher is not registered with the requested context".into());
        };
        catchers.remove(index);
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_report_catcher_delete_by_name_call(
        &self,
        call: &Value,
    ) -> Result<Option<Stmt>, String> {
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if method_name != "delete_by_name" {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        if arguments.len() != 2 && arguments.len() != 3 {
            return Ok(None);
        }
        if let Some(root) = arguments.get(2) {
            if string_field(unwrap_conversion(root), "kind").as_deref() != Some("NullLiteral") {
                return Err(
                    "non-null UVM report catcher delete_by_name roots are unsupported".into(),
                );
            }
        }
        let component_name = string_literal(unwrap_conversion(&arguments[0]))
            .ok_or_else(|| "UVM report catcher names must be string literals".to_string())?;
        let Some(catcher_key) =
            self.class_object_key_for_value(unwrap_conversion(&arguments[1]))?
        else {
            return Ok(None);
        };
        let matching_contexts = self
            .class_object_declared_full_names
            .borrow()
            .iter()
            .filter(|(_, full_name)| wildcard_matches(&component_name, full_name))
            .map(|(object_key, _)| object_key.clone())
            .collect::<Vec<_>>();
        if matching_contexts.is_empty() {
            return Err(format!(
                "UVM report catcher name '{component_name}' matches no component"
            ));
        }
        let mut catchers = self.uvm_report_catcher.borrow_mut();
        if !catchers
            .iter()
            .any(|catcher| catcher.object_key == catcher_key)
        {
            return Ok(None);
        }
        let before = catchers.len();
        catchers.retain(|catcher| {
            !(catcher.object_key == catcher_key
                && catcher
                    .context_key
                    .as_ref()
                    .is_some_and(|context| matching_contexts.contains(context)))
        });
        if catchers.len() == before {
            return Err("UVM report catcher is not registered with the requested component".into());
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    fn lower_uvm_report_catcher(
        &self,
        active_object: Option<&str>,
    ) -> Result<(Vec<Stmt>, bool, UvmReportCatcherModification), String> {
        let catchers = self
            .uvm_report_catcher
            .borrow()
            .iter()
            .filter(|catcher| {
                catcher
                    .context_key
                    .as_deref()
                    .is_none_or(|context_key| Some(context_key) == active_object)
                    && catcher.enabled
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut statements = Vec::new();
        let mut modification = UvmReportCatcherModification::default();
        for catcher in catchers {
            let (catcher_statements, caught, catcher_modification) =
                self.lower_one_uvm_report_catcher(catcher)?;
            statements.extend(catcher_statements);
            modification.merge(catcher_modification);
            if caught {
                return Ok((statements, true, modification));
            }
        }
        Ok((statements, false, modification))
    }

    fn lower_one_uvm_report_catcher(
        &self,
        catcher: super::AstUvmReportCatcher,
    ) -> Result<(Vec<Stmt>, bool, UvmReportCatcherModification), String> {
        let address = catcher
            .method
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| "UVM report catcher catch method has no address".to_string())?;
        let catch_call = json!({
            "kind": "Call",
            "subroutine": format!("{address} catch"),
            "thisClass": {
                "kind": "NamedValue",
                "type": catcher.class_name,
                "symbol": catcher.object_key,
            },
            "arguments": [],
        });
        let chain = self.class_chain(&catcher.class_name)?;
        let official_index = official_uvm_report_catcher_index(&chain)
            .ok_or_else(|| "registered UVM report catcher is no longer official".to_string())?;
        let owner = user_uvm_class_method(&chain, official_index, "catch")
            .map(|(owner, _)| owner)
            .ok_or_else(|| "registered UVM report catcher has no catch method".to_string())?;
        let body = self.specialized_method_body(
            &catcher.object_key,
            &owner,
            &catcher.method,
            &catch_call,
        )?;
        let statements = body
            .get("list")
            .and_then(Value::as_array)
            .ok_or_else(|| "UVM report catcher catch method has no body".to_string())?;
        if statements.is_empty() {
            return Err("UVM report catcher catch method has no return".into());
        }
        let mut modification = UvmReportCatcherModification::default();
        self.active_class_objects
            .borrow_mut()
            .push(catcher.object_key.clone());
        let lowered: Result<Vec<Stmt>, String> = (|| {
            let mut lowered = Vec::new();
            for statement in &statements[..statements.len() - 1] {
                if let Some((setter, arguments)) = uvm_report_catcher_setter(statement) {
                    modification.apply(setter, arguments)?;
                } else {
                    lowered.push(self.lower_statement(statement)?);
                }
            }
            Ok(lowered)
        })();
        self.active_class_objects.borrow_mut().pop();
        let catcher_statements = lowered?;

        let return_statement = &statements[statements.len() - 1];
        if string_field(return_statement, "kind").as_deref() != Some("Return") {
            return Err("UVM report catcher catch method must end in a return".into());
        }
        let return_value = return_statement
            .get("expr")
            .ok_or_else(|| "UVM report catcher return has no value".to_string())?;
        let symbol = string_field(unwrap_conversion(return_value), "symbol").map(|symbol| {
            symbol
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .to_string()
        });
        let constant = symbol
            .or_else(|| string_field(unwrap_conversion(return_value), "constant"))
            .or_else(|| string_field(unwrap_conversion(return_value), "value"))
            .ok_or_else(|| "UVM report catcher return must be THROW or CAUGHT".to_string())?;
        match constant.as_str() {
            "THROW" | "0" => Ok((catcher_statements, false, modification)),
            "CAUGHT" | "1" => Ok((catcher_statements, true, modification)),
            other => Err(format!(
                "unsupported UVM report catcher return constant '{other}'"
            )),
        }
    }

    fn uvm_report_hook_call(
        &self,
        object_key: &str,
        class_name: &str,
        method: &Value,
        report_call: &Value,
    ) -> Result<Value, String> {
        let address = method
            .get("addr")
            .and_then(Value::as_u64)
            .ok_or_else(|| "UVM report hook has no address".to_string())?;
        let empty_arguments = Vec::new();
        let report_arguments = report_call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let default_verbosity = json!({
            "kind": "Conversion",
            "type": "int",
            "operand": {
                "kind": "IntegerLiteral",
                "type": "int",
                "value": "200",
            },
        });
        let empty_filename = json!({
            "kind": "Conversion",
            "type": "string",
            "operand": {
                "kind": "StringLiteral",
                "type": "bit[7:0]",
                "literal": "",
            },
        });
        let zero_line = json!({
            "kind": "IntegerLiteral",
            "type": "int",
            "value": "0",
        });
        let id = report_arguments.first().cloned().unwrap_or_else(|| {
            json!({
                "kind": "Conversion",
                "type": "string",
                "operand": {
                    "kind": "StringLiteral",
                    "type": "bit[7:0]",
                    "literal": "",
                },
            })
        });
        let message = report_arguments.get(1).cloned().unwrap_or_else(|| {
            json!({
                "kind": "Conversion",
                "type": "string",
                "operand": {
                    "kind": "StringLiteral",
                    "type": "bit[7:0]",
                    "literal": "",
                },
            })
        });
        let verbosity = report_arguments
            .get(2)
            .cloned()
            .unwrap_or(default_verbosity);
        let filename = report_arguments.get(3).cloned().unwrap_or(empty_filename);
        let line = report_arguments.get(4).cloned().unwrap_or(zero_line);
        Ok(json!({
            "kind": "Call",
            "subroutine": format!("{address} {}", string_field(method, "name").unwrap_or_default()),
            "thisClass": {
                "kind": "NamedValue",
                "type": class_name,
                "symbol": object_key,
            },
            "arguments": [id, message, verbosity, filename, line],
        }))
    }

    pub(super) fn lower_uvm_report_control_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        if !is_this_reference(receiver) {
            return Ok(None);
        }
        let active_object = self
            .active_class_objects
            .borrow()
            .last()
            .cloned()
            .ok_or_else(|| "UVM report control has no active object".to_string())?;
        let active_class = self
            .class_object_types
            .get(&active_object)
            .cloned()
            .ok_or_else(|| format!("class object '{active_object}' has no type"))?;
        let chain = self.class_chain(&active_class)?;
        if official_uvm_test_index(&chain).is_none() {
            return Ok(None);
        }
        let Some(method_name) = string_field(call, "subroutine").and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        }) else {
            return Ok(None);
        };
        if !matches!(
            method_name.as_str(),
            "set_report_verbosity_level"
                | "set_report_id_verbosity"
                | "set_report_severity_id_verbosity"
                | "set_report_severity_action"
                | "set_report_id_action"
                | "set_report_severity_id_action"
        ) {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let expected_argument_count = match method_name.as_str() {
            "set_report_verbosity_level" => 1,
            "set_report_id_verbosity" | "set_report_severity_action" | "set_report_id_action" => 2,
            _ => 3,
        };
        if arguments.len() != expected_argument_count {
            return Err(format!(
                "UVM report control {method_name} requires {expected_argument_count} arguments"
            ));
        }
        let mut controls = self.uvm_report_controls.borrow_mut();
        match method_name.as_str() {
            "set_report_verbosity_level" => {
                let verbosity = uvm_report_integer(&arguments[0])?;
                controls.default_verbosity = Some(verbosity);
            }
            "set_report_id_verbosity" => {
                let id = uvm_report_string(&arguments[0])?;
                let verbosity = uvm_report_integer(&arguments[1])?;
                controls.id_verbosity.insert(id, verbosity);
            }
            "set_report_severity_id_verbosity" => {
                let severity = uvm_report_severity(&arguments[0])?;
                let id = uvm_report_string(&arguments[1])?;
                let verbosity = uvm_report_integer(&arguments[2])?;
                controls
                    .severity_id_verbosity
                    .insert((severity, id), verbosity);
            }
            "set_report_severity_action" => {
                let severity = uvm_report_severity(&arguments[0])?;
                let action = uvm_report_action_value(&arguments[1])?;
                controls.severity_actions.insert(severity, action);
            }
            "set_report_id_action" => {
                let id = uvm_report_string(&arguments[0])?;
                let action = uvm_report_action_value(&arguments[1])?;
                controls.id_actions.insert(id, action);
            }
            _ => {
                let severity = uvm_report_severity(&arguments[0])?;
                let id = uvm_report_string(&arguments[1])?;
                let action = uvm_report_action_value(&arguments[2])?;
                controls.severity_id_actions.insert((severity, id), action);
            }
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_report_query_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        if !is_this_reference(receiver) {
            return Ok(None);
        }
        let method_name = string_field(call, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if !matches!(
            method_name.as_str(),
            "get_report_verbosity_level" | "get_report_max_verbosity_level" | "get_report_action"
        ) {
            return Ok(None);
        }
        let active_object = self
            .active_class_objects
            .borrow()
            .last()
            .cloned()
            .ok_or_else(|| "UVM report query has no active object".to_string())?;
        let active_class = self
            .class_object_types
            .get(&active_object)
            .cloned()
            .ok_or_else(|| format!("class object '{active_object}' has no type"))?;
        let chain = self.class_chain(&active_class)?;
        if official_uvm_test_index(&chain).is_none() {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let value = match method_name.as_str() {
            "get_report_max_verbosity_level" => {
                if !arguments.is_empty() {
                    return Err("UVM get_report_max_verbosity_level requires no arguments".into());
                }
                self.uvm_report_controls
                    .borrow()
                    .default_verbosity
                    .unwrap_or(200)
            }
            "get_report_verbosity_level" => {
                if arguments.len() > 2 {
                    return Err(
                        "UVM get_report_verbosity_level requires at most two arguments".into(),
                    );
                }
                let severity = if let Some(argument) = arguments.first() {
                    uvm_report_severity(argument)?
                } else {
                    "INFO".to_string()
                };
                let id = if let Some(argument) = arguments.get(1) {
                    uvm_report_string(argument)?
                } else {
                    String::new()
                };
                self.uvm_report_verbosity_threshold(&severity, &id)
            }
            _ => {
                if arguments.len() != 2 {
                    return Err("UVM get_report_action requires two arguments".into());
                }
                let severity = uvm_report_severity(&arguments[0])?;
                let id = uvm_report_string(&arguments[1])?;
                self.uvm_report_action(&severity, &id)
            }
        };
        Ok(Some(Expr::Const(LogicWord::from_u64(32, value))))
    }

    pub(super) fn lower_uvm_report_server_control_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Stmt>, String> {
        if !is_uvm_report_server_receiver(receiver) {
            return Ok(None);
        }
        let method_name = string_field(call, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if !matches!(
            method_name.as_str(),
            "set_max_quit_count"
                | "set_quit_count"
                | "set_severity_count"
                | "set_id_count"
                | "reset_quit_count"
                | "reset_severity_counts"
        ) {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let mut controls = self.uvm_report_controls.borrow_mut();
        match method_name.as_str() {
            "set_max_quit_count" => {
                if arguments.is_empty() || arguments.len() > 2 {
                    return Err("UVM set_max_quit_count requires one or two arguments".into());
                }
                let count = uvm_report_count(&arguments[0])?;
                let overridable = if let Some(argument) = arguments.get(1) {
                    uvm_report_bool(argument)?
                } else {
                    true
                };
                if !controls.max_quit_overridable {
                    return Err("non-overridable UVM max quit count is not yet supported".into());
                }
                controls.max_quit_count = count;
                controls.max_quit_overridable = overridable;
            }
            "set_quit_count" => {
                if arguments.len() != 1 {
                    return Err("UVM set_quit_count requires one argument".into());
                }
                controls.quit_count = uvm_report_count(&arguments[0])?;
            }
            "set_severity_count" => {
                if arguments.len() != 2 {
                    return Err("UVM set_severity_count requires two arguments".into());
                }
                let severity = uvm_report_severity(&arguments[0])?;
                let count = uvm_report_count(&arguments[1])?;
                controls.severity_counts.insert(severity, count);
            }
            "set_id_count" => {
                if arguments.len() != 2 {
                    return Err("UVM set_id_count requires two arguments".into());
                }
                let id = uvm_report_string(&arguments[0])?;
                let count = uvm_report_count(&arguments[1])?;
                controls.id_counts.insert(id, count);
            }
            "reset_quit_count" => {
                if !arguments.is_empty() {
                    return Err("UVM reset_quit_count requires no arguments".into());
                }
                controls.quit_count = 0;
            }
            _ => {
                if !arguments.is_empty() {
                    return Err("UVM reset_severity_counts requires no arguments".into());
                }
                controls.severity_counts.clear();
            }
        }
        Ok(Some(Stmt::Block(Vec::new())))
    }

    pub(super) fn lower_uvm_report_server_query_call(
        &self,
        call: &Value,
        receiver: &Value,
    ) -> Result<Option<Expr>, String> {
        if !is_uvm_report_server_receiver(receiver) {
            return Ok(None);
        }
        let method_name = string_field(call, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if !matches!(
            method_name.as_str(),
            "get_max_quit_count"
                | "get_quit_count"
                | "is_quit_count_reached"
                | "get_severity_count"
                | "get_id_count"
        ) {
            return Ok(None);
        }
        let empty_arguments = Vec::new();
        let arguments = call
            .get("arguments")
            .and_then(Value::as_array)
            .unwrap_or(&empty_arguments);
        let controls = self.uvm_report_controls.borrow();
        let value = match method_name.as_str() {
            "get_max_quit_count" => {
                if !arguments.is_empty() {
                    return Err("UVM get_max_quit_count requires no arguments".into());
                }
                controls.max_quit_count
            }
            "get_quit_count" => {
                if !arguments.is_empty() {
                    return Err("UVM get_quit_count requires no arguments".into());
                }
                controls.quit_count
            }
            "is_quit_count_reached" => {
                if !arguments.is_empty() {
                    return Err("UVM is_quit_count_reached requires no arguments".into());
                }
                u64::from(controls.quit_count >= controls.max_quit_count)
            }
            "get_severity_count" => {
                if arguments.len() != 1 {
                    return Err("UVM get_severity_count requires one argument".into());
                }
                let severity = uvm_report_severity(&arguments[0])?;
                controls
                    .severity_counts
                    .get(&severity)
                    .copied()
                    .unwrap_or(0)
            }
            _ => {
                if arguments.len() != 1 {
                    return Err("UVM get_id_count requires one argument".into());
                }
                let id = uvm_report_string(&arguments[0])?;
                controls.id_counts.get(&id).copied().unwrap_or(0)
            }
        };
        Ok(Some(Expr::Const(LogicWord::from_u64(32, value))))
    }

    fn uvm_report_action(&self, severity: &str, id: &str) -> u64 {
        let controls = self.uvm_report_controls.borrow();
        controls
            .severity_id_actions
            .get(&(severity.to_string(), id.to_string()))
            .or_else(|| controls.id_actions.get(id))
            .or_else(|| controls.severity_actions.get(severity))
            .copied()
            .unwrap_or(match severity {
                "ERROR" => UVM_DISPLAY_ACTION | UVM_COUNT_ACTION,
                "FATAL" => UVM_DISPLAY_ACTION | UVM_EXIT_ACTION,
                _ => UVM_DISPLAY_ACTION,
            })
    }

    fn uvm_report_verbosity_threshold(&self, severity: &str, id: &str) -> u64 {
        let controls = self.uvm_report_controls.borrow();
        controls
            .severity_id_verbosity
            .get(&(severity.to_string(), id.to_string()))
            .or_else(|| controls.id_verbosity.get(id))
            .or_else(|| controls.severity_verbosity.get(severity))
            .copied()
            .or(controls.default_verbosity)
            .unwrap_or(200)
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
                string_literal_allow_empty(unwrap_conversion(&arguments[1])).ok_or_else(|| {
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
            Some("object_by_type") => {
                if arguments.len() != 3 {
                    return Err("UVM factory create_object_by_type requires three arguments".into());
                }
                (
                    wrapper_type_name(&arguments[0]).ok_or_else(|| {
                        "UVM factory requested object type must be Class::get_type()".to_string()
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

fn uvm_conditional_call(statement: &Value) -> Result<Option<(&Value, bool)>, String> {
    let Some(condition) = statement
        .get("conditions")
        .and_then(Value::as_array)
        .and_then(|conditions| conditions.first())
        .and_then(|condition| condition.get("expr"))
    else {
        return Ok(None);
    };
    if string_field(condition, "kind").as_deref() == Some("UnaryOp")
        && string_field(condition, "op").as_deref() == Some("LogicalNot")
    {
        let call = condition
            .get("operand")
            .ok_or_else(|| "logical-not expression has no operand".to_string())?;
        return Ok(Some((call, true)));
    }
    Ok(Some((condition, false)))
}

fn contains_subroutine_call(value: &Value, subroutine_name: &str) -> bool {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine")
                .and_then(|subroutine| {
                    subroutine
                        .split_whitespace()
                        .next_back()
                        .map(str::to_string)
                })
                .as_deref()
                == Some(subroutine_name)
        {
            return true;
        }
        return fields
            .values()
            .any(|nested| contains_subroutine_call(nested, subroutine_name));
    }
    value.as_array().is_some_and(|values| {
        values
            .iter()
            .any(|nested| contains_subroutine_call(nested, subroutine_name))
    })
}

fn uvm_report_integer(value: &Value) -> Result<u64, String> {
    let value = unwrap_conversion(value);
    if string_field(value, "kind").as_deref() == Some("BinaryOp")
        && matches!(
            string_field(value, "op").as_deref(),
            Some("BitwiseOr" | "BinaryOr")
        )
    {
        let left = value
            .get("left")
            .ok_or_else(|| "UVM report bitwise-or has no left operand".to_string())?;
        let right = value
            .get("right")
            .ok_or_else(|| "UVM report bitwise-or has no right operand".to_string())?;
        return Ok(uvm_report_integer(left)? | uvm_report_integer(right)?);
    }
    if let Some(symbol) = string_field(value, "symbol") {
        let symbol = symbol.split_whitespace().next_back().unwrap_or_default();
        return match symbol {
            "UVM_NONE" => Ok(0),
            "UVM_LOW" => Ok(100),
            "UVM_MEDIUM" => Ok(200),
            "UVM_HIGH" => Ok(300),
            "UVM_FULL" => Ok(400),
            "UVM_DEBUG" => Ok(500),
            "UVM_NO_ACTION" => Ok(0),
            "UVM_DISPLAY" => Ok(1),
            "UVM_LOG" => Ok(UVM_LOG_ACTION),
            "UVM_COUNT" => Ok(4),
            "UVM_EXIT" => Ok(8),
            "UVM_CALL_HOOK" => Ok(16),
            "UVM_STOP" => Ok(32),
            "UVM_RM_RECORD" => Ok(64),
            other => Err(format!("unsupported UVM report constant '{other}'")),
        };
    }
    string_field(value, "constant")
        .or_else(|| string_field(value, "value"))
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| "UVM report control value must be a literal".to_string())
}

fn uvm_report_verbosity_value(value: &str) -> Option<u64> {
    match value {
        "UVM_NONE" => Some(0),
        "UVM_LOW" => Some(100),
        "UVM_MEDIUM" => Some(200),
        "UVM_HIGH" => Some(300),
        "UVM_FULL" => Some(400),
        "UVM_DEBUG" => Some(500),
        numeric => numeric.parse::<u64>().ok(),
    }
}

const UVM_DISPLAY_ACTION: u64 = 1;
const UVM_LOG_ACTION: u64 = 2;
const UVM_COUNT_ACTION: u64 = 4;
const UVM_EXIT_ACTION: u64 = 8;
const UVM_CALL_HOOK_ACTION: u64 = 16;

fn is_uvm_tlm_endpoint_class(class_name: &str) -> bool {
    matches!(
        class_name,
        "uvm_put_port" | "uvm_get_peek_port" | "uvm_put_export" | "uvm_get_peek_export"
    )
}

fn fifo_count_equals(fifo: &super::AstUvmTlmFifo, value: usize) -> Expr {
    Expr::Bin {
        op: BinOp::Eq,
        a: Box::new(class_expr(&fifo.count)),
        b: Box::new(Expr::Const(LogicWord::from_u64(
            fifo.count.width,
            value as u64,
        ))),
    }
}

fn fifo_count_less(fifo: &super::AstUvmTlmFifo, value: usize) -> Expr {
    Expr::Bin {
        op: BinOp::Lt,
        a: Box::new(class_expr(&fifo.count)),
        b: Box::new(Expr::Const(LogicWord::from_u64(
            fifo.count.width,
            value as u64,
        ))),
    }
}

fn fifo_count_greater(fifo: &super::AstUvmTlmFifo, value: usize) -> Expr {
    Expr::Bin {
        op: BinOp::Gt,
        a: Box::new(class_expr(&fifo.count)),
        b: Box::new(Expr::Const(LogicWord::from_u64(
            fifo.count.width,
            value as u64,
        ))),
    }
}

#[derive(Clone, Default)]
struct UvmReportCatcherModification {
    severity: Option<String>,
    message: Option<String>,
    action: Option<u64>,
}

impl UvmReportCatcherModification {
    fn merge(&mut self, later: Self) {
        if later.severity.is_some() {
            self.severity = later.severity;
        }
        if later.message.is_some() {
            self.message = later.message;
        }
        if later.action.is_some() {
            self.action = later.action;
        }
    }

    fn apply(&mut self, setter: &str, arguments: &[Value]) -> Result<(), String> {
        if arguments.len() != 1 {
            return Err(format!(
                "UVM report catcher {setter} requires one literal argument"
            ));
        }
        match setter {
            "set_severity" => {
                self.severity = Some(uvm_report_severity(&arguments[0])?);
            }
            "set_message" => {
                self.message = Some(
                    string_literal_allow_empty(unwrap_conversion(&arguments[0])).ok_or_else(
                        || "UVM report catcher set_message requires a string literal".to_string(),
                    )?,
                );
            }
            _ => {
                self.action = Some(uvm_report_action_value(&arguments[0])?);
            }
        }
        Ok(())
    }
}

fn uvm_report_catcher_setter(statement: &Value) -> Option<(&'static str, &[Value])> {
    let call = if string_field(statement, "kind").as_deref() == Some("ExpressionStatement") {
        statement.get("expr")?
    } else {
        statement
    };
    if string_field(call, "kind").as_deref() != Some("Call") || call.get("thisClass").is_some() {
        return None;
    }
    let subroutine = string_field(call, "subroutine")?;
    let setter = match subroutine.split_whitespace().next_back() {
        Some("set_severity") => "set_severity",
        Some("set_message") => "set_message",
        Some("set_action") => "set_action",
        _ => return None,
    };
    let arguments = call.get("arguments")?.as_array()?;
    Some((setter, arguments))
}

fn uvm_report_call_arguments(call: &Value) -> &[Value] {
    call.get("arguments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn uvm_report_call_id(call: &Value) -> Result<String, String> {
    uvm_report_call_arguments(call)
        .first()
        .map(uvm_report_string)
        .transpose()
        .map(|id| id.unwrap_or_default())
}

fn uvm_report_call_verbosity(call: &Value) -> Result<u64, String> {
    let Some(argument) = uvm_report_call_arguments(call).get(2) else {
        return Ok(200);
    };
    let argument = unwrap_conversion(argument);
    if let Some(symbol) = string_field(argument, "symbol") {
        let verbosity = symbol.split_whitespace().next_back().unwrap_or_default();
        return uvm_report_verbosity_value(verbosity)
            .ok_or_else(|| format!("unsupported UVM report verbosity '{verbosity}'"));
    }
    if string_field(argument, "kind").as_deref() == Some("IntegerLiteral") {
        let verbosity = string_field(argument, "value")
            .ok_or_else(|| "UVM report verbosity has no value".to_string())?;
        return uvm_report_verbosity_value(&verbosity)
            .ok_or_else(|| format!("unsupported UVM report verbosity '{verbosity}'"));
    }
    Err("UVM report verbosity must be a literal".into())
}

fn uvm_report_severity(value: &Value) -> Result<String, String> {
    let value = unwrap_conversion(value);
    let severity = string_field(value, "symbol")
        .map(|symbol| {
            symbol
                .split_whitespace()
                .next_back()
                .unwrap_or_default()
                .to_string()
        })
        .or_else(|| string_field(value, "constant"))
        .ok_or_else(|| "UVM report severity must be a literal".to_string())?;
    match severity.as_str() {
        "UVM_INFO" | "0" => Ok("INFO".to_string()),
        "UVM_WARNING" | "1" => Ok("WARNING".to_string()),
        "UVM_ERROR" | "2" => Ok("ERROR".to_string()),
        "UVM_FATAL" | "3" => Ok("FATAL".to_string()),
        other => Err(format!("unsupported UVM report severity '{other}'")),
    }
}

fn uvm_report_string(value: &Value) -> Result<String, String> {
    string_literal(unwrap_conversion(value))
        .ok_or_else(|| "UVM report IDs must be string literals".to_string())
}

fn uvm_report_action_value(value: &Value) -> Result<u64, String> {
    let action = uvm_report_integer(value)?;
    let supported_actions = UVM_DISPLAY_ACTION
        | UVM_LOG_ACTION
        | UVM_COUNT_ACTION
        | UVM_EXIT_ACTION
        | UVM_CALL_HOOK_ACTION;
    if action & !supported_actions != 0 {
        return Err(
            "only UVM_NO_ACTION, UVM_DISPLAY, UVM_LOG, UVM_COUNT, UVM_EXIT, UVM_CALL_HOOK, and their combinations are supported"
                .into(),
        );
    }
    Ok(action)
}

fn uvm_report_count(value: &Value) -> Result<u64, String> {
    let value = unwrap_conversion(value);
    let count = string_field(value, "constant")
        .or_else(|| string_field(value, "value"))
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| "UVM report count must be an integer literal".to_string())?;
    Ok(count.max(0) as u64)
}

fn uvm_report_bool(value: &Value) -> Result<bool, String> {
    let value = unwrap_conversion(value);
    match string_field(value, "constant")
        .or_else(|| string_field(value, "value"))
        .as_deref()
    {
        Some("0") => Ok(false),
        Some("1") => Ok(true),
        _ => Err("UVM report boolean must be a literal 0 or 1".into()),
    }
}

fn is_uvm_report_server_receiver(receiver: &Value) -> bool {
    string_field(receiver, "kind").as_deref() == Some("Call")
        && string_field(receiver, "subroutine")
            .and_then(|subroutine| {
                subroutine
                    .split_whitespace()
                    .next_back()
                    .map(str::to_string)
            })
            .as_deref()
            == Some("get_server")
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

fn user_uvm_class_method(
    chain: &[(String, Value)],
    official_index: usize,
    method_name: &str,
) -> Option<(String, Value)> {
    chain
        .iter()
        .take(official_index)
        .find_map(|(owner, class)| {
            let method = class
                .get("members")
                .and_then(Value::as_array)
                .and_then(|members| {
                    members.iter().find(|member| {
                        string_field(member, "kind").as_deref() == Some("Subroutine")
                            && string_field(member, "name").as_deref() == Some(method_name)
                    })
                })?;
            Some((owner.clone(), method.clone()))
        })
}

fn official_uvm_report_catcher_index(chain: &[(String, Value)]) -> Option<usize> {
    chain.iter().position(|(owner, class)| {
        owner.split('@').next() == Some("uvm_report_catcher")
            && string_field(class, "source_file")
                .and_then(|file| file.split('/').next_back().map(str::to_string))
                .as_deref()
                == Some("uvm_report_catcher.svh")
    })
}

fn official_uvm_callback_index(chain: &[(String, Value)]) -> Option<usize> {
    chain.iter().position(|(owner, class)| {
        owner.split('@').next() == Some("uvm_callback")
            && string_field(class, "source_file")
                .and_then(|file| file.split('/').next_back().map(str::to_string))
                .as_deref()
                == Some("uvm_callback.svh")
    })
}

fn official_uvm_subscriber_index(chain: &[(String, Value)]) -> Option<usize> {
    chain.iter().position(|(owner, class)| {
        owner.split('@').next() == Some("uvm_subscriber")
            && string_field(class, "source_file")
                .and_then(|file| file.split('/').next_back().map(str::to_string))
                .as_deref()
                == Some("uvm_subscriber.svh")
    })
}

fn official_uvm_sequence_index(chain: &[(String, Value)]) -> Option<usize> {
    chain.iter().position(|(owner, class)| {
        matches!(
            owner.split('@').next(),
            Some("uvm_sequence" | "uvm_sequence_base")
        ) && string_field(class, "source_file")
            .and_then(|file| file.split('/').next_back().map(str::to_string))
            .is_some_and(|file| {
                matches!(file.as_str(), "uvm_sequence.svh" | "uvm_sequence_base.svh")
            })
    })
}

fn uvm_callback_specialization(
    call: &Value,
    class_definitions: &std::collections::BTreeMap<String, Value>,
    method_name: &str,
) -> Option<(String, String)> {
    let address = string_field(call, "subroutine")?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    for class in class_definitions.values() {
        let is_callback_pool = string_field(class, "name").as_deref() == Some("uvm_callbacks")
            || string_field(class, "genericClass")
                .and_then(|generic| generic.split_whitespace().next_back().map(str::to_string))
                .as_deref()
                == Some("uvm_callbacks");
        if !is_callback_pool {
            continue;
        }
        let Some(members) = class.get("members").and_then(Value::as_array) else {
            continue;
        };
        if !members.iter().any(|member| {
            string_field(member, "kind").as_deref() == Some("Subroutine")
                && string_field(member, "name").as_deref() == Some(method_name)
                && member.get("addr").and_then(Value::as_u64) == Some(address)
        }) {
            continue;
        }
        let target_type = members
            .iter()
            .find(|member| {
                string_field(member, "kind").as_deref() == Some("TypeParameter")
                    && string_field(member, "name").as_deref() == Some("T")
            })
            .and_then(|member| string_field(member, "type"))
            .and_then(|type_name| type_name.split_whitespace().next_back().map(str::to_string))?;
        let callback_type = members
            .iter()
            .find(|member| {
                string_field(member, "kind").as_deref() == Some("TypeParameter")
                    && string_field(member, "name").as_deref() == Some("CB")
            })
            .and_then(|member| string_field(member, "type"))
            .and_then(|type_name| type_name.split_whitespace().next_back().map(str::to_string))?;
        return Some((target_type, callback_type));
    }
    None
}

fn uvm_callback_mode_value(value: &Value) -> Result<i64, String> {
    if string_field(value, "kind").as_deref() == Some("UnaryOp")
        && string_field(value, "op").as_deref() == Some("Minus")
    {
        let operand = value
            .get("operand")
            .ok_or_else(|| "negative UVM callback mode has no operand".to_string())?;
        let magnitude = string_field(operand, "value")
            .or_else(|| string_field(operand, "constant"))
            .and_then(|text| text.parse::<i64>().ok())
            .ok_or_else(|| "UVM callback mode must be -1, 0, or 1".to_string())?;
        return Ok(-magnitude);
    }
    string_field(value, "value")
        .or_else(|| string_field(value, "constant"))
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| "UVM callback mode must be -1, 0, or 1".to_string())
}

fn expression_call(expression: &Value) -> Option<&Value> {
    (string_field(expression, "kind").as_deref() == Some("Call")).then_some(expression)
}

fn uvm_callback_macro_method(statement: &Value) -> Option<&Value> {
    if string_field(statement, "kind").as_deref() != Some("Block") {
        return None;
    }
    let body = statement.get("body")?;
    if string_field(body, "kind").as_deref() != Some("List") {
        return None;
    }
    let list = body.get("list")?.as_array()?;
    if list.len() != 3
        || list
            .iter()
            .take(2)
            .any(|item| string_field(item, "kind").as_deref() != Some("VariableDeclaration"))
        || string_field(&list[2], "kind").as_deref() != Some("WhileLoop")
    {
        return None;
    }

    let loop_statement = &list[2];
    let condition = loop_statement.get("cond")?;
    if string_field(condition, "kind").as_deref() != Some("BinaryOp")
        || string_field(condition, "op").as_deref() != Some("Inequality")
        || condition
            .get("right")
            .is_none_or(|right| string_field(right, "kind").as_deref() != Some("NullLiteral"))
    {
        return None;
    }
    let callback_symbol = string_field(condition.get("left")?, "symbol")?;

    let loop_body = loop_statement.get("body")?;
    if string_field(loop_body, "kind").as_deref() != Some("Block") {
        return None;
    }
    let loop_body_body = loop_body.get("body")?;
    if string_field(loop_body_body, "kind").as_deref() != Some("List") {
        return None;
    }
    let loop_items = loop_body_body.get("list")?.as_array()?;
    if loop_items.len() != 2 {
        return None;
    }
    let method_expression = loop_items[0].get("expr")?;
    let method_call = expression_call(method_expression)?;
    method_call.get("thisClass")?;

    let advance_expression = loop_items[1].get("expr")?;
    let advance = advance_expression.get("right")?;
    let advance_receiver = advance.get("thisClass")?;
    if string_field(advance, "subroutine")
        .and_then(|subroutine| {
            subroutine
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        })
        .as_deref()
        != Some("next")
        || !string_field(advance_receiver, "type")
            .is_some_and(|type_name| type_name.contains("uvm_callback_iter#"))
        || string_field(advance_expression.get("left")?, "symbol").as_deref()
            != Some(callback_symbol.as_str())
    {
        return None;
    }

    Some(method_call)
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

fn collect_uvm_config_wait_calls<'value>(value: &'value Value, calls: &mut Vec<&'value Value>) {
    if let Some(fields) = value.as_object() {
        if string_field(value, "kind").as_deref() == Some("Call")
            && string_field(value, "subroutine")
                .and_then(|subroutine| {
                    subroutine
                        .split_whitespace()
                        .next_back()
                        .map(str::to_string)
                })
                .as_deref()
                == Some("wait_modified")
        {
            calls.push(value);
        }
        for nested in fields.values() {
            collect_uvm_config_wait_calls(nested, calls);
        }
        return;
    }
    if let Some(values) = value.as_array() {
        for nested in values {
            collect_uvm_config_wait_calls(nested, calls);
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
        "create_object_by_type" => "object_by_type",
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
