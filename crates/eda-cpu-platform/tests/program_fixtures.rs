use eda_cpu_platform::i8086::{I8086Machine, I8086Stop};
use eda_cpu_platform::rv32i::{Rv32iMachine, StopReason};
use eda_cpu_platform::systemverilog::Rv32iSvResult;
use raven_riscv_engine::falcon::errors::FalconError;
use raven_riscv_engine::falcon::exec;
use raven_riscv_engine::falcon::memory::{AmoOp, Bus};
use raven_riscv_engine::falcon::Cpu;
use raven_riscv_engine::host::Console;

struct SparseRavenBus {
    memory: eda_cpu_platform::SparseMemory,
}

impl Bus for SparseRavenBus {
    fn mem_len(&self) -> u32 {
        u32::MAX
    }

    fn load8(&self, address: u32) -> Result<u8, FalconError> {
        Ok(self.memory.read_u8(address))
    }

    fn load16(&self, address: u32) -> Result<u16, FalconError> {
        Ok(self.memory.read_le_u16(address))
    }

    fn load32(&self, address: u32) -> Result<u32, FalconError> {
        Ok(self.memory.read_le_u32(address))
    }

    fn store8(&mut self, address: u32, value: u8) -> Result<(), FalconError> {
        self.memory.write_u8(address, value);
        Ok(())
    }

    fn store16(&mut self, address: u32, value: u16) -> Result<(), FalconError> {
        self.memory.write_le_u16(address, value);
        Ok(())
    }

    fn store32(&mut self, address: u32, value: u32) -> Result<(), FalconError> {
        self.memory.write_le_u32(address, value);
        Ok(())
    }

    fn lr_w(&mut self, _hart_id: u32, address: u32) -> Result<u32, FalconError> {
        self.load32(address)
    }

    fn sc_w(&mut self, _hart_id: u32, address: u32, value: u32) -> Result<bool, FalconError> {
        self.store32(address, value)?;
        Ok(true)
    }

    fn amo_w(
        &mut self,
        _hart_id: u32,
        address: u32,
        operation: AmoOp,
        operand: u32,
    ) -> Result<u32, FalconError> {
        let old = self.load32(address)?;
        let result = match operation {
            AmoOp::Swap => operand,
            AmoOp::Add => old.wrapping_add(operand),
            AmoOp::Xor => old ^ operand,
            AmoOp::And => old & operand,
            AmoOp::Or => old | operand,
            AmoOp::Max => (old as i32).max(operand as i32) as u32,
            AmoOp::Min => (old as i32).min(operand as i32) as u32,
            AmoOp::MaxU => old.max(operand),
            AmoOp::MinU => old.min(operand),
        };
        self.store32(address, result)?;
        Ok(old)
    }
}

struct IndependentElfResult {
    regs: [u32; 32],
    pc: u32,
    exit_code: u32,
    memory: eda_cpu_platform::SparseMemory,
}

fn run_independent_rv32i_elf(
    initial_memory: &eda_cpu_platform::SparseMemory,
    entry: u32,
    stack_pointer: u32,
) -> IndependentElfResult {
    let mut bus = SparseRavenBus {
        memory: initial_memory.clone(),
    };
    let mut cpu = Cpu {
        pc: entry,
        ..Default::default()
    };
    cpu.x[2] = stack_pointer;
    let mut console = Console::default();
    for _ in 0..100 {
        let _ = exec::step(&mut cpu, &mut bus, &mut console)
            .expect("independent RV32I ELF oracle failed");
        if let Some(exit_code) = cpu.exit_code {
            return IndependentElfResult {
                regs: cpu.x,
                pc: cpu.pc,
                exit_code,
                memory: bus.memory,
            };
        }
    }
    panic!("independent RV32I ELF oracle did not exit");
}

fn assert_rtl_matches_independent_elf(
    rtl: &Rv32iSvResult,
    golden: &Rv32iMachine,
    oracle: &IndependentElfResult,
    ranges: &[(u32, u32)],
    context: &str,
) {
    assert!(rtl.halted, "{context}: RTL did not halt");
    assert!(rtl.trap, "{context}: RTL did not report Linux exit ECALL");
    assert_eq!(rtl.trap_cause, 11, "{context}: RTL exit trap differs");
    assert_eq!(rtl.pc, oracle.pc, "{context}: independent PC differs");
    assert_eq!(
        rtl.regs, oracle.regs,
        "{context}: independent registers differ"
    );
    assert_eq!(
        rtl.a0, oracle.exit_code,
        "{context}: independent exit code differs"
    );
    assert_eq!(
        golden.regs, oracle.regs,
        "{context}: built-in registers differ"
    );
    assert_eq!(golden.pc, oracle.pc, "{context}: built-in PC differs");
    for &(start, length) in ranges {
        for offset in 0..length {
            let address = start.wrapping_add(offset);
            assert_eq!(
                rtl.memory.read_u8(address),
                oracle.memory.read_u8(address),
                "{context}: memory differs at {address:#010x}"
            );
            assert_eq!(
                golden.memory.read_u8(address),
                oracle.memory.read_u8(address),
                "{context}: built-in memory differs at {address:#010x}"
            );
        }
    }
}

#[test]
fn checked_in_compiled_program_fixtures_run() {
    let mut i8086 = I8086Machine::com(include_bytes!("../../../platform/programs/hello.com"), "")
        .expect("load checked-in COM fixture");
    assert_eq!(i8086.run(100), I8086Stop::Halt);
    assert_eq!(i8086.output, b"Hello");

    let mut rv32i = Rv32iMachine::load_elf(
        include_bytes!("../../../platform/programs/rv32i_exit42.elf"),
        &[],
    )
    .expect("load checked-in ELF fixture");
    assert_eq!(rv32i.run(100), StopReason::Exit(42));
}

#[test]
fn checked_in_com_runs_on_systemverilog_core() {
    let program = include_bytes!("../../../platform/programs/hello.com");
    let mut golden = I8086Machine::com(program, "").expect("golden COM loader");
    golden.run(100);
    let rtl = eda_cpu_platform::systemverilog::run_i8086_com(program)
        .expect("SystemVerilog COM execution");
    assert_eq!(rtl.output, b"Hello");
    assert_eq!(rtl.output, golden.output);
    assert!(rtl.halted);
}

#[test]
fn high_memory_com_runs_on_golden_and_systemverilog_models() {
    let program = include_bytes!("../../../platform/programs/high_memory.com");
    assert!(program.len() > 64);
    let mut golden = I8086Machine::com(program, "").expect("golden high-memory COM loader");
    assert_eq!(golden.run(100), I8086Stop::Halt);
    assert_eq!(golden.output, b"HIGH");

    let rtl = eda_cpu_platform::systemverilog::run_i8086_com(program)
        .expect("SystemVerilog high-memory COM execution");
    assert!(rtl.halted);
    assert!(!rtl.trap);
    assert_eq!(rtl.output, golden.output);
    assert_eq!(rtl.ip, golden.ip);
}

#[test]
fn maximum_size_com_image_uses_the_complete_real_mode_segment() {
    let mut program = vec![0_u8; 0xff00];
    program[..12].copy_from_slice(&[
        0xba, 0x00, 0xff, // mov dx, 0xff00
        0xb4, 0x09, // mov ah, 9
        0xcd, 0x21, // int 0x21
        0xb8, 0x00, 0x4c, // mov ax, 0x4c00
        0xcd, 0x21, // int 0x21
    ]);
    program[0xfe00..0xfe04].copy_from_slice(b"END$");

    let mut golden =
        I8086Machine::com(&program, "").expect("golden loader accepts maximum-size COM image");
    assert_eq!(golden.run(100), I8086Stop::Halt);
    assert_eq!(golden.output, b"END");

    let rtl = eda_cpu_platform::systemverilog::run_i8086_com(&program)
        .expect("SystemVerilog runtime executes maximum-size COM image");
    assert!(rtl.halted);
    assert!(!rtl.trap);
    assert_eq!(rtl.output, golden.output);
    assert_eq!(rtl.ip, golden.ip);
}

#[test]
fn relocatable_mz_runs_on_golden_and_systemverilog_models() {
    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn image(with_relocation: bool) -> Vec<u8> {
        let mut program = vec![0_u8; 0x51];
        program[0] = b'M';
        program[1] = b'Z';
        put_u16(&mut program, 2, 0x51); // bytes in last page
        put_u16(&mut program, 4, 1); // one 512-byte page
        put_u16(&mut program, 6, u16::from(with_relocation));
        put_u16(&mut program, 8, 4); // 64-byte header
        put_u16(&mut program, 0x0c, 1); // minimum extra paragraphs
        put_u16(&mut program, 0x0e, 0x10); // stack segment after header
        put_u16(&mut program, 0x10, 0x0100); // stack pointer
        put_u16(&mut program, 0x14, 0); // initial IP
        put_u16(&mut program, 0x16, 0); // code segment after header
        put_u16(&mut program, 0x18, 0x1c); // relocation table
        if with_relocation {
            // Patch the immediate operand of MOV AX, imm16 at file offset 0x41.
            put_u16(&mut program, 0x1c, 0x41);
            put_u16(&mut program, 0x1e, 0);
        }

        let code: [u8; 17] = [
            0xb8, 0x00, 0x00, // mov ax, image base (relocated)
            0xbb, 0x78, 0x56, // mov bx, 0x5678
            0xb9, 0x34, 0x12, // mov cx, 0x1234
            0xba, 0x00, 0x30, // mov dx, 0x3000
            0x89, 0x06, 0x00, 0x01, // mov [0x0100], ax
            0xf4, // hlt
        ];
        program[0x40..0x51].copy_from_slice(&code);
        program
    }

    for (with_relocation, expected_ax) in [(false, 0), (true, 0x1000)] {
        let program = image(with_relocation);
        let mut golden = I8086Machine::mz(&program, 0x1000)
            .unwrap_or_else(|error| panic!("construct golden MZ image: {error}"));
        assert_eq!(golden.run(100), I8086Stop::Halt);
        let rtl = eda_cpu_platform::systemverilog::run_i8086_mz(&program, 0x1000)
            .unwrap_or_else(|error| panic!("execute MZ image in RTL: {error}"));
        assert!(rtl.halted);
        assert!(!rtl.trap);
        assert_eq!(rtl.ax, expected_ax);
        assert_eq!(rtl.cx, 0x1234);
        assert_eq!(rtl.dx, 0x3000);
        assert_eq!(rtl.bx, 0x5678);
        assert_eq!(rtl.sp, golden.regs[4]);
        assert_eq!(rtl.ip, golden.ip);
        assert_eq!(rtl.flags, golden.flags);
        assert_eq!(
            rtl.memory.read_le_u16(0x10100),
            golden.memory.read_le_u16(0x10100)
        );
    }
}

#[test]
fn checked_in_elf_runs_on_systemverilog_core() {
    let program = include_bytes!("../../../platform/programs/rv32i_exit42.elf");
    let mut golden = Rv32iMachine::load_elf(program, &[]).expect("golden ELF loader");
    let initial_memory = golden.memory.clone();
    let entry = golden.pc;
    assert_eq!(golden.run(100), StopReason::Exit(42));
    let rtl = eda_cpu_platform::systemverilog::run_rv32i_elf(program)
        .expect("SystemVerilog ELF execution");
    assert!(rtl.halted);
    assert_eq!(rtl.a0, golden.regs[10]);
    let oracle = run_independent_rv32i_elf(&initial_memory, entry, golden.regs[2]);
    assert_eq!(oracle.exit_code, 42);
    assert_rtl_matches_independent_elf(&rtl, &golden, &oracle, &[(0x1000, 0xc)], "checked-in ELF");
}

#[test]
fn high_memory_elf_runs_on_golden_and_systemverilog_models() {
    let program = include_bytes!("../../../platform/programs/rv32i_high_memory.elf");
    assert!(program.len() > 64);
    let mut golden = Rv32iMachine::load_elf(program, &[]).expect("golden high-memory ELF loader");
    let initial_memory = golden.memory.clone();
    let entry = golden.pc;
    assert_eq!(golden.run(100), StopReason::Exit(42));

    let rtl = eda_cpu_platform::systemverilog::run_rv32i_elf(program)
        .expect("SystemVerilog high-memory ELF execution");
    assert!(rtl.halted);
    assert!(rtl.trap);
    assert_eq!(rtl.trap_cause, 11);
    assert_eq!(rtl.a0, golden.regs[10]);
    let oracle = run_independent_rv32i_elf(&initial_memory, entry, golden.regs[2]);
    assert_eq!(oracle.exit_code, 42);
    assert_rtl_matches_independent_elf(
        &rtl,
        &golden,
        &oracle,
        &[(0x1000, 0x88)],
        "checked-in high-memory ELF",
    );
}

#[test]
fn sparse_rv32i_elf_runs_from_a_high_32_bit_address() {
    fn instruction(opcode: u32, rd: u32, rs1: u32, funct3: u32, immediate: u32) -> [u8; 4] {
        let encoded_immediate = if opcode == 0x37 {
            immediate & 0xfffff
        } else {
            immediate & 0xfff
        };
        let immediate_shift = if opcode == 0x37 { 12 } else { 20 };
        let value = (encoded_immediate << immediate_shift)
            | (rs1 << 15)
            | (funct3 << 12)
            | (rd << 7)
            | opcode;
        value.to_le_bytes()
    }

    let entry = 0x7000_0000_u32;
    let instructions = [
        instruction(0x37, 6, 0, 0, 0x70000),   // lui t1, 0x70000
        instruction(0x03, 5, 6, 0b010, 0x180), // lw t0, 0x180(t1)
        instruction(0x13, 10, 5, 0, 1),        // addi a0, t0, 1
        instruction(0x13, 17, 0, 0, 93),       // addi a7, zero, 93
        0x0000_0073_u32.to_le_bytes(),         // ecall
    ];
    let file_offset = 0x100_usize;
    let mut segment = vec![0_u8; 0x184];
    for (index, instruction) in instructions.iter().enumerate() {
        segment[index * 4..index * 4 + 4].copy_from_slice(instruction);
    }
    segment[0x180..0x184].copy_from_slice(&41_u32.to_le_bytes());

    let mut elf = vec![0_u8; file_offset + segment.len()];
    elf[0..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    elf[16..52].copy_from_slice(&{
        let mut header = [0_u8; 36];
        header[0..2].copy_from_slice(&2_u16.to_le_bytes());
        header[2..4].copy_from_slice(&243_u16.to_le_bytes());
        header[4..8].copy_from_slice(&1_u32.to_le_bytes());
        header[8..12].copy_from_slice(&entry.to_le_bytes());
        header[12..16].copy_from_slice(&52_u32.to_le_bytes());
        header[24..26].copy_from_slice(&52_u16.to_le_bytes());
        header[26..28].copy_from_slice(&32_u16.to_le_bytes());
        header[28..30].copy_from_slice(&1_u16.to_le_bytes());
        header
    });
    let program_header = [
        1_u32,
        file_offset as u32,
        entry,
        entry,
        segment.len() as u32,
        segment.len() as u32,
        5,
        0x1000,
    ];
    for (destination, field) in elf[52..84]
        .chunks_exact_mut(4)
        .zip(program_header.map(|field| field.to_le_bytes()))
    {
        destination.copy_from_slice(&field);
    }
    elf[file_offset..].copy_from_slice(&segment);

    let mut golden =
        Rv32iMachine::load_elf(&elf, &[]).expect("golden loader accepts high-address ELF");
    assert_eq!(golden.pc, entry);
    let initial_memory = golden.memory.clone();
    let stop = golden.run(100);
    assert_eq!(stop, StopReason::Exit(42));

    let rtl = eda_cpu_platform::systemverilog::run_rv32i_elf(&elf)
        .expect("SystemVerilog runtime executes high-address ELF");
    assert!(rtl.halted);
    assert!(rtl.trap);
    assert_eq!(rtl.trap_cause, 11);
    assert_eq!(rtl.a0, 42);
    let oracle = run_independent_rv32i_elf(&initial_memory, entry, golden.regs[2]);
    assert_eq!(oracle.exit_code, 42);
    assert_rtl_matches_independent_elf(
        &rtl,
        &golden,
        &oracle,
        &[(entry, 0x184)],
        "sparse high-address ELF",
    );
}
