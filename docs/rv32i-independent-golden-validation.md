# RV32I independent golden validation

This record describes the current executable RV32I differential evidence. It is
intentionally scoped: it does not claim complete privileged-ISA, ELF, exception,
or i8086 closure.

## Oracle and lanes

| Lane | Implementation | Role |
| --- | --- | --- |
| Built-in ISA model | `eda-cpu-platform::rv32i::Rv32iMachine` | Repository behavioral reference; useful for regression, but not independent evidence by itself. |
| RTL runtime | `Rv32iSvRunner` over `platform/rtl/rv32i_full_core.sv` | SystemVerilog frontend, elaboration, simulation, memory, and architectural state under test. |
| Independent oracle | `raven-riscv-engine` 0.1.1, MIT OR Apache-2.0 | External RV32 implementation executed from an independent codebase. |

The independent oracle is loaded from the original sparse-memory image, not from
post-run RTL state. This matters because directed programs intentionally load
and then overwrite the same data addresses.

## Compared state

Every externally checked case compares:

1. all 32 integer registers, including forced `x0 == 0`;
2. the PC at the terminating `EBREAK`;
3. halt/trap classification;
4. every byte in the declared comparison window, not just selected words.

## Base instruction matrix

Test: `rv32i_base_instruction_matrix_matches_internal_and_independent_golden_models`

| RV32I family | Covered operations |
| --- | --- |
| `LUI` / `AUIPC` | Upper-immediate construction and PC-relative upper value. |
| `OP-IMM` | `ADDI`, `SLTI`, `SLTIU`, `XORI`, `ORI`, `ANDI`, `SLLI`, `SRLI`, `SRAI`, including negative and boundary immediates. |
| `OP` | `ADD`, `SUB`, `SLL`, `SLT`, `SLTU`, `XOR`, `SRL`, `SRA`, `OR`, `AND`. |
| Loads | `LB`, `LH`, `LW`, `LBU`, `LHU`. |
| Stores | `SB`, `SH`, `SW`. |
| Branches | `BEQ`, `BNE`, `BLT`, `BGE`, `BLTU`, `BGEU`, with taken and untaken control paths. |
| Jumps | `JAL`, `JALR`, including link and return behavior. |
| Order/barrier | `FENCE`. |
| Termination | `EBREAK`. |

Comparison window: bytes `0x0000_0000..0x0000_1fff`.

## Deterministic randomized differential

Test: `rv32i_randomized_differential_fuzz_matches_internal_and_independent_golden_models`

| Property | Current evidence |
| --- | ---: |
| Seeds | 64 |
| Random instructions after register setup | 24 per seed |
| Data bytes per seed | 256 |
| Data window | `0x0000_2000..0x0000_20ff` |
| Full byte comparison window | `0x0000_0000..0x0000_20ff` |

The generator mixes register-register operations, immediate operations, aligned
byte/half/word loads and stores, and all six branch predicates. It uses a fixed
xorshift sequence, so failures are reproducible by seed.

## ELF and high-address execution

The program-fixture lane uses the same external Raven executor through a custom
sparse implementation of Raven's public `Bus` trait. This preserves the
repository's sparse-memory model and allows a synthetic ELF entry at
`0x7000_0000` without allocating almost two gigabytes of contiguous oracle
memory.

Covered images:

1. checked-in `rv32i_exit42.elf`;
2. checked-in `rv32i_high_memory.elf`;
3. a generated sparse ELF with entry and data at `0x7000_0000`.

Each image compares:

- all 32 integer registers;
- final PC;
- Linux exit code;
- every byte in the loaded segment, including data modified by the program.

This lane exposed that the SystemVerilog ELF runner reset `x2/SP` to zero even
though the built-in ELF loader supplies the Linux ABI stack pointer. The RTL now
has an explicit `init_sp` input, the bare-metal runner passes zero, and the ELF
runner passes the loader-provided stack pointer.

## Privileged subset status

The machine-CSR and trap/MRET matrices now add a third lane. Raven executes
the integer, CSR, and MRET instructions. Because its public `ECALL` behavior is
a host syscall, the test harness implements the bare-metal M-mode environment
call entry contract directly:

- save `mepc`;
- set `mcause = 11` for environment call from M-mode;
- clear `mtval`;
- update `mstatus.MPP/MPIE/MIE`;
- vector through `mtvec`.

The directed privileged tests compare:

- all integer registers;
- EBREAK PC;
- the full checked memory window;
- `mtvec`;
- `mepc`;
- `mcause`.

Covered operations include:

- `CSRRW`, `CSRRS`, and `CSRRC` on `mtvec`;
- `CSRRW` on `mepc`;
- `CSRRW`, `CSRRS`, and `CSRRC` on `mstatus`, `medeleg`, `mideleg`,
  `mcause`, and `mtval`;
- `CSRRWI`, `CSRRSI`, and `CSRRCI` on `mstatus`;
- bare-metal `ECALL`;
- `MRET`;
- return-PC advancement.

The legacy `mscratch` CSR read/write/set/clear test remains an internal-model
differential because Raven does not retain arbitrary scratch CSRs. Using a host
syscall oracle for that CSR would compare incompatible platform
contracts rather than prove RV32I RTL correctness.

## Remaining gaps

1. Compare instruction counts and trap-state metadata when an independent oracle
   exposes them.
2. Extend randomized load/store addresses beyond aligned offsets in one 256-byte
   page, including page crossings and address edges.
3. Extend independent privileged coverage beyond the CSR surface supported by
   the selected external executor (`mscratch`, `mie`, and `mip` require a second
   oracle or explicit bare-metal extension).
4. Repeat the same external-oracle methodology for i8086; its current reference
   is a separate repository-owned spec-derived implementation.

## Validation command

```text
cargo test -p eda-cpu-platform --test rv32i_instruction_matrix -- --nocapture
```

Latest full result: 5 tests, 5 passed, 0 failed, 789.03 seconds.

The full run includes the 64-seed randomized differential. The non-randomized
directed subset currently passes in 34.17 seconds:

```text
cargo test -p eda-cpu-platform --test rv32i_instruction_matrix -- --skip randomized

5 passed, 0 failed, 34.17 seconds
```

ELF/fixture lane:

```text
cargo test -p eda-cpu-platform --test program_fixtures -- --nocapture

7 passed, 0 failed, 9.40 seconds
```
