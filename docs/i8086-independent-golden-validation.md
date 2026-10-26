# i8086 independent golden validation

This record describes the current machine-code differential evidence for the
8086 execution lanes. It is scoped to the tested integer, data-movement,
ModRM, flag, shift/rotate, and unary groups; it does not claim a complete
8086 instruction set, interrupt controller, peripheral, DOS/BIOS, or ELF/MZ
loader oracle.

## Lanes

| Lane | Implementation | Role |
| --- | --- | --- |
| Built-in model | `eda-cpu-platform::i8086::I8086Machine` | Repository behavioral reference. |
| RTL runtime | `I8086SvRunner` over `platform/rtl/i8086_full_core.sv` | Machine-code decoder, core, real-mode memory, flags, and architectural state under test. |
| Independent reference | `ReferenceI8086` in the integration-test support module | A separate spec-derived model with a flat one-megabyte memory representation and independent decode helpers. It does not call `I8086Machine`. |

The reference is internal to this repository rather than a third-party crate.
It is nevertheless an independent implementation lane: it has a different memory
representation, decoder, register-access helpers, ALU implementation, and shift
implementation. Existing third-party candidates were rejected for this lane
because they were incomplete executors, required SDL at link time, used an
incompatible license, or executed assembly ASTs instead of machine code.

## Directed matrices

The directed lane compares:

- all eight general registers;
- all four segment registers where the reference observes them;
- IP;
- FLAGS;
- retired instruction count;
- every byte in the one-megabyte real-mode address space.

Covered machine-code families include:

| Family | Covered behavior |
| --- | --- |
| Segment movement and overrides | `MOV sreg`, default DS/SS selection, and ES/CS/SS/DS override prefixes. |
| Primary ALU | ADD, OR, ADC, SBB, AND, SUB, XOR, CMP in byte and word r/m-register and register-r/m directions. |
| ModRM addressing | mod 00/01/10/11, all r/m base/index forms, direct addressing, signed byte displacement, and word displacement. |
| Immediate groups | 0x80/0x81/0x83 forms with byte, word, and sign-extended byte immediates. |
| Data movement | Register-to-register/memory and memory-to-register MOV in byte and word forms, including accumulator direct forms. |
| Variable shifts/rotates | ROL, ROR, RCL, RCR, SHL, and SAR by CL. |
| Immediate shifts/rotates | 0xc0/0xc1 forms, including counts 0, 1, 7, 8, 9, 15, 16, 17, 30, and 31. |
| Unary groups | TEST, NOT, NEG, MUL, IMUL, DIV, and IDIV in byte and word forms where defined by the directed operands. |
| Flag control | CMC, CLC, STC, CLI, STI, CLD, STD, and LAHF as exercised by the matrices. |
| Stack and control flow | PUSH/POP GPRs and segment registers, PUSHF/POPF, near CALL/RET, direct far CALL, RETF, RETF imm16, all short conditional-jump predicates, JCXZ, LOOP, short JMP, and near JMP. |
| Strings and repeat prefixes | Byte/word MOVSB/MOVSW, CMPSB/CMPSW, STOSB/STOSW, LODSB/LODSW, and SCASB/SCASW; REP MOVSW/STOSW/LODSW; REPNE SCASW; forward and DF=1 reverse string operation; CX=0 zero-iteration behavior. |

The control-flow lane also uses forward branches over an ALU mutation so both
taken and untaken paths remain architectural and observable.

Directed result:

```text
cargo test -p eda-cpu-platform --test i8086_opcode_matrix -- --skip randomized

19 passed, 0 failed, 297.46 seconds
```

## Deterministic randomized differential

Properties:

| Property | Evidence |
| --- | ---: |
| Seeds | 32 |
| Generated instructions per seed | 40 |
| Byte comparison window per seed | `0x0000_0f00..0x0000_11ff` |

Each seed mixes:

- byte and word primary ALU operations;
- immediate ALU groups;
- immediate and CL-controlled shifts/rotates;
- byte and word TEST/NOT/NEG/MUL/IMUL/DIV/IDIV forms;
- MOV loads/stores through ModRM memory forms;
- segment override prefixes;
- mod 01 direct/displacement and mod 10 displacement forms.

The reference and built-in model execute instruction-by-instruction. On every
step they compare architectural registers, segment registers, IP, and FLAGS;
the first divergence reports the original instruction address and bytes. Final
comparison also checks retired instruction count and the full data window. RTL
then executes the original image and is compared with the built-in result.

Fuzz chunk results:

| Seeds | Result | Time |
| ---: | --- | ---: |
| 0–7 | 1 passed, 0 failed | 628.30 s |
| 8–15 | 1 passed, 0 failed | 633.55 s |
| 16–23 | 1 passed, 0 failed | 604.87 s |
| 24–31 | 1 passed, 0 failed | 623.33 s |

## Defects exposed and fixed

The independent lane exposed the following machine-code correctness defects:

1. Word-primary ALU opcode forms 01h and 03h reversed the r/m and register
   destination/source roles in both the behavioral model and RTL.
2. SBB did not subtract the incoming carry in the behavioral model, primary RTL
   path, or immediate-group RTL path.
3. The RTL ADC carry expression truncated its 17-bit sum before comparison.
4. Behavioral-model NEG accidentally used SBB semantics and consumed incoming
   CF.
5. Logical and TEST AF behavior differed across lanes; it is now deterministically
   cleared because AF is undefined for those operations.
6. Undefined rotate SF/ZF/PF behavior differed across lanes; the model, RTL, and
   reference now leave those flags unchanged and compare only the defined CF and
   count-one OF behavior.
7. REP-prefixed string instructions had two termination defects. The behavioral
   model executed once with CX=0 and wrapped CX to 0xffff, while RTL performed
   one additional iteration after the requested nonzero count. Both lanes now
   execute zero iterations at CX=0 and exactly CX iterations for non-comparison
   strings.
8. Direct far CALL pushed the newly selected target CS instead of the caller's
   CS, so RETF returned to the target routine rather than the caller and could
   loop until the event budget expired. RTL now saves both caller CS and IP.

## Remaining gaps

1. Add a full external machine-code oracle or a broader spec-derived reference
   for opcodes outside the current matrices.
2. Add interrupts, I/O, BCD adjust, indirect far calls, and full MZ/COM loader
   coverage.
3. Compare instruction timings only after defining an authoritative 8086 timing
   source; current tests compare retired instruction count, not cycles.
4. Extend memory comparisons beyond the directed one-megabyte space and fuzz
   window, including aliased 20-bit wrap edge cases.
