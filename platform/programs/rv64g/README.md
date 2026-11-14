# RV64G conformance images

LLVM-assembled tests for the RV64G user-level ISS
(`crates/eda-cpu-platform/src/rv64g.rs`) and the RV64I RTL core
(`platform/rtl/rv64i_full_core.sv`).

## Build

Requires LLVM 20 with the RISC-V backend and `ld.lld`:

```sh
./build.sh
```

The script compiles each `src/*.S` with:

```text
--target=riscv64-unknown-elf -march=rv64g -mabi=lp64d
-ffreestanding -fno-builtin -fno-stack-protector -fno-pic
-mno-relax -msmall-data-limit=0 -nostdlib
```

and links with `platform/programs/rv64g/link.ld`, producing a flat `.bin`.

## Images

| Image | Covers | Runs on RTL |
|---|---|---|
| `rv64i.bin` | RV64I integer, 64-bit loads/stores, W-forms, branches, jumps | yes |
| `rv64m.bin` | M multiply/divide including corner cases | yes |
| `rv64a.bin` | A LR/SC and AMOs | yes |
| `rv64f.bin` | F single precision, conversions, flags, FMA | ISS only (P3 arithmetic) |
| `rv64f_ops.bin` | FP moves, sign injection, min/max, compares, FCLASS | yes |
| `rv64d.bin` | D double precision, conversions, flags, FMA | ISS only (P3) |
| `rv64csr.bin` | Zicsr/fflags/frm/fcsr/trap CSRs | yes |
| `rv64exit.bin` | ECALL with `mtvec == 0` exit-code path | yes |

Each program checks its own results and writes the failing check id to the
platform shutdown register (`0x1000_0004`); a clean run exits 0.

## Run

```sh
cargo test --release -p eda-cpu-platform --test rv64g_iss
cargo test --release -p eda-cpu-platform --test rv64i_rtl
cargo run --release -p eda-cli -- sim rv64i.bin --cpu rv64i
```
