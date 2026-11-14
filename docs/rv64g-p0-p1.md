# RV64G P0/P1: user-level ISS, RV64I RTL, LEC and simulation

Date: 2026-11-08
Status: **P0 user-level RV64G ISS complete; P1 RV64I RTL complete; P2 RTL M/A complete; P3 partial (FP register file and non-arithmetic F subset in RTL); P4-P6 pending.**
The full commercial desktop RV64G CPU (P2-P6) is **not** complete and cannot be
claimed from the current environment; see the scope table at the end.

## 1. P0: RV64G user-level ISS

Implementation: `crates/eda-cpu-platform/src/rv64g.rs`

Supported user-level ISA:

| Extension | Coverage |
|---|---|
| RV64I | 64-bit GPR/PC, LD/SD, LWU, ADDIW/ADDW/SUBW/SLLW/SRLW/SRAW, 6-bit shifts, all loads/stores/branches/jumps, LUI/AUIPC sign extension |
| M | MUL/MULH/MULHSU/MULHU, DIV/DIVU/REM/REMU, all W forms, divide-by-zero and signed-overflow corner cases |
| A | LR/SC (.W/.D), AMOADD/SWAP/XOR/AND/OR/MIN/MAX/MINU/MAXU (.W/.D), sign-extended 32-bit results |
| F | add/sub/mul/div/sqrt, min/max, sign injection, compares, FCVT/FMV/FCLASS, FMA family, NaN boxing, `fflags` |
| D | same operation set in double precision, FCVT.S.D/D.S, FMV.X.D/D.X |
| Zicsr | fflags/frm/fcsr, mstatus, misa, mtvec, mscratch, mepc/mcause/mtval/mip, cycle/time/instret, CSRRS/CSRRC/immediate forms |
| Zifencei | FENCE/FENCE.I treated as ordering no-ops in the sequential ISS |

Platform MMIO matches the RV32I platform: console TX `0x1000_0000`,
shutdown `0x1000_0004`, status `0x1000_0008`. The Linux-like `ecall` ABI
(`a7=64` write, `a7=93/94` exit) is supported as well.

### Committed tests

LLVM 20 RISC-V sources: `platform/programs/rv64g/src/*.S`
Rebuild: `platform/programs/rv64g/build.sh`
Binaries are committed under `platform/programs/rv64g/*.bin`.

| Program | Checks |
|---|---:|
| `rv64i.S` | 43 integer checks |
| `rv64m.S` | 30 multiply/divide checks |
| `rv64a.S` | 29 atomic/reservation checks |
| `rv64f.S` | 41 single-precision checks |
| `rv64d.S` | 42 double-precision checks |
| `rv64csr.S` | 19 CSR checks |

Regression:

```sh
cargo test --release -p eda-cpu-platform --test rv64g_iss
# 7 passed (six ISA images + sparse-memory widths)
```

## 2. P1/P2/P3(partial): RV64IMA + F-subset RTL

Files:

- `platform/rtl/gen_rv64i_core.py` - generator
- `platform/rtl/rv64i_full_core.sv` - generated core (644 lines)
- `platform/rtl/rv64i_full_runtime_tb.sv` - simulation harness
- `crates/eda-cpu-platform/src/systemverilog.rs` - `Rv64iPlatformRunner`

The core is a multicycle RV64IMA machine with an FP register file: 64-bit GPRs/PC, external 64-bit
instruction/data buses, byte/half/word/dword loads and stores, W-form ALU
operations, 6-bit shifts, RV64M multiply/divide (sequential 64-bit shift-add /
shift-subtract units), LR/SC and all AMO operations (atomic read-modify-write
state), a 32x64-bit FP register file, FP moves/sign-injection/min-max/compares/FCLASS,
machine-mode CSRs, ecall/ebreak traps, and the same
MMIO platform devices as the RV32I core.

### LEC evidence

Netlist scale after adding RV64M, RV64A, and the FP register file / F subset:
reference 283,488 cells, mapped 225,046 cells, 5,757 flops, 647 output bits.

Independent frontend (slang-vs-eda-hdl) on the final RV64IM netlist:

```text
rv64i_full_core: independent slang-vs-hdl equivalent=true
slang_cells=282647 hdl_cells=225046
proved all 5757 flop next-state functions and 647 output bits
coverage: gate 5757 flops + 219289 combinational cells in proof cones (0 dangling)
elapsed=1193.6s
```

RTL-vs-mapped equivalence is executed with the strict no-cut parallel engine
(the localized-cut proof of this larger netlist hit a hard SAT obligation).
The strict result is recorded below once the run completes.

Strict no-internal-cut LEC (every flop next-state function and output bit
proven over its complete combinational fan-in cone):

```text
rv64i_full_core: strict equivalent=true reference_cells=283488 mapped_cells=225046
strict LEC ran 10 parallel workers
proved all 5757 flop next-state functions and 647 output bits
coverage: gate 5757 flops + 219289 combinational cells in proof cones (0 dangling)
strict full-circuit LEC: 0 internal cuts
elapsed=1491.3s
```

Reproduce:

```sh
cargo test --release -p eda-tests --test rv64i_rtl_lec -- --test-threads=1 --nocapture
SLANG_BIN=/tmp/slang-build/bin/slang \
  cargo test --release -p eda-tests --test rv64i_rtl_lec \
  rv64i_full_core_independent_frontend_lec -- --nocapture
```

### Simulation evidence

The same committed images run on the RTL platform and produce
register-for-register identical architectural state to the ISS:

```text
rv64i:  rtl exit=Some(0) halted=false trap=false inst=226 cycles=453
        rtl regs == ISS regs (all 31 registers)
rv64csr: rtl exit=Some(0) halted=false trap=false inst=88 cycles=177
        rtl regs == ISS regs (all 31 registers)
rv64m:  rtl exit=Some(0) halted=false trap=false inst=137 cycles=2131
        rtl regs == ISS regs (all 31 registers)
rv64a:  rtl exit=Some(0) halted=false trap=false inst=145 cycles=306
        rtl regs == ISS regs (all 31 registers)
rv64f_ops: rtl exit=Some(0) halted=false trap=false inst=82 cycles=165
        rtl regs == ISS regs (all 31 registers)
```

The ECALL trap path is covered by `rv64exit.bin`: with `mtvec == 0` the core
halts with `mcause = 11` and the platform maps `a0` to the process exit code
(exit 7).

VCD capture is exercised by `rv64_rtl_run_can_emit_a_vcd_waveform`.

CLI:

```sh
cargo run --release -p eda-cli -- sim platform/programs/rv64g/rv64i.bin --cpu rv64i
# sim rv64i: exit=Some(0) ... instructions=226 cycles=453
```

Other RTL bugs found by these tests and fixed:

1. `fflags`/`frm`/`fcsr` had been independent registers; they are now aliases
   of one `fcsr` register as required.
2. The first RTL draft had same-process combinational feedback on
   `csr_legal_c`/`wresult_c`; the logic is now split into separate processes
   and sign-extension wires, which the event simulator accepts.
3. The synthesis/simulation flow does not support 128-bit multiplication or
   division/modulo operators, so RV64M is implemented as a 64-cycle shift-add
   multiplier using two 64-bit product halves and a 64-cycle shift-subtract
   divider with a 64-bit remainder plus an explicit shifted MSB.
4. RV64A uses a dedicated atomic read-modify-write state: EXECUTE performs the
   read and latches the old value, S_AMO performs the conditional write and
   retires the instruction; LR/SC reservations are tracked explicitly and
   invalidated by matching stores.

## 3. Known limitations of the current RV64I RTL

- Naturally aligned multi-byte data accesses are assumed; misaligned-access
  exceptions are not implemented yet.
- Machine mode only; no S/U mode, interrupts, MMU, TLB, or cache.
- `EBREAK` terminates the core regardless of `mtvec`.
- No RTL M/A/F/D execution yet; those extensions exist only in the ISS.
- The multicycle core is not pipelined and has no branch prediction or caches.

## 4. Scope boundary

What exists now is a verified RV64I core and a verified user-level RV64G ISS.
It is **not** a commercial desktop CPU:

| Capability | Current | Commercial desktop RV64G |
|---|---|---|
| RTL ISA | RV64IMA + F subset + Zicsr | RV64G + privilege + MMU |
| RTL M/A/F/D | M/A + non-arithmetic F; arithmetic F/D ISS only | required in RTL |
| Privilege | M-mode subset | M/S/U, delegation, interrupts |
| MMU | none | Sv39+ |
| Pipeline/caches | multicycle, no cache | pipelined, L1/L2, coherence |
| Desktop I/O | UART/shutdown MMIO | DDR/PCIe/display/PLIC/CLINT |
| Multi-core | no | SMP |
| LEC scale | 70,583 mapped cells | millions of gates + commercial tools |
| PDK/signoff | synthetic library | certified PDK, DRC/LVS/STA/DFT, tapeout |

P2 (M/A RTL), P3 (F/D RTL), P4 (privilege/MMU), P5 (desktop SoC), and P6
(commercial signoff) remain open and require the external resources listed in
`docs/rv64g-desktop-feasibility.md`.
