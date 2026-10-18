# Rust EDA Toolchain

This workspace is an API-first, Rust-native EDA toolchain intended for agent
driven digital, analog, and physical-design flows. It is deliberately modular:
each domain is independently consumable and the `eda-api` facade provides the
one-call surface preferred by automation.

## Run and test

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## CPU program platform

`crates/eda-cpu-platform` contains an executable-centric ISA validation
platform. It loads ELF32 RV32I images plus DOS `.COM` and MZ `.EXE` i8086
images, executes them through independent golden models, captures output and
exit status, and exposes directed validation cases. `platform/rtl` contains
the full multicycle RV32I core, a broad multicycle i8086 core, directed
simulation SOCs, and the i8086 prefix/opcode/ModRM decoder contracts. The
`--sv` runners attach the external RTL memory buses to sparse host memory, so
checked-in RV32I ELF and real-mode COM images can execute beyond the small
directed SOC memories. These cores are compiled and simulated, and their
release gates include RTL-to-mapped-netlist LEC. See `platform/README.md` for
current scope and the remaining exhaustive-opcode acceptance gates.
The release matrices now correlate both golden models against RTL for the
RV32I base integer/data/control/CSR/trap classes and the broad i8086 ALU,
ModRM, segment, shift/rotate, and unary groups. RV32I additionally runs 64
deterministic randomized differential seeds, while i8086 runs 32 seeds mixing
ALU, shift/rotate, unary, prefix, and sparse-memory load/store streams.

```sh
cargo run -p eda-cpu-platform -- validate
cargo run -p eda-cpu-platform -- i8086 run platform/programs/hello.com
cargo run -p eda-cpu-platform -- i8086 run platform/programs/hello.com --sv
cargo run -p eda-cpu-platform -- rv32i run platform/programs/rv32i_exit42.elf
cargo run -p eda-cpu-platform -- rv32i run platform/programs/rv32i_exit42.elf --sv
cargo run -p eda-cpu-platform -- i8086 run platform/programs/high_memory.com --sv
cargo run -p eda-cpu-platform -- rv32i run platform/programs/rv32i_high_memory.elf --sv
```

The optional `eda-slang` front-end adapter delegates full SystemVerilog
validation, preprocessing, and AST serialization to
[slang](https://github.com/MikePopoloski/slang). It accepts include paths and
macro definitions, rejects malformed RTL with slang diagnostics, emits
post-elaboration AST JSON, and can feed slang's single-unit preprocessed output
to the bounded repository elaborator. After bounded lowering, it records slang
source spans and authoritative per-instance post-elaboration parameter values,
then compares the result's top name, ordered port inventory/directions, simple
declaration widths, top parameter metadata, continuous/procedural assignment
targets, recursive RHS operand shapes/order/symbol identity, sized
integer-literal values, simple type width/signedness, and direct/generated
instance inventory against the slang AST. `compile_files_via_ast`
additionally lowers a simple module subset with elaborated parameter-constant
leaves, initial/always procedures, blocking/non-blocking assignments, blocks,
if/case control flow, bounded string literals, concatenation, byte reads and
writes (`getc`, indexed byte assignment, and `putc`), length/case conversion,
comparison, inclusive `substr` extraction, and radix conversions
(`atoi`/`atohex`/`atobin`/`itoa`/`hextoa`/`bintoa`), scalar event lists with
any-edge, posedge, and negedge signals, constant procedural delay statements,
delay-driven always generators, delay-bearing `forever` loops, `wait`
statements, constant-count `repeat` loops, and concurrent `fork` branches with
`join`, `join_any`, and `join_none`, recursive
parameterized module hierarchy with port connections, generated members, and
multidimensional sparse-memory whole/part-select operations, inline initializers, compile-time `$readmemh`/`$readmemb` loads with constant bounds and runtime `$writememh`/`$writememb` dumps, and direct inout child-memory sharing directly from slang's elaborated
AST—without preprocessing and reparsing—and reports
unsupported constructs explicitly. Its conformance tests also validate
the repository RV32I/i8086 RTL units and the official Accellera UVM 2020.3.1
reference source with `UVM_NO_DPI`. This establishes a full-language validation
path and the first direct-AST lowering path, while downstream execution remains
explicit about constructs the bounded IR can represent.
The bounded internal frontend also provides `HdlSource`, `SourceManager`, and
`compile_sources` for deterministic named multi-unit compilation with parser
errors mapped back to their originating file and line.
Module unpacked arrays now support whole-element reads and writes, including
fixed multidimensional selections. Large memories lower to first-class sparse
`MemoryDecl` objects with explicit read, blocking-write, and NBA-write
operations; a natural 4 KiB byte array uses 30 flat bits rather than 32,797,
and the sparse-memory regression extends to 1 MiB-by-8. Sparse memories
support indexed part-select reads and writes with runtime starts, reversed
elements, continuous lvalue preservation, disjoint NBA-field merging, and
large `inout` memory-port sharing through the hierarchy. Post-elaboration
inference marks runtime-written canonical alias groups as RAM and
initializer-only memories as ROM, then assigns technology-independent timing
templates (`AsyncRom`, `SyncRom`, `MixedRom`, `AsyncRam`, `SyncRam`, or
`MixedRam`) plus conservative runtime write-mask, aligned byte-lane, and logical-bank
contracts. Synthesis now emits technology-independent distributed/block macro candidate plans from these contracts; vendor mapping and block-macro netlist lowering remain future work; small deterministic DistributedRom objects and clocked DistributedRam objects now lower to gates/registers, including aligned byte-enable part writes. Sparse-memory VCD output dynamically declares only touched words rather than expanding a 1 MiB memory. Direct slang-AST
lowering supports inline multidimensional sparse-memory initializers and
compile-time `$readmemh`/`$readmemb` loads with constant bounds and runtime `$writememh`/`$writememb` dumps. The bounded path expands
two-to-four-argument memory-file loads/dumps with constant bounds relative to the first source file and accepts
combined net/data declarations such as `wire logic`, allowing the preprocessed
and direct slang paths to share sparse-memory differentials.

The data-driven regression target currently contains **15,159 unique named
cases**. It is not a collection of three or four broad assertions: every case
has a distinct name and independently reports success or failure. Of those,
`eda_tests::ams_cases` contributes exactly 3,110 analog/mixed-signal
circuit cases spanning:

The HDL frontend now executes a SystemVerilog class subset that underpins
agent-driven verification: `class`/`virtual class` declarations with
`extends` inheritance, `this`/`super` member and method dispatch (including
`super.new`), constructors via `h = new(args)` with trailing default
arguments, explicit `null` handle assignment/comparison with alias-group
detachment, object-identity `==`/`!=` across aliased handles, and declaration
initializers, `$cast` between handles on statically known inheritance chains,
shallow-copy construction via `dst = new source`, void `function`/`task`
methods, value-returning methods in
statement and expression positions (direct assignment calls also execute
method side effects and virtual dispatch), shared `static` properties and
`static function` methods addressed through the class name, handle copy semantics, property
qualifiers (`rand`, `randc`, `local`, `protected`, `const`), classes with
parameter lists (`class C #(parameter W = 8)` with explicit
`C #(.W(4))` / `C #(4)` integer specializations and default-value
substitution across widths, defaults, and method bodies, plus parameterized
base selection and forwarded actuals in `extends`), package-scope
classes with `pkg::Type` visibility, and a built-in UVM-lite `uvm_object`
base class (`new`/`get_id`) that derived transaction classes can extend
without any external library. A `string` subset (512-bit NUL-padded
byte packing, up to 64 characters) supports declaration, assignment,
literal comparison, `len()`/`getc()`/`putc()`/`substr()`/`atoi()`/
`atohex()`/`atobin()`/`itoa()`/`hextoa()`/`bintoa()`/`compare()`/
`icompare()`/`toupper()`/`tolower()` methods, class properties, and constructor arguments, plus
`$sformatf` with fully constant
arguments (compile-time `%0d/%h/%b/%s/%%` formatting into the packed
string subset) a `uvm_objection`
counter (`raise`/`drop` with call counts and a `drained`/`check_drained`
termination check), a depth-four
`uvm_tlm_fifo` mailbox (`try_put`/`try_get`/`peek` with FIFO ordering,
full rejection, and empty reads; sharable by sequencer and driver
handles), a sequential UVM-lite `uvm_event`
(`trigger`/`wait_trigger`/`reset`/`is_on` with trigger and consumption
counts), a `uvm_barrier` threshold counter, a bounded depth-four
`uvm_queue` int deque (`push_front`/`push_back`/`pop_front`/`pop_back`/
`size`/`delete`, plus pure `peek_front` and statement-side-effect
`shift_front`), a bounded depth-four `uvm_pool` int-to-int map
(`add`/`find`/`exists`/`remove`/`num`/`delete` with unique keys), and the
static depth-four `uvm_config_db` string-to-int map
(`set`/`get`/`exists`/`remove` with class-scope storage), a
static `uvm_factory` type-override table
(`register`/`set_type_override`/`get_override`/`has_override`/
`is_registered`, plus depth-four instance overrides that take priority and
fall back after removal), the
UVM-lite structural surface (`uvm_report_object`, `uvm_transaction`,
`uvm_test`, `uvm_env`, `uvm_agent`, `uvm_monitor`, `uvm_scoreboard`, and
analysis export/imp forwarding), the
UVM-lite sequence flow (`uvm_sequence_item`, virtual-body `uvm_sequence`,
queue-backed `uvm_sequencer`, `uvm_seq_item_port`, and `uvm_driver`),
an analysis port with four explicit subscriber slots and fan-out writes,
a callback base/pool with four virtual callback slots plus enable/disable
filtering,
and the
UVM message family — `$uvm_info(tag, fmt, args..., verbosity)`,
`$uvm_warning/$uvm_error(tag, fmt, args...)`, and `$uvm_fatal` (which
also terminates the simulation) — emitting
`[UVM_INFO][tag][UVM_MEDIUM]`-style prefixed formatted messages, with
`%0d` width modifiers and `%s` string-argument printing, through the
simulator display channel.

Classes additionally support **composition** (class-typed properties with
nested storage, deep dotted paths) and **virtual-method polymorphism**:
each handle carries a runtime type id, `b = d` propagates it, and calls
through a base-typed handle lower to a `case` on that id, selecting the
most-derived override for every concrete derived class (statement and
expression forms; `super.m` stays statically bound; non-virtual methods
keep static binding). Handle assignment implements **reference
aliasing**: `q = p` makes both handles share storage (writes through
either are visible through the other), and `h = new(...)` splits `h`
back into its own object while former aliases retain the old one. A built-in virtual `uvm_component` base class extends `uvm_object`
with a `run_phase()` hook, an integrated objection lifecycle, component
names, parent back-references, and four explicit child slots, giving
testbenches the env/driver/monitor hierarchical structure used by UVM
without requiring the full class library. The built-in component executes the
canonical common phase sequence — build, connect, end-of-elaboration,
start-of-simulation, run, extract, check, report, and final — with virtual
overrides at every hook.

## RV32I CPU (signoff vehicle)

`rv32i/` hosts the end-to-end foundry-signoff target: a single-cycle RV32I
subset core written in the synthesizable register + case-multiplexer style
(16-word instruction/data storage, 32-entry register file with hardwired
x0, one-cycle 512-bit program-image load port, EBREAK halt, and
`dbg_pc/dbg_x10/dbg_x11` debug outputs). The implemented instruction set
covers all RV32I base OP/OP-IMM arithmetic-logic-shift operations
(including SRA via a whole-word arithmetic identity), LUI, AUIPC, JAL,
JALR, every branch condition, the complete memory subset
(LW/LH/LHU/LB/LBU loads with sign/zero extension and byte/half
selection; SW/SH/SB stores via read-modify-write merging), and SYSTEM
(EBREAK/ECALL halt, FENCE nop). `rv32i_cases.rs` drives machine-code programs through the compiled
design with a Rust-side encoder (including a LUI+ADDI constant loader) and
checks architectural results: arithmetic overflow pairs, logic ops, logical
vs arithmetic shifts, signed/unsigned compares, LUI/AUIPC, counted
branch loops, JAL/JALR call-return, LW/SW round trips, a Fibonacci
mixed program, a class-composed Monitor/Scoreboard/Env testbench that
samples the debug ports and reports pass/fail, RTL scan-chain DFT
insertion with a full-chain audit, and an MMMC PDK signoff case (model
correlation, per-corner implementation, timing, DRC, LEF/DEF/SDF
physical databases, and full sequential-equivalence LEC) for the complete
2243-flop netlist. The full `rv32i_pdk_signoff_mmmc_pass` gate is enabled
and PASSES. A staged physical signoff
(`rv32i_pdk_physical_signoff_mmmc_pass`) runs in-suite and PASSES —
model correlation, two-corner timing (worst 201.125 ns), zero DRC
errors, and LEF/DEF/SDF generation — via a new staged-flow API that
can run physical closure independently when a manifest does not require
LEC. The generated artifact lives at
`rv32i/verify/signoff_report.txt` and covers physical signoff,
power/IR, single- and multi-layer power meshes with EM, UPF
power-domain IR (with per-domain multilayer meshes), and CTS.
The `rv32i_default_physical_flow_routes_full_core` gate exercises the
default 256-site-by-256-track physical grid without the signoff manifest's
explicit 2048-track override, then checks routing, DRC, and sequential-count
invariants. Power/IR and CTS stages also run in-suite on RV32I:
`rv32i_power_and_ir_analysis` drives full-chip
switching activity with a wide-vector activity format (the 512-bit
program image is supplied directly as a sized Verilog literal `512'h...`
on the top-level port; the power/IR pipeline now accepts arbitrary-width
inputs), asserting positive total power/current, a worst IR
row, and mapped-cell accounting, while `rv32i_cts_inserts_and_audits_
clock_tree` synthesizes and audits the post-synthesis clock tree
(nonempty trees, buffers inserted, sequential count preserved), `rv32i_power_mesh_ir_and_em` solves the placed design's power-mesh IR
by Gauss-Seidel iteration (positive current, finite worst drop,
nonempty nodes/segments, converged iterations), and
`rv32i_multilayer_mesh_em_lifetime` runs the two-layer M8/M7 mesh with
Black-equation electromigration lifetime estimation (positive finite
worst EM lifetime, converged solver), and
`rv32i_upf_domain_ir_and_mesh` parses a UPF power-domain view
(`set_design_top`, `create_power_domain TOP`, VDD/VSS supply ports),
runs domain-aware IR, and produces per-domain multilayer meshes for
every physically populated domain. Driving that case already surfaced and
fixed two real tool bugs: `always @*` case blocks without defaults
inferred combinational loops, and synthesis dropped registers that were
only assigned in an `if` branch's `else` path (2049 state bits collapsed
to 545). PDK manifests now carry per-layer routing track capacity
(`tracks N`) that feeds the global router. Sequential equivalence is now
open on the full RV32I gold-versus-mapped design and **passes**. The proof
follows the scalable parts of Kepler Formal's LEC flow: normalize shared
boundary variables, construct cached Boolean/CNF miters, solve with the
industrial Kissat CDCL backend, and only decompose individual outputs when a
miter is satisfiable. The Rust engine retains its sound correspondence-point
induction for this reordered, technology-mapped netlist and adds the same
Kissat revision used by Kepler as a vendored backend.

The former full-netlist pre-encode was dead overhead and has been removed.
Cone encoding now takes cut predicates, the inductive gate-side cut set is
maintained incrementally, dense NetId sets avoid SipHash overhead, already
discharged internal points are not replayed, and topological flop scheduling
is computed once instead of walking every full cone. SAT contexts keep the
in-tree incremental CDCL as a fallback but use Kissat first, including model
extraction for CEGAR refinement.

The full RV32I LEC profile reports `equivalent=true` with no counterexample:
362/3128 directly named points, 8660 accepted correspondence points in
total, and 2406 localized final miter queries. The enabled
`rv32i_pdk_signoff_mmmc_pass` gate passes model correlation, LEC, two-corner
timing, zero-DRC physical databases, and the complete `rv32i_` regression
suite passes as well.

The analog contribution includes:

* 301 unique resistive divider DC solutions;
* 20 physical `SHEET/L/W` resistor solutions.
* 20 temperature/width-corrected `.rmodel` layer-resistor solutions.
* 20 width-bin sheet-rho/offset interpolation resistor solutions.
* 251 unique RC transient charging solutions;
* 150 unique RL transient solutions with initial-condition enforcement;
* 100 unique AC low-pass transfer sweeps;
* 100 SIN/PULSE/PWL/DC source-waveform checks;
* 50 voltage-controlled source networks;
* 48 nonlinear diode operating-point/KCL checks.
* 40 NMOS Level-1 saturation solutions;
* 25 PMOS Level-1 saturation solutions;
* 20 BSIM3 long-channel `LEVEL=8` DC diode-connected solutions.
* 20 BSIM3 short-channel/DIBL `DVT0/DVT1/DVT2` DC diode-connected solutions.
* 20 BSIM3 narrow-width `DVT0W/DVT1W/W0` DC diode-connected solutions.
* 20 BSIM3 temperature `KT1/KT2/KT1L/UTE` DC diode-connected solutions.
* 20 BSIM3 rectangular `WMIN/WMAX/LMIN/LMAX` geometry-bin selection/current checks.
* 20 NMOS/PMOS mobility-degradation and velocity-saturation checks.
* 35 forward-active Ebers-Moll BJT solutions.
* 60 resistor thermal-noise Johnson-Nyquist checks;
* 40 diode shot-noise operating-point checks.
* 60 MOS gate-charge transient RC checks;
* 40 BJT junction-capacitance AC-pole checks.
* 50 resistor-divider Monte Carlo statistical sweeps;
* 50 RC transient Monte Carlo statistical sweeps.
* 50 fully correlated process Monte Carlo divider sweeps.
* 50 explicit cross-parameter covariance Monte Carlo divider sweeps.
* 20 distance-dependent spatial-mismatch Monte Carlo divider sweeps.
* 20 normal/lognormal marginal-distribution Monte Carlo divider sweeps.
* 20 full-covariance Monte Carlo divider sweeps.
* 20 imported vendor-style statistical-model Monte Carlo divider sweeps.
* 20 deterministic power-law resistor/MOS aging checks.
* 50 diode depletion-capacitance voltage-dependence checks;
* 50 BJT junction-capacitance voltage-dependence checks.
* 50 MOS saturation charge-partition AC-coupling checks.
* 50 MOS flicker-noise frequency-slope checks;
* 50 diode flicker-noise frequency-slope checks.
* 50 resistor thermal-noise temperature checks;
* 50 MOS mobility temperature checks.
* 50 diode transit/diffusion charge checks;
* 50 BJT transit/diffusion charge checks.
* 50 resistor-divider analytic sensitivity checks.
* 20 exact linear-DC adjoint gradient/Hessian checks.
* 20 diode-connected ideal-NMOS DC implicit gradient/Hessian checks.
* 20 diode-connected ideal-NPN DC implicit gradient/Hessian checks including instance `AREA`.
* 20 exact linear-AC magnitude adjoint gradient/Hessian checks.
* 20 exact nonlinear diode DC implicit gradient/Hessian checks.
* 20 exact series-RC transient-final gradient/Hessian checks.
* 20 general backward-Euler linear RCL transient-final gradient/Hessian checks.
* 20 general trapezoidal linear RCL transient-final gradient/Hessian checks.
* 20 nonlinear diode AC magnitude gradient/Hessian checks.
* 10 nonlinear diode transit-time AC magnitude gradient/Hessian checks.
* 10 nonlinear diode depletion-capacitance AC magnitude gradient/Hessian checks.
* 10 nonlinear diode junction-parameter AC magnitude gradient/Hessian checks.
* 10 AC magnitude optimization cases using analytic Hessian/Gauss-Newton polish.
* 50 Sobol variance-decomposition checks.
* 50 correlated Sobol covariance-decomposition checks.
* 50 resistor TC1 temperature checks;
* 50 resistor TC2 temperature checks.
* 50 bounded differential-evolution optimization checks.
* 50 quantized mixed-integer optimization checks (integer/E-series R and pF C grids).
* 50 two-objective Pareto frontier optimization checks.
* 25 constrained Pareto frontier optimization checks.
* 20 algebraic-constraint Pareto frontier optimization checks.
* 10 transcendental/min-max function-constraint Pareto checks.
* 15 three-objective Pareto frontier optimization checks.
* 10 NSGA-III-style reference-direction Pareto checks.
* 10 adaptive two-layer reference-direction Pareto checks.
* 50 periodic-steady-state RC low-pass checks.
* 50 automatic-fundamental PSS checks.
* 20 autonomous CMOS ring-oscillator PSS startup checks.
* 50 AM source/envelope-following checks.
* 100 deterministic clock period/high/low/duty waveform-measurement checks.
* 100 deterministic multi-bit signal Hamming-comparison checks.
* 100 deterministic different-net Manhattan polygon/rectangle spacing checks.
* 100 deterministic via bottom/top enclosure checks.
* 100 rectangle overlap sweeps covering union, intersection, difference, and exclusive-or.
* 100 arbitrary-angle simple-polygon exact area/bounds/containment/translation
  checks.
* 100 arbitrary-angle simple-polygon exact spacing and intersection checks.
* 100 arbitrary-angle layout-document storage, hit-test, SVG, translation, and
  DRC integration checks.
* 100 arbitrary-angle polygon editor tool, preview, vertex-drag, undo, and
  redo session checks.
* 100 direct arbitrary-angle two-pin route exact-length, blockage, conflict,
  and spacing checks.
* 100 schematic pointer-drag event sweeps.
* 100 schematic wire-tool snap/connectivity event sweeps.
* 100 browser-host wire-event HTTP round trips.
* 100 browser-host layout rectangle-event HTTP round trips.
* 100 browser-host layout polygon-event HTTP round trips.
* 100 parameterized generate-for elaboration cases spanning widths 1..100.
* 100 parameterized generate-case branch-selection elaboration/simulation
  cases.
* 100 generated-clock divide-by cases.
* 100 asynchronous/logically-exclusive/physically-exclusive clock-group cases.
* 100 RTL flow-cache source hit/miss cases plus invalid-source and concurrent-agent checks.
* 100 casez/casex priority-match simulation cases.
* Browser schematic/layout zoom-pan-property-grid integration checks.
* 100 SystemVerilog variable-type/default-width elaboration cases.
* 100 fixed one-dimensional module unpacked-net-array update/simulation cases.
* 100 fixed multidimensional module unpacked-net-array update/simulation
  cases.
* 100 fixed unpacked module array-port whole-vector connection/simulation
  cases.
* 100 fixed one-dimensional unpacked array-slice port connection/simulation
  cases.
* 100 SystemVerilog ascending/descending indexed part-select elaboration and
  simulation cases.
* 100 runtime-base indexed part-select simulation and out-of-range rejection
  checks.
* 100 runtime-base indexed part-select blocking-write simulation cases.
* 100 inline-function runtime-base indexed part-select local/package-function
  simulation cases.
* 100 runtime-base indexed part-select non-blocking write and multi-field NBA
  merge simulation cases.
* 100 runtime-base indexed part-select NBA RTL-versus-gate synthesis and
  simulation cases.
* 100 reversed packed-range indexed part-select constant/runtime simulation
  cases.
* 100 reversed packed-range indexed part-select blocking/non-blocking write
  simulation cases.
* 100 reversed packed-range inline-function indexed part-select simulation
  cases.
* 100 reversed packed-range continuous indexed-lvalue drive/simulation cases.
* 100 fixed unpacked-array element indexed part-select read/write simulation
  cases.
* 100 inline-function unpacked-array element indexed part-select
  read/write/preservation simulation cases.
* 100 runtime unpacked-array element-index plus packed indexed-select
  simulation cases.
* 100 inline-function runtime array element-index plus packed indexed-select
  simulation cases.
* 100 runtime unpacked-array element-index indexed blocking/non-blocking
  assignment simulation cases.
* 100 runtime unpacked-array element-index indexed assignment synthesis and
  RTL-versus-gate simulation cases.
* Direct sparse-memory IR and frontend tests covering multidimensional
  logical/physical mapping, reversed elements, part-select reads/writes,
  blocking writes, NBA field merging, continuous lvalues, runtime indexes, X
  out-of-range reads, 4 KiB arrays, and 1 MiB-by-8 arrays.
* 100 runtime unpacked-array element-index constant-packed-base continuous
  indexed-lvalue simulation cases.
* 100 inline-function runtime array element-index indexed assignment
  simulation cases.
* 100 scalar runtime packed-base continuous indexed-lvalue simulation cases.
* 100 SystemVerilog package-constant/import/scoped-reference elaboration cases.
* 100 local/package SystemVerilog enum-typedef width/constant elaboration cases.
* 100 local/package packed-struct member-selection simulation cases.
* 100 local/package packed-union overlapping-member simulation cases.
* 100 simple SystemVerilog function inlining/simulation cases.
* 100 simple package-function import/inlining/simulation cases.
* 100 nested non-recursive SystemVerilog function inlining/simulation cases
  plus recursion rejection.
* 100 SystemVerilog function local-variable inlining/simulation cases.
* 100 SystemVerilog function if/else inlining/simulation cases.
* 100 SystemVerilog function exact-case inlining/simulation cases.
* 100 SystemVerilog function bounded-for unrolling/simulation cases.
* 100 SystemVerilog function casez/casex wildcard-case inlining/simulation
  cases.
* 100 SystemVerilog function constant-repeat unrolling/simulation cases.
* 100 SystemVerilog function bounded-while unrolling/simulation cases.
* 100 SystemVerilog function packed-struct local field-update/simulation
  cases.
* 100 SystemVerilog function fixed unpacked-array local update/simulation
  cases.
* 100 SystemVerilog function fixed multidimensional unpacked-array local
  update/simulation cases.
* 100 SystemVerilog interface/modport producer-consumer elaboration,
  direction-enforcement, and simulation cases.
* 100 SystemVerilog fixed interface-array element-selection simulation cases.
* 100 fixed multidimensional interface-array element-selection simulation
  cases.
* 100 fixed module-instance-array vector-connection slicing simulation cases.
* 100 fixed multidimensional module-instance-array row-major slicing
  simulation cases.
* 100 static virtual-interface port producer/consumer simulation cases.
* 100 SystemVerilog immediate-assertion simulation/failure-rejection cases.
* 100 signed SystemVerilog arithmetic/cast/shift simulation cases.
* 100 signed arithmetic/comparison/shift synthesis and RTL-versus-gate
  simulation cases.
* 100 clocked overlapping-implication SystemVerilog property simulation and
  failure-rejection cases.
* 100 non-overlapping `|=>` SystemVerilog next-sample property simulation and
  failure-rejection cases.
* 100 SystemVerilog `assume property` / labeled `cover property` simulation
  and coverage-hit cases.
* 100 fixed-cycle `##[n]` SystemVerilog property-delay simulation and
  coverage/failure-rejection cases.
* 100 bounded-delay-range `##[a:b]` SystemVerilog property simulation and
  coverage/failure-rejection cases.
* 100 bounded consecutive-repetition `[*n]` SystemVerilog property simulation
  and coverage/failure-rejection cases.
* 100 bounded repetition-range `[*a:b]` SystemVerilog property simulation and
  coverage/failure-rejection cases.
* 100 bounded goto-repetition `[->n]` SystemVerilog property simulation and
  coverage/failure-rejection cases.
* 100 bounded goto-repetition-range `[->a:b]` SystemVerilog property
  simulation and coverage/failure-rejection cases.
* 100 bounded nonconsecutive-repetition `[=n]` / `[=a:b]` SystemVerilog
  property simulation and coverage/failure-rejection cases.
* 100 `disable iff` SystemVerilog property abort/re-enable simulation cases.
* 100 bounded same-delay SystemVerilog sequence `and`/`or` composition
  property simulation cases.
* 100 bounded fixed-delay two-sequence SystemVerilog composition property
  simulation cases.
* 100 SDC design-wide transition/capacitance/fanout quality-rule parsing and
  analysis cases.
* 100 object-scoped SDC transition/capacitance/fanout quality-rule parsing,
  override, wildcard, and analysis cases.
* 100 SDC `set_input_transition` / `set_load` port/net/pin parsing and
  quality-analysis cases.
* 100 SDC `set_clock_transition` root/generated-clock parsing and clock-network
  transition-analysis cases.
* 100 SDC independent minimum/maximum clock-transition setup, hold, recovery,
  and removal lookup cases.

Run a subset by case-name prefix:

```sh
SUITE_FILTER=ams_rc cargo test -p eda-tests --test suite
```

## Major crates

| Area | Crates |
|---|---|
| Shared IR/logic/library/netlist | `eda-core`, `eda-ir` |
| RTL frontend | `eda-hdl`, `eda-slang` |
| Digital simulation and waves | `eda-sim`, `eda-waveform` |
| Analog/mixed-signal simulation | `eda-ams` |
| Synthesis and netlist export | `eda-synth` |
| Formal/equivalence | `eda-formal`, `eda-assert` |
| Timing and constraints | `eda-sta`, `eda-sdf`, `eda-spef` |
| Physical implementation | `eda-pnr`, `eda-cts`, `eda-eco` |
| Editable schematic database | `eda-schematic` |
| Power/IR/reliability | `eda-power`, `eda-upf`, `eda-ir` |
| Verification quality | `eda-coverage`, `eda-cdc`, `eda-dft`, `eda-fault`, `eda-lvs` |
| Agent/API/CLI surfaces | `eda-api`, `eda-cli`, `eda-tests` |
| Long-running agent service | `eda-daemon` |

## Agent API examples

```rust
// RTL to mapped netlist, equivalence, timing, placement, and routing.
let flow = eda_api::run_flow(rtl, Some("top"))?;

// SPICE DC/AC/transient analyses.
let results = eda_api::run_spice(spice)?;
let text = eda_api::spice_report(&results);

// Machine-readable capability/gap discovery.
let gaps = eda_api::missing_capabilities();
let audit = eda_api::capability_report();

// Launch the bundled browser schematic editor host.
let editor = eda_api::SchematicBrowserServer::bind(document, "127.0.0.1:0")?;

// Launch the bundled browser layout/DRC editor host.
let layout = eda_api::LayoutBrowserServer::bind(
    layout_document, "M1", "vdd", "VIA1", "127.0.0.1:0",
)?;
```

The CLI mirrors key API flows:

```sh
cargo run -p eda-cli -- capabilities
cargo run -p eda-cli -- daemon --listen 127.0.0.1:7822
cargo run -p eda-cli -- pdk design.v --top top --pdk process.pdk
cargo run -p eda-cli -- ams design.spice
cargo run -p eda-cli -- ams design.spice --stats vendor.stats
cargo run -p eda-cli -- ams design.spice --def-locations layout.def
cargo run -p eda-cli -- ams design.spice --rmodels layers.rmodels
cargo run -p eda-cli -- flow design.v --top top
```

## Scope and current gaps

This project is not yet, and must not be represented as, a drop-in replacement
for Cadence or Synopsys. The capability API is intentionally explicit about
implemented, partial, and missing domains. The current high-priority gaps are:

1. advanced foundry MOSFET/BJT compact models beyond Level-1/Ebers-Moll and their advanced charge equations;
2. complete foundry sheet-resistance/aging models, noise correlation, nonlinear covariance propagation, multi-carrier envelope solving, and harmonic balance;
3. externally certified foundry PDK calibration and golden model correlation;
   portable typed PDK manifests, explicit PVT-tagged delay-scale corners, and
   checksummed inline per-corner Liberty views with deterministic per-corner
   signoff checks are implemented.
4. a commercial-strength SystemVerilog/SDC language surface;
5. an interactive schematic/layout editor shell (the API-first editable schematic database and SVG renderer exist);
6. complete daemon coverage for every API and externally cancellable long-running jobs;
   the current JSON/TCP daemon, remote job queue, and typed job manager cover a
   foundational subset including formal RTL equivalence, standalone RTL
   STA/PnR/DFT/CDC/fault/power/IR-EM/temporal-property BMC, functional
   coverage, and timed RTL simulation, with cooperative RTL/PDK stage,
   SAT/BMC/equivalence/k-induction search, simulator delta scheduling, router
   search, single-corner physical-timing stages, and AMS
   optimizer/quantized-refine loop cancellation.

Use `eda_api::capabilities()` or `eda capabilities` to obtain the authoritative
current-state audit rather than relying on this document alone.
