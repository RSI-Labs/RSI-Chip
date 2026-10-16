use eda_cpu_platform::i8086::{I8086Machine, I8086Stop};
use eda_cpu_platform::systemverilog::I8086SvRunner;

struct Vector {
    name: &'static str,
    code: Vec<u8>,
}

fn register_prologue() -> Vec<u8> {
    let values = [
        0x0f34_u16, 0x1234, 0x5678, 0x9abc, 0xfffe, 0x1000, 0x1002, 0x1004,
    ];
    values
        .iter()
        .flat_map(|value| [0xb8, *value as u8, (value >> 8) as u8])
        .collect()
}

fn vectors() -> Vec<Vector> {
    let mut result = Vec::new();

    for operation in 0..8 {
        result.push(Vector {
            name: "alu_rm8_r8",
            code: vec![0x00 + operation * 8, 0xc0, 0xf4],
        });
        result.push(Vector {
            name: "alu_rm16_r16",
            code: vec![0x01 + operation * 8, 0xc0, 0xf4],
        });
        result.push(Vector {
            name: "alu_group_80",
            code: vec![0x80, 0x06 | (operation << 3), 0x00, 0x10, 0x7f, 0xf4],
        });
        result.push(Vector {
            name: "alu_group_81",
            code: vec![0x81, 0x06 | (operation << 3), 0x00, 0x10, 0x34, 0x12, 0xf4],
        });
        result.push(Vector {
            name: "alu_group_83",
            code: vec![0x83, 0x06 | (operation << 3), 0x00, 0x10, 0xfe, 0xf4],
        });
    }

    for operation in 0..8 {
        result.push(Vector {
            name: "shift_d2_cl",
            code: vec![0xd2, 0xc1 | (operation << 3), 0xf4],
        });
    }

    for register in 0..8 {
        result.push(Vector {
            name: "mov_r8_imm",
            code: vec![0xb0 + register, 0x5a, 0xf4],
        });
        result.push(Vector {
            name: "mov_r16_imm",
            code: vec![0xb8 + register, 0xcd, 0xab, 0xf4],
        });
    }

    let modrm_cases: &[(&str, &[u8])] = &[
        ("mov_al_si", &[0x8a, 0x04, 0xf4]),
        ("mov_al_si_disp8", &[0x8a, 0x44, 0x02, 0xf4]),
        ("mov_al_si_disp16", &[0x8a, 0x84, 0x05, 0x00, 0xf4]),
        ("mov_ax_si", &[0x8b, 0x04, 0xf4]),
        ("mov_al_esi", &[0x26, 0x8a, 0x04, 0xf4]),
        ("lea_ax_bx_si", &[0x8d, 0x00, 0xf4]),
        ("xchg_ax_cx", &[0x91, 0xf4]),
        ("push_ax_pop_bx", &[0x50, 0x59, 0xf4]),
    ];
    for (name, code) in modrm_cases {
        result.push(Vector {
            name,
            code: code.to_vec(),
        });
    }

    result
}

#[test]
fn deterministic_i8086_opcode_matrix_matches_golden_model() {
    let runner = I8086SvRunner::new().expect("compile i8086 runtime once");
    for vector in vectors() {
        let mut executable = register_prologue();
        executable.extend_from_slice(&vector.code);
        let mut golden = I8086Machine::com(&executable, "").expect("construct golden case");
        golden.memory.write_le_u16(0x1000, 0xa55a);
        golden.memory.write_le_u16(0x1002, 0x3cc3);
        golden.memory.write_le_u16(0x1004, 0x8117);
        golden.memory.write_le_u16(0x1006, 0x2492);
        golden.memory.write_le_u16(0x1008, 0x5e6d);
        golden.memory.write_le_u32(0x1500, 0x1357_2468);
        golden.memory.write_le_u32(0xfffc, 0);
        let initial_memory = golden.memory.clone();

        let golden_stop = golden.run(100);
        assert_eq!(
            golden_stop,
            I8086Stop::Halt,
            "{}: golden did not halt",
            vector.name
        );
        let rtl = runner
            .run_memory(initial_memory)
            .unwrap_or_else(|error| panic!("{}: RTL execution failed: {error}", vector.name));

        let actual = [
            rtl.ax, rtl.cx, rtl.dx, rtl.bx, rtl.sp, rtl.bp, rtl.si, rtl.di,
        ];
        assert_eq!(
            actual, golden.regs,
            "{}: architectural registers differ",
            vector.name
        );
        assert_eq!(rtl.ip, golden.ip, "{}: IP differs", vector.name);
        assert_eq!(rtl.flags, golden.flags, "{}: FLAGS differ", vector.name);
        for address in [0x1000_u32, 0x1002, 0x1004, 0x1006, 0x1008, 0xfffc] {
            assert_eq!(
                rtl.memory.read_le_u16(address),
                golden.memory.read_le_u16(address),
                "{}: memory {address:#06x} differs",
                vector.name
            );
        }
    }
}
