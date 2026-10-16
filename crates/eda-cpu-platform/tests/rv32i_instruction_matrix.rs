use eda_cpu_platform::rv32i::{Rv32iMachine, StopReason};
use eda_cpu_platform::systemverilog::{Rv32iSvResult, Rv32iSvRunner};
use eda_cpu_platform::SparseMemory;

fn i_type(opcode: u32, rd: u32, funct3: u32, rs1: u32, immediate: i32) -> u32 {
    ((immediate as u32 & 0xfff) << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

fn s_type(funct3: u32, rs1: u32, rs2: u32, immediate: i32) -> u32 {
    let immediate = immediate as u32;
    ((immediate >> 5 & 0x7f) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (funct3 << 12)
        | ((immediate & 0x1f) << 7)
        | 0x23
}

fn b_type(funct3: u32, rs1: u32, rs2: u32, offset: i32) -> u32 {
    let offset = offset as u32;
    ((offset >> 12 & 1) << 31)
        | ((offset >> 5 & 0x3f) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (funct3 << 12)
        | ((offset >> 1 & 0xf) << 8)
        | ((offset >> 11 & 1) << 7)
        | 0x63
}

fn u_type(opcode: u32, rd: u32, immediate: u32) -> u32 {
    (immediate & 0xffff_f000) | (rd << 7) | opcode
}

fn j_type(rd: u32, offset: i32) -> u32 {
    let offset = offset as u32;
    ((offset >> 20 & 1) << 31)
        | ((offset >> 1 & 0x3ff) << 21)
        | ((offset >> 11 & 1) << 20)
        | ((offset >> 12 & 0xff) << 12)
        | (rd << 7)
        | 0x6f
}

fn addi(rd: u32, rs1: u32, immediate: i32) -> u32 {
    i_type(0x13, rd, 0, rs1, immediate)
}

fn nop() -> u32 {
    addi(0, 0, 0)
}

fn ebreak() -> u32 {
    0x0010_0073
}

fn csr(funct3: u32, rd: u32, rs1: u32, address: u32) -> u32 {
    i_type(0x73, rd, funct3, rs1, address as i32)
}

fn load_upper(rd: u32, value: u32) -> Vec<u32> {
    let lower = (value & 0xfff) as u16 as i16 as i32;
    let upper = value.wrapping_sub(lower as u32);
    vec![u_type(0x37, rd, upper), addi(rd, rd, lower)]
}

fn base_matrix_program() -> Vec<u32> {
    let mut program = Vec::new();
    for (register, value) in [
        (1_u32, 0x1234_5678_u32),
        (2, 0x8000_0000),
        (3, 0x7fff_ffff),
        (4, 0x0f0f_0f0f),
        (5, 0xffff_fff0),
    ] {
        program.extend(load_upper(register, value));
    }
    program.push(addi(6, 0, 31));
    program.extend(load_upper(7, 0x1000));

    // OP-IMM: every RV32I immediate operation, including signed edges.
    program.extend([
        addi(8, 1, -1),
        i_type(0x13, 9, 2, 5, -1),  // slti
        i_type(0x13, 10, 3, 5, -1), // sltiu
        i_type(0x13, 11, 4, 1, -1), // xori
        i_type(0x13, 12, 6, 4, 0x7f0),
        i_type(0x13, 13, 7, 4, -1),            // andi
        i_type(0x13, 14, 1, 6, 7),             // slli
        i_type(0x13, 15, 5, 6, 7),             // srli
        i_type(0x13, 16, 5, 6, 0x20 << 5 | 7), // srai
    ]);

    // OP: every RV32I register-register operation.
    program.extend([
        r_type(0, 3, 1, 0, 17, 0x33),
        r_type(0x20, 3, 1, 0, 18, 0x33),
        r_type(0, 6, 1, 1, 19, 0x33),
        r_type(0, 3, 2, 2, 20, 0x33),
        r_type(0, 3, 2, 3, 21, 0x33),
        r_type(0, 4, 1, 4, 22, 0x33),
        r_type(0, 6, 2, 5, 23, 0x33),
        r_type(0x20, 6, 2, 5, 24, 0x33),
        r_type(0, 4, 1, 6, 25, 0x33),
        r_type(0, 4, 1, 7, 26, 0x33),
    ]);

    // Every store width followed by every signed/unsigned load width.
    program.extend([
        s_type(0, 7, 5, 0),
        s_type(1, 7, 5, 4),
        s_type(2, 7, 5, 8),
        i_type(0x03, 27, 0, 7, 0),
        i_type(0x03, 28, 1, 7, 4),
        i_type(0x03, 29, 2, 7, 8),
        i_type(0x03, 30, 4, 7, 0),
        i_type(0x03, 31, 5, 7, 4),
    ]);

    // LUI/AUIPC and all branch predicates. Taken/not-taken paths both end at
    // the next conventional instruction boundary.
    program.push(u_type(0x37, 10, 0x1234_5000));
    program.push(u_type(0x17, 11, 0x0001_2000));
    for funct3 in 0_u32..8 {
        if funct3 == 2 || funct3 == 3 {
            continue;
        }
        let (rs1, rs2) = match funct3 {
            0 => (0, 0),
            1 => (0, 1),
            4 => (2, 3),
            5 => (3, 2),
            6 => (2, 3),
            _ => (3, 2),
        };
        program.push(b_type(funct3, rs1, rs2, 8));
        program.push(nop());
    }

    // JAL and JALR link/return behavior.
    program.push(j_type(8, 8));
    program.push(nop());
    program.push(i_type(0x67, 9, 0, 8, 8));
    program.push(nop());
    program.push(ebreak());
    program
}

fn write_program(memory: &mut SparseMemory, program: &[u32], base: u32) {
    for (index, instruction) in program.iter().enumerate() {
        memory.write_le_u32(base + (index as u32) * 4, *instruction);
    }
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

fn assert_matches_golden(golden: &Rv32iMachine, rtl: &Rv32iSvResult) {
    assert_eq!(golden.stop, StopReason::Ebreak);
    assert!(rtl.halted);
    assert!(!rtl.trap || rtl.trap_cause == 3);
    assert_eq!(rtl.regs, golden.regs);
    let expected_pc = if rtl.trap && rtl.trap_cause == 3 {
        golden.pc.wrapping_sub(4)
    } else {
        golden.pc
    };
    assert_eq!(rtl.pc, expected_pc);
    for address in [0x1000_u32, 0x1004, 0x1008, 0x100c] {
        assert_eq!(
            rtl.memory.read_le_u32(address),
            golden.memory.read_le_u32(address),
            "memory differs at {address:#010x}"
        );
    }
}

#[test]
fn rv32i_base_instruction_matrix_matches_golden_model() {
    let program = base_matrix_program();
    let mut memory = SparseMemory::new();
    write_program(&mut memory, &program, 0);
    let mut golden = Rv32iMachine::baremetal(0, memory.clone());
    assert_eq!(golden.run(1000), StopReason::Ebreak);

    let runner = Rv32iSvRunner::new().expect("compile RV32I runtime");
    let rtl = runner
        .run_memory(0, memory)
        .expect("execute RV32I base instruction matrix");
    assert_matches_golden(&golden, &rtl);
}

#[test]
fn rv32i_csr_read_write_modify_matrix_matches_golden_model() {
    const MSCRATCH: u32 = 0x340;
    let program = [
        addi(1, 0, -1),
        addi(2, 0, 0xff),
        csr(1, 8, 1, MSCRATCH),  // csrrw
        csr(2, 9, 2, MSCRATCH),  // csrrs
        csr(3, 10, 2, MSCRATCH), // csrrc
        csr(1, 0, 0, MSCRATCH),  // clear
        ebreak(),
    ];
    let mut memory = SparseMemory::new();
    write_program(&mut memory, &program, 0);
    let mut golden = Rv32iMachine::baremetal(0, memory.clone());
    assert_eq!(golden.run(100), StopReason::Ebreak);

    let runner = Rv32iSvRunner::new().expect("compile RV32I runtime");
    let rtl = runner
        .run_memory(0, memory)
        .expect("execute RV32I CSR matrix");
    assert_matches_golden(&golden, &rtl);
}

#[test]
fn rv32i_trap_csr_and_mret_matrix_matches_golden_model() {
    const MTVEC: u32 = 0x305;
    const MEPC: u32 = 0x341;
    let mut main = Vec::new();
    main.extend(load_upper(7, 0x1000));
    main.push(csr(1, 0, 7, MTVEC));
    main.push(0x0000_0073); // ecall
    main.push(ebreak());

    let handler = [
        csr(1, 8, 0, MEPC), // read mepc
        addi(8, 8, 4),
        csr(1, 0, 8, MEPC), // advance return PC past ECALL
        0x3020_0073,        // mret
        ebreak(),
    ];

    let mut memory = SparseMemory::new();
    write_program(&mut memory, &main, 0);
    write_program(&mut memory, &handler, 0x1000);
    let mut golden = Rv32iMachine::baremetal(0, memory.clone());
    assert_eq!(golden.run(100), StopReason::Ebreak);

    let runner = Rv32iSvRunner::new().expect("compile RV32I runtime");
    let rtl = runner
        .run_memory(0, memory)
        .expect("execute RV32I trap/mret matrix");
    assert_matches_golden(&golden, &rtl);
}

#[test]
fn rv32i_randomized_differential_fuzz_matches_golden_model() {
    const SEEDS: u64 = 64;
    const DATA_BASE: u32 = 0x2000;
    const DATA_LENGTH: u32 = 0x100;
    let runner = Rv32iSvRunner::new().expect("compile RV32I runtime once");

    for seed in 0_u64..SEEDS {
        let mut rng = DeterministicRng(0x9e37_79b9_7f4a_7c15 ^ (seed + 1));
        let mut program = Vec::new();

        // Register x1 remains a fixed data-segment base. The other seedable
        // registers cover positive, negative, all-ones, and mixed bit patterns.
        program.extend(load_upper(1, DATA_BASE));
        for register in 2_u32..=8 {
            let value = rng.next_u32();
            program.extend(load_upper(register, value));
        }

        for _ in 0..24 {
            match rng.below(4) {
                0 => {
                    let (funct7, funct3) = match rng.below(10) {
                        0 => (0, 0),
                        1 => (0x20, 0),
                        2 => (0, 1),
                        3 => (0, 2),
                        4 => (0, 3),
                        5 => (0, 4),
                        6 => (0, 5),
                        7 => (0x20, 5),
                        8 => (0, 6),
                        _ => (0, 7),
                    };
                    program.push(r_type(
                        funct7,
                        rng.below(31) + 1,
                        rng.below(31) + 1,
                        funct3,
                        rng.below(23) + 9,
                        0x33,
                    ));
                }
                1 => {
                    let funct3 = match rng.below(8) {
                        0 => 0,
                        1 => 2,
                        2 => 3,
                        3 => 4,
                        4 => 6,
                        5 => 7,
                        6 => 1,
                        _ => 5,
                    };
                    let immediate = if funct3 == 1 || funct3 == 5 {
                        let shamt = rng.below(32);
                        if funct3 == 5 && rng.below(2) == 0 {
                            0x20 << 5 | shamt
                        } else {
                            shamt
                        }
                    } else {
                        rng.next_u32() & 0xfff
                    };
                    program.push(i_type(
                        0x13,
                        rng.below(23) + 9,
                        funct3,
                        rng.below(31) + 1,
                        immediate as i16 as i32,
                    ));
                }
                2 => {
                    let width = rng.below(3);
                    // Keep every possible byte/half/word access aligned.
                    let offset = rng.below(16) * 4;
                    if rng.below(2) == 0 {
                        program.push(i_type(
                            0x03,
                            rng.below(23) + 9,
                            [0, 1, 2, 4, 5][rng.below(5) as usize],
                            1,
                            offset as i32,
                        ));
                    } else {
                        program.push(s_type(width, 1, rng.below(31) + 1, offset as i32));
                    }
                }
                _ => {
                    let funct3 = [0, 1, 4, 5, 6, 7][rng.below(6) as usize];
                    program.push(b_type(funct3, rng.below(31) + 1, rng.below(31) + 1, 8));
                    program.push(nop());
                }
            }
        }

        program.push(ebreak());
        let mut memory = SparseMemory::new();
        write_program(&mut memory, &program, 0);
        for offset in 0..DATA_LENGTH {
            memory.write_u8(
                DATA_BASE + offset,
                (seed as u8)
                    .wrapping_mul(31)
                    .wrapping_add(offset as u8)
                    .wrapping_mul(17),
            );
        }

        let mut golden = Rv32iMachine::baremetal(0, memory.clone());
        assert_eq!(
            golden.run(1000),
            StopReason::Ebreak,
            "seed {seed}: golden model did not reach EBREAK"
        );
        let rtl = runner
            .run_memory(0, memory)
            .unwrap_or_else(|error| panic!("seed {seed}: RTL execution failed: {error}"));
        assert!(rtl.halted, "seed {seed}: RTL did not halt");
        assert!(
            rtl.trap && rtl.trap_cause == 3,
            "seed {seed}: unexpected RTL trap {}",
            rtl.trap_cause
        );
        if rtl.regs != golden.regs {
            for (index, instruction) in program.iter().enumerate() {
                eprintln!("seed {seed} program[{index:02}] = {instruction:08x}");
            }
            for (index, (rtl_value, golden_value)) in
                rtl.regs.iter().zip(golden.regs.iter()).enumerate()
            {
                if rtl_value != golden_value {
                    eprintln!(
                        "seed {seed} x{index}: rtl={rtl_value:#010x}, golden={golden_value:#010x}"
                    );
                }
            }
        }
        assert_eq!(
            rtl.regs, golden.regs,
            "seed {seed}: architectural registers differ"
        );
        assert_eq!(
            rtl.pc,
            golden.pc.wrapping_sub(4),
            "seed {seed}: EBREAK PC differs"
        );
        for offset in 0..DATA_LENGTH {
            let address = DATA_BASE + offset;
            assert_eq!(
                rtl.memory.read_u8(address),
                golden.memory.read_u8(address),
                "seed {seed}: memory differs at {address:#010x}"
            );
        }
    }
}
