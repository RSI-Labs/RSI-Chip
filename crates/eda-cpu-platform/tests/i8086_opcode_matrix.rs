use eda_cpu_platform::i8086::{I8086Machine, I8086Stop};
use eda_cpu_platform::systemverilog::{I8086SvResult, I8086SvRunner};
use std::sync::OnceLock;

mod i8086_support;

use i8086_support::ReferenceI8086;

fn shared_i8086_runner() -> &'static I8086SvRunner {
    static RUNNER: OnceLock<I8086SvRunner> = OnceLock::new();
    RUNNER.get_or_init(|| I8086SvRunner::new().expect("compile shared i8086 SystemVerilog runtime"))
}

fn initial_program(values: [u16; 8], operations: &[u8]) -> Vec<u8> {
    let mut executable: Vec<u8> = values
        .iter()
        .enumerate()
        .flat_map(|(register, value)| [0xb8 + register as u8, *value as u8, (value >> 8) as u8])
        .collect();
    executable.extend_from_slice(operations);
    executable.push(0xf4);
    executable
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn next_u32(&mut self) -> u32 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value as u32
    }

    fn below(&mut self, limit: u32) -> u32 {
        self.next_u32() % limit
    }
}

fn assert_matches_golden(executable: &[u8], context: &str) -> I8086SvResult {
    assert_matches_golden_with_options(executable, context, true, 200_000)
}

fn assert_matches_golden_with_event_budget(
    executable: &[u8],
    context: &str,
    event_budget: u64,
) -> I8086SvResult {
    assert_matches_golden_with_options(executable, context, true, event_budget)
}

fn assert_matches_golden_with_options(
    executable: &[u8],
    context: &str,
    compare_flags: bool,
    event_budget: u64,
) -> I8086SvResult {
    let mut golden = I8086Machine::com(executable, "")
        .unwrap_or_else(|error| panic!("construct golden {context} program: {error}"));
    for address in [0x1000_u32, 0x1001, 0x1010, 0x1011, 0x1020, 0x1021] {
        golden.memory.write_u8(address, address as u8 ^ 0xa5);
    }
    for address in 0x2000_u32..0x2040 {
        golden.memory.write_u8(address, address as u8 ^ 0x5a);
    }
    for address in 0x2100_u32..0x2140 {
        golden.memory.write_u8(address, address as u8 ^ 0x3c);
    }
    for (address, value) in [
        (0x10100_u32, 0x11_u8),
        (0x10200, 0x21),
        (0x30100, 0x31),
        (0x30200, 0x41),
        (0x40100, 0x51),
        (0x40200, 0x61),
    ] {
        golden.memory.write_u8(address, value);
    }
    let initial_memory = golden.memory.clone();
    assert_eq!(
        golden.run(1000),
        I8086Stop::Halt,
        "{context}: golden model did not halt"
    );

    let mut reference = ReferenceI8086::com(&initial_memory);
    reference
        .run(1000)
        .unwrap_or_else(|error| panic!("execute {context} in independent reference: {error}"));
    assert_eq!(
        reference.registers(),
        golden.regs,
        "{context}: independent architectural registers differ"
    );
    assert_eq!(
        reference.segments(),
        golden.segments,
        "{context}: independent segment registers differ"
    );
    assert_eq!(
        reference.instruction_pointer(),
        golden.ip,
        "{context}: independent IP differs"
    );
    assert_eq!(
        reference.flags(),
        golden.flags,
        "{context}: independent FLAGS differ"
    );
    assert_eq!(
        reference.retired_instructions(),
        golden.instructions_retired,
        "{context}: independent instruction count differs"
    );
    for address in 0..0x10_0000_u32 {
        assert_eq!(
            reference.memory_byte(address),
            golden.memory.read_u8(address),
            "{context}: independent memory differs at {address:#06x}"
        );
    }

    let rtl = shared_i8086_runner()
        .run_memory_with_budget(initial_memory, event_budget)
        .unwrap_or_else(|error| panic!("execute {context} in RTL: {error}"));
    assert!(!rtl.trap, "{context}: RTL trapped");
    assert_eq!(rtl.ip, golden.ip, "{context}: IP differs");
    if compare_flags {
        assert_eq!(rtl.flags, golden.flags, "{context}: FLAGS differ");
    }
    let actual_registers = [
        rtl.ax, rtl.cx, rtl.dx, rtl.bx, rtl.sp, rtl.bp, rtl.si, rtl.di,
    ];
    assert_eq!(
        actual_registers, golden.regs,
        "{context}: architectural registers differ"
    );
    for address in [0x1000_u32, 0x1001, 0x1010, 0x1011, 0x1020, 0x1021, 0xfffe] {
        assert_eq!(
            rtl.memory.read_le_u16(address),
            golden.memory.read_le_u16(address),
            "{context}: memory {address:#06x} differs"
        );
    }
    if context.contains("unary arithmetic matrix") {
        for address in 0x3200_u32..0x3240 {
            assert_eq!(
                rtl.memory.read_le_u16(address),
                golden.memory.read_le_u16(address),
                "{context}: unary log differs at {address:#06x}"
            );
        }
    }
    rtl
}

#[test]
fn deterministic_i8086_segment_override_matrix_matches_golden_model() {
    let values = [
        0x1000_u16, 0x5678, 0x3000, 0x4000, 0xfffe, 0x0000, 0x0200, 0x0300,
    ];
    let mut operations = vec![
        0x8e, 0xc0, // mov es, ax
        0x8e, 0xd2, // mov ss, dx
        0x8e, 0xdb, // mov ds, bx
        0xbb, 0x00, 0x01, // mov bx, 0x0100
    ];
    let loads: &[(Vec<u8>, u8)] = &[
        (vec![0x8a, 0x07], 0x51),       // default DS:[BX]
        (vec![0x26, 0x8a, 0x07], 0x11), // ES:[BX]
        (vec![0x36, 0x8a, 0x07], 0x31), // SS:[BX]
        (vec![0x3e, 0x8a, 0x07], 0x51), // explicit DS:[BX]
        (vec![0x8a, 0x04], 0x61),       // default DS:[SI]
        (vec![0x26, 0x8a, 0x04], 0x21), // ES:[SI]
        (vec![0x36, 0x8a, 0x04], 0x41), // SS:[SI]
    ];
    for (index, (load, _expected)) in loads.iter().enumerate() {
        operations.extend_from_slice(load);
        let log_address = 0x3100_u16 + index as u16;
        operations.extend_from_slice(&[0xa2, log_address as u8, (log_address >> 8) as u8]);
    }

    let rtl = assert_matches_golden(
        &initial_program(values, &operations),
        "segment override matrix",
    );
    for (index, (_load, expected)) in loads.iter().enumerate() {
        let address = 0x43100_u32 + index as u32;
        assert_eq!(
            rtl.memory.read_u8(address),
            *expected,
            "segment override log byte at {address:#06x} differs"
        );
    }
}

#[test]
fn deterministic_i8086_primary_word_alu_matrix_matches_golden_model() {
    let values = [
        0x7c5b_u16, 0x3ba9, 0x5e6d, 0x2492, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    let operations = (0..8_u8)
        .flat_map(|operation| [operation * 8 + 1, 0xc0 + operation])
        .collect::<Vec<_>>();
    assert_matches_golden(&initial_program(values, &operations), "word ALU matrix");
}

#[test]
fn deterministic_i8086_stack_call_return_matrix_matches_golden_model() {
    let values = [
        0x1234_u16, 0x5678, 0x9abc, 0x1000, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    let operations = [
        0x50, // push ax
        0x51, // push cx
        0x5a, // pop dx
        0x5b, // pop bx
        0x9c, // pushf
        0x32, 0xc0, // xor al, al
        0x9d, // popf
        0xe8, 0x05, 0x00, // call subroutine
        0xb3, 0x2a, // mov bl, 42
        0xeb, 0x04, // jump over subroutine
        0x90, 0x50, // push ax
        0x5f, // pop di
        0xc3, // ret
    ];
    let rtl = assert_matches_golden(
        &initial_program(values, &operations),
        "stack/call/return matrix",
    );
    assert_eq!(rtl.dx, 0x5678);
    assert_eq!(rtl.bx, 0x122a);
    assert_eq!(rtl.di, 0x1200);
    assert_eq!(rtl.sp, 0xfffe);
}

#[test]
fn deterministic_i8086_control_flow_matrix_matches_golden_model() {
    let values = [0x0000_u16, 0x0002, 0x0000, 0x0000, 0xfffe, 0, 0, 0];
    let mut operations = Vec::new();
    for operation in 0..16_u8 {
        operations.extend_from_slice(&[0x70 + operation, 0x03, 0x04, 0x01, 0x90]);
    }
    operations.extend_from_slice(&[
        0xb9, 0x02, 0x00, // mov cx, 2
        0xe3, 0x03, 0x04, 0x01, 0x90, // jcxz forward
        0xb9, 0x02, 0x00, // mov cx, 2
        0xe2, 0x03, 0x04, 0x01, 0x90, // loop forward
        0xeb, 0x03, 0x04, 0x01, 0x90, // short jmp
        0xe9, 0x03, 0x00, 0x04, 0x01, 0x90, // near jmp
    ]);
    assert_matches_golden(&initial_program(values, &operations), "control-flow matrix");
}

#[test]
fn deterministic_i8086_string_and_repeat_matrix_matches_golden_model() {
    let values = [
        0x1234_u16, 0x0000, 0x0000, 0x0000, 0xfffe, 0, 0x2000, 0x2100,
    ];

    let single_operations = [
        0xa4, // movsb
        0xa5, // movsw
        0xbe, 0x00, 0x20, // mov si, 0x2000
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xa6, // cmpsb
        0xa7, // cmpsw
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xaa, // stosb
        0xab, // stosw
        0xbe, 0x00, 0x20, // mov si, 0x2000
        0xac, // lodsb
        0xad, // lodsw
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xae, // scasb
        0xaf, // scasw
    ];

    let repeat_move_operations = [
        0xbe, 0x00, 0x20, // mov si, 0x2000
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xb9, 0x04, 0x00, // mov cx, 4
        0xf3, 0xa5, // rep movsw
    ];

    let repeat_store_operations = [
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xb9, 0x04, 0x00, // mov cx, 4
        0xf3, 0xab, // rep stosw
    ];

    let repeat_load_operations = [
        0xbe, 0x00, 0x20, // mov si, 0x2000
        0xb9, 0x04, 0x00, // mov cx, 4
        0xf3, 0xad, // rep lodsw
    ];

    let repeat_scan_operations = [
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xb9, 0x04, 0x00, // mov cx, 4
        0xf2, 0xaf, // repne scasw
    ];

    let zero_repeat_operations = [
        0xbe, 0x00, 0x20, // mov si, 0x2000
        0xbf, 0x00, 0x21, // mov di, 0x2100
        0xb9, 0x00, 0x00, // mov cx, 0
        0xf3, 0xa5, // rep movsw must execute zero iterations
        0xf3, 0xab, // rep stosw must execute zero iterations
        0xf3, 0xad, // rep lodsw must execute zero iterations
        0xf2, 0xaf, // repne scasw must execute zero iterations
    ];

    let reverse_operations = [
        0xfc, // cld
        0xbe, 0x08, 0x20, // mov si, 0x2008
        0xbf, 0x08, 0x21, // mov di, 0x2108
        0xb9, 0x04, 0x00, // mov cx, 4
        0xfd, // std
        0xf3, 0xa5, // rep movsw backwards
        0xfc, // cld
    ];

    assert_matches_golden(
        &initial_program(values, &single_operations),
        "single string matrix",
    );
    assert_matches_golden(
        &initial_program(values, &repeat_move_operations),
        "repeat string move matrix",
    );
    assert_matches_golden(
        &initial_program(values, &repeat_store_operations),
        "repeat string store matrix",
    );
    assert_matches_golden(
        &initial_program(values, &repeat_load_operations),
        "repeat string load matrix",
    );
    assert_matches_golden(
        &initial_program(values, &repeat_scan_operations),
        "repeat string scan matrix",
    );
    assert_matches_golden(
        &initial_program(values, &zero_repeat_operations),
        "zero-count repeat string matrix",
    );
    assert_matches_golden(
        &initial_program(values, &reverse_operations),
        "reverse string matrix",
    );
}

#[test]
fn deterministic_i8086_modrm_addressing_matrix_matches_golden_model() {
    let values = [
        0x1234_u16, 0x5678, 0x9abc, 0x1000, 0xfffe, 0x1100, 0x1010, 0x1020,
    ];
    let mut operations = Vec::new();
    let mut log_address = 0x3000_u16;
    let mut emit = |operations: &mut Vec<u8>, modrm: u8, extra: &[u8]| {
        operations.extend_from_slice(&[0x8a, modrm]);
        operations.extend_from_slice(extra);
        operations.extend_from_slice(&[0xa2, log_address as u8, (log_address >> 8) as u8]);
        log_address += 1;
    };

    // mod=00: no displacement, direct address for rm=110.
    for rm in 0_u8..8 {
        let extra: &[u8] = if rm == 6 { &[0x30, 0x13] } else { &[] };
        emit(&mut operations, rm, extra);
    }

    // mod=01: signed/eight-bit displacement around every base/index pair.
    for rm in 0_u8..8 {
        emit(&mut operations, 0x40 | rm, &[0x20]);
    }

    // mod=10: sixteen-bit displacement around every base/index pair.
    for rm in 0_u8..8 {
        emit(&mut operations, 0x80 | rm, &[0x30, 0x11]);
    }

    // mod=11: register-to-register moves.
    for rm in 0_u8..8 {
        emit(&mut operations, 0xc0 | rm, &[]);
    }

    let rtl = assert_matches_golden(
        &initial_program(values, &operations),
        "ModRM addressing matrix",
    );
    let expected_log = [
        0x4a, 0x7a, 0x2c, 0x1c, 0xb5, 0x85, 0x00, 0xa5, // mod=00
        0x6a, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x85, // mod=01
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, // mod=10
        0x0c, 0x78, 0xbc, 0x00, 0x12, 0x56, 0x9a, 0x10, // mod=11
    ];
    for (index, expected) in expected_log.into_iter().enumerate() {
        let address = 0x3000_u32 + index as u32;
        assert_eq!(
            rtl.memory.read_u8(address),
            expected,
            "ModRM log byte at {address:#06x} differs"
        );
    }
}

#[test]
fn deterministic_i8086_primary_byte_alu_matrix_matches_golden_model() {
    let values = [
        0x7c5b_u16, 0x3ba9, 0x5e6d, 0x2492, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    let operations = (0..8_u8)
        .flat_map(|operation| [operation * 8, 0xc0 + operation])
        .collect::<Vec<_>>();
    assert_matches_golden(&initial_program(values, &operations), "byte ALU matrix");
}

#[test]
fn deterministic_i8086_immediate_group_matrix_matches_golden_model() {
    let values = [
        0x7c5b_u16, 0x3ba9, 0x5e6d, 0x2492, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    let mut operations = Vec::new();
    for operation in 0..8_u8 {
        let modrm = 0xc0 + operation + (operation << 3);
        operations.extend_from_slice(&[0x81, modrm, 0xa9, 0x3b]);
    }
    for operation in 0..8_u8 {
        let modrm = 0xc0 + operation + (operation << 3);
        operations.extend_from_slice(&[0x83, modrm, 0xfe]);
    }
    assert_matches_golden(
        &initial_program(values, &operations),
        "immediate-group matrix",
    );
}

#[test]
fn immediate_subtraction_ignores_incoming_carry() {
    let program = [
        0xb8, 0xa2, 0x42, // MOV AX, 0x42a2
        0xf9, // STC
        0x80, 0xec, 0x17, // SUB AH, 0x17
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "immediate SUB after STC");
    assert_eq!(rtl.ax, 0x2ba2);
}

#[test]
fn register_subtraction_ignores_incoming_carry() {
    let program = [
        0xb9, 0x00, 0x00, // MOV CX, 0
        0xbb, 0x00, 0x10, // MOV BX, 0x1000
        0xf9, // STC
        0x28, 0xd9, // SUB CL, BL
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "register SUB after STC");
    assert_eq!(rtl.cx, 0);
    assert_eq!(rtl.flags & 0x0001, 0);
}

#[test]
fn deterministic_i8086_variable_shift_rotate_matrix_matches_golden_model() {
    let legal_operations = [0_u8, 1, 2, 3, 4, 5, 7];
    let values = [
        0x7c5b_u16, 0x0003, 0x5e6d, 0x2492, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];

    let byte_operations = legal_operations
        .iter()
        .flat_map(|operation| [0xd2, 0xc0 + operation * 8])
        .collect::<Vec<_>>();
    assert_matches_golden(
        &initial_program(values, &byte_operations),
        "byte variable shift/rotate matrix",
    );

    let word_operations = legal_operations
        .iter()
        .flat_map(|operation| [0xd3, 0xc0 + operation * 8])
        .collect::<Vec<_>>();
    assert_matches_golden(
        &initial_program(values, &word_operations),
        "word variable shift/rotate matrix",
    );
}

#[test]
fn deterministic_i8086_immediate_shift_rotate_matrix_matches_golden_model() {
    let legal_operations = [0_u8, 1, 2, 3, 4, 5, 7];
    let values = [
        0x7c5b_u16, 0x3ba9, 0x5e6d, 0x2492, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];

    let byte_operations = legal_operations
        .iter()
        .flat_map(|operation| [0xc0, 0xc0 + operation * 8, 0x03])
        .collect::<Vec<_>>();
    assert_matches_golden(
        &initial_program(values, &byte_operations),
        "byte immediate shift/rotate matrix",
    );

    let word_operations = legal_operations
        .iter()
        .flat_map(|operation| [0xc1, 0xc0 + operation * 8, 0x03])
        .collect::<Vec<_>>();
    assert_matches_golden(
        &initial_program(values, &word_operations),
        "word immediate shift/rotate matrix",
    );
}

#[test]
fn deterministic_i8086_immediate_shift_count_boundary_matrix_matches_golden_model() {
    let values = [
        0x7c5b_u16, 0xa53b, 0x5e6d, 0xc492, 0xfffe, 0x3000, 0x102a, 0x0040,
    ];
    let mut operations = Vec::new();
    let counts = [0_u8, 1, 7, 8, 9, 15, 16, 17, 30, 31];
    for opcode in [0xc0_u8, 0xc1] {
        for operation in 0_u8..8 {
            for count in counts {
                operations.extend_from_slice(&[opcode, 0xc0 | (operation << 3), count]);
            }
        }
    }
    assert_matches_golden_with_event_budget(
        &initial_program(values, &operations),
        "immediate shift/rotate count boundaries",
        1_000_000,
    );
}

#[test]
fn zero_count_shift_preserves_flags() {
    let program = [
        0xbb, 0x34, 0x12, // MOV BX, 0x1234
        0xb9, 0x00, 0x00, // MOV CX, 0
        0xf9, // STC
        0xd2, 0xff, // SAR BH, CL
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "zero-count shift flags");
    assert_eq!(rtl.flags & 0x0001, 1);
    assert_eq!(rtl.bx, 0x1234);
}

#[test]
fn wide_left_shift_carries_become_zero_after_operand_width() {
    let program = [
        0xbc, 0xfe, 0xff, // MOV SP, 0xfffe
        0xb9, 0x1e, 0x00, // MOV CX, 30
        0xd3, 0xe4, // SHL SP, CL
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "wide SHL carry");
    assert_eq!(rtl.sp, 0);
    assert_eq!(rtl.flags & 0x0001, 0);
}

#[test]
fn wide_arithmetic_shift_carries_sign_after_operand_width() {
    let program = [
        0xbe, 0x84, 0xb2, // MOV SI, 0xb284
        0xb9, 0x1d, 0x00, // MOV CX, 29
        0xd3, 0xfe, // SAR SI, CL
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "wide SAR carry");
    assert_eq!(rtl.si, 0xffff);
    assert_eq!(rtl.flags & 0x0001, 1);
}

#[test]
fn word_rotate_right_through_carry_reports_the_shifted_out_bit() {
    let program = [
        0xf9, // STC
        0xbc, 0xfe, 0xff, // MOV SP, 0xfffe
        0xc1, 0xdc, 0x01, // RCR SP, 1
        0xf4, // HLT
    ];
    let rtl = assert_matches_golden(&program, "word RCR carry");
    assert_eq!(rtl.sp, 0xffff);
    assert_eq!(rtl.flags & 0x0001, 0);
}

#[test]
fn deterministic_i8086_unary_group_matrix_matches_golden_model() {
    const LOG_BASE: u16 = 0x3200;
    let values = [
        0x1234_u16, 0x5678, 0x9abc, 0x0402, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    let mut byte_operations = Vec::new();
    let mut byte_log_address = LOG_BASE;

    for operation in 0_u8..8 {
        // AX=0x00f0, BL=0x0c: signed operands are -16 and +12.
        byte_operations.extend_from_slice(&[0xb8, 0xf0, 0x00]);
        byte_operations.extend_from_slice(&[0xb3, 0x0c]);
        byte_operations.extend_from_slice(&[0xba, 0x00, 0x00]);
        byte_operations.extend_from_slice(&[0xf6, 0xc0 + operation * 8]);
        if operation <= 1 {
            byte_operations.push(0x3c);
        }
        byte_operations.extend_from_slice(&[
            0xa2,
            byte_log_address as u8,
            (byte_log_address >> 8) as u8,
        ]);
        byte_operations.push(0x9f); // LAHF
        let flags_address = byte_log_address + 1;
        byte_operations.extend_from_slice(&[
            0x88,
            0x26,
            flags_address as u8,
            (flags_address >> 8) as u8,
        ]);
        byte_log_address += 2;
    }

    let mut word_operations = Vec::new();
    let mut word_log_address = LOG_BASE;
    for operation in 0_u8..8 {
        // AX=0xfff0 (-16), BX=0x000c (+12), DX=0.
        word_operations.extend_from_slice(&[0xb8, 0xf0, 0xff]);
        word_operations.extend_from_slice(&[0xbb, 0x0c, 0x00]);
        word_operations.extend_from_slice(&[0xba, 0x00, 0x00]);
        word_operations.extend_from_slice(&[0xf7, 0xc0 + operation * 8]);
        if operation <= 1 {
            word_operations.extend_from_slice(&[0x34, 0x12]);
        }
        word_operations.extend_from_slice(&[
            0xa3,
            word_log_address as u8,
            (word_log_address >> 8) as u8,
        ]);
        word_operations.push(0x9f);
        let flags_address = word_log_address + 2;
        word_operations.extend_from_slice(&[
            0x88,
            0x26,
            flags_address as u8,
            (flags_address >> 8) as u8,
        ]);
        word_log_address += 3;
    }

    assert_matches_golden(
        &initial_program(values, &byte_operations),
        "byte unary arithmetic matrix",
    );
    assert_matches_golden(
        &initial_program(values, &word_operations),
        "word unary arithmetic matrix",
    );
}

fn run_i8086_randomized_differential_fuzz(first_seed: u64, seeds: u64) {
    const MEMORY_FIRST: u32 = 0x0f00;
    const MEMORY_LAST: u32 = 0x1200;
    let runner = shared_i8086_runner();

    for seed in first_seed..first_seed.saturating_add(seeds) {
        let mut rng = DeterministicRng(0x2545_f491_4f6c_dd1d ^ (seed + 1));
        let values = [
            rng.next_u32() as u16,
            rng.below(32) as u16,
            rng.next_u32() as u16,
            0x1000,
            0xfffe,
            0x1020,
            0,
            0,
        ];
        let mut operations = Vec::new();

        for _ in 0..40 {
            match rng.below(6) {
                0 => {
                    let operation = rng.below(8) as u8;
                    let width = rng.below(2) as u8;
                    let source = rng.below(8) as u8;
                    let destination = rng.below(8) as u8;
                    operations.extend_from_slice(&[
                        operation * 8 + width,
                        0xc0 | source << 3 | destination,
                    ]);
                }
                1 => {
                    let operation = [0, 1, 2, 3, 4, 5, 7][rng.below(7) as usize];
                    let word = rng.below(2) == 0;
                    let destination = rng.below(8) as u8;
                    operations.extend_from_slice(&[
                        if word { 0x81 } else { 0x80 },
                        0xc0 | operation << 3 | destination,
                    ]);
                    let immediate = rng.next_u32();
                    if word {
                        operations.extend_from_slice(&immediate.to_le_bytes()[..2]);
                    } else {
                        operations.push(immediate as u8);
                    }
                }
                2 => {
                    let operation = [0, 1, 2, 3, 4, 5, 7][rng.below(7) as usize];
                    let word = rng.below(2) == 0;
                    let destination = rng.below(8) as u8;
                    operations.extend_from_slice(&[
                        if word { 0xc1 } else { 0xc0 },
                        0xc0 | operation << 3 | destination,
                        1 + rng.below(31) as u8,
                    ]);
                }
                3 => {
                    let operation = rng.below(8) as u8;
                    let word = rng.below(2) == 0;
                    let operand = rng.below(8) as u8;
                    operations.extend_from_slice(&[
                        if word { 0xd3 } else { 0xd2 },
                        0xc0 | operation << 3 | operand,
                    ]);
                }
                4 => {
                    let word = rng.below(2) == 0;
                    let operation = match rng.below(4) {
                        0 => 0,
                        1 => 1,
                        2 => 2,
                        3 => 3,
                        _ => 4 + rng.below(2) as u8,
                    };
                    let operand = rng.below(8) as u8;
                    operations.extend_from_slice(&[
                        if word { 0xf7 } else { 0xf6 },
                        0xc0 | operation << 3 | operand,
                    ]);
                    if operation <= 1 {
                        let immediate = rng.next_u32();
                        if word {
                            operations.extend_from_slice(&immediate.to_le_bytes()[..2]);
                        } else {
                            operations.push(immediate as u8);
                        }
                    }
                }
                _ => {
                    if rng.below(2) == 0 {
                        let prefix = [0x26, 0x2e, 0x36, 0x3e][rng.below(4) as usize];
                        operations.push(prefix);
                    }
                    let store = rng.below(2) == 0;
                    let word = rng.below(2) == 0;
                    let register = if word {
                        rng.below(3) as u8
                    } else {
                        rng.below(3) as u8 + 4 * rng.below(2) as u8
                    };
                    let (modrm, displacement) = match rng.below(3) {
                        0 => (0x40 | rng.below(8) as u8, vec![rng.next_u32() as i8 as u8]),
                        1 => {
                            let address = 0x1080_u16 + rng.below(0x80) as u16;
                            (0x06, address.to_le_bytes().to_vec())
                        }
                        _ => {
                            let displacement = rng.below(0x80) as u16;
                            (
                                0x80 | rng.below(8) as u8,
                                displacement.to_le_bytes().to_vec(),
                            )
                        }
                    };
                    operations.extend_from_slice(&[
                        if store {
                            if word {
                                0x89
                            } else {
                                0x88
                            }
                        } else if word {
                            0x8b
                        } else {
                            0x8a
                        },
                        modrm | register << 3,
                    ]);
                    operations.extend(displacement);
                }
            }
        }

        let executable = initial_program(values, &operations);
        let mut golden = I8086Machine::com(&executable, "")
            .unwrap_or_else(|error| panic!("seed {seed}: construct golden model: {error}"));
        for address in MEMORY_FIRST..MEMORY_LAST {
            golden.memory.write_u8(
                address,
                (seed as u8)
                    .wrapping_mul(29)
                    .wrapping_add(address as u8)
                    .wrapping_mul(19),
            );
        }
        let initial_memory = golden.memory.clone();
        let mut reference = ReferenceI8086::com(&initial_memory);
        let mut stop = I8086Stop::Running;
        for instruction_index in 0..2000 {
            let instruction_ip = golden.ip;
            reference.step().unwrap_or_else(|error| {
                panic!("seed {seed}: independent reference failed: {error}")
            });
            stop = golden.step();
            if reference.registers() != golden.regs
                || reference.segments() != golden.segments
                || reference.instruction_pointer() != golden.ip
                || reference.flags() != golden.flags
            {
                let mut instruction_bytes = [0_u8; 12];
                for (offset, byte) in instruction_bytes.iter_mut().enumerate() {
                    *byte = initial_memory.read_u8(
                        ((golden.segments[1] as u32) << 4)
                            .wrapping_add(instruction_ip as u32 + offset as u32)
                            & 0xff_ffff,
                    );
                }
                eprintln!(
                    "seed {seed}: reference regs={:#06x?}, segments={:#06x?}, ip={:#06x}, flags={:#06x}",
                    reference.registers(),
                    reference.segments(),
                    reference.instruction_pointer(),
                    reference.flags()
                );
                eprintln!(
                    "seed {seed}: golden regs={:#06x?}, segments={:#06x?}, ip={:#06x}, flags={:#06x}",
                    golden.regs,
                    golden.segments,
                    golden.ip,
                    golden.flags
                );
                panic!(
                    "seed {seed}: independent divergence at instruction {instruction_index}, IP {instruction_ip:#06x}, bytes {instruction_bytes:02x?}"
                );
            }
            if stop != I8086Stop::Running {
                break;
            }
        }
        assert_eq!(
            stop,
            I8086Stop::Halt,
            "seed {seed}: golden model did not halt"
        );
        assert_eq!(
            reference.registers(),
            golden.regs,
            "seed {seed}: independent architectural registers differ"
        );
        assert_eq!(
            reference.segments(),
            golden.segments,
            "seed {seed}: independent segment registers differ"
        );
        assert_eq!(
            reference.instruction_pointer(),
            golden.ip,
            "seed {seed}: independent IP differs"
        );
        assert_eq!(
            reference.flags(),
            golden.flags,
            "seed {seed}: independent FLAGS differ"
        );
        assert_eq!(
            reference.retired_instructions(),
            golden.instructions_retired,
            "seed {seed}: independent instruction count differs"
        );
        for address in MEMORY_FIRST..MEMORY_LAST {
            assert_eq!(
                reference.memory_byte(address),
                golden.memory.read_u8(address),
                "seed {seed}: independent memory differs at {address:#06x}"
            );
        }
        let rtl = runner
            .run_memory(initial_memory)
            .unwrap_or_else(|error| panic!("seed {seed}: RTL execution failed: {error}"));
        let actual_registers = [
            rtl.ax, rtl.cx, rtl.dx, rtl.bx, rtl.sp, rtl.bp, rtl.si, rtl.di,
        ];
        if !rtl.halted
            || rtl.trap
            || actual_registers != golden.regs
            || rtl.ip != golden.ip
            || rtl.flags != golden.flags
        {
            eprintln!("seed {seed} executable={executable:02x?}");
            eprintln!(
                "seed {seed} rtl regs={:04x?}, ip={:#06x}, flags={:#06x}",
                actual_registers, rtl.ip, rtl.flags
            );
            eprintln!(
                "seed {seed} golden regs={:04x?}, ip={:#06x}, flags={:#06x}",
                golden.regs, golden.ip, golden.flags
            );
        }
        assert!(rtl.halted, "seed {seed}: RTL did not halt");
        assert!(!rtl.trap, "seed {seed}: RTL trapped");
        assert_eq!(
            actual_registers, golden.regs,
            "seed {seed}: architectural registers differ"
        );
        assert_eq!(rtl.ip, golden.ip, "seed {seed}: IP differs");
        assert_eq!(rtl.flags, golden.flags, "seed {seed}: FLAGS differ");
        for address in MEMORY_FIRST..MEMORY_LAST {
            assert_eq!(
                rtl.memory.read_u8(address),
                golden.memory.read_u8(address),
                "seed {seed}: memory differs at {address:#06x}"
            );
        }
    }
}

#[test]
fn deterministic_i8086_randomized_differential_fuzz_seeds_000_007() {
    run_i8086_randomized_differential_fuzz(0, 8);
}

#[test]
fn deterministic_i8086_randomized_differential_fuzz_seeds_008_015() {
    run_i8086_randomized_differential_fuzz(8, 8);
}

#[test]
fn deterministic_i8086_randomized_differential_fuzz_seeds_016_023() {
    run_i8086_randomized_differential_fuzz(16, 8);
}

#[test]
fn deterministic_i8086_randomized_differential_fuzz_seeds_024_031() {
    run_i8086_randomized_differential_fuzz(24, 8);
}
