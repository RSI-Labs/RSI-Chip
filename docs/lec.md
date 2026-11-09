# Logic equivalence checking (LEC)

The LEC tooling is exposed through `eda-api` and the `lec` CLI command. All
entry points share the `eda-formal` miter/CNF engine, sequential flop matching,
counterexample extraction, and cooperative cancellation.

## Entry points

| API | What it compares |
|---|---|
| `check_rtl_equivalence(src, top)` | RTL-derived reference vs standard-cell-mapped netlist of the same source |
| `check_rtl_equivalence_between(ref_src, ref_top, impl_src, impl_top)` | Two RTL sources (revision/refactor vs golden) |
| `check_rtl_vs_netlist_equivalence(src, top, &netlist)` | RTL-derived reference vs an externally supplied mapped/PnR netlist |
| `check_design_equivalence(&reference_design, &implementation_design)` | Two already-lowered `Design` values (for independent frontends) |
| `run_flow_with_rtl_reference_lec(...)` | Full flow where the LEC reference is the RTL-derived netlist |

Each returns `equivalent`, a binary counterexample (`BTreeMap<String, String>`),
and solver notes. `*_cancellable` variants accept a `FlowCancellation` token.

## CLI

```sh
# RTL vs the standard-cell-mapped view.
eda lec design.sv --top top

# Two-source RTL LEC.
eda lec design.sv --top top --reference golden.sv --reference-top top
```

The command prints reference/implementation cell and net counts, the verdict,
solver notes, and a counterexample when the designs differ. `--reference`
requires the `lec` command and `--reference-top` requires `--reference`.

## Reference-block regression

`crates/eda-tests/tests/eda_blocks.rs` runs every reference block (8-bit
counter, 32-bit shift register, 8-bit ALU, UART TX, SPI master, PWM, LFSR)
through:

1. RTL simulation against a golden trace,
2. RTL-derived LEC against the mapped netlist,
3. the full physical flow (place, route, DRC, SDF),
4. mapped gate-level simulation compared bit-for-bit with the RTL trace,
5. an SDF-annotated counter run that exercises setup/hold timing.

`crates/eda-tests/tests/lec_tools.rs` covers the two-source, external-netlist,
and Design-to-Design APIs including negative (bug-detection) cases.

## SAT engines

LEC is parameterized by SAT engine. The same engine is used for every
correspondence-point query and for the final miter.

| Engine | How it runs | Selection |
|---|---|---|
| `kissat` | Vendored Kissat compiled into `eda-formal` (FFI, production default) | `--solver kissat`, `SatBackend::Kissat`, thread default |
| `native` | In-tree CDCL solver (watched literals, VSIDS, 1UIP, Luby restarts) | `--solver native` |
| `z3` | External `z3 -dimacs` process; SAT model parsed from DIMACS output | `--solver z3`, `EDA_Z3` for a custom binary |
| `cvc5` | Official cvc5 Python bindings driven through a DIMACS bridge | `--solver cvc5`, `EDA_PYTHON` for the interpreter |
| `auto` | Resolves to Kissat | CLI/default |

Environment overrides:

- `EDA_Z3` — path to a z3 binary (otherwise `PATH`, Homebrew paths, and the
  pip user bin directory are searched).
- `EDA_PYTHON` — Python interpreter used for the cvc5 bindings.
- `EDA_SAT_TIMEOUT_SECS` — external-engine timeout (default 600 s).

External backends are fail-closed: an unavailable engine is an error for an
explicit `--solver` request, and never silently becomes a different verdict.

## Multi-engine cross-check

```sh
eda lec design.sv --top top --cross-check kissat,native,z3,cvc5
```

`check_rtl_equivalence_cross_checked` (and the two-source, external-netlist,
and Design-to-Design variants) reruns the proof with every available engine and
fails if two engines disagree. Report notes identify each participating engine.
Unavailable engines are listed as skipped.

## API

```rust
use eda_api::SatBackend;

let r = eda_api::check_rtl_equivalence_with_solver(rtl, Some("top"), SatBackend::Z3)?;
let r = eda_api::check_rtl_equivalence_cross_checked(
    rtl,
    Some("top"),
    &[SatBackend::Kissat, SatBackend::Native, SatBackend::Z3, SatBackend::Cvc5],
)?;
```

## Multi-engine verification matrix

`cargo test --release -p eda-tests --test lec_tools -- --nocapture` runs every
entry below through all four engines and requires the same verdict:

| Design | native | kissat | z3 | cvc5 |
|---|---|---|---|---|
| `counter8` | equivalent | equivalent | equivalent | equivalent |
| `shift_register32` | equivalent | equivalent | equivalent | equivalent |
| `alu8` | equivalent | equivalent | equivalent | equivalent |
| `uart_tx` | equivalent | equivalent | equivalent | equivalent |
| `spi_master` | equivalent | equivalent | equivalent | equivalent |
| `pwm` | equivalent | equivalent | equivalent | equivalent |
| `lfsr8` | equivalent | equivalent | equivalent | equivalent |
| `i8086_prefix_decoder` | equivalent | equivalent | equivalent | equivalent |
| `i8086_opcode_decoder` | equivalent | equivalent | equivalent | equivalent |
| `i8086_modrm_decoder` | equivalent | equivalent | equivalent | equivalent |

The same test also checks the negative direction: an increment-by-two counter
must be reported non-equivalent with a counterexample by every engine.

Full-core regressions keep using Kissat (production) in the default path:

```text
SLANG_BIN=... cargo test --release -p eda-tests --test i8086_independent_lec
→ 1 passed, 172.60s

cargo run --release -p eda-tests --example i8086_rtl_lec_probe
→ equivalent=true gold_cells=251734 mapped_cells=93773 drc_errors=0
```

## Full-core, 10k+ gate LEC validation

The full cores are checked as complete sequential netlists, not by per-opcode
sampling. Both a same-frontend RTL-vs-mapped proof and an independent
slang-vs-eda-hdl proof pass:

| Core | Mapped gates | RTL vs mapped | Independent frontends |
|---|---:|---|---|
| i8086 full core | 93,773 | equivalent (`gold=251,734`) | equivalent |
| RV32I platform full core (`rv32i_full_core.sv`) | 37,119 | equivalent (`gold=52,552`) | equivalent (`slang=52,012`) |
| RV32I signoff core (`rv32i_core.v`) | 31,320 | equivalent (`gold=42,196`) | equivalent (`slang=42,196`) |

Reproduce:

```sh
# i8086 full core
SLANG_BIN=/tmp/slang-build/bin/slang \
  cargo test --release -p eda-tests --test i8086_independent_lec
cargo run --release -p eda-tests --example i8086_rtl_lec_probe

# RV32I both cores
SLANG_BIN=/tmp/slang-build/bin/slang \
  cargo test --release -p eda-tests --test rv32i_independent_lec -- --test-threads=1
cargo run --release -p eda-tests --example rv32i_lec_probe

# RV32I RTL-vs-mapped full-core gate + PDK signoff
SUITE_FILTER=platform_rv32i_full_core_passes_rtl_to_mapped_lec \
  cargo test --release -p eda-tests --test suite full_suite_all_green
SUITE_FILTER=rv32i_pdk_signoff_mmmc_pass \
  cargo test --release -p eda-tests --test suite full_suite_all_green
```

"Full-range" here means the SAT miter covers the entire sequential state and
input space of the concrete netlist pair. It does not mean every architecturally
defined instruction was simulation-tested, nor that the RTL implements the
complete commercial ISA (see the i8086 flow audit for ISA/foundry limits).

## All-circuit LEC matrix (2026-11-07)

Every synthesizable 8086, RV32I, and RV64I circuit is checked as a complete netlist.
`crates/eda-tests/tests/full_circuit_lec.rs` runs RTL-vs-mapped LEC and an
independent slang-vs-eda-hdl LEC for each row:

| Circuit | Reference / mapped cells | Flop next-state proofs | Output bits | Independent frontends |
|---|---:|---:|---:|---|
| `rv32i_isa_decoder` | 1,532 / 210 | 0 | 193 | yes (1,207 / 210) |
| `rv32i_full_core` | 52,552 / 37,119 | 1,572 | 327 | yes (52,012 / 37,119) |
| `rv32i_core` (signoff) | 42,196 / 31,320 | 2,243 | 163 | yes (42,196 / 31,320) |
| `rv32i_full_soc_16` | 60,348 / 44,175 | 2,084 | 226 | yes (59,808 / 44,175) |
| `rv64ima_full_core` | 218,552 / 168,463 | 3,709 | 647 | yes (217,711 / 168,463) |
| `i8086_prefix_decoder` | 500 / 76 | 0 | 8 | yes (500 / 76) |
| `i8086_opcode_decoder` | 6,316 / 941 | 0 | 17 | yes (5,588 / 941) |
| `i8086_modrm_decoder` | 795 / 584 | 0 | 25 | yes (795 / 584) |
| `i8086_full_core` | 251,734 / 93,773 | 544 | 396 | yes (244,570 / 93,773) |
| `i8086_full_soc_64` | 287,247 / 109,910 | 1,056 | 235 | yes (280,083 / 109,910) |
| `cpu8086` | 5,511 / 2,037 | 175 | 41 | yes (3,684 / 2,037) |
| `sys8086` | 46,336 / 39,600 | 1,216 | 35 | yes (44,509 / 39,600) |

`cargo test --release -p eda-tests --test full_circuit_lec -- --test-threads=1`
runs every row. The 8086/RV32I rows completed in 4,342.65 s; the RV64I row is
also independently covered by `crates/eda-tests/tests/rv64i_rtl_lec.rs`
(independent frontend: 849.08 s) and by the strict no-cut probe (996.3 s).

Testbenches (`*_tb*`, `*_runtime_tb*`) are excluded because they contain
clocks/initial blocks and are not synthesizable circuits.

### Coverage audit

Each successful result now reports:

```text
coverage: gold ... flops + ... combinational cells in proof cones (... dangling);
          gate ... flops + ... combinational cells in proof cones (0 dangling)
```

All matched flop next-state functions and all output bits are proven; on the
mapped (DUT) side every combinational cell is inside at least one proof cone
(0 dangling). The generic gold reference is intentionally unoptimized, so it
may still contain dead cells that drive no observable sink; these are reported
explicitly rather than silently ignored.

### Strict no-cut mode

`--strict` (or `with_lec_strict`) disables internal correspondence points.
Every obligation is proven over its full combinational fan-in cone; flop
outputs remain free boundary variables (the standard sequential-equivalence
composition). The whole 8086 core passes:

```text
i8086_full_core: strict equivalent=true reference_cells=251734 mapped_cells=93773
proved all 544 flop next-state functions and 396 output bits with 940 miter queries
coverage: ... gate 544 flops + 93229 combinational cells in proof cones (0 dangling)
strict full-circuit LEC: 0 internal cuts
elapsed=457.9s with 10 parallel workers
```

```sh
eda lec design.sv --top top --strict
```

### Strict no-cut full-circuit results

Every synthesizable circuit also passes with **zero internal cuts** (each flop
next-state function and output bit proven over its complete combinational
fan-in cone, flop outputs as free boundary variables). `EDA_LEC_THREADS`
controls proof parallelization (results below use 10 workers):

| Circuit | Mapped cells | Flops + outputs proven | Strict time |
|---|---:|---:|---:|
| `i8086_full_core` | 93,773 | 544 + 396 | 457.9 s |
| `i8086_full_soc_64` | 109,910 | 1,056 + 235 | 644.0 s |
| `rv32i_core` (signoff) | 31,320 | 2,243 + 163 | 1,380.4 s |
| `rv32i_full_core` | 37,119 | 1,572 + 327 | 647.4 s |
| `rv32i_full_soc_16` | 44,175 | 2,084 + 226 | 326.5 s |
| `sys8086` | 39,600 | 1,216 + 35 | 5.6 s |
| `rv32i_isa_decoder` | 210 | 0 + 193 | < 1 s |
| `rv64ima_full_core` | 168,463 | 3,709 + 647 | 996.3 s |

All results: `equivalent=true`, `strict full-circuit LEC: 0 internal cuts`,
and `gate ... (0 dangling)` coverage. The mapped DUT side has no combinational
cell outside the proof cones.

```sh
EDA_LEC_THREADS=10 cargo run --release -p eda-tests \
  --example strict_full_lec_probe -- i8086_soc
EDA_LEC_THREADS=10 cargo run --release -p eda-tests \
  --example strict_full_lec_probe -- rv32i_soc
```
