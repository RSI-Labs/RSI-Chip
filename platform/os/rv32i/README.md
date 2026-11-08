# MiniOS-RV32I

A small unikernel for the `platform/rtl/rv32i_full_core.sv` RV32I core,
executed by the event-driven `eda-sim` RTL simulator through
`Rv32iPlatformRunner`.

## What it demonstrates

- RTL reset/boot and stack setup
- `mtvec` trap-vector installation
- `ecall` trap entry, register-frame save/restore, and `mret` return
- syscall dispatch (`write`, `exit`, print-decimal)
- memory-mapped console and shutdown devices
- launching a user asm program at `0x1000`
- clean exit and simulator shutdown

The user program (`user.S`) prints a banner, computes `1+2+...+10`, prints the
result through a syscall, and exits. The kernel also self-checks decimal
formatting at boot.

## Memory map

| Address | Use |
|---|---|
| `0x0000_0000` | kernel image (flat binary entry) |
| `0x0000_1000` | user asm program |
| `0x0000_8000` | kernel stack top |
| `0x0000_9000` | user stack top |
| `0x1000_0000` | console TX (byte store) |
| `0x1000_0004` | shutdown (word store; low bits exit status) |
| `0x1000_0008` | console status (bit 0 always ready) |

## Syscall ABI

| `a7` | Call | Arguments | Result |
|---:|---|---|---|
| 64 | `write` | `a0=fd(1)`, `a1=buf`, `a2=len` | `a0=len` |
| 93/94 | `exit` | `a0=code` | shutdown |
| 1000 | `print_decimal` | `a0=value` | `a0=0` |

## Build

Requires LLVM with the RISC-V backend and `ld.lld`:

```sh
./platform/os/rv32i/build.sh
```

This produces `minios.elf` (debug/disassembly) and `minios.bin` (committed
flat image used by tests).

## Run

On the RTL platform through the main EDA CLI:

```sh
cargo run --release -p eda-cli -- sim platform/os/rv32i/minios.bin
cargo run --release -p eda-cli -- sim platform/os/rv32i/minios.bin --wave minios.vcd
```

Or through the CPU-platform binary:

```sh
cargo run --release -p eda-cpu-platform -- rv32i os platform/os/rv32i/minios.bin
```

Expected output:

```text
MiniOS-RV32I: boot
MiniOS-RV32I: console ready
MiniOS-RV32I: launching user asm program at 0x1000
user-asm: hello from the RV32I user program
user-asm: sum(1..10) = 55
MiniOS: shutdown code 0
```

The regression `crates/eda-cpu-platform/tests/rv32i_os.rs` also runs the same
image on the fast RV32I golden model with identical MMIO behavior and requires
the console bytes, exit code, PC, and all 32 architectural registers to match
the RTL run.
