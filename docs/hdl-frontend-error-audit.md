# HDL frontend error and gap audit

Date: 2026-10-23 (local time)
Repository state audited: `fix-digital` including the current bounded scalar dynamic packed-lvalue lowering, package-qualified sparse-memory member fingerprints, and exact hierarchy-global class-static property fingerprints.

This is an engineering audit, not a marketing maturity claim. “All errors”
below means **all currently known, code-visible, or probe-reproduced frontend
errors and gaps**. It cannot mean a mathematical proof that no other error
exists: the project has no exhaustive IEEE 1800 LRM conformance suite and no
commercial-tool differential oracle yet. Those missing oracles are themselves
listed as audit findings.

## 1. Executive conclusion

The frontend now has one default executable authority:

1. slang validates the full source, expands macros/includes, checks types,
   elaborates parameters/generate, and emits authoritative AST JSON.
2. `compile_files`, `compile_source`, and the CLI use direct slang-AST
   lowering. Unsupported constructs fail closed; they no longer silently fall
   back to preprocess/reparse lowering.
3. `compile_files_bounded` is an explicit compatibility/differential entry
   point, not the public executable default.

This removes the hidden two-authority hazard. It does not make direct lowering
complete: dynamic types, full class/UVM semantics, timing/assertion/coverage,
and many commercial system interfaces still fail closed.

The recent bounded-string ordering mistake was exactly the kind of error this
split makes possible: slang understood the source type, but repository-specific
lowering/tests supplied the semantic interpretation. The ASCII ordering error
was corrected (`"a" < "-"` is false), but the audit below lists the remaining
places where repository semantics are incomplete or under-validated.

### 1.1 Root cause of the severe generate-loop truncation

The former silent ten-thousand-iteration stop was a downstream bounded-elaborator
defect, not a slang parsing/elaboration failure. Its root causes were:

1. A resource guard was implemented as successful early termination instead of
   a fail-closed diagnostic.
2. The loop expander conflated “we reached an internal work cap” with “the LRM
   construct is complete,” so a partial result could be returned as `Ok`.
3. The regression suite treated the truncated instance count as the expected
   oracle, codifying the wrong behavior.
4. There was no boundary test immediately above the cap and no independent
   elaboration oracle proving the full generated hierarchy.

The current guard reports an error after 131,072 steps, detects non-advancing
loops, and has regressions both at the final iteration and above the cap. The
standing engineering rule is stricter: every resource boundary must fail closed;
partial generated hierarchies must never be returned, cached, or validated as a
complete design.

### 1.2 Finding inventory

The open inventory below contains 20 rolled-up frontend findings (`F-*`), 64
direct-AST coverage gaps (`D-*`), 12 bounded-path limitations (`B-*`), and 17
additional confirmed bounded-path defects/blind spots (`O-*`). It also records
19 explicit resource limits (`L-*`). These categories overlap intentionally:
`F-*` rows are architectural rollups, while `D-*`, `B-*`, and `O-*` expose their
specific known consequences. The 24 `R-*` rows are former defects retained only
to document their fixes; they are not current errors.

### 1.3 Exact remaining unresolved-fingerprint sites

F-03 currently has six production sites that can emit a wildcard `"?"`
fingerprint plus one wildcard matcher in the current worktree. They must be
removed class-by-class; a global hard-error conversion previously exposed about
65 tests that still depended on these wildcards and was therefore reverted.

| Location | Construct class | Cleanup requirement |
| --- | --- | --- |
| `summary/validation.rs`, continuous assignment unresolved memory target | A slang fingerprint begins with `MemoryRead` or contains `PackedSelect`, but no matching repository memory target exists outside strict generated-hierarchy mode. | Preserve an exact canonical memory/suffix mapping and reject unknown targets. |
| `summary/validation.rs`, continuous assignment unsupported expression | `slang_expression_fingerprint_with_parameters` returns `None`. | Add the missing slang and repository fingerprints plus a wrong-shape negative test. |
| `summary/validation.rs`, legacy procedural arrays | Non-strict legacy per-element array expansion intentionally uses `"?"`; class-static property reads now use exact canonical symbols. | Represent the source assignment once or add a legal explicit expansion fingerprint. |
| `summary/validation.rs`, unresolved strict memory-member lvalue | The strict member-path helper returns `None` for remaining layouts; package-qualified nested aliases now resolve exactly. | Extend the remaining layout/member-path coverage instead of downgrading to `"?"`. |
| `summary/validation.rs`, procedural unresolved memory target | A generic procedural RHS references a memory/packed select whose repository target cannot be confirmed. | Resolve hierarchical memory identity and reject unconfirmed references. |
| `summary/validation.rs`, procedural unsupported expression | A generic procedural RHS has no slang expression fingerprint. | Add exact expression fingerprints and runtime differentials. |
| `summary/expressions.rs`, wildcard matcher | `contains_expected_shapes` accepts any remaining actual shape when expected is `"?"`. | Remove this branch only after the corresponding production class is exact. |

## 2. Validation performed for this audit

All commands were run against the current worktree:

External oracle versions: slang commit `54c19b53a4cc1beb875c4529409d9314ba675b76`
and official UVM commit `78c06547a2a0a29b3dc9dcafae62b75b2ff61544`.

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test -p eda-hdl --all-targets -- --nocapture` | 90 passed |
| `cargo test -p eda-slang --all-targets -- --nocapture` | 232 passed, 1612.25 s with the pinned official UVM checkout |
| `cargo test -p eda-core --all-targets -- --nocapture` | 29 passed |
| `cargo test -p eda-sim --all-targets -- --nocapture` | 85 passed |
| `cargo test -p eda-synth --all-targets -- --nocapture` | 44 passed |
| `SUITE_FILTER=sv_ cargo test -p eda-tests --test suite full_suite_all_green` | Pass in 134.54 s |

A temporary compatibility probe also compared the direct slang-AST path with
the bounded/preprocessed path on representative valid constructs. The probe
was removed after collecting results; permanent conformance tests are a
required remediation in P0-7.

These results prove that the currently named regressions pass. They do **not**
prove commercial SystemVerilog correctness or exhaustive LRM coverage.

The official-UVM integration tests use an explicit source contract: when
`UVM_HOME` is provided, `src/uvm.sv` and the Accellera UVM 2020.3.1 version
marker must be present, and an invalid checkout fails the test rather than
silently returning success. Omitting `UVM_HOME` still selects the optional
lane; the required gate must set it explicitly.

## 3. Previously identified severe defects that are fixed

These items are no longer current errors. They are listed to prevent stale
information from re-entering the roadmap.

| ID | Former defect | Current status and evidence |
| --- | --- | --- |
| R-01 | Generate loops silently stopped after 10,000 iterations | Fixed. The limit is now explicit at 131,072 steps; `generate_for_loop_reports_excessive_steps_instead_of_truncating` and `generate_for_loop_reaches_the_final_iteration` cover it. |
| R-02 | Multiple candidate tops were silently guessed | Fixed. `ambiguous_top_is_an_error` requires an explicit top. |
| R-03 | False `generate if` without `else` was rejected | Fixed. `false_generate_if_without_else_generates_no_items` passes. |
| R-04 | Descending generate loops were rejected | Fixed. `generate_for_loop_accepts_declared_genvar_and_descending_iteration` passes. |
| R-05 | Standalone `genvar` declarations were rejected | Fixed. Generate-region/standalone genvar tests pass. |
| R-06 | Runtime whole unpacked-array elements required special indexed syntax | Fixed. `runtime_whole_unpacked_array_elements_compile_as_reads_and_writes` passes. |
| R-07 | Every unpacked array was flattened into at most 4,096 packed bits | Substantially fixed. Large one-dimensional arrays now use first-class sparse memories; 1 MiB-by-8 regressions exist. Multidimensional arrays still have explicit bounded limits (L-06/L-07). |
| R-08 | Multi-file syntax errors lost source identity | Partially fixed. Named source compilation maps parser line errors to originating files. Full column/range/elaboration spans remain incomplete (F-17/O-07). |
| R-09 | Direct lowering errors had no source locations | Partially fixed. Many direct AST errors now include slang spans. Not every bounded/elaboration error has one (F-17/O-07). |
| R-10 | ASCII string ordering was expected incorrectly | Fixed in `b4a2b34`. `"a"` is greater than `"-"`, so `mixed < right` is false/zero for the tested operands. |
| R-11 | Unsigned relational expressions were cast from `u64` to `i64` | Fixed. Comparisons now order full-width packed bit vectors; direct-versus-bounded regressions cover values above `I64::MAX` and signed/unsigned 65-bit ordering. |
| R-12 | A 65-bit decimal literal could panic while shifting beyond `u64::BITS` | Fixed. Decimal literal decoding checks the bit index before shifting, and a 65-bit zero regression covers the former panic boundary. |
| R-13 | Task `return` had no runtime subroutine boundary | Fixed. Inlined task bodies lower into `Stmt::Scope`; `Stmt::ReturnBarrier` bounds `Stmt::Return`, so a return skips only the task body and resumes its caller. `slang_ast_direct_lowering_supports_task_returns` covers taken/non-taken returns and caller continuation. |
| R-14 | Ordinary unpacked-array ports and child connections shared the sparse-memory IR | Fixed. Final elaboration partitions top-level ports into `ArrayPortDecl` and sparse-only `MemoryPortDecl`, and child aliases into `ArrayConnection` and sparse-only `MemoryConnection`. Directional APIs reject cross-kind use; input, output, inout, and nested child differentials execute. |
| R-15 | Matching fixed unpacked-array copies failed and `$size` was unknown | Fixed. The direct path lowers blocking and nonblocking whole-array assignment to `Stmt::MemoryCopy`, samples source values before blocking updates, supports matching multidimensional shapes up to the explicit 4,096-element limit, and evaluates `$size(array[, dimension])`. |
| R-16 | Fixed-array bound queries were unknown | Fixed. `$left`, `$right`, `$low`, `$high`, and `$increment` execute with explicit dimension selection; descending and ascending multidimensional regressions cover negative and positive increments, and slang rejects out-of-range dimensions. |
| R-17 | Array dimension-count and bit-count queries were unknown | Fixed. `$dimensions`, `$unpacked_dimensions`, and `$bits` execute for fixed unpacked arrays plus packed vectors/scalars; regressions distinguish packed and unpacked dimensions and compute aggregate bit counts. |
| R-18 | Procedural unpacked assignment patterns were rejected | Fixed. Positional patterns execute for multidimensional arrays with blocking or nonblocking semantics; one-dimensional structured patterns execute index setters with a default value. Element-count, shape, duplicate-index, unsupported structured-form, and negative-index cases fail closed. |
| R-19 | Direct dynamic arrays were rejected | Fixed. One-dimensional integral dynamic arrays now lower to explicit dynamic `MemoryDecl` objects with capacity 256; runtime `new[size]`, blocking/nonblocking allocation, `size()`, `delete()`, indexed reads/writes, out-of-range X/no-op behavior, `$size`, and runtime `$bits` execute. |
| R-20 | Direct queues were rejected | Fixed. One-dimensional integral queues now lower to dynamic FIFO memories; unbounded queues use capacity 256 and bounded `[$:n]` queues use `n+1` capacity. `push_back`, `push_front`, `pop_front`, `pop_back`, `size()`, all/indexed `delete()`, `insert()`, indexed access, empty-pop and invalid-index behavior, runtime `$size`, and runtime `$bits` execute. |
| R-21 | Direct associative arrays were rejected | Fixed. One-dimensional integral associative arrays now use sparse runtime key/value storage with capacity 256. Indexed read/write, `num()`, `exists()`, keyed `delete()`, clear-all `delete()`, missing-key X behavior, runtime `$size`, and runtime `$bits` execute; inserting the 257th key fails closed. |
| R-22 | Bounded queue overflow failed closed | Fixed. The IR now preserves the declared queue bound. Full bounded queues discard an out-of-bounds tail as required by IEEE 1800: `push_back` leaves a full queue unchanged, while `push_front`/interior `insert` drop the final element; a regression covers both paths. The 256-element unbounded-queue representation still fails closed as an explicit resource limit. |
| R-23 | Dynamic-container whole-object copies were absent | Fixed. `MemoryCopy` now carries fixed-array, dynamic-array, queue, and associative kinds. Same-shape one-dimensional integral copies execute with blocking/nonblocking sampling, dynamic size propagation, queue ordering, bounded-queue tail discard, associative key/value preservation, destination clearing, capacity checks, and shape/index mismatch diagnostics. |
| R-24 | Associative traversal methods were unsupported | Fixed. Integral associative arrays now execute `first`, `last`, `next`, and `prev` as blocking status assignments. They preserve declared signed/unsigned ordering, update the ref key immediately on success, leave it unchanged on failure/empty traversal, and reject nonblocking calls and key-width mismatches; an exact structural fingerprint rejects wrong traversal directions. |

## 4. Current P0 and P1 findings

Progress percentages describe executable frontend capability, not the roadmap’s
broader tool-flow percentage.

| ID | Name | Initial requirement | Current state / error | Severity | Progress | Next required evidence |
| --- | --- | --- | --- | --- | ---: | --- |
| F-01 | Single authoritative lowering path | Every valid input accepted by the authoritative frontend must either lower through the same semantic model or fail closed with an actionable diagnostic. | `compile_files`, `compile_source`, and the CLI now use the direct path. The bounded preprocess/reparse implementation remains as explicit `compile_files_bounded` for compatibility differentials and still has non-authoritative semantics. | P0 architecture | 65% | Remove or formally isolate `compile_files_bounded` and migrate every remaining differential to explicit conformance-only entry points. |
| F-02 | Direct AST language coverage | Direct lowering must cover slang’s elaborated SystemVerilog surface construct-by-construct. | Member/statement/expression matches reject many dynamic, class, timing/assertion/checker, and system-call forms; simple non-virtual classes, single inheritance, direct base/derived handle aliasing with runtime virtual dispatch, hierarchy-global static class properties/methods, package-qualified classes, elaborated value-parameterized class specializations with method-body parameter substitution, constant/dynamic class-array read/write/handle-assignment subsets, null handle assignment/comparison, scalar plus one-dimensional class-valued composition arrays with nested field/method access, simple integral/string type aliases, class property initializers, direct lowering of the official Accellera UVM reference source, explicit construction/run_phase execution for a class derived from official `uvm_test`, exact/wildcard-path name/type instance overrides, literal `set_type_alias(...)` and path-scoped `set_inst_alias(...)` resolution, and a minimal literal `run_test`, sequential common-phase, runtime-identity objection, asynchronous objection-drain, name/type-based factory-override, and component/object-registry creation with compatible derived overrides/runtime virtual dispatch, preserved component/full names, parent identity, checked parent-handle recovery, and literal child lookup, and configured-timeout bootstrap now execute. | P0 | 62% | Maintain a machine-checked construct matrix with one positive test and one fail-closed unsupported test per construct. |
| F-03 | Complete structural/semantic post-check | Slang and repository IR must agree on declarations, types, statements, expressions, hierarchy, and behavior. | Current summary checks top, ports, declarations, assignment shapes, literals, simple width/signedness, processes, instance inventory, exact associative first/last/next/prev traversal fingerprints, exact dynamic-container size/num, associative exists, string-comparison, mode-aware simple integral conversion, unary plus/minus/reductions, binary xor/xnor, logical implication/equivalence, direct wildcard equality/inequality mask fingerprints, dynamic allocation, whole-container copies, queue pop, fixed/dynamic `$size`, direct string length, byte indexing, concatenation/literal conversion, case/compare/substring/radix fingerprints, exact direct enum `.name()` receiver/value/name/order fingerprints, including an integral type-alias base, and module-unit-aware `$time`/`$stime` fingerprints, exact whole-element and indexed-part sparse-memory read memory/index/start fingerprints, including alias-backed packed aggregate element shapes and recursive multidimensional index order (with unique hierarchical interface-memory suffix resolution), exact constant packed element/range/indexed select physical-range fingerprints for descending and ascending declarations, exact dynamic packed element conditional-tree fingerprints, exact module- and procedure-local dynamic packed lvalue conditional-concatenation fingerprints, including direct and bounded ascending/descending scalar bit-select differentials, exact simple, nested, forward-declared, and package-qualified nested packed sparse-memory member-write memory/index/member-path/RHS concatenation fingerprints, including scoped package/local alias disambiguation, exact hierarchy-global class-static property read fingerprints with wrong-source rejection and exact direct non-virtual instance methods that directly return one receiver property or one one-level composed receiver-property method, including value-parameterized specializations, parameterized two-property addition methods, direct concrete-receiver parameter-constant virtual methods, one-source parameter-only runtime virtual-dispatch conditional trees, guarded `$cast` target method calls with direct constant-return virtual methods, and dynamic class-array source reads copied into a named handle and dynamic class-array target writes from named or dynamic-array sources, and non-strict sparse-memory member lvalue fingerprints and exact direct simple and nested class-property read paths and exact whole-handle copies from resolved class-property paths, named field-bearing class sources, and stable field-less class metadata, and exact no-argument static-function returns that directly return a static property, and exact single-return package-function body/argument fingerprints with imported parameter constants. Fingerprints still fall back to `"?"` for remaining unresolved type layouts, complex inlined class-static call expressions, legacy array expansion, and remaining expression forms. | P0 | 96% | Remove all remaining `"?"` equivalence cases and add runtime differential simulation for each construct. |
| F-04 | Full official UVM execution | Compile and execute the official Accellera UVM library and user environments, including phasing, objections, factory, configuration, sequences, TLM, callbacks, reports, and coverage. | The official Accellera UVM 2020.3.1 source now direct-lowers with `UVM_NO_DPI`; a class derived from official `uvm_test` can be explicitly constructed and its overridden `run_phase` task can execute. A minimal literal `run_test("class")` entry applies official compile-time `uvm_factory::get().set_type_override_by_name` string overrides and `set_type_override_by_type(Class::get_type(), ...)` wrapper overrides, allocates the selected class, applies explicit constructor defaults, and supports component/object `Class::type_id::create(...)` assignment to a prepared class property with an explicit synthetic depth-0 `uvm_root`, official fixed `uvm_test_top` constructor naming and constructor-name/full-name preservation for `get_name()`/`get_full_name()` identity comparison for `get_parent()`, `$cast(parent, component.get_parent())` with runtime virtual dispatch, literal direct `create_component_by_name(...)`/`create_object_by_name(...)` and checked type-wrapper `create_component_by_type(...)`/`create_object_by_type(...)`, `get_child(...)`, relative/leading-dot absolute `lookup(...)`, `get_num_children()`, literal `has_child(...)`, and `get_depth()` through the explicit synthetic `uvm_root` and prepared, duplicate-checked hierarchy, including compatible derived selection and virtual method dispatch, and sequentially executes user-overridden `build`, `connect`, `end_of_elaboration`, `start_of_simulation`, `run`, `extract`, `check`, `report`, and `final` phase methods with synthetic phase objects. A deliberately narrow bootstrap recognizes a class derived from the official `uvm_test.svh` definition, omits inherited UVM state, and turns `super.new` at that boundary into an empty statement. The omitted state and constructor remain fail closed when referenced. The bootstrap phase object supports `raise_objection`/`drop_objection` calls with positive compile-time counts, selects prepared owner slots by runtime object identity, tracks separate owner totals, waits asynchronously for total drain before later phases, enforces the default 1000-tick timeout or a preceding literal `uvm_root::get().set_timeout(...)`, and fails unknown-owner or wrong-owner drops; the registry and timeout are finite/pre-elaborated and do not implement official objection callbacks or configurable scheduling. Direct literal global `uvm_report_info(...)`/`uvm_report_warning(...)`/`uvm_report_error(...)` calls display literal IDs/messages; all four basic `uvm_info/warning/error/fatal` macros lower with literal default/id/severity/id+severity verbosity and action controls using official precedence, and literal `get_report_max_verbosity_level()`, `get_report_verbosity_level(...)`, and `get_report_action(...)` queries return those configured values. Literal report-server counters track severity/id/quit counts and max-quit termination for `UVM_COUNT`; user-overridden literal `report_hook(...)` and severity hooks execute only for `UVM_CALL_HOOK` actions after verbosity filtering and before catchers/reporting, with literal-zero suppression; multiple `uvm_report_catcher` registrations support literal APPEND/PREPEND ordering, null and object-scoped contexts, literal callback_mode enable/disable, context-matching deletion, literal delete_by_name patterns, literal set_severity/set_message/set_action modifications carried into a subsequent THROW report, and stop at the first matching enabled CAUGHT result. Supported actions are display, default-file `UVM_LOG` stdout suppression, count, exit, and `UVM_CALL_HOOK`; `UVM_STOP`, recorder actions, configured file handles, and any other action bits fail closed. A literal `uvm_callbacks#(T,CB)::add(...)` subset registers prepared callback objects by runtime context or a null typewide context, resolves the specialized T/CB pair from slang AST, applies typewide entries to matching target derivatives, and executes `uvm_do_callbacks` methods in append/prepend order with derived callback virtual dispatch; literal callback_mode(0/1), callback_mode() queries, object/typewide delete(...), and null-root literal wildcard delete_by_name(...), while iterator APIs, runtime-created objects, full derivation registries, and dynamic pools fail closed. A literal official-UVM sequence bootstrap executes user `uvm_sequence#(REQ)::body()` with prepared `uvm_sequence_item` handles, `start_item`/`finish_item`, direct `uvm_sequencer#(REQ)` `get_next_item`/`item_done(response)` handshakes, blocking `get_response`, `set_transaction_id`/`get_transaction_id`, runtime transaction-id-selective `get_response`, an official-depth-eight FIFO response queue with fail-closed overflow, FIFO-preserving out-of-order removal, and scalar-field request/response copies, while configurable depth, the official overflow/drop/warning policy, class-valued fields, and the full sequence/methodology runtime remain fail closed. A literal official-UVM `uvm_analysis_port#(T)` bootstrap connects prepared `uvm_analysis_export#(T)` chains and `uvm_subscriber#(T)` analysis exports in order, detects export cycles, and lowers `write(...)` to each subscriber virtual `write` method; a bounded `uvm_tlm_fifo#(int)` subset supports explicit sizes 1..4, statement-form try_put/try_get/try_peek/flush, process-suspending blocking put/get/peek, and size/used/is_empty/is_full/can_* queries, while literal `uvm_put_port#(int)`, `uvm_put_export#(int)`, `uvm_get_peek_port#(int)`, and `uvm_get_peek_export#(int)` endpoint chains route matching try_*, blocking put/get/peek, and can_* calls through official FIFO exports, reject direction mismatches, and detect cycles; side-effecting expressions, dynamic connections, arbitrary port graphs, and remaining TLM transports fail closed. A literal `uvm_config_db#(T)` bootstrap records typed integer/string resources, applies the UVM-no-DPI glob subset (`*`, `?`, and empty global scope), accepts slash-delimited literal regex scopes with official-style context composition and anchored full matches, selects build-time hierarchy precedence (`1000 - component depth`) or runtime/default last-write behavior, and lowers conditional `get(...)`/`exists(...)` success/failure branches (including slang’s default spell-check-zero argument) plus prepared literal `wait_modified(...)` events that wake on later matching sets, global cross-type Levenshtein spell-check suggestions for missing fields, and literal `uvm_resource_db#(T)::set/set_default/set_anonymous/set_override/set_override_type/set_override_name/read_by_name/write_by_name/read_by_type/write_by_type` resources sharing the config DB pool with name/type/both priority directions, int/string defaults, anonymous type-readable resources, and conditional writes that update only an existing matching resource; non-literal values/arguments/accessors, direct value-returning resource calls, and unprepared waiters fail closed. Regex dialect equivalence with the official POSIX-backed implementation and regex field names is not yet differentially proven; it also has no dynamic resource-pool API, dynamic waiter pool, callback priority controls, resources outside the literal bootstrap, runtime-created objects, or complete official `uvm_resource_db` semantics. This is not the official parallel phase graph. These exceptions must not be generalized, and they are not a full UVM object model or phase scheduler. Type aliases require literal names and `Class::get_type()` wrappers. Type overrides require literal strings or `Class::get_type()` wrappers, registration before `run_test`, and known compatible derived classes; new entries use official-style front insertion, repeated registrations update the existing entry when `replace` is true/default and retain the earlier entry/mode when `replace` is false. Instance aliases require literal names, `Class::get_type()` originals, and literal paths. Instance overrides require literal paths; `*`/`?` wildcard patterns are matched in official registration order (first matching override wins), while runtime paths fail closed. Component/object registry creation currently requires a prepared target property compatible with the selected type, literal names/contexts, and matching one/two-argument constructors; derived classes that introduce storage not present on the prepared target, override queues/aliases, runtime registration, dynamic target allocation, and full hierarchy integration remain unsupported. Unsupported package-only class/queue aliases and class-valued static state are likewise deferred only while unused and fail closed when referenced. Full official UVM parallel phasing, resource-pool configuration, TLM, sequences, full generic callback pools/derivation registries, and coverage remain unexecuted, so this is not official UVM runtime completion. Direct AST lowering also executes a small class subset with object fields, constructors, methods, single-inheritance field flattening, constructor chaining, receiver-resolved virtual calls, direct base/derived handle reassignment, object-identity propagation, runtime virtual dispatch, hierarchy-global static class properties/methods, package-qualified classes, elaborated value-parameterized class specializations with method-body parameter substitution, constant-indexed and dynamic class-array read/write/handle-assignment subsets, null handle assignment/comparison, checked prepared-handle `$cast` statements/conditions, and scalar plus one-dimensional class-valued composition arrays with nested field/method access. It does not yet execute complete dynamic object semantics or the full official UVM inheritance, phase graph, resource configuration, TLM, sequence, callback, or objection schedulers. The bounded frontend has a separate UVM-lite subset. | P0 | 99% direct/runtime bootstrap; 0% for full official UVM execution | Implement resource callback/priority/dynamic-waiter, full derivation registries plus iterator APIs and STOP/recorder/file-backed action semantics, prove POSIX regex equivalence, and run an official UVM example through phasing, objections, configuration, TLM, and sequences. |
| F-05 | Full data types | Support integral, real, string, event, chandle, enum, packed/unpacked aggregates, dynamic arrays, queues, associative arrays, classes, virtual interfaces, type parameters, and typed packages. | Direct path supports simple integral vectors, packed arrays, module and package enums, packed structs/unions, fixed-layout unpacked structs/unions with nested field reads/writes, whole-object copies, and unpacked-union aliasing, bounded strings, sparse memories, scalar named events, and fixed-size event arrays with element triggers/waits/`.triggered`, and simple non-virtual class objects with isolated field storage, inherited fields, constructor chaining, receiver-resolved virtual calls, direct base/derived handle reassignment, object-identity propagation, runtime virtual dispatch, hierarchy-global static class properties/methods, package-qualified classes, elaborated value-parameterized class specializations with method-body parameter substitution, constant-indexed and dynamic class-array read/write/handle-assignment subsets, null handle assignment/comparison, scalar plus one-dimensional class-valued composition arrays with nested field/method access, simple integral/string type aliases, and string parameters. It also supports one-dimensional integral dynamic-array, queue, and associative-array subsets with capacity 256. Dynamic arrays execute runtime `new[size]`, `size()`, `delete()`, indexed access, out-of-range X behavior, `$size`, and `$bits`; queues execute push/pop ends and bounded FIFO lifecycle; associative arrays execute indexed key/value access, `num()`, `exists()`, keyed/all delete, missing-key X behavior, `$size`, and `$bits`. It rejects real, unpacked-struct assignment patterns, tagged unions, multidimensional/non-integral dynamic/queue/associative storage, dynamic/queue/associative ports and whole-object copies, arbitrary event expressions, chandle, and type parameters. | P0 | 59% | Add type-category tests with layout, assignment, comparison, and selected-member semantics. |
| F-06 | Process timing and concurrency | Model delays, event controls, wait, fork/join modes, process lifetime, ordering regions, clocking, and nondeterminism. | Direct procedural lowering supports initial/always/final subsets, `$finish` shutdown execution, implicit sensitivity, scalar any-edge signals, event lists containing scalar any-edge/posedge/negedge signals, constant procedural delays, delay-driven always generators, delay-bearing `forever` loops, `wait` statements, blocking/nonblocking named-event triggers with scalar, edge-qualified event-list, and fixed event-array element waits, time-slot `.triggered`, ordered `wait_order`, lexical sequential named-block `disable`, hierarchical clocking-block event controls with scalar input snapshots, no-skew output drives, and constant `##n` cycle delays, runtime `repeat`, `while`, and `do...while` loops with `break`/`continue`, module-scope and initialized automatic procedure-local `for` loops, scalar force/release, and concurrent `fork` branches with `join`, `join_any`, `join_none`, and nested joins. It still rejects process cancellation/`disable fork`, named-process termination, jump propagation across fork boundaries, `disable` interaction with fork/forever, clocking skews/full reactive regions, procedural `deassign`, min:typ:max delays, intra-assignment delays, and general event expressions. | P0 | 70% | Golden scheduling traces for active/NBA regions, delays, event races, fork join modes, and clocking blocks. |
| F-07 | Full temporal assertions and coverage | Support complete SVA sequences/properties, clocking, disable iff, recursion, local variables, matched/triggered, sampled-value functions, checkers, assumptions, and covergroups. | A bounded module-level temporal subset exists. Direct AST path rejects `ConcurrentAssertion`, checker instances, and covergroups; clocking blocks have only the scalar event/sampling/drive/cycle subset described in F-06. | P0 | 10% | Import an external SVA conformance suite and compare pass/fail/cover traces. |
| F-08 | System task/function surface | Support display/write/read, files, strings, random/stochastic, time, assertions control, coverage, cast, and simulation control semantics. | Direct path supports `$readmemh`, `$readmemb`, `$writememh`, `$writememb`, `$finish`, integral `$time`/`$stime`, per-module timescale/timeunit/timeprecision scaling, `$timeformat`, `$printtimescale`, and `$display`/`$write`/`$monitor` formatting subsets for `%b`, `%d`, `%h`, `%s`, argument-consuming `%t`, and `%%`, with argument-count and unsupported-specifier fail-closed checks. File IO, random, full formatting/width controls, fractional delay rounding, `$realtime`, timeunit inheritance edge cases, coverage controls, VPI/PLI, and DPI-C remain absent/incomplete. | P0 | 29% | System-task matrix covering formats, unknowns, file handles, errors, and argument checking. |
| F-09 | Interfaces, programs, checkers, and hierarchy | Support interfaces/modports, program blocks, checkers, generate hierarchy, arrays of instances, binds, configs, libraries, defparams, and primitives. | Direct path handles simple module instances, generated blocks, one-/multidimensional/generated/multidimensional interface instance arrays up to 4,096 total elements, one-/multidimensional interface array ports, value-parameterized interface specializations, and scalar/simple/multidimensional packed-vector, packed struct/union including nested packed structs, nested fixed-layout unpacked struct/union members, plus multidimensional unpacked-array interface members with input/output modport connections and input-write fail-closed checks. It still rejects type-parameter/generic interfaces, primitive instances, checker instances, specify blocks, bind, and wildcard imports. Bounded path supports selected interface/module-array subsets but not all forms. | P0 | 49% | Multi-file hierarchy matrix, including port coercion, modports, arrays, config resolution, and generated instance identity. |
| F-10 | Dynamic and unpacked storage semantics | Support dynamic allocation, queues, associative arrays, normal unpacked array variables/ports, array methods, assignment patterns, copies, bounds, and lifetime management. | The IR now separates `MemoryStorage::Ordinary` value arrays from `MemoryStorage::Sparse` RAM/ROM storage. Ordinary aggregates use dense simulator element vectors and receive no memory timing template or macro plan; process-accessed arrays and storage above 4,096 bits become sparse and retain RAM/ROM, write-mask, byte-enable, banking, and port inference. Top-level input/output/inout ordinary-array ports now have a separate `ArrayPortDecl` contract, nested child aliases have separate `ArrayConnection` records, and both sparse contracts remain independently typed. Single-, bulk-element, and multidimensional rectangular simulator stimulus/read APIs execute for both storage contracts and reject cross-kind use; bulk index validation is transactional. Direct-versus-bounded nested child input/output regressions execute. Matching fixed arrays now support blocking/nonblocking whole-object copies, positional multidimensional assignment patterns, one-dimensional default/index structured patterns, `$size`, `$left`, `$right`, `$low`, `$high`, `$increment`, `$dimensions`, `$unpacked_dimensions`, and `$bits`. One-dimensional integral dynamic arrays now execute runtime allocation, blocking/nonblocking `new[size]`, `size()`, `delete()`, indexed reads/writes, out-of-range X/no-op behavior, `$size`, and `$bits` with capacity 256, including blocking/nonblocking same-shape whole-object copies. One-dimensional integral queues execute push/pop on both ends, `size()`, all/indexed `delete()`, `insert()`, indexed access, empty-pop X behavior, no-effect invalid indexes, `$size`, and `$bits`; unbounded queues use capacity 256, bounded queues preserve IEEE 1800 discard-tail semantics when full, indexed insertion can drop the tail without changing size, and whole-object copies preserve order while a smaller bounded destination discards its out-of-bound tail. One-dimensional integral associative arrays execute sparse key/value indexed access, `num()`, `exists()`, keyed/all delete, missing-key X behavior, `$size`, `$bits`, blocking/nonblocking whole-object key/value copies with capacity 256, and `first`/`last`/`next`/`prev` as blocking status assignments using the declared index ordering. Queue slice/range methods, traversal calls outside blocking status assignments, multidimensional structured patterns, and non-integral/multidimensional dynamic/queue/associative storage remain rejected/incomplete. | P0 | 78% | Add queue slice/range methods, direct traversal control forms, and broader dynamic/queue/associative shape differentials. |
| F-11 | Complete width/signedness/type propagation | Implement IEEE 1800 self-determined and context-determined expression rules, literal sizing, mixed-sign rules, casts, port coercion, and X/Z propagation. | Many basic tests pass and summary checks simple width/sign. Full-width relational ordering now handles 64-bit and 65-bit signed/unsigned boundaries, and a direct-versus-bounded differential covers 64-bit wrap plus values above `I64::MAX`. Propagation is still not a complete LRM implementation, mixed-sign/cast/X-Z corpora remain incomplete, and bounded constant evaluation is historically `i64`-oriented. | P0 | 48% | Differential expression corpus against an independent evaluator, including mixed signs, shifts, casts, context widths, X/Z propagation, and arbitrary >64-bit constants. |
| F-12 | Net types, strength, and resolution | Preserve net kinds, implicit nets, strengths, tri-state resolution, multi-driver behavior, supply nets, and charge decay. | Basic `wire`/tri-state values work in tested cases. Direct lowering does not preserve a complete strength/net-resolution model; strength qualifiers are not a first-class IR property. | P1 correctness risk | 25% | Systematic multi-driver strength/tri-state truth tables and implicit-net diagnostics. |
| F-13 | Complete string model | Support dynamic SystemVerilog strings, Unicode/locale behavior where required by methods, mutation, substrings, conversions, formatting, and concatenation overflow. | Current model is a fixed 512-bit, 64-byte NUL-padded ASCII representation. Direct path supports literals, concatenation, length, byte reads/index writes, case conversion, comparison, `putc()`, `substr()`, and decimal/hex/binary radix conversions. Dynamic growth beyond 64 bytes, Unicode behavior, and synthesis remain incomplete/fail closed. | P1 | 55% of bounded string subset; 0% for unbounded strings | String method conformance tests and explicit overflow/unknown/NUL behavior tests. |
| F-14 | Complete function/task execution model | Support recursive/nonrecursive routines, all argument directions/default values, static/automatic lifetimes, void functions, virtual dispatch, out-of-line calls, loops, jumps, and runtime control flow. | Direct inlining supports many straight-line/control-flow functions/tasks, output/inout/ref arguments, locals, conditionals, bounded loops, and task return boundaries that skip only the inlined task body before resuming the caller. Recursion is rejected; loop unrolling is capped at 256; simple class constructors/methods inline with receiver-field substitution; inherited fields, constructor chaining, receiver-resolved virtual calls, direct base/derived handle reassignment, object-identity propagation, runtime virtual dispatch, hierarchy-global static class functions/tasks, package-qualified class methods, value-parameterized class methods with parameter-symbol substitution, and class-array method/property reads plus sampled dynamic construction/property writes, dynamic handle assignment, nested composed-receiver methods including class-valued array properties, and instance property initializers execute. Full dynamic object semantics, complete static/automatic routine lifetimes, and general out-of-line subroutine/process control remain absent. | P0 | 54% | Separate compile-time inlining tests from a real runtime subroutine/process model. |
| F-15 | Package and namespace elaboration | Support packages, imports, exports, scoped references, nested packages, compilation-unit scopes, typedef visibility, and configuration. | Bounded path handles selected package constants/functions and wildcard imports. Direct path imports compilation-unit package constants, functions, typedefs, and enums through wildcard and explicit imports, supports scoped constants/functions in imported packages, executes enum/struct-typed package constants and package enum `first`/`last`/`num`/`next`/`prev`/`name` methods, and rejects imported symbol collisions. Simple integral/string aliases and string parameters execute, package-qualified classes including duplicate unqualified names resolve through slang type identity, and the official Accellera UVM package source loads through the direct path with a trivial wrapper. Unsupported package-only class/queue aliases are deferred and fail closed when referenced. Exports, nested packages, and full shadowing remain incomplete. | P1 | 62% | Multi-package name-collision/shadowing/order tests. |
| F-16 | Parameterization and elaboration diagnostics | Support integer/string/type parameters, localparams, complex override graphs, default values, parameter ports, defparam, and cycle/dependency diagnostics. | Direct path handles simple elaborated constants, generated parameter values, and value-parameterized interface specializations with aggregate layouts. Type parameters and defparam are unsupported; bounded constant evaluation and diagnostics remain incomplete. | P0 | 42% | Parameter dependency/cycle/override matrix and preserve-overrides metadata in IR. |
| F-17 | Source diagnostics engine | Every parser/elaboration/lowering error needs file, line, column, range, severity, diagnostic code, notes, related locations, suppression, and multi-error recovery. | Direct errors often include slang spans. Bounded parser diagnostics are mostly line-only; many bounded elaboration errors are plain strings. There is no complete structured diagnostic object or suppression system. | P1 | 30% | Snapshot diagnostics for malformed and unsupported corpora; no raw `Result<_, String>` at public boundaries. |
| F-18 | Include/preprocess traceability | Diagnostics must point through macros and include stacks to user source. | Slang produces authoritative diagnostics and source spans. The bounded compatibility path loses some preprocessor provenance after single-unit preprocessing; direct spans can print non-normalized relative paths. | P1 | 50% | Include/macro expansion stack snapshots and path normalization tests. |
| F-19 | Separate compilation/library manager | Compile projects using file lists, libraries, configs, search order, encrypted/protected IP, and incremental artifact reuse. | Multiple named files work, but there is no complete config/library manager or protected-IP handling. | P1 | 15% | Project manifest tests with duplicate modules, search paths, selected configs, and cache invalidation. |
| F-20 | Golden RTL execution | RV32I and i8086 must match independent golden ISA models across memory maps and opcodes. | Repository compilation units pass slang validation and many internal regressions, but exhaustive independent golden closure is still incomplete. | P0 | 50% | Publish opcode/memory-map coverage matrices and differential results against external golden models. |

## 5. Detailed direct slang-AST lowering gaps

These findings come from the current match arms in
`crates/eda-slang/src/lowering/*`, existing tests, and the temporary direct
versus bounded probe.

### 5.1 Module members

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-01 | Class definitions/instances | Simple non-virtual classes and single inheritance lower object properties into isolated `object.property` storage; constructor arguments, `super.new` chaining, void methods, value-returning methods, direct property reads, inherited methods, receiver-resolved virtual calls, direct base/derived handle reassignment, object-identity propagation, runtime virtual dispatch, hierarchy-global static properties/methods shared across object and module instances, package-qualified classes, elaborated value-parameterized specializations, method-body class-parameter substitution, constant-indexed object arrays, dynamic object-array method/property reads, sampled dynamic object-array construction/property writes, dynamic object-array handle assignment, null handle assignment/comparison/detachment, instance property initializers, scalar class composition, and one-dimensional class-valued composition arrays with nested construction/field/method access and alias propagation execute. Property shadowing, interface classes, casts outside statements/conditions or without a provable prepared dynamic source, recursive composition, multidimensional object arrays, and full dynamic object semantics fail closed. |
| D-02 | Full interface ports/instances/modports | Scalar, simple/multidimensional packed-vector, packed struct/union including nested packed structs, fixed-layout unpacked struct/union, and multidimensional unpacked-array interface members with input/output modport connections execute through proxy nets or memory aliases; input modport writes fail closed. Multidimensional interface instance arrays with per-element modport consumers and one-/multidimensional interface array ports and value-parameterized specializations execute. Type-parameter/generic interfaces, full modport checking, virtual interfaces, and import/export methods remain unsupported. |
| D-03 | Full clocking blocks | Clocking-block declarations, scalar clocking-event controls, scalar input snapshot reads, no-skew output drives, constant `##n` cycle delays, and child-clocking event/sample/drive remapping are accepted; skews, full prepona/reactive region ordering, and reactivity semantics remain unsupported. |
| D-04 | Specify blocks | Rejects `SpecifyBlock`; no module path delay/check semantics. |
| D-05 | Checker definitions/instances | Rejects checker instantiation. |
| D-06 | Primitive/UDP instances | Rejects `PrimitiveInstance`. |
| D-07 | Full instance-array surface | One-, multidimensional, and generate-scoped elaborated instance arrays lower and simulate up to 4,096 total elements. One-/multidimensional interface array ports execute; other unsupported child/member forms still fail closed. |
| D-08 | Type parameters | Rejects `TypeParameter`. |
| D-09 | Full package imports | Wildcard and explicit imports can use package constants and inlined functions; scoped references, package enums, packed structs, enum-typed constants, package enum methods, simple integral/string aliases, string parameters, and package classes work; duplicate imported names fail closed. The official Accellera UVM source direct-loads with unsupported package-only class/queue aliases deferred and fail closed when referenced. Exports, nested packages, and full shadowing semantics remain incomplete. |
| D-10 | Defparam | Unsupported and can fail through instance-connection lowering. |
| D-11 | Bind directives | No bind member/lowering model. |
| D-12 | Program blocks | No direct program-block lifetime/scheduling semantics. |
| D-13 | Full final-procedure semantics | Straight-line final blocks execute at `$finish`, including hierarchy remapping. Timing controls, waits, process spawning, and repeated shutdown scheduling inside final procedures remain unsupported/fail-closed. |
| D-14 | Non-memory inout ports | Direct non-memory ports support only `In`/`Out`; top/child scalar inout is unsupported. Sparse child-memory inout has a special path. |

### 5.2 Types and declarations

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-16 | Real/shortreal/realtime | Type resolution rejects real types; no floating-point IR. |
| D-17 | Full event/chandle/void type semantics | Scalar named events and fixed event-array elements are executable; chandle, void, general event objects, and event ports are not represented. |
| D-18 | Full dynamic arrays | One-dimensional integral dynamic arrays with capacity 256 execute. Multidimensional and non-integral dynamic arrays, dynamic ports, and remaining array methods fail closed; same-shape one-dimensional integral whole-object copies execute. |
| D-19 | Full queues | One-dimensional integral bounded/unbounded queues with push/pop ends, size, delete, indexed access, and bit/size queries execute up to capacity 256. Range methods, non-integral/multidimensional queues, queue ports, and synthesis fail closed; insert/indexed delete and same-element whole-object copies execute. |
| D-20 | Full associative arrays | One-dimensional integral associative arrays with 64-bit-or-smaller index types, sparse key/value runtime storage, indexed access, `num()`, `exists()`, keyed/all delete, and runtime size/bit queries execute up to 256 entries. Wildcard indexes, string/class index types, slices, multidimensional/non-integral storage, ports, and synthesis fail closed; matching-index whole-object copies and blocking status-assignment forms of `first`/`last`/`next`/`prev` execute. |
| D-21 | Unpacked structs/unions | Nested fixed-layout unpacked structs and untagged unions lower to flat field-indexed storage; field reads/writes, same-type whole-object copies, and union aliasing execute. Assignment patterns and tagged unions fail closed. |
| D-22 | Typedef aliases beyond current packed/unpacked struct/union/enum forms | Unsupported target kinds. |
| D-23 | Class handles/virtual interfaces | No direct type semantics. |
| D-24 | User-defined type parameters | Unsupported. |
| D-25 | Packed struct/union patterns with padding | Explicit fail-closed errors. |
| D-26 | Packed union assignment patterns | Explicit fail-closed error. |
| D-27 | Multidimensional packed arrays as first-class selected groups | Some forms work; dimension-aware indexed/range groups report unsupported for remaining cases. |
| D-28 | Reversed simple range forms in selected paths | Explicit “not yet lowered” for affected forms. |

### 5.3 Expressions and methods

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-29 | Real arithmetic/comparisons | No real expression model. |
| D-30 | Dynamic array/queue/associative methods | Dynamic arrays support `new[]`, `size()`, `delete()`, and indexed access; queues support `size()`, all/no-index `delete()`, `insert()`, `push_*`, `pop_*`, and indexed access; associative arrays support `num()`, `exists()`, keyed/all `delete`, indexed access, and blocking status assignments for `first`/`last`/`next`/`prev`. Queue slice/range forms, dynamic-array iteration methods, direct traversal calls outside status assignments, and remaining method forms remain unsupported. |
| D-31 | Class/virtual method calls | Direct/receiver-resolved calls, hierarchy-global static class calls, package-qualified class calls, parameterized-class calls with parameter-symbol substitution, object-array method reads/writes, sampled object-array construction, dynamic object-array handle assignment, nested composed-receiver calls, and direct base-to-derived alias dispatch execute. Full OOP dispatch for arbitrary object graphs, casts, and null-safe dereference checks remains incomplete/fail closed; null equality itself executes. |
| D-32 | Full cast surface | Integral conversions and selected streaming conversions work; aggregate/real/type-param casts are incomplete. |
| D-33 | Full `inside` value grammar | Supports only simple value/range forms accepted by current helper. |
| D-34 | Full wildcard equality patterns | Requires an integer constant pattern operand. |
| D-35 | Unpacked aggregate assignment patterns | Unsupported. |
| D-36 | Multi-packed selects | Remaining dimension-aware/range cases report unsupported. |
| D-40 | Dynamic string growth/reallocation | Fixed 512-bit representation; no growth model. |
| D-41 | String synthesis | String operations fail closed in synthesis. |
| D-42 | Event expressions/triggers | Blocking/nonblocking named-event triggers, scalar event waits, fixed event-array element triggers/waits/`.triggered`, ordered `wait_order` with success/failure actions, same-time-slot `.triggered` persistence, and edge-qualified event-list statement controls are supported; arbitrary event expressions remain unsupported. |
| D-43 | Full sampled-value/past/future functions | Unsupported. |

### 5.4 Statements and scheduling

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-46 | Process cancellation | `join`, `join_any`, and `join_none` are supported, but `disable fork`, named-process termination, and sibling cancellation remain unsupported. |
| D-47 | Full named block/process termination | Initialized automatic statement-block scopes and lexical sequential named-block `disable` execute. Named/process termination and cancellation across suspended children remain unsupported. |
| D-50 | Remaining general loop forms | Runtime `repeat`, `while`/`do...while`, loop jumps, module-scope `for`, and initialized automatic procedure-local `for` with assignment/increment steps are supported. Arbitrary for-step forms and locals without entry initializers remain unsupported. |
| D-51 | Loop jumps across process/fork boundaries | `break`/`continue` work in a runtime loop continuation, but cancellation and jump propagation through fork children or named processes remain unsupported. |
| D-52 | `disable fork`/named process termination | Unsupported and fails closed; sequential lexical block `disable` is supported. |
| D-54 | Procedural `deassign` | Scalar static-lvalue `force` and `release` are supported; procedural variable `deassign` and aggregate/dynamic force targets remain unsupported. |
| D-55 | Qualified wildcard/inside checks | Normal `priority`/`unique`/`unique0` if/case checks execute, including runtime no-match and overlap diagnostics. Qualified wildcard and case-inside forms still fail closed. |
| D-56 | Full wildcard/inside case grammar | Direct statements support `casez`, `casex`, and simple `case ... inside` value/range arms. Remaining exotic range/pattern forms still fail closed. |
| D-58 | General event expressions in sensitivity | Scalar signal references, explicit edges in procedure sensitivity, edge-qualified event-list statement controls, and dynamic fixed-event-array element statement selectors are supported; arbitrary expressions and wildcard events remain unsupported in both procedure and statement sensitivity. |
| D-59 | Clocking-variable sampling/drive semantics | Scalar input snapshot reads and no-skew output drives are supported; skew semantics fail closed. |
| D-60 | Wait ordering/process arbitration | Ordered `wait_order` with success/failure actions is supported; general process arbitration and ordering across suspended processes remain unsupported. |

### 5.5 Assertions, coverage, and system tasks

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-61 | Concurrent assertions in direct procedures | Rejects `ConcurrentAssertion`. |
| D-62 | Immediate assertion action blocks/control | Direct path does not provide the full assertion statement family. |
| D-63 | Sequence/property reuse and recursion | No direct temporal AST model. |
| D-64 | Sequence local variables | Unsupported. |
| D-65 | Sequence `matched()` | Named-event `.triggered` is supported; sequence `matched()` remains unsupported. |
| D-66 | `$past` and related sampled functions | Unsupported. |
| D-67 | Covergroups/coverpoints/bins/crosses | No direct covergroup member/lowering. |
| D-68 | `$display`/`$write` family | Direct path executes `$display`, `$displayb`, `$displayh`, `$write`, `$writeb`, `$writeh`, `$monitor`, `$monitorb`, and `$monitorh` for the checked `%b/%d/%h/%s/%t/%%` subset. Other display family tasks, width controls, file handles, and format semantics remain unsupported. |
| D-69 | `$fopen`/file read-write tasks | No direct file-handle model. |
| D-70 | `$random`/`$urandom`/`$urandom_range` | No direct stochastic model. |
| D-71 | Full `$time`/`$realtime`/timescale semantics | `$time` lowers to signed 64-bit simulation time and `$stime` to unsigned 32-bit simulation time. `$timeformat` executes for supported units/precision/suffix/width and `%t` consumes an explicit time argument. Per-module time units are scaled to the global precision for procedural and continuous delays, `$time` reports in its module unit, VCD emits the canonical precision, and `timeunit`/`timeprecision` declarations plus `$printtimescale` execute for the checked subset; `$realtime`, real arithmetic, fractional/rounded delays, `$timeunit`/`$timeprecision` functions, and remaining inheritance edge cases remain unsupported. |
| D-72 | `$asserton`/`$assertoff`/coverage controls | Unsupported. |
| D-73 | DPI-C/DPI pass-fail | Unsupported. |
| D-74 | VPI/PLI | Unsupported. |

## 6. Bounded `eda-hdl` frontend limitations that remain

Even where the bounded path can accept more than the direct path, it is not a
complete commercial frontend.

| ID | Area | Remaining issue |
| --- | --- | --- |
| B-01 | Parser authority | It is a hand-written bounded parser, not the full slang grammar. Several valid forms are rejected after slang validation. |
| B-02 | Diagnostics | Parser errors often lack column/range; elaboration errors are commonly plain strings. |
| B-03 | Preprocessor | It relies on slang preprocessing when used through `eda-slang`; standalone `eda-hdl` does not implement the complete directive language. |
| B-04 | Type checking | Slang checks the original source, but compatibility reparsing/lowering does not retain all elaborated types and symbol identity. |
| B-05 | Real/dynamic data | Real, dynamic arrays, queues, and associative arrays are rejected. |
| B-06 | Classes/UVM | Bounded UVM-lite is not official UVM execution and does not execute the full Accellera class library. |
| B-07 | SVA | A selected temporal subset is not complete IEEE 1800 assertion semantics. |
| B-08 | Timing/concurrency | Basic delays and forever clocks exist, but complete event regions, process cancellation, and clocking semantics are absent. |
| B-09 | Width/sign/type semantics | Selected tests pass, but complete LRM propagation and port compatibility checking are absent. |
| B-10 | Net resolution/strength | Selected tri-state tests exist; complete strength and multi-driver semantics are absent. |
| B-11 | Function elaboration | Arrays/loops/locals have 256/4096/64-bit caps and compile-time inlining assumptions. |
| B-12 | Structural validation gaps | Slang compatibility checking cannot fingerprint every expanded array/dynamic-lvalue shape and uses `"?"` fallbacks. |

## 7. Current hard limits

These are explicit resource guards, not silent behavior. They can still reject
commercial-scale RTL.

| ID | Limit | Current value/location |
| --- | --- | --- |
| L-01 | Generate loop steps | 131,072 in `eda-hdl/src/elab.rs`. |
| L-02 | Hierarchy depth | 64. |
| L-03 | Packed declaration/assignment width | 4,096 bits in selected declaration/assignment/resize paths. |
| L-04 | Replication count | 4,096. |
| L-05 | One-dimensional sparse memory elements | 16,777,216. |
| L-06 | Multidimensional unpacked array elements | 4,096 total. |
| L-07 | Multidimensional unpacked array bits | 1,048,576 total. |
| L-08 | Non-sparse dynamic array/queue/associative capacity | Dynamic arrays, unbounded queues, and associative arrays are 0..=256; bounded queues are `max_bound + 1`; associative index width is 1..=64 bits; total packed width <=4,096. |
| L-09 | Function-local scalar width | 1..=64 bits in bounded function elaboration. |
| L-10 | Function/task loop unrolling | 256 iterations in direct inlining and bounded function elaboration. |
| L-11 | Function unpacked arrays | Each dimension <=256, total <=4,096 elements/bits guards. |
| L-12 | Module/interface instance-array dimensions | Bounded path: each <=256, total <=4,096. Direct slang path: total <=4,096. |
| L-13 | Bounded strings | 64 bytes / 512 bits, NUL padded. |
| L-14 | Class composition nesting | 8 levels in bounded class elaboration. |
| L-15 | Power operands | Direct path rejects operands wider than 64 bits. |
| L-16 | Property sequence repetition/delay constants | Mostly 0..=256 in bounded temporal subset. |
| L-17 | Direct runtime repeat loops | 65,536 iterations per activation; count width is limited to 64 bits. |
| L-18 | Direct runtime while loops | 65,536 iterations per loop statement. |
| L-19 | Direct one-dimensional class object arrays | 4,096 total handle elements; dynamic construction/property writes are sampled once per statement; dynamic handle assignment follows the selected source/target pair. |

## 8. Why runtime memory is still a bottleneck

The frontend and simulator are correct to reject unbounded resource use, but
the implementation remains memory-hungry even for accepted designs:

1. `LogicWord` stores a `Vec<LogicValue>` with roughly one byte per bit. A
   million-bit packed object can therefore require megabytes before clone and
   temporary overhead.
2. AST JSON and `serde_json::Value` duplicate source metadata and strings
   throughout lowering.
3. Lowering frequently clones symbols, members, expression trees, parameter
   maps, and generated-block environments instead of using arena/interned IDs.
4. Dynamic packed lvalue lowering expands bit-by-bit conditional mux trees.
5. Function/task inlining duplicates the body at every call and unrolls loops.
6. Compatibility checking builds many vectors/maps of fingerprints.
7. Hierarchy flattening retains source names, metadata, aliases, and connection
   tables without a compact arena/slab representation.
8. There is no incremental compilation cache or serialized elaboration cache.
9. There is no borrowing/cow strategy for untouched memory words or expressions.
10. The simulator has no partitioned compiled-code representation or lazy event
    evaluation comparable with commercial simulation engines.

Required fixes are data-layout and architecture work, not merely increasing the
guards.

## 9. Required remediation order

| Priority | Work package | Why it comes first |
| --- | --- | --- |
| P0-1 | Keep the direct path as the only public executable default, then remove or conformance-only isolate `compile_files_bounded`. | The hidden fallback is gone; the remaining explicit compatibility implementation must not regain product authority. |
| P0-2 | Replace remaining `"?"` structural fingerprints and add direct-versus-bounded simulation differentials. | Prevents semantic mismatches from passing validation. |
| P0-3 | Finish normal unpacked array variables/ports independently from large sparse memories. | Dense Ordinary storage, dedicated APIs, separate top-level `ArrayPortDecl` contracts, separate child `ArrayConnection` aliases, nested child differentials, fixed and dynamic-container whole-object copies, `$size`, fixed bound queries, dimension counts, bit counts, positional patterns, one-dimensional structured patterns, and one-dimensional integral dynamic-array, queue, and associative-array allocation/push/pop/insert/delete/key-value/traversal lifecycle now exist; remaining array methods, multidimensional structured patterns, broader storage shapes, and broader hierarchy shapes remain incomplete. |
| P0-4 | Add a real procedure/scheduling IR for delays, event lists, wait, fork, loops, jumps, and force/release. | Timing and behavioral RTL/UVM cannot be complete without it. |
| P0-5 | Add class/object and interface execution models, then official UVM phasing/objections/factory/TLM. | Full UVM remains blocked by OOP/interface semantics, not merely syntax. |
| P0-6 | Complete width/signedness/X-Z differential corpus, including 64-bit and unsigned-`i64` boundaries. | This is the highest-density source of silent semantic errors. |
| P0-7 | Add external LRM/SVA/UVM conformance and commercial-tool differential suites. | Internal tests currently prove intent, not completeness. |
| P0-8 | Compact IR/AST representations, interning, arenas, COW, incremental cache, and memory budgets. | Addresses the runtime-memory bottleneck rather than hiding it. |
| P1-9 | Structured diagnostics with spans/codes/severities/notes and multi-error recovery. | Required for usable debugging at commercial scale. |
| P1-10 | Complete project/config/library manager and protected-IP strategy. | Required for real design databases, not just isolated files. |

## 10. Additional confirmed bounded-path defects and blind spots

These defects were independently confirmed in the earlier repository audit and
remain open in whole or in part. They are in addition to the direct-AST matrix
above.

| ID | Area | Confirmed defect / blind spot | Current impact |
| --- | --- | --- | --- |
| O-01 | Bidirectional ports | Top-level scalar/vector `inout` now fails closed, but a true bidirectional connection/strength model is absent. | Valid RTL can be rejected; no general bidirectional signoff semantics. |
| O-02 | Constant width | Elaboration constants wider than 64 bits cannot be represented by the bounded `i64`/`to_u64` paths. | Valid wide parameters/ranges can be rejected or mislabeled as X/Z. |
| O-03 | Constant arithmetic | Bounded parameter arithmetic uses wrapping `i64` operations with incomplete type context. | Overflow and unsigned values above `i64::MAX` are high-risk. |
| O-04 | Function types | Full formal/return signedness and source types are not preserved through every bounded inlining boundary. | Signed calls, casts, and comparisons can lose context. |
| O-06 | Generate hierarchy names | The bounded path does not preserve every standard generate label/scope in hierarchical names. | Waveforms, hierarchical references, coverage, and physical naming can diverge. The direct path has scoped-path work, but this is not a complete bounded-path guarantee. |
| O-07 | Diagnostics | Many elaboration failures remain plain strings without file/column/span; parser mapping is incomplete. | Multi-file diagnostics and error suppression are not commercial-grade. |
| O-08 | String lexer | Backslash-newline, unknown escapes, and non-ASCII byte reconstruction are incomplete. | Valid or malformed string literals can be handled incorrectly. |
| O-09 | Imports | Wildcard-import collisions are silently overwritten in bounded package elaboration. | Ambiguous names can select the wrong constant/type/function. |
| O-10 | Negative bases | Negative unpacked-array bases can lose the optimized lowering path and fall back to per-element expansion. | Valid ranges such as `[-4:3]` have a hidden performance cliff. |
| O-11 | Dynamic arrays | Bounded dynamic storage is fixed at 256 elements while its size state is nine bits; allocation semantics are incomplete. | `new[size]` cannot provide correct dynamic-array behavior. |
| O-12 | Elaboration lint | No complete multiple-driver, combinational-loop, latch, incomplete-case, unused-name, or waiver analysis. | Commercial elaboration/lint prerequisites are missing. |
| O-13 | Port checking | Width/truncation/extension/signedness port mismatch diagnostics and implicit conversion contracts are incomplete. | Silent semantic divergence or missing commercial warnings. |
| O-14 | Assertion coverage | Overlapping attempts for selected repetition forms are merged in the bounded cover model. | Functional coverage can under-count distinct attempts. |
| O-15 | Standalone preprocessing | `eda-hdl` alone has no complete `include`, macro, conditional-compilation, or timescale directive implementation. | Correct preprocessing depends on the external slang adapter. |
| O-16 | Selected syntax | `let`, `alias`, `tran`, package `export`, and several declaration/operator forms remain unsupported. | Slang-valid sources can fail after validation. |
| O-17 | Randomization/constraints | No full `rand`/`randc`, constraint solver, distribution, pre/post randomize, or coverage integration. | Verification methodology remains far below commercial tools. |
| O-18 | slang lifecycle | No pinned runtime version check, timeout, AST schema contract, cache, or incremental compilation-unit lifecycle. | Adapter behavior can change with the installed slang binary. |

## 11. Success criteria

The frontend should not be called commercially complete until all of the
following are true:

1. One authoritative slang AST lowering path handles every accepted construct.
2. Every unsupported construct has a stable diagnostic code and source span.
3. No structural validator uses an unknown-shape `"?"` pass.
4. Every supported expression has an independent differential oracle.
5. Official UVM runs through phase completion and objection drain.
6. RV32I and i8086 match independent golden models with published coverage.
7. Resource limits are explicit, configurable, and accompanied by usage metrics.
8. Large accepted designs fit documented memory/time budgets.
9. External conformance/differential suites run continuously in CI.
10. Documentation never claims signoff/tapeout completeness without certification
    evidence.
