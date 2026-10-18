# HDL frontend error and gap audit

Date: 2026-10-18 (local time)
Repository state audited: `fix-digital` at `972712f` plus the uncommitted
multidimensional-interface-vector changes currently in the worktree.

This is an engineering audit, not a marketing maturity claim. “All errors”
below means **all currently known, code-visible, or probe-reproduced frontend
errors and gaps**. It cannot mean a mathematical proof that no other error
exists: the project has no exhaustive IEEE 1800 LRM conformance suite and no
commercial-tool differential oracle yet. Those missing oracles are themselves
listed as audit findings.

## 1. Executive conclusion

The frontend has two different strengths that must not be conflated:

1. **Language validation strength:** slang 12 validates the full source,
   expands macros/includes, checks types, elaborates parameters/generate, and
   emits authoritative AST JSON. This part is strong because it delegates the
   hard parser/type-checker work to slang.
2. **Executable lowering strength:** only a subset of slang's elaborated AST is
   directly lowered into repository IR. The older `compile_files` path still
   asks slang to preprocess the source and then reparses it with the bounded
   `eda-hdl` parser. That bounded parser has useful RTL/UVM-lite subsets, but it
   is not a complete SystemVerilog implementation.

Therefore, the statement “the frontend uses slang” is only completely true for
validation. It is not yet completely true for executable semantic lowering.
The major remaining frontend error is architectural: **there are still two
lowering authorities, and the direct slang-AST path is much smaller than the
validation path.**

The recent bounded-string ordering mistake was exactly the kind of error this
split makes possible: slang understood the source type, but repository-specific
lowering/tests supplied the semantic interpretation. The ASCII ordering error
was corrected (`"a" < "-"` is false), but the audit below lists the remaining
places where repository semantics are incomplete or under-validated.

## 2. Validation performed for this audit

All commands were run against the current worktree:

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | Pass |
| `cargo test -p eda-hdl --all-targets -- --nocapture` | 89 passed |
| `cargo test -p eda-slang --all-targets -- --nocapture` | 100 passed |
| `cargo test -p eda-core --all-targets -- --nocapture` | 28 passed |
| `cargo test -p eda-synth --all-targets -- --nocapture` | 43 passed |
| `SUITE_FILTER=sv_ cargo test -p eda-tests --test suite full_suite_all_green` | Pass in 130.99 s |

A temporary compatibility probe also compared the direct slang-AST path with
the bounded/preprocessed path on representative valid constructs. The probe
was removed after collecting results; permanent conformance tests are a
required remediation in P0-7.

These results prove that the currently named regressions pass. They do **not**
prove commercial SystemVerilog correctness or exhaustive LRM coverage.

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

## 4. Current P0 and P1 findings

Progress percentages describe executable frontend capability, not the roadmap’s
broader tool-flow percentage.

| ID | Name | Initial requirement | Current state / error | Severity | Progress | Next required evidence |
| --- | --- | --- | --- | --- | ---: | --- |
| F-01 | Single authoritative lowering path | Every valid input accepted by the authoritative frontend must either lower through the same semantic model or fail closed with an actionable diagnostic. | `compile_files` validates with slang, preprocesses, then reparses with the bounded parser. `compile_files_via_ast` is the better path but supports only a subset. | P0 architecture | 35% | Make direct slang-AST lowering the only executable path, or formally prove the compatibility fallback for every construct. |
| F-02 | Direct AST language coverage | Direct lowering must cover slang’s elaborated SystemVerilog surface construct-by-construct. | Member/statement/expression matches reject dynamic types, classes, interfaces, many timing/assertion/checker forms, and many system calls. | P0 | 28% | Maintain a machine-checked construct matrix with one positive test and one fail-closed unsupported test per construct. |
| F-03 | Complete structural/semantic post-check | Slang and repository IR must agree on declarations, types, statements, expressions, hierarchy, and behavior. | Current summary checks top, ports, declarations, assignment shapes, literals, simple width/signedness, processes, and instance inventory. Fingerprints fall back to `"?"` for legacy array expansion and dynamic packed lvalues. | P0 | 45% | Remove all `"?"` equivalence cases and add runtime differential simulation for each construct. |
| F-04 | Full official UVM execution | Compile and execute the official Accellera UVM library and user environments, including phasing, objections, factory, configuration, sequences, TLM, callbacks, reports, and coverage. | Official UVM 2020.3.1 is only checked/elaborated with `UVM_NO_DPI`. Direct AST lowering rejects `ClassType`. The bounded frontend has a separate UVM-lite subset. | P0 | 10% executable; 0% for full official UVM execution | Run an official UVM example through phasing and objections and compare event/report/coverage outcomes. |
| F-05 | Full data types | Support integral, real, string, event, chandle, enum, packed/unpacked aggregates, dynamic arrays, queues, associative arrays, classes, virtual interfaces, type parameters, and typed packages. | Direct path supports simple integral vectors, packed arrays, enums, packed structs/unions, bounded strings, sparse memories, scalar named events, and fixed-size event arrays with element triggers/waits/`.triggered`. It rejects real, ordinary unpacked aggregates, dynamic storage, queues, associative arrays, classes, arbitrary event expressions, chandle, and type parameters. | P0 | 31% | Add type-category tests with layout, assignment, comparison, and selected-member semantics. |
| F-06 | Process timing and concurrency | Model delays, event controls, wait, fork/join modes, process lifetime, ordering regions, clocking, and nondeterminism. | Direct procedural lowering supports initial/always subsets, implicit sensitivity, scalar any-edge signals, event lists containing scalar any-edge/posedge/negedge signals, constant procedural delays, delay-driven always generators, delay-bearing `forever` loops, `wait` statements, blocking/nonblocking named-event triggers with scalar, edge-qualified event-list, and fixed event-array element waits, time-slot `.triggered`, ordered `wait_order`, lexical sequential named-block `disable`, hierarchical clocking-block event controls with scalar input snapshots, no-skew output drives, and constant `##n` cycle delays, runtime `repeat`, `while`, and `do...while` loops with `break`/`continue`, module-scope and initialized automatic procedure-local `for` loops, scalar force/release, and concurrent `fork` branches with `join`, `join_any`, `join_none`, and nested joins. It still rejects process cancellation/`disable fork`, named-process termination, jump propagation across fork boundaries, `disable` interaction with fork/forever, clocking skews/full reactive regions, procedural `deassign`, min:typ:max delays, intra-assignment delays, and general event expressions. | P0 | 69% | Golden scheduling traces for active/NBA regions, delays, event races, fork join modes, and clocking blocks. |
| F-07 | Full temporal assertions and coverage | Support complete SVA sequences/properties, clocking, disable iff, recursion, local variables, matched/triggered, sampled-value functions, checkers, assumptions, and covergroups. | A bounded module-level temporal subset exists. Direct AST path rejects `ConcurrentAssertion`, checker instances, and covergroups; clocking blocks have only the scalar event/sampling/drive/cycle subset described in F-06. | P0 | 10% | Import an external SVA conformance suite and compare pass/fail/cover traces. |
| F-08 | System task/function surface | Support display/write/read, files, strings, random/stochastic, time, assertions control, coverage, cast, and simulation control semantics. | Direct path supports only `$readmemh`, `$readmemb`, `$writememh`, and `$writememb`. Bounded path has `$display`/finish and string formatting subsets. File IO, random, full formatting, time scale, coverage controls, VPI/PLI, and DPI-C are absent/incomplete. | P0 | 15% | System-task matrix covering formats, unknowns, file handles, errors, and argument checking. |
| F-09 | Interfaces, programs, checkers, and hierarchy | Support interfaces/modports, program blocks, checkers, generate hierarchy, arrays of instances, binds, configs, libraries, defparams, and primitives. | Direct path handles simple module instances, generated blocks, one-/multidimensional/generated instance arrays up to 4,096 total elements, and scalar/simple/multidimensional packed-vector interface instance signals with input/output modport connections and input-write fail-closed checks. It still rejects generic/aggregate interface ports, interface arrays, primitive instances, checker instances, specify blocks, bind, and wildcard imports. Bounded path supports selected interface/module-array subsets but not all forms. | P0 | 35% | Multi-file hierarchy matrix, including port coercion, modports, arrays, config resolution, and generated instance identity. |
| F-10 | Dynamic and unpacked storage semantics | Support dynamic allocation, queues, associative arrays, normal unpacked array variables/ports, array methods, assignment patterns, copies, bounds, and lifetime management. | Direct lowering treats unpacked declarations as sparse memories. Consequently ordinary small unpacked ports can be rejected unless they use the supported child-memory `inout` form. Dynamic arrays, queues, and associative arrays are rejected. | P0 | 25% | Runtime allocation/deletion/copy/size/bounds tests and normal unpacked input/output port tests. |
| F-11 | Complete width/signedness/type propagation | Implement IEEE 1800 self-determined and context-determined expression rules, literal sizing, mixed-sign rules, casts, port coercion, and X/Z propagation. | Many basic tests pass and summary checks simple width/sign. Propagation is not a complete LRM implementation, and bounded constant evaluation is historically `i64`-oriented. | P0 | 45% | Differential expression corpus against an independent evaluator, including 64-bit boundaries, unsigned values above `i64::MAX`, mixed signs, shifts, casts, and context widths. |
| F-12 | Net types, strength, and resolution | Preserve net kinds, implicit nets, strengths, tri-state resolution, multi-driver behavior, supply nets, and charge decay. | Basic `wire`/tri-state values work in tested cases. Direct lowering does not preserve a complete strength/net-resolution model; strength qualifiers are not a first-class IR property. | P1 correctness risk | 25% | Systematic multi-driver strength/tri-state truth tables and implicit-net diagnostics. |
| F-13 | Complete string model | Support dynamic SystemVerilog strings, Unicode/locale behavior where required by methods, mutation, substrings, conversions, formatting, and concatenation overflow. | Current model is a fixed 512-bit, 64-byte NUL-padded ASCII representation. Direct path supports literals, concatenation, length, byte reads/index writes, case conversion, comparison, `putc()`, `substr()`, and decimal/hex/binary radix conversions. Dynamic growth beyond 64 bytes, Unicode behavior, and synthesis remain incomplete/fail closed. | P1 | 55% of bounded string subset; 0% for unbounded strings | String method conformance tests and explicit overflow/unknown/NUL behavior tests. |
| F-14 | Complete function/task execution model | Support recursive/nonrecursive routines, all argument directions/default values, static/automatic lifetimes, void functions, virtual dispatch, out-of-line calls, loops, jumps, and runtime control flow. | Direct inlining supports many straight-line/control-flow functions/tasks, output/inout/ref arguments, locals, conditionals, and bounded loops. Recursion is rejected; loop unrolling is capped at 256; class/virtual calls and general procedure jumps are absent. | P0 | 35% | Separate compile-time inlining tests from a real runtime subroutine/process model. |
| F-15 | Package and namespace elaboration | Support packages, imports, exports, scoped references, nested packages, compilation-unit scopes, typedef visibility, and configuration. | Bounded path handles selected package constants/functions and wildcard imports. Direct member lowering rejects `WildcardImport`; package-scope classes/types are not fully executable. | P1 | 30% | Multi-package name-collision/shadowing/order tests. |
| F-16 | Parameterization and elaboration diagnostics | Support integer/string/type parameters, localparams, complex override graphs, default values, parameter ports, defparam, and cycle/dependency diagnostics. | Direct path handles simple elaborated constants and generated parameter values. Type parameters and defparam are unsupported; bounded constant evaluation and diagnostics remain incomplete. | P0 | 40% | Parameter dependency/cycle/override matrix and preserve-overrides metadata in IR. |
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
| D-01 | Class definitions/instances | Rejects `ClassType`; no direct OOP storage or dispatch. |
| D-02 | Full interface ports/instances/modports | Scalar, simple packed-vector, and multidimensional packed-vector interface members with input/output modport connections execute through proxy nets; input modport writes fail closed. Generic/aggregate interface ports, interface arrays, full modport checking, virtual interfaces, and import/export methods remain unsupported. |
| D-03 | Full clocking blocks | Clocking-block declarations, scalar clocking-event controls, scalar input snapshot reads, no-skew output drives, constant `##n` cycle delays, and child-clocking event/sample/drive remapping are accepted; skews, full prepona/reactive region ordering, and reactivity semantics remain unsupported. |
| D-04 | Specify blocks | Rejects `SpecifyBlock`; no module path delay/check semantics. |
| D-05 | Checker definitions/instances | Rejects checker instantiation. |
| D-06 | Primitive/UDP instances | Rejects `PrimitiveInstance`. |
| D-07 | Full instance-array surface | One-, multidimensional, and generate-scoped elaborated instance arrays lower and simulate up to 4,096 total elements. Interface-port arrays and other unsupported child/member forms still fail closed. |
| D-08 | Type parameters | Rejects `TypeParameter`. |
| D-09 | Package imports | Rejects `WildcardImport`; package elaboration is incomplete. |
| D-10 | Defparam | Unsupported and can fail through instance-connection lowering. |
| D-11 | Bind directives | No bind member/lowering model. |
| D-12 | Program blocks | No direct program-block lifetime/scheduling semantics. |
| D-13 | Final procedures | No `Final` procedure kind. |
| D-14 | Non-memory inout ports | Direct non-memory ports support only `In`/`Out`; top/child scalar inout is unsupported. Sparse child-memory inout has a special path. |
| D-15 | Ordinary unpacked array input/output ports | Unpacked declarations become sparse-memory declarations; only the supported child-memory `inout` form is accepted. |

### 5.2 Types and declarations

| ID | Gap | Current behavior |
| --- | --- | --- |
| D-16 | Real/shortreal/realtime | Type resolution rejects real types; no floating-point IR. |
| D-17 | Full event/chandle/void type semantics | Scalar named events and fixed event-array elements are executable; chandle, void, general event objects, and event ports are not represented. |
| D-18 | Dynamic arrays | Type string resolves as empty/unsupported; `new[]` rejected. |
| D-19 | Queues | Queue syntax unsupported. |
| D-20 | Associative arrays | Associative index types and storage unsupported. |
| D-21 | Unpacked structs/unions | Type alias target is rejected. |
| D-22 | Typedef aliases beyond current packed struct/union/enum forms | Unsupported target kinds. |
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
| D-30 | Dynamic array/queue/associative methods | Unsupported (`size`, `delete`, `exists`, `push_*`, `pop_*`, insertion, etc.). |
| D-31 | Class/virtual method calls | No OOP dispatch. |
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
| D-53 | Procedure-level `return` | Unsupported as a general statement (function inlining has selected return handling). |
| D-54 | Procedural `deassign` | Scalar static-lvalue `force` and `release` are supported; procedural variable `deassign` and aggregate/dynamic force targets remain unsupported. |
| D-55 | Qualified wildcard/inside checks | Normal `priority`/`unique`/`unique0` if/case checks execute, including runtime no-match and overlap diagnostics. Qualified wildcard and case-inside forms still fail closed. |
| D-56 | Full wildcard/inside case grammar | Direct statements support `casez`, `casex`, and simple `case ... inside` value/range arms. Remaining exotic range/pattern forms still fail closed. |
| D-58 | General event expressions in sensitivity | Scalar signal references, explicit edges in procedure sensitivity, edge-qualified event-list statement controls, and dynamic fixed-event-array element statement selectors are supported; arbitrary expressions and wildcard events remain unsupported in both procedure and statement sensitivity. |
| D-59 | Clocking-variable sampling/drive semantics | Scalar input snapshot reads and no-skew output drives are supported; skew semantics fail closed. |
| D-60 | Wait ordering/process arbitration | Unsupported. |

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
| D-68 | `$display`/`$write` family | Direct path rejects these calls; bounded path has a subset. |
| D-69 | `$fopen`/file read-write tasks | No direct file-handle model. |
| D-70 | `$random`/`$urandom`/`$urandom_range` | No direct stochastic model. |
| D-71 | Full `$time`/`$realtime`/timescale semantics | Incomplete. |
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
| L-08 | Non-sparse dynamic array size | 0..=256 and total packed width <=4,096. |
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
| P0-1 | Turn `compile_files_via_ast` into the only default executable path and remove unsupported compatibility fallbacks. | Eliminates the two-authority correctness hazard. |
| P0-2 | Replace remaining `"?"` structural fingerprints and add direct-versus-bounded simulation differentials. | Prevents semantic mismatches from passing validation. |
| P0-3 | Add normal unpacked array variables/ports independently from large sparse memories. | Small arrays and large memories currently have conflated lowering models. |
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
