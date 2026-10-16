# CPU Program Execution and Validation Platform

This directory is the executable-centric bridge between compiled program images,
ISA simulation, SystemVerilog contract modules, and netlist LEC. It is under
active construction and must not yet be described as a complete commercial CPU
platform.

## Current capabilities

### RV32I

* ELF32 little-endian `PT_LOAD` loader.
* RV32I base integer instruction execution.
* Sparse 32-bit memory with wrapping byte/half/word access.
* Machine-mode CSR subset.
* Initial Linux ABI support for `write`, `brk`, anonymous `mmap`, `exit`, and
  `exit_group`; unsupported syscalls return `-ENOSYS`.
* Directed smoke validation for arithmetic edges, signed/unsigned comparisons,
  and negative load/store immediates.

### 8086

* DOS `.COM` loader with PSP command tail.
* MZ `.EXE` loader with relocations and initial CS/IP/SS/SP state.
* Broad real-mode instruction execution, including ALU, ModRM addressing,
  segment overrides, prefixes, stack, moves, string operations, shifts/rotates,
  multiply/divide, control transfer, interrupts, flags, ESC, and I/O opcodes.
* DOS INT 21h services needed by simple freestanding programs: character and
  string output, write, interrupt-vector installation, version query, and exit.
* Directed validation for registers, arithmetic carry, REP MOVSB, and MZ load.

The 8086 golden model is still being expanded and correlated against the complete
commercial 8086 opcode map. It is not yet the final full SystemVerilog core.

### SystemVerilog contract modules

* `rtl/rv32i_isa_decoder.sv` decodes the complete RV32I base instruction groups.
* `rtl/i8086_prefix_decoder.sv` decodes LOCK, REP/REPNE, and segment prefixes.
* `rtl/i8086_opcode_decoder.sv` classifies the primary 8086 opcode map,
  including ModRM, immediate-size, string, repeat, and control-flow properties.
* `rtl/i8086_modrm_decoder.sv` computes all 8086 base/index/direct ModRM
  addressing modes and default segment selection.
* `rtl/rv32i_full_core.sv` is a multicycle RV32I core with external
  instruction/data buses, machine-mode CSR/trap state, and all base-integer
  load/store/control/arithmetic instructions.
* `rtl/rv32i_full_soc_16.sv` provides a synthesizable 16-word unified program
  and data memory for directed simulation.
* `rtl/rv32i_full_tb_16.sv` provides a self-clocking simulation harness.
* `rtl/rv32i_full_runtime_tb.sv` exposes the core buses to the host runtime,
  which supplies sparse 32-bit instruction/data memory and captures stores at
  each rising edge.
* `rtl/i8086_full_core.sv` is a 20-bit real-mode core with prefix decode,
  ModRM addressing, ALU, BCD adjust, unary arithmetic, MUL/IMUL and a
  sequential DIV/IDIV engine, stack, register/data movement, string, control,
  far call/return, interrupt/IRET, external IRQ/NMI inputs, I/O, shift/rotate,
  and DOS-console execution state.
* `rtl/i8086_full_soc_64.sv` attaches the core to a 64-byte directed-program
  memory and exposes console output.
* `rtl/i8086_full_tb_64.sv` provides a self-clocking simulation harness.
* `rtl/i8086_full_runtime_tb.sv` exposes the core buses to the host runtime,
  which supplies sparse real-mode memory, captures writes, and records console
  output while the SystemVerilog core remains the architectural executor.

The ISA decoder modules and both full cores compile in the repository HDL
frontend, pass directed simulation where applicable, and pass the current
RTL-versus-mapped-netlist LEC flow. The RV32I core executes the checked-in
ELF code image and subword load/store tests. The i8086 core executes immediate
ALU, ModRM memory, MUL/DIV, AAM with arbitrary non-zero base, rotate, NOT, far
call/return, REP MOVSB, hardware interrupt/IRET, and DOS INT 21h Hello World
programs through its SystemVerilog SOC.

## Commands

```sh
cargo run -p eda-cpu-platform -- validate
cargo run -p eda-cpu-platform -- rv32i run program.elf
cargo run -p eda-cpu-platform -- rv32i run program.elf --sv
cargo run -p eda-cpu-platform -- i8086 run program.com
cargo run -p eda-cpu-platform -- i8086 run program.exe
cargo run -p eda-cpu-platform -- i8086 run program.com --sv

cargo run -p eda-cli -- flow platform/rtl/rv32i_isa_decoder.sv --top rv32i_isa_decoder
cargo run -p eda-cli -- flow platform/rtl/i8086_prefix_decoder.sv --top i8086_prefix_decoder
cargo run -p eda-cli -- flow platform/rtl/i8086_opcode_decoder.sv --top i8086_opcode_decoder
cargo run -p eda-cli -- flow platform/rtl/rv32i_full_core.sv --top rv32i_full_core
cargo run -p eda-cli -- flow platform/rtl/i8086_modrm_decoder.sv --top i8086_modrm_decoder
```

The standalone CLI command for `i8086_full_core.sv` is intentionally not shown
because that source must be concatenated with its prefix, opcode, and ModRM
decoder modules in the current single-file HDL frontend. Its regression test
performs that concatenation and passes LEC with explicit 512-track routing
capacity.

The `--sv` runners execute real images directly on the SystemVerilog cores by
attaching their external memory buses to sparse host memory. COM images may use
the complete 64 KiB load segment, and RV32I images may use sparse 32-bit ELF
addresses. The `high_memory.com` and `rv32i_high_memory.elf` fixtures exercise
instructions/data beyond the directed SOC capacities and are correlated with
the Rust golden models. The RV32I RTL does not implement Linux syscalls; an
ECALL is represented as architectural trap 11 with registers such as `a0`
retained.

## Remaining acceptance gates

1. Correlate the 8086 golden model and SystemVerilog core against every legal
   8088/8086 opcode, ModRM form, prefix, and exception combination. Directed
   tests cover representative classes, but full-row exhaustive coverage remains
   incomplete.
3. Add randomized instruction/concurrency traces and compare final architectural
   state against independent references.
4. Add independently written implementation/golden RTL pairs beyond the current
   RTL-versus-mapped-netlist LEC, with identical sequential-state naming and
   proof equivalence.
5. Run compiled program corpora through both the golden ISA model and the
   SystemVerilog cores.
6. Extend release regression to cover ELF relocations, DOS file I/O, and all
   documented exceptions.
