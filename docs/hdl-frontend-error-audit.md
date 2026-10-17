# HDL frontend error and coverage audit

Date: 2026-10-17

Repository state: branch `fix-digital` after the sparse-memory and slang direct-AST follow-up work.
Scope: `crates/eda-hdl` and the `crates/eda-slang` integration boundary. Simulator behavior is discussed only where the frontend lowering causes the limitation.

## Executive summary

The serious frontend failures are structural, not isolated typos:

1. The internal frontend is a hand-written, bounded Verilog/SystemVerilog subset. It is not an IEEE 1800 compiler and was never a replacement for a commercial frontend by itself.
2. The old runtime array lowering expanded dynamic array element selects into per-element condition trees. The current IR now has first-class `MemoryDecl` objects plus sparse whole-element and indexed part-select read/write operations. Large one-dimensional module arrays use sparse storage and retain only a one-bit flat placeholder; small arrays, multidimensional arrays, and input/output aggregate ports retain their legacy bounded representation, while a direct large `inout` memory port can share a parent sparse-memory object.
3. The initial audit found malformed elaborations silently accepted, including unknown or duplicate named port/parameter connections and excess positional connections. The current worktree now rejects those cases; see the status column below.
4. A representative slang-validated probe set contained 42 valid SystemVerilog constructs. The internal frontend accepted only 2 of them; 40 were rejected. This is expected for a bounded subset, but it quantifies how far the internal parser is from full-language coverage.
5. The `eda-slang` adapter can validate full SystemVerilog and export slang AST JSON. Normal compilation still feeds slang-preprocessed text into the bounded parser, but a new `SlangAstSummary` now records slang source spans and per-instance post-elaboration parameter values and checks the lowered top name, ordered ports/directions, simple declaration widths, top parameter metadata, continuous/procedural assignment targets, recursive RHS operand shapes/order/symbol identity, sized integer literal values, simple width/signedness, and direct/generated instances against slang after compilation. `compile_files_via_ast` now directly lowers a deliberately small module subset (including parameter-constant leaves, procedural assignments, if/case control flow, single-edge sensitivity, recursive hierarchy, generated members, and multidimensional sparse-memory whole/part-select operations and direct inout child-memory sharing) from slang's elaborated AST, simulates it, and rejects unsupported constructs explicitly. Full slang-AST-to-repository-IR lowering remains absent.
6. Concurrent `compile_source` calls previously generated temporary filenames from only process ID plus nanosecond time and could collide during parallel tests. The adapter now adds a process-local atomic counter; the new default-parallel frontend suite exercises the fix.
7. The bounded parser now preserves and lowers `mem[i][msb:lsb]`; both direct slang-AST and preprocessed bounded paths pass a behavioral differential test.
8. The first AST-summary implementation recursively collected every named symbol under an assignment LHS, so `mem[addr]` incorrectly reported both `mem` and `addr` as assignment targets. Target extraction now follows only the selected base value. Legacy small-array procedural assignments still use bounded per-element expansion; their slang comparison preserves assignment-count checking without pretending that the expanded expression tree has source-level shape equivalence.

Current automated status at the time of this audit:

| Gate | Result |
|---|---|
| `cargo test -p eda-hdl --all-targets -- --nocapture` | 87 passed after array-element range-select remediation |
| `cargo test -p eda-core --all-targets -- --nocapture` | 23 passed, including multidimensional and reversed-element sparse-memory index/bounds tests |
| `cargo test -p eda-sim --all-targets -- --nocapture` | 83 passed, including consecutive sparse whole/part-write tests |
| `cargo test -p eda-sim frontend_regressions -- --nocapture` | 14 passed |
| `SLANG_BIN=/tmp/slang-build/bin/slang UVM_HOME=/tmp/uvm-core cargo test -p eda-slang --all-targets -- --nocapture` | 17 passed |
| `SUITE_FILTER=sv_ cargo test -p eda-tests --test suite full_suite_all_green -- --nocapture` | Passed in 131.54 s after the direct-AST follow-up |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` and `git diff --check` | Passed |

Passing tests do not establish commercial frontend completeness. The tests are intentionally focused on the implemented subset.

## Why the runtime-memory failure happened

The original elaborator represented every unpacked array as one packed flat net. That design turned a 4 KiB memory into 32,768 storage bits even when only two elements were initialized.

The current architecture adds:

- `MemoryDecl` with element width, logical dimensions, stride/direction, and flat length;
- sparse simulator element storage (`HashMap<physical_index, LogicWord>`);
- `Expr::MemoryRead` and `Expr::MemoryReadPart`;
- blocking `Stmt::MemoryBl` and `Stmt::MemoryBlPart`;
- non-blocking `Stmt::MemoryNBl` and `Stmt::MemoryNBlPart`;
- memory-change notification to continuous readers and combinational processes;
- logical-index bounds checks with X reads / no-op writes outside the range.

A large one-dimensional module array whose flattened width exceeds 4096 bits now lowers whole-element and indexed part-select reads/writes to these operations and leaves only a one-bit storage declaration placeholder in the flat net space. The checked-in 4 KiB regression now reports 30 flat bits rather than 32,797. A new 1 MiB (`1,048,576 x 8`) memory regression reports 38 flat bits and stores only initialized/touched elements.

Remaining memory limitations:

- indexed part-select reads and writes work on large sparse memories, including runtime starts, reversed packed elements, continuous lvalues, and disjoint NBA-field merging;
- sparse-memory operations are not synthesizable to RAM macros yet;
- direct large `inout` memory ports share a parent sparse-memory object, but input/output aggregate-port contracts, typed byte enables/write-mask metadata, banks, initialization files, and memory macros are not yet first-class;
- multidimensional and small compatibility arrays still use flat representation;
- VCD memory visualization is not yet implemented;
- the sparse element cap is currently 16,777,216 elements, not unbounded host memory.

This closes the original dense-storage blocker for whole-element and indexed part-select one-dimensional RTL memories, but not the full commercial memory-modeling surface.

## A. Confirmed correctness and diagnostic defects

| ID | Severity | Area | Defect | Evidence / consequence | Required fix | Status |
|---:|---|---|---|---|---|---|
| HF-C01 | P0 | Lexer | Unterminated block comments are silently accepted. | `Lexer::skip_trivia` returns when EOF is reached without closing `/*`. slang rejects the same source; `eda info` accepts it after a complete module. | Return an unterminated-comment diagnostic with source span. | Fixed in current worktree |
| HF-C02 | P0 | Top selection | Automatic top selection ignores instances nested in generate constructs. | A design with one true top and children instantiated only in `generate if` is reported as ambiguous because `pick_top` scans only direct `Item::Inst` nodes before generate expansion. | Resolve parameters and flatten generate constructs before choosing top, or construct the instantiation graph recursively. | Fixed in current worktree |
| HF-C03 | P0 | Instance ports | Unknown named port connections are silently ignored. | `child u(.does_not_exist(x))` is accepted; slang reports that the port does not exist. | Validate every named connection against the child port table. | Fixed in current worktree |
| HF-C04 | P0 | Instance ports | Duplicate named port connections are accepted. | Repeated `.a(...)` connections are accepted; `find_conn` returns the first match and never diagnoses the duplicate. | Reject duplicate connections and report both source locations. | Fixed in current worktree |
| HF-C05 | P0 | Instance ports | Excess positional port connections are silently ignored. | A two-port child instantiated with three positional expressions is accepted; slang reports too many connections. | Enforce positional connection count and report missing/unconnected ports according to language rules. | Fixed in current worktree |
| HF-C06 | P1 | Instance ports | Mixed named/positional connection matching is not robust. | `find_conn` tracks positional order only as tokens are encountered and does not model the child declaration index when named and positional connections are mixed. | Build and validate a complete connection map before lowering. | Fixed in current worktree |
| HF-C07 | P0 | Parameter overrides | Unknown named parameter overrides are silently ignored. | `#(.DOES_NOT_EXIST(3))` is accepted; slang reports an undefined parameter override. | Validate names against declared child parameters. | Fixed in current worktree |
| HF-C08 | P1 | Parameter overrides | Duplicate parameter overrides silently use the last value. | Overrides are inserted into a map without duplicate detection. | Reject duplicates and preserve declaration/override order diagnostics. | Fixed in current worktree |
| HF-C09 | P1 | Parameter overrides | Excess positional parameter overrides are silently ignored. | More positional actuals than declared parameters are accepted. | Enforce declared parameter count and diagnostics. | Fixed in current worktree |
| HF-C10 | P0 | Port directions | Top-level `inout` is coerced to `Dir::In`. | Only output is distinguished when top ports are emitted; child inout is separately rejected. The resulting design direction metadata is wrong. | Add a true bidirectional connection/strength model or reject inout consistently until supported. | Explicit rejection fixed; full inout remains open |
| HF-C11 | P0 | Constants | Known constants wider than 64 bits cannot be elaborated and are mislabeled as X/Z. | `const_eval` first calls `LogicWord::to_u64`; that function returns `None` for every width above 64, so the error claims `constant contains x/z`. | Use an arbitrary-sized constant representation and context-correct width/signedness semantics. | Open |
| HF-C12 | P1 | Constants | Constant arithmetic is `i64` with wrapping operations. | Parameter values have no arbitrary-precision representation and little type context. Overflow is not reported. | Use checked, typed, arbitrary-sized elaboration constants. | Open |
| HF-C13 | P1 | Functions | Function/class-method return and formal signedness is discarded. | `FunctionDef` stores only ranges for returns/arguments; parser-detected signedness is not carried into the IR boundary. Formal casts and signed comparison contexts are therefore incomplete. | Preserve full source types through AST, inlining, and IR. | Open |
| HF-C14 | P1 | Sensitivity | Mixed edge and level event expressions collapse to combinational sensitivity. | Any plain identifier in `@(a or posedge clk)` causes the entire list to become `Sens::Comb`; the edge list is discarded. | Represent all event kinds exactly and implement legal mixed-event scheduling semantics. | Open |
| HF-C15 | P1 | Generate hierarchy | Generate block labels and generated-object hierarchy names are discarded. | `GenerateBlock::label` is not used as a hierarchical scope; an instance in `g` appears as `top.u`, not `top.g.u`. This breaks hierarchical references, waves, coverage, and physical naming. | Preserve generate scopes and standard generated-block naming, including unnamed blocks. | Open |
| HF-C16 | P0 | Diagnostics | Elaboration diagnostics have no file, column, or original source span. | `SourceManager` remaps only parser errors. `compile_sources` elaboration failures still have no file mapping; parse remapping also retains the global concatenated line inside the message. | Attach spans to every AST/IR node and emit file:line:column diagnostics with notes. | Open |
| HF-C17 | P1 | Strings | String lexical semantics are incomplete. | Backslash-newline is stored as a newline rather than continuing the literal; unknown escapes are accepted unchanged; non-ASCII bytes are reconstructed through `u8 as char`, corrupting UTF-8 contents. | Implement SystemVerilog string lexical rules on Unicode scalar/byte boundaries. | Open |
| HF-C18 | P1 | Imports | Collisions between wildcard imports are silently overwritten. | `apply_import` iterates package tables and inserts into local maps without conflict reporting. | Detect ambiguous imported constants/types/functions and local/import collisions. | Open |
| HF-C19 | P1 | Arrays | Negative array bases silently lose the optimized lowering path. | `runtime_array_physical_index` returns `None` if any dimension base is negative, after which exact-width access falls back to a per-element tree. This is a hidden performance cliff for valid indices such as `[-4:3]`. | Implement signed normalized physical indices or represent logical ranges directly. | Open |
| HF-C20 | P0 | Dynamic arrays | Dynamic array storage is fixed at 256 elements while the size register is nine bits. | `new[size]` truncates size to nine bits, zeroes the fixed storage, and records values up to 511; access does not implement true allocation or complete `size()` semantics. | Reject >256 sizes immediately or, preferably, implement real dynamic storage. | Open |
| HF-C21 | P1 | Lint/elaboration | The frontend does not diagnose multiple drivers, combinational loops, accidental latches, or incomplete `always_comb` behavior. | No such analysis exists at the frontend boundary. These are commercial-elaboration/lint prerequisites. | Add a driver/algebraic-cycle/latch analysis stage with waivers and source spans. | Open |
| HF-C22 | P1 | Port checking | Width/truncation/extension and signedness mismatch diagnostics are incomplete. | Connections are lowered without a full port type/width contract. | Implement implicit port conversion rules and issue required warnings/errors. | Open |
| HF-C23 | P1 | Assertions | Concurrent assertion coverage semantics merge overlapping attempts for some repetition forms. | The current implementation documents that goto attempts sharing a count bucket are merged; assertion/assumption behavior is preserved but distinct cover attempts are not. | Track individual attempts or define and validate a conservative cover model. | Open |
| HF-C24 | P1 | Arrays/parser | The internal parser rejected a simple range select of an unpacked-array element, such as `mem[i][msb:lsb]`. | slang accepted the syntax while the bounded parser reported `expected ']', found ':'`; there was no array-element range AST node. | Add an array-element range-select AST node and lower it to indexed `MemoryReadPart`/`MemoryBlPart` semantics. | Fixed in current worktree |

## B. Language and semantic coverage gaps

| ID | Severity | Area | Missing or incomplete capability |
|---:|---|---|---|
| HF-G01 | P0 | Preprocessor | No internal `include`, macro definition/expansion, conditional compilation, `` `define``/`` `undefine``, `` `ifdef`` family, or command-line define/include environment. The internal lexer rejects the backtick character. |
| HF-G02 | P0 | Source model | No full source manager: no columns, ranges, include expansion maps, macro expansion maps, compilation-unit identity, or notes/carets. |
| HF-G03 | P0 | Compilation model | No library/config resolution, `` `config``, `design`, `instance`, `liblist`, separate compilation-unit semantics, or source-order/package dependency management beyond a concatenated unit. |
| HF-G04 | P0 | Types | No native `real`, `shortreal`, `realtime`, `time`, `chandle`, `event`, or complete `void` type system. |
| HF-G05 | P0 | Aggregates | Only packed structs/unions. No unpacked structs, tagged unions, unpacked union layout, arbitrary aggregate assignment, or full assignment patterns. |
| HF-G06 | P0 | Arrays | No complete dynamic arrays, queues, associative arrays, array query/manipulation methods, `inside`, `with`, sorting, reduction methods, or streaming concatenation. |
| HF-G07 | P0 | Strings | Fixed 64-byte packed representation. Longer strings and dynamic string semantics are unsupported. |
| HF-G08 | P0 | Parameters | No type parameters, parameter ranges/kinds, string/type parameter values, arbitrary-width constants, or complete localparam type syntax. Multiple `parameter` declaration groups in one module header are not accepted. |
| HF-G09 | P0 | Expressions | No full context-determined/self-determined expression sizing and signedness algorithm. The AST has no general type layer. |
| HF-G10 | P1 | Operators | Arithmetic left/right shifts, compound assignments, wildcard equality `==?`/`!=?`, set membership, `dist`, `matches`, and streaming operators are absent or partial. |
| HF-G11 | P0 | Procedures | No top-level tasks, `return`, `break`, `continue`, `do...while`, `foreach`, `wait`, `wait_order`, `disable`, procedural labels, or complete event controls. |
| HF-G12 | P0 | Concurrency | No `fork`/`join`, `join_any`, `join_none`, process control, interprocess communication, event triggers, semaphores, or mailboxes as general language features. |
| HF-G13 | P0 | Timing | Delays are integer nanoseconds up to 1,000,000. No `timescale`, time units/precision, real delays, parameterized delays, intra-assignment delays, net delays, or full scheduling regions. |
| HF-G14 | P0 | Procedural force | No `force`, `release`, `assign`/`deassign` procedural statements. |
| HF-G15 | P0 | Hierarchy | No general hierarchical references/upwards references, cross-module references, `bind`, `defparam`, or preserve-generate hierarchy. |
| HF-G16 | P0 | Module connectivity | No complete bidirectional/inout resolution, net strengths, tri-state resolution, user-defined nettypes, `interconnect`, or port connection type checking. |
| HF-G17 | P1 | Gates/primitives | Basic AND/OR/XOR/BUF/NOT gates only. No gate delays/strengths, `bufif`/`notif`, MOS/CMOS switches, UDPs, or specify-path timing. |
| HF-G18 | P1 | Specify | No `specify`, `specparam`, module path delays, pulse controls, timing checks, or SDF annotation at the frontend language level. |
| HF-G19 | P0 | Functions | Functions are symbolically inlined. No runtime stack, general recursion, automatic/static lifetimes, side-effect ordering, task timing, ref arguments, or complete modern function signatures. |
| HF-G20 | P1 | Interfaces | Static interface/modport subset only. No procedural rebinding, null virtual interfaces, virtual-interface arrays, interface classes, `implements`, or full modport procedure exports. |
| HF-G21 | P0 | OOP | Class support is a bounded subset. No complete constructor/super rules, static/protected/local enforcement, package/class visibility, interfaces, nested classes, covergroups in classes, or general object lifetime. |
| HF-G22 | P0 | Randomization | No constraint solver, `randomize`/`srandom`/`pre/post_randomize`, soft constraints, solve-before, distribution constraints, inheritance-aware randomization, or reproducible random state. |
| HF-G23 | P0 | UVM | Built-in UVM-lite classes are not official UVM. Official Accellera UVM is elaborated by slang only; it is not lowered or executed. Factory/config/TLM/sequences/phases/callbacks/objections/reporting are all partial. |
| HF-G24 | P0 | Assertions | SVA subset is bounded and mostly two-sequence. No complete sequences, local variables, recursion, unbounded repetition, first_match/throughout/within/intersect, multiclocking, clocking blocks, or full pass/fail actions. |
| HF-G25 | P0 | Coverage | No SystemVerilog `covergroup`, coverpoints, crosses, bins, illegal/ignore bins, per-instance coverage, or coverage sampling semantics in the HDL frontend. |
| HF-G26 | P0 | Elaboration lint | No `unique`/`priority` enforcement, full case-overlap analysis, full always kind checking, unused/conflicting name diagnostics, or elaboration-policy waivers. |
| HF-G27 | P1 | System tasks/functions | Only a selected display/format/control subset. No complete file I/O, math/random conversion, assertion control, PLI/VPI/DPI tasks, or vendor-task policy. |
| HF-G28 | P0 | Memory architecture | Foundational sparse `MemoryDecl`, whole-element, and indexed part-select IR plus simulator storage now exist for large one-dimensional module memories. Memory macros, typed byte enables, banks, hierarchical memories, initialization files, sparse-memory synthesis, general input/output aggregate ports, and VCD visualization remain. |
| HF-G29 | P1 | Literal limits | Packed literals/types are capped at 4096 bits; string literals at 64 bytes. Valid larger constructs are rejected rather than represented. |
| HF-G30 | P1 | Elaboration bounds | Generate loops stop at 131072 steps, function loops at 256 iterations, class composition at eight levels, hierarchy at 64 levels. These are safety caps, not language semantics. |
| HF-G31 | P0 | slang integration | Full slang elaborated AST to repository IR lowering remains absent. The normal `compile_files` path validates with slang, serializes an include/macro-aware structural summary, preprocesses to one text unit, invokes the bounded parser, and compares the result with slang. A new explicit `compile_files_via_ast` path directly lowers a small ports/nets/continuous-expression/parameter-constant/procedural/recursive-hierarchy/generated-member/sparse-memory/shared-memory-port subset without preprocessed reparsing; unsupported constructs such as mixed edge/level sensitivity fail closed. |
| HF-G32 | P1 | slang diagnostics | slang diagnostics are passthrough strings. No structured severities, spans, artifact capture, filtering, or mapping from preprocessed output back to original source. |
| HF-G33 | P1 | slang lifecycle | No runtime-pinned version check, timeout, cache, incremental compilation unit, or AST schema contract. Any compatible-looking `SLANG_BIN` path is accepted. |

## C. Representative full-language probe results

The following probe set was first accepted by local slang 12.0.0; the unchanged sources were then passed to the internal `eda info` path. Forty of the forty-two slang-valid probes were rejected. This is not an exhaustive IEEE 1800 matrix, but it shows that current passing tests are subset tests.

| Probe | Internal result |
|---|---|
| ANSI top with `inout` | Now explicitly rejected until bidirectional semantics are implemented (HF-C10). |
| Concatenated/sliced named port actual | Accepted. |
| Parameter type parameter | Rejected: `expected identifier, found Ident("type")`. |
| Typed `localparam logic` | Rejected before elaboration. |
| Enum `.next()` | Rejected as an unsupported dynamic-array method. |
| Unpacked struct | Rejected: only packed structs/unions supported. |
| Tagged union | Rejected. |
| Dynamic array | Rejected at `[]`. |
| Queue | Rejected at `[$]`. |
| Associative array | Rejected. |
| 70-byte string | Rejected by 64-byte string limit. |
| `real` | Rejected. |
| `time` | Rejected. |
| `automatic` function with modern argument list | Rejected: classic `input` argument syntax expected. |
| `let` declaration | Rejected. |
| Task | Rejected. |
| Function with `return` | Rejected. |
| `foreach` | Rejected. |
| `break`/`continue` | Rejected. |
| `do...while` | Rejected. |
| `wait` | Rejected. |
| `fork`/`join` | Rejected. |
| `` `timescale`` directive | Rejected at the backtick. |
| `force`/`release` | Rejected. |
| `alias` | Rejected. |
| `tran` switch | Rejected. |
| `specify` path | Rejected. |
| Hierarchical reference | Rejected: undeclared identifier. |
| `always_latch` | Rejected. |
| `priority case` | Rejected. |
| `case ... inside` | Rejected. |
| Assignment pattern | Rejected. |
| Streaming concatenation | Rejected. |
| Array assignment pattern | Rejected. |
| `with` clause | Rejected. |
| Randomization constraint | Rejected. |
| `covergroup` | Rejected. |
| Multiclock assertion | Rejected. |
| Package `export` | Rejected. |
| `program` block | Rejected. |
| `checker` block | Rejected. |
| `config` declaration | Rejected. |

## D. Highest-priority corrective plan

### P0 correctness stopgaps

1. Add malformed-instance matrices for unknown, duplicate, missing, mixed, and excess port/parameter connections; reject all of them.
2. Fix unterminated block comments and string lexical handling.
3. Choose automatic top after generate elaboration or through a recursive instantiation graph.
4. Preserve top `inout` direction or reject it until bidirectional semantics exist.
5. Replace `i64` elaboration constants with typed arbitrary-sized constants.
6. Attach source spans to parser and elaboration errors; remap all `compile_sources` failures.
7. Preserve function formal/return signedness and full types.
8. Add a negative-index and slice-scaling regression so the optimized path cannot silently degrade.

### P0 architecture

1. Extend the new sparse-memory IR with initialization policy, typed byte enables, bank/interface metadata, general port contracts, memory macros, synthesis mapping, and waveform visibility.
2. Stop lowering general dynamic array accesses through per-element expansion. Keep expansion only for tiny explicitly requested constants.
3. Expand the direct slang elaborated-AST lowering path until preprocessing and reparsing are no longer needed for representable constructs.
4. Introduce a typed frontend IR before flattening so signedness, widths, lifetimes, aggregate shape, and source identity survive elaboration.
5. Make frontend diagnostics structured (`file`, `line`, `column`, `end`, `severity`, `code`, `notes`) rather than strings.

### Validation requirements

- Add a slang-conformance corpus that is valid under slang and deliberately unsupported in the internal frontend; expected results must be explicit, not accidental parse errors.
- Add malformed-source diagnostics for every accepted-by-mistake case in section A.
- Add array scaling tests at 1, 256, 4096, >4096, negative bases, multidimensional forms, additional partial-width forms, and out-of-range X indices.
- Add expression width/signedness differential matrices, including >64-bit constants.
- Keep official UVM elaboration separate from any claim that UVM runs.
- Add source-location golden tests for parser and elaborator errors.
- Add compile-time and runtime memory budgets to CI so a flat-array regression cannot merge unnoticed.

## Acceptance criteria for calling the frontend signoff-grade

- Full slang or equivalent standard frontend is the parser/type/elaboration authority.
- No valid input is silently transformed into a different hierarchy, direction, type, width, or driver structure.
- Every unsupported construct has a precise, source-located diagnostic.
- Whole-element one-dimensional sparse memories do not require dense allocation; extend the direct `inout` shared-memory-port guarantee to general input/output port contracts, typed byte enables, banks, and aggregates.
- Parser, type checker, elaborator, linter, and simulator use one typed source-span-carrying representation.
- Official UVM execution, if claimed, is validated against a documented behavioral reference, not merely slang elaboration.
