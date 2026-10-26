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

## Privileged subset status

The CSR and trap tests still compare RTL against `Rv32iMachine`. They cover:

- `CSRRW`, `CSRRS`, and `CSRRC` on `mscratch`;
- `mtvec` and `mepc`;
- `ECALL`;
- `MRET`;
- trap return-PC advancement.

They are not yet counted as independent closure. The selected external oracle
models `ECALL` as a host syscall and does not retain arbitrary machine CSRs such
as `mscratch`, so using it for that test would compare incompatible platform
contracts rather than prove RV32I RTL correctness.

## Remaining gaps

1. Add an independent oracle lane for the machine-CSR/trap contract, or select a
   second oracle that implements bare-metal `ECALL`, `MRET`, and arbitrary CSRs.
2. Compare instruction counts and trap-state metadata when an independent oracle
   exposes them.
3. Extend randomized load/store addresses beyond aligned offsets in one 256-byte
   page, including page crossings and address edges.
4. Add independent ELF and high-memory image execution.
5. Repeat the same three-lane methodology for i8086; its current matrix still
   uses the repository model as golden.

## Validation command

```text
cargo test -p eda-cpu-platform --test rv32i_instruction_matrix -- --nocapture
```

Latest result: 4 passed, 0 failed, 807.55 seconds.
