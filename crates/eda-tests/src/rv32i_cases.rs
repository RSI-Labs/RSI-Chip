//! rv32i — RV32I CPU 程序级回归
//!
//! 每个用例从 Rust 侧汇编一段机器码程序，生成 IMEM 初始化的 SystemVerilog
//! 源，编译后由时钟边沿驱动至停机，再检查 `dbg_x10/dbg_x11` 等架构状态。

use crate::CaseList;
use eda_core::logic::LogicWord as LW;
use eda_sim::Simulator;
use std::sync::OnceLock;

const CORE: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");
const TB_CLASS: &str = include_str!("../../../rv32i/tb/tb_class.v");

// ---------- RV32I 指令编码器 ----------

fn enc_r(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn enc_i(imm: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    ((imm & 0xfff) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}
fn enc_s(imm: u32, rs2: u32, rs1: u32, f3: u32, op: u32) -> u32 {
    (((imm >> 5) & 0x7f) << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | ((imm & 0x1f) << 7) | op
}
fn enc_b(imm: u32, rs2: u32, rs1: u32, f3: u32, op: u32) -> u32 {
    let imm = imm as i32 as u32;
    (((imm >> 12) & 1) << 31)
        | (((imm >> 5) & 0x3f) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (f3 << 12)
        | (((imm >> 1) & 0xf) << 8)
        | (((imm >> 11) & 1) << 7)
        | op
}
fn enc_u(imm20: u32, rd: u32, op: u32) -> u32 {
    ((imm20 & 0xfffff) << 12) | (rd << 7) | op
}
fn enc_j(imm: u32, rd: u32, op: u32) -> u32 {
    let imm = imm as i32 as u32;
    (((imm >> 20) & 1) << 31)
        | (((imm >> 1) & 0x3ff) << 21)
        | (((imm >> 11) & 1) << 20)
        | (((imm >> 12) & 0xff) << 12)
        | (rd << 7)
        | op
}

pub fn add(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 0, rd, 0x33)
}
pub fn sub(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x20, rs2, rs1, 0, rd, 0x33)
}
pub fn sll(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 1, rd, 0x33)
}
pub fn slt(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 2, rd, 0x33)
}
pub fn sltu(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 3, rd, 0x33)
}
pub fn xor_(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 4, rd, 0x33)
}
pub fn srl(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 5, rd, 0x33)
}
pub fn sra(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x20, rs2, rs1, 5, rd, 0x33)
}
pub fn or_(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 6, rd, 0x33)
}
pub fn and_(rd: u32, rs1: u32, rs2: u32) -> u32 {
    enc_r(0x00, rs2, rs1, 7, rd, 0x33)
}

pub fn addi(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 0, rd, 0x13)
}
pub fn slti(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 2, rd, 0x13)
}
pub fn sltiu(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 3, rd, 0x13)
}
pub fn xori(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 4, rd, 0x13)
}
pub fn ori(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 6, rd, 0x13)
}
pub fn andi(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 7, rd, 0x13)
}
pub fn slli(rd: u32, rs1: u32, sh: u32) -> u32 {
    enc_i(sh, rs1, 1, rd, 0x13)
}
pub fn srli(rd: u32, rs1: u32, sh: u32) -> u32 {
    enc_i(sh, rs1, 5, rd, 0x13)
}
pub fn srai(rd: u32, rs1: u32, sh: u32) -> u32 {
    enc_i(0x400 | sh, rs1, 5, rd, 0x13)
}

pub fn lui(rd: u32, imm20: u32) -> u32 {
    enc_u(imm20, rd, 0x37)
}
pub fn auipc(rd: u32, imm20: u32) -> u32 {
    enc_u(imm20, rd, 0x17)
}
pub fn jal(rd: u32, imm: i32) -> u32 {
    enc_j(imm as u32, rd, 0x6f)
}
pub fn jalr(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 0, rd, 0x67)
}
pub fn beq(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 0, 0x63)
}
pub fn bne(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 1, 0x63)
}
pub fn blt(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 4, 0x63)
}
pub fn bge(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 5, 0x63)
}
pub fn bltu(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 6, 0x63)
}
pub fn bgeu(rs1: u32, rs2: u32, imm: i32) -> u32 {
    enc_b(imm as u32, rs2, rs1, 7, 0x63)
}
pub fn lw(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 2, rd, 0x03)
}
pub fn lh(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 1, rd, 0x03)
}
pub fn lhu(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 5, rd, 0x03)
}
pub fn lb(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 0, rd, 0x03)
}
pub fn lbu(rd: u32, rs1: u32, imm: i32) -> u32 {
    enc_i(imm as u32, rs1, 4, rd, 0x03)
}
pub fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    enc_s(imm as u32, rs2, rs1, 2, 0x23)
}
pub fn sh(rs2: u32, rs1: u32, imm: i32) -> u32 {
    enc_s(imm as u32, rs2, rs1, 1, 0x23)
}
pub fn sb(rs2: u32, rs1: u32, imm: i32) -> u32 {
    enc_s(imm as u32, rs2, rs1, 0, 0x23)
}
pub fn ebreak() -> u32 {
    0x00100073
}
pub fn ecall() -> u32 {
    0x00000073
}
pub fn fence() -> u32 {
    0x0ff0000f
}
pub fn fence_i() -> u32 {
    0x0000100f
}
pub fn csrrw(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | (rs1 << 15) | (1 << 12) | (rd << 7) | 0x73
}
pub fn csrrs(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x73
}
pub fn csrrc(rd: u32, rs1: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | (rs1 << 15) | (3 << 12) | (rd << 7) | 0x73
}
pub fn csrrwi(rd: u32, zimm: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | ((zimm & 0x1f) << 15) | (5 << 12) | (rd << 7) | 0x73
}
pub fn csrrsi(rd: u32, zimm: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | ((zimm & 0x1f) << 15) | (6 << 12) | (rd << 7) | 0x73
}
pub fn csrrci(rd: u32, zimm: u32, csr: u32) -> u32 {
    ((csr & 0xfff) << 20) | ((zimm & 0x1f) << 15) | (7 << 12) | (rd << 7) | 0x73
}
pub fn mret() -> u32 {
    0x30200073
}

// ---------- 仿真驱动 ----------

fn program_image(words: &[u32]) -> LW {
    let mut bits = Vec::with_capacity(512);
    for i in 0..16 {
        let w = words.get(i).copied().unwrap_or(0);
        bits.extend(LW::from_u64(32, w as u64).bits);
    }
    LW { bits }
}

fn program_source() -> String {
    format!(
        "{CORE}\nmodule top(input clk, input rst, input prog_ld, \
         input [511:0] prog, output halted, output [1:0] trap_cause, output [31:0] dbg_pc, \
         output [31:0] dbg_x10, output [31:0] dbg_x11, \
         output [31:0] dbg_mepc, output [31:0] dbg_mcause);\n\
         rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog), \
         .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10), \
         .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));\n\
         endmodule\n"
    )
}

/// 顶层与程序无关（程序经 prog 端口装载），编译一次全局共享。
fn cached_design() -> Result<&'static eda_core::ir::Design, String> {
    static DESIGN: OnceLock<Result<eda_core::ir::Design, String>> = OnceLock::new();
    DESIGN
        .get_or_init(|| eda_hdl::compile(&program_source(), Some("top")))
        .as_ref()
        .map_err(|e| e.clone())
}

struct RunResult {
    #[allow(dead_code)]
    halted: bool,
    #[allow(dead_code)]
    cycles: usize,
    x10: u64,
    x11: u64,
    #[allow(dead_code)]
    pc: u64,
    trap_cause: u64,
    mepc: u64,
    mcause: u64,
}

fn run_program(words: &[u32], max_cycles: usize) -> Result<RunResult, String> {
    let design = cached_design()?;
    let mut sim = Simulator::new(design).map_err(|e| e.to_string())?;
    sim.set_input("clk", &LW::from_u64(1, 0))?;
    sim.set_input("rst", &LW::from_u64(1, 1))?;
    sim.set_input("prog_ld", &LW::from_u64(1, 1))?;
    sim.set_input("prog", &program_image(words))?;
    sim.set_input("clk", &LW::from_u64(1, 1))?; // 复位 + 装载沿
    sim.set_input("clk", &LW::from_u64(1, 0))?;
    sim.set_input("rst", &LW::from_u64(1, 0))?;
    sim.set_input("prog_ld", &LW::from_u64(1, 0))?;
    let mut halted = false;
    let mut cycles = 0;
    for cycle in 0..max_cycles {
        let h = sim
            .read("halted")
            .and_then(|w| w.to_u64())
            .ok_or("halted 未定义")?;
        if h == 1 {
            halted = true;
            cycles = cycle;
            break;
        }
        sim.set_input("clk", &LW::from_u64(1, 1))?;
        sim.set_input("clk", &LW::from_u64(1, 0))?;
    }
    if !halted {
        return Err(format!("程序未在 {max_cycles} 周期内停机"));
    }
    let mepc = sim.read("dbg_mepc").and_then(|w| w.to_u64()).unwrap_or(0);
    let mcause = sim.read("dbg_mcause").and_then(|w| w.to_u64()).unwrap_or(0);
    let trap_cause = sim
        .read("trap_cause")
        .and_then(|w| w.to_u64())
        .ok_or("trap_cause 未定义")?;
    // 未写入的调试寄存器读作 X：归一为 0，断言只在程序写过的寄存器上做。
    let x10 = sim.read("dbg_x10").and_then(|w| w.to_u64()).unwrap_or(0);
    let x11 = sim.read("dbg_x11").and_then(|w| w.to_u64()).unwrap_or(0);
    let pc = sim
        .read("dbg_pc")
        .and_then(|w| w.to_u64())
        .ok_or("dbg_pc 未定义")?;
    Ok(RunResult {
        halted,
        cycles,
        x10,
        x11,
        pc,
        trap_cause,
        mepc,
        mcause,
    })
}

fn expect(got: u64, want: u64, what: &str) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what} = {got:#x}, want {want:#x}"))
    }
}

pub fn register(c: &mut CaseList) {
    arithmetic(c);
    logic(c);
    shifts(c);
    compares(c);
    upper(c);
    branch_loop(c);
    call_return(c);
    memory(c);
    mixed_program(c);
    subword_memory(c);
    system_instructions(c);
    class_tb(c);
    signoff_cases(c);
}

/// SYSTEM 类指令：ECALL/EBREAK 陷阱原因、FENCE/FENCE.I nop、CSR 非法。
fn system_instructions(c: &mut CaseList) {
    c.add("rv32i_ecall_ebreak_trap_causes", || {
        let ecall_prog = vec![addi(10, 0, 111), ecall()];
        let r = run_program(&ecall_prog, 20)?;
        expect(r.trap_cause, 1, "ECALL trap cause")?;
        expect(r.x10, 111, "ECALL pre-trap state")?;

        let ebreak_prog = vec![addi(10, 0, 222), ebreak()];
        let r2 = run_program(&ebreak_prog, 20)?;
        expect(r2.trap_cause, 2, "EBREAK trap cause")?;
        expect(r2.x10, 222, "EBREAK pre-trap state")?;
        Ok(())
    });

    c.add("rv32i_fence_fence_i_are_nops", || {
        // FENCE 与 FENCE.I 均应顺序执行为 nop：pc 前进、状态不变。
        let program = vec![
            addi(1, 0, 5), // 0: x1 = 5
            fence(),       // 1: FENCE (nop)
            addi(2, 0, 6), // 2: x2 = 6
            fence_i(),     // 3: FENCE.I (nop)
            add(10, 1, 2), // 4: x10 = 11
            ebreak(),      // 5
        ];
        let r = run_program(&program, 20)?;
        expect(r.trap_cause, 2, "terminating EBREAK")?;
        expect(r.x10, 11, "FENCE nop semantics")?;
        expect(r.pc, 5 * 4, "pc at EBREAK")?;
        Ok(())
    });

    c.add("rv32i_zicsr_read_modify_write", || {
        // mtvec=0x305, mscratch=0x340, mepc=0x341, mcause=0x342。
        let program = vec![
            addi(1, 0, 0x40),   // 0: x1 = 0x40
            csrrw(2, 1, 0x305), // 1: x2 = old mtvec(0); mtvec = 0x40
            csrrs(3, 1, 0x305), // 2: x3 = 0x40; mtvec |= 0x40（不变）
            addi(4, 0, 0x0f),   // 3: x4 = 0x0f
            csrrs(5, 4, 0x305), // 4: x5 = 0x40; mtvec = 0x4f
            csrrc(6, 4, 0x305), // 5: x6 = 0x4f; mtvec = 0x40
            csrrs(7, 0, 0x305), // 6: x7 = 0x40（rs1=x0：只读不清）
            add(10, 2, 3),      // 7: x10 = 0x40
            addi(11, 6, 0),     // 8: x11 = 0x4f
            ebreak(),           // 9
        ];
        let r = run_program(&program, 40)?;
        expect(r.x10, 0x40, "CSRRW/CSRRS old values")?;
        expect(r.x11, 0x4f, "CSRRC old value")?;
        expect(r.mepc, 9 * 4, "mepc = trap pc")?;
        expect(r.mcause, 3, "mcause = EBREAK")?;
        expect(r.trap_cause, 2, "trap cause EBREAK")?;
        Ok(())
    });

    c.add("rv32i_zicsr_immediate_forms", || {
        let program = vec![
            csrrwi(1, 5, 0x305), // 0: x1 = old(0); mtvec = 5
            csrrsi(2, 3, 0x305), // 1: x2 = 5; mtvec = 5|3 = 7
            csrrci(3, 1, 0x305), // 2: x3 = 7; mtvec = 7 & ~1 = 6
            csrrsi(4, 0, 0x305), // 3: x4 = 6（zimm=0：只读）
            csrrci(5, 0, 0x305), // 4: x5 = 6（zimm=0：只读）
            add(10, 1, 2),       // 5: x10 = 5
            addi(11, 3, 0),      // 6: x11 = 7
            ebreak(),            // 7
        ];
        let r = run_program(&program, 30)?;
        expect(r.x10, 5, "CSRRWI/CSRRSI old values")?;
        expect(r.x11, 7, "CSRRCI old value")?;
        expect(r.mepc, 7 * 4, "mepc")?;
        Ok(())
    });

    c.add("rv32i_zicsr_ecall_records_mcause", || {
        let program = vec![
            addi(1, 0, 0x80),
            csrrw(0, 1, 0x305), // mtvec = 0x80
            ecall(),            // mepc=8, mcause=11
        ];
        let r = run_program(&program, 30)?;
        expect(r.mepc, 8, "ECALL mepc")?;
        expect(r.mcause, 11, "ECALL mcause")?;
        expect(r.trap_cause, 1, "ECALL cause")?;
        Ok(())
    });

    c.add("rv32i_mtvec_trap_jump_and_mret", || {
        // 布局：主程序 word 0-6；handler 在 word 8（字节 0x20）。
        let mut program = vec![
            addi(1, 0, 0x20),    // 0: x1 = 0x20（zimm 仅 5 位，用寄存器形式写 mtvec）
            csrrw(0, 1, 0x305),  // 1: mtvec = 0x20
            csrrsi(0, 8, 0x300), // 2: mstatus.MIE = 1
            ecall(),             // 3: 陷入 → handler（mepc=16, mcause=11）
            addi(1, 1, 1),       // 4: 返回后执行
            csrrci(0, 8, 0x300), // 5: 关 MIE
            ebreak(),            // 6: 停机（terminating EBREAK）
            0,                   // 7
            addi(10, 0, 1),      // 8 (0x20): handler 体（x10 = 1）
            mret(),              // 9: 返回 mepc
        ];
        program.resize(16, 0);
        let r = run_program(&program, 40)?;
        expect(r.x10, 1, "handler executed once")?;
        expect(r.mcause, 3, "final EBREAK mcause (ECALL history = 11)")?;
        expect(r.mepc, 6 * 4, "final EBREAK mepc (ECALL history at 16)")?;
        expect(r.trap_cause, 2, "terminating EBREAK")?;
        Ok(())
    });

    c.add("rv32i_csr_instruction_marks_unsupported", || {
        let program = vec![addi(1, 0, 1), csrrw(0, 1, 0x7c0)];
        let r = run_program(&program, 20)?;
        expect(r.trap_cause, 3, "CSR unsupported cause")?;
        Ok(())
    });
}

/// 子字访存：LB/LBU/LH/LHU 符号/零扩展 + SB/SH 读改写。
fn subword_memory(c: &mut CaseList) {
    // 1. 字节装载：符号扩展与零扩展对比。
    c.add("rv32i_lb_lbu_sign_zero_extend", || {
        let program: Vec<u32> = vec![
            lui(1, 0x80000), // 0: x1 = 0x80000000 基址
            addi(2, 0, -1),  // 1: x2 = 0xFFFFFFFF
            sw(2, 1, 0),     // 2: d[0] = 0xFFFFFFFF
            lbu(3, 1, 0),    // 3: x3 = 0x000000FF
            lb(4, 1, 0),     // 4: x4 = 0xFFFFFFFF
            addi(10, 3, 0),  // 5: x10 = 0xFF
            addi(11, 4, 0),  // 6: x11 = -1
            ebreak(),        // 7
        ];
        let r = run_program(&program, 50)?;
        expect(r.x10, 0xff, "x10 lbu")?;
        expect(r.x11, 0xffff_ffff, "x11 lb sign-extend")?;
        Ok(())
    });

    // 2. 半字装载：低半字/高半字 + 符号/零扩展。
    c.add("rv32i_lh_lhu_halfword_select", || {
        // 简洁程序：存 0xFFFF8001，读各半字
        let program: Vec<u32> = vec![
            lui(1, 0x80000), // 0: base
            addi(2, 0, 1),   // 1: x2 = 1
            lui(3, 0xffff8), // 2: x3 = 0xffff8000
            or_(4, 2, 3),    // 3: x4 = 0xffff8001 — or_ 在后文定义顺序可用
            sw(4, 1, 0),     // 4: d[0] = 0xffff8001
            lhu(5, 1, 0),    // 5: x5 = 0x00008001
            lh(6, 1, 0),     // 6: x6 = 0xffff8001
            lhu(7, 1, 2),    // 7: x7 = 0x0000ffff
            lh(8, 1, 2),     // 8: x8 = 0xffffffff
            add(10, 5, 7),   // 9: x10 = 0x8010+0xffff... 检查用
            add(11, 6, 8),   // 10: x11 = 0xffff8000
            ebreak(),        // 11
        ];
        let r = run_program(&program, 50)?;
        expect(r.x10, 0x8001 + 0xffff, "x10 lhu pair")?;
        expect(r.x11, 0xffff8000u32 as u64, "x11 lh sign pair")?;
        Ok(())
    });

    // 3. 字节/半字存储读改写：其它字节保持。
    c.add("rv32i_sb_sh_read_modify_write", || {
        let program: Vec<u32> = vec![
            lui(1, 0x80000),    // 0: base
            addi(2, 0, -1),     // 1: x2 = 0xffffffff
            sw(2, 1, 0),        // 2: d[0] = 0xffffffff
            addi(3, 0, 0x5a),   // 3: x3 = 0x5a
            sb(3, 1, 1),        // 4: d[0] byte1 = 0x5a → 0xffff5aff
            lbu(4, 1, 0),       // 5: x4 = 0xff
            lbu(5, 1, 1),       // 6: x5 = 0x5a
            addi(6, 0, 0x1234), // 7: x6 = 0x1234（超 12 位；改用 lui+addi）
            0x00000000,         // 8: 占位
        ];
        let _ = program;
        let program: Vec<u32> = vec![
            lui(1, 0x80000),   // 0: base
            addi(2, 0, -1),    // 1: x2 = 0xffffffff
            sw(2, 1, 0),       // 2: d[0] = 0xffffffff
            addi(3, 0, 0x5a),  // 3: x3 = 0x5a
            sb(3, 1, 1),       // 4: d[0] = 0xffff5aff
            lbu(4, 1, 0),      // 5: x4 = 0xff
            lbu(5, 1, 1),      // 6: x5 = 0x5a
            lui(6, 0x1),       // 7: x6 = 0x1000
            addi(6, 6, 0x234), // 8: x6 = 0x1234
            sh(6, 1, 2),       // 9: d[0] = 0x1234_5aff
            lw(7, 1, 0),       // 10: x7 = 0x12345aff
            add(10, 4, 5),     // 11: x10 = 0x159
            addi(11, 7, 0),    // 12: x11 = 0x12345aff
            ebreak(),          // 13
        ];
        let r = run_program(&program, 60)?;
        expect(r.x10, 0x159, "x10 sb bytes")?;
        expect(r.x11, 0x1234_5aff, "x11 sb+sh rmw")?;
        Ok(())
    });
}

fn load32(rd: u32, v: u32) -> Vec<u32> {
    let iv = v as i32;
    if (-2048..=2047).contains(&iv) {
        vec![addi(rd, 0, iv)]
    } else {
        let hi = (v.wrapping_add(0x800)) >> 12;
        let lo = v.wrapping_sub(hi << 12);
        let mut out = vec![lui(rd, hi)];
        if lo != 0 {
            out.push(addi(rd, rd, lo as i16 as i32));
        }
        out
    }
}

fn arithmetic(c: &mut CaseList) {
    for (a, b) in [
        (1u64, 2u64),
        (100, 55),
        (0x7fffffff, 1),
        (0x80000000, 0x80000000),
    ] {
        c.add(format!("rv32i_arith_add_sub_{a:#x}_{b:#x}"), move || {
            let ia = a as u32;
            let ib = b as u32;
            let mut words = Vec::new();
            words.extend(load32(1, ia));
            words.extend(load32(2, ib));
            words.extend([
                add(3, 1, 2),
                sub(4, 3, 1),
                addi(10, 3, 0),
                addi(11, 4, 0),
                ebreak(),
            ]);
            let r = run_program(&words, 50)?;
            expect(r.x10, ia.wrapping_add(ib) as u64, "x10 (add)")?;
            expect(r.x11, ib as u64, "x11 (sub)")?;
            Ok(())
        });
    }
}

fn logic(c: &mut CaseList) {
    c.add("rv32i_logic_ops", || {
        let words = [
            lui(1, 0x12345), // x1 = 0x12345000
            addi(2, 0, -1),  // x2 = 0xffffffff
            xor_(3, 1, 2),
            or_(4, 1, 2),
            and_(5, 1, 2),
            xori(6, 3, 0x0f0),
            ori(7, 3, 0x0f0),
            andi(8, 4, 0x0f0),
            xor_(10, 6, 7),
            and_(11, 8, 5),
            ebreak(),
        ];
        let r = run_program(&words, 50)?;
        let x1 = 0x12345000u32;
        let x2 = 0xffff_ffffu32;
        let x3 = x1 ^ x2;
        let x6 = x3 ^ 0x0f0;
        let x7 = x3 | 0x0f0;
        let x4 = x1 | x2;
        let x8 = x4 & 0x0f0;
        let x5 = x1 & x2;
        expect(r.x10, (x6 ^ x7) as u64, "x10 logic")?;
        expect(r.x11, (x8 & x5) as u64, "x11 logic")?;
        Ok(())
    });
}

fn shifts(c: &mut CaseList) {
    c.add("rv32i_shifts_logical_and_arithmetic", || {
        let words = [
            addi(1, 0, -16), // x1 = 0xfffffff0
            addi(2, 0, 4),
            slli(3, 1, 4),
            srli(4, 1, 4),
            srai(5, 1, 4),
            sll(6, 3, 2),
            srl(7, 4, 2),
            sra(8, 5, 2),
            add(10, 3, 6),
            add(11, 5, 8),
            ebreak(),
        ];
        let r = run_program(&words, 50)?;
        let x1 = 0xffff_fff0u32;
        let x3 = x1 << 4;
        let x6 = x3 << 4; // sll by x2=4
        expect(r.x10, (x3.wrapping_add(x6)) as u64, "x10 shifts")?;
        let x5 = ((x1 as i32) >> 4) as u32;
        let x8 = ((x5 as i32) >> 4) as u32; // sra by x2=4
        expect(r.x11, (x5.wrapping_add(x8)) as u64, "x11 sra")?;
        Ok(())
    });
}

fn compares(c: &mut CaseList) {
    c.add("rv32i_slt_sltu_signed_and_unsigned", || {
        let words = [
            addi(1, 0, -1), // 0xffffffff
            addi(2, 0, 1),
            slt(3, 1, 2),    // signed: -1 < 1 → 1
            sltu(4, 1, 2),   // unsigned: max < 1 → 0
            slti(5, 1, 0),   // -1 < 0 → 1
            sltiu(6, 1, -1), // imm=0xffffffff; 0xffffffff < 0xffffffff → 0
            add(10, 3, 4),
            add(11, 5, 6),
            ebreak(),
        ];
        let r = run_program(&words, 50)?;
        expect(r.x10, 1, "x10 slt/sltu")?;
        expect(r.x11, 1, "x11 slti/sltiu")?;
        Ok(())
    });
}

fn upper(c: &mut CaseList) {
    c.add("rv32i_lui_auipc", || {
        let words = [
            lui(1, 0xdead0),
            auipc(2, 0x1), // pc(8) + 0x1000
            add(3, 1, 2),
            addi(10, 3, 0),
            addi(11, 2, 0),
            ebreak(),
        ];
        let r = run_program(&words, 50)?;
        expect(r.x10, 0xdead0000u64 + 4 + 0x1000, "x10 lui+auipc")?;
        expect(r.x11, 4 + 0x1000, "x11 auipc")?;
        Ok(())
    });
}

fn branch_loop(c: &mut CaseList) {
    for n in [3u64, 5, 10] {
        c.add(format!("rv32i_branch_sum_1_to_{n}"), move || {
            // x1 = sum, x2 = i; 循环 x2 = 1..=n
            let mut words = vec![
                addi(1, 0, 0),            // 0: sum = 0
                addi(2, 0, 1),            // 1: i = 1
                addi(3, 0, n as i32 + 1), // 2: limit = n+1（循环含 i=n）
                add(1, 1, 2),             // 3: sum += i       <- loop
                addi(2, 2, 1),            // 4: i++
                bne(2, 3, -8),            // 5: if i != limit goto 3
                addi(10, 1, 0),           // 6: x10 = sum
                addi(11, 2, 0),           // 7: x11 = i
                ebreak(),                 // 8
            ];
            words.resize(16, 0);
            let r = run_program(&words, 200)?;
            let want = n * (n + 1) / 2;
            expect(r.x10, want, "x10 loop sum")?;
            expect(r.x11, n + 1, "x11 final i (= limit)")?;
            Ok(())
        });
    }
}

fn call_return(c: &mut CaseList) {
    c.add("rv32i_jal_jalr_call_return", || {
        let words = [
            addi(2, 0, 21), // 0: x2 = 21
            jal(1, 20),     // 1: call func at word6, x1 = 8
            addi(4, 3, 0),  // 2: x4 = x3
            addi(10, 4, 0), // 3: x10 = x4
            addi(11, 1, 0), // 4: x11 = return addr
            ebreak(),       // 5
            add(3, 2, 2),   // 6: func: x3 = x2 * 2
            jalr(0, 1, 0),  // 7: return to x1
        ];
        let r = run_program(&words, 50)?;
        expect(r.x10, 42, "x10 call result")?;
        expect(r.x11, 8, "x11 return address")?;
        Ok(())
    });
}

fn memory(c: &mut CaseList) {
    c.add("rv32i_lw_sw_roundtrip", || {
        let words = [
            lui(1, 0x80000), // 0: base = 0x80000000 (镜像到 word 0)
            addi(2, 0, -1),  // 1: x2 = 0xffffffff
            sw(2, 1, 0),     // 2: dmem[0] = x2
            addi(2, 0, 42),  // 3: 覆盖 x2
            lw(3, 1, 0),     // 4: x3 = dmem[0]
            sw(3, 1, 8),     // 5: dmem[2] = x3
            lw(4, 1, 8),     // 6: x4 = dmem[2]
            add(10, 3, 4),   // 7: x10 = 2*0xffffffff
            addi(11, 2, 0),  // 8: x11 = 42
            ebreak(),        // 9
        ];
        let r = run_program(&words, 50)?;
        expect(r.x10, 0xffff_fffeu64, "x10 lw/sw")?;
        expect(r.x11, 42, "x11 overwrite")?;
        Ok(())
    });
}

fn mixed_program(c: &mut CaseList) {
    c.add("rv32i_fibonacci_memory_branch", || {
        // fib: x1=a, x2=b, x3=i, x4=n=10
        let mut words = vec![
            addi(1, 0, 0),   // 0: a = 0
            addi(2, 0, 1),   // 1: b = 1
            addi(3, 0, 0),   // 2: i = 0
            addi(4, 0, 10),  // 3: n = 10
            lui(5, 0x80000), // 4: base
            add(6, 1, 2),    // 5: t = a + b     <- loop
            addi(1, 2, 0),   // 6: a = b
            addi(2, 6, 0),   // 7: b = t
            addi(3, 3, 1),   // 8: i++
            blt(3, 4, -16),  // 9: if i < n goto word5
            addi(2, 2, 0),   // 10: keep b
            sw(2, 5, 0),     // 11: dmem[0] = b
            lw(10, 5, 0),    // 12: x10 = b
            addi(11, 1, 0),  // 13: x11 = a
            ebreak(),        // 14
        ];
        words.resize(32, 0);
        let r = run_program(&words, 200)?;
        // 迭代 10 次后 b = fib(11) = 89, a = fib(10) = 55
        expect(r.x10, 89, "x10 fib(11)")?;
        expect(r.x11, 55, "x11 fib(10)")?;
        Ok(())
    });
}

fn class_tb(c: &mut CaseList) {
    c.add("rv32i_class_testbench_monitor_scoreboard", || {
        let source = format!("{CORE}\n{TB_CLASS}");
        let design = eda_hdl::compile(&source, Some("tb_class")).map_err(|e| e.to_string())?;
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.run_until(600)?;
        let pass = sim
            .read("tb_ok")
            .and_then(|w| w.to_u64())
            .ok_or("tb_ok 未定义")?;
        let done = sim
            .read("tb_done")
            .and_then(|w| w.to_u64())
            .ok_or("tb_done 未定义")?;
        if done != 1 || pass != 1 {
            return Err(format!("class TB done={done} pass={pass}, want 1/1"));
        }
        Ok(())
    });
}

/// RV32I signoff 顶层：暴露程序装载端口与调试输出。
fn signoff_source() -> String {
    format!(
        "{CORE}\nmodule rv32i_top(input clk, input rst, input prog_ld, \
         input [511:0] prog, output halted, output [1:0] trap_cause, output [31:0] dbg_pc, \
         output [31:0] dbg_x10, output [31:0] dbg_x11, \
         output [31:0] dbg_mepc, output [31:0] dbg_mcause);\n\
         rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog), \
         .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10), \
         .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));\n\
         endmodule\n"
    )
}

fn signoff_cases(c: &mut CaseList) {
    // DFT：对 RV32I 核插入并审计一条扫描链。
    c.add("rv32i_dft_scan_insertion_and_audit", || {
        let result = eda_api::insert_rtl_scan(
            &signoff_source(),
            Some("rv32i_top"),
            &eda_dft::ScanOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        if result.scan.chain.is_empty() {
            return Err("scan chain is empty".into());
        }
        if !result.dft.unscanned_sequential.is_empty() {
            return Err(format!(
                "unscanned sequential cells: {:?}",
                result.dft.unscanned_sequential
            ));
        }
        if result.dft.scan_cells != result.scan.chain.len() {
            return Err("scan cell accounting mismatch".to_string());
        }
        if result.scanned_stats.seq < result.original_stats.seq {
            return Err("scan insertion dropped sequencing".into());
        }
        if !result.report.contains("DFT") && !result.report.contains("scan") {
            return Err("scan report missing".to_string());
        }
        Ok(())
    });

    // 综合回归：仅在 if/else 的 else 分支被赋值的寄存器也必须综合出触发器。
    // （曾发现 else 分支 written 集被丢弃 → 2049 位状态只剩 545 位。）
    c.add("rv32i_synth_else_written_regs_get_flops", || {
        let rtl = r#"
module t(input clk, input rst, input en, input [31:0] d, output [31:0] q);
  reg [31:0] a, b;
  always @(posedge clk) begin
    if (rst) begin
      a <= 32'd1;
    end else begin
      if (en) a <= d;
      if (!en) b <= d + 32'd7;
    end
  end
  assign q = a ^ b;
endmodule
"#;
        let design = eda_hdl::compile(rtl, Some("t")).map_err(|e| e.to_string())?;
        let netlist = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: false,
                map_lib: false,
            },
        )
        .map_err(|e| e.to_string())?
        .netlist;
        let stats = netlist.stats();
        // a(32) + b(32) = 64 个触发器；b 仅在 else 分支写入。
        if stats.seq != 64 {
            return Err(format!("seq = {}, want 64", stats.seq));
        }
        Ok(())
    });

    // Foundry signoff：MMMC（fast/slow）实现 + 全项检查。
    #[allow(clippy::redundant_field_names)]
    // TODO(next): 本地化 LEC 已把 2049 触发器等价从「求解器卡死」变为可完成
    // （gold/mapped 均为 2049 flops；dmem16 级设计全点证明通过）。当前 RV32I
    // 在 pc[0] 对应点上报告差异，需判定是映射器真实 bug 还是割点完备性
    // 问题——下一步用差分仿真定位。其余签核项（模型相关/timing/DRC/物理
    // 库/布线 tracks 2048）均已通过。
    // 功耗 + 静态 IR 压降：宽矢量活动格式直接驱动 512 位 prog 端口
    // （无需 power_top 常量封装）。
    c.add("rv32i_power_and_ir_analysis", || {
        let program = "512'h00600093014001130020815300100073";
        let vectors: Vec<_> = [0u64, 1, 2, 3, 4, 5]
            .into_iter()
            .map(|t| eda_api::RtlSimulationVector {
                time_ns: t,
                inputs: [
                    ("clk", (t % 2).to_string()),
                    ("rst", if t == 0 { "1".into() } else { "0".into() }),
                    ("prog_ld", if t == 0 { "1".into() } else { "0".into() }),
                    ("prog", program.to_string()),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            })
            .collect();
        let result = eda_api::analyze_rtl_ir(
            &signoff_source(),
            Some("rv32i_top"),
            &vectors,
            &eda_api::PowerOptions::default(),
            &eda_api::IrOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        if result.power.total_power_uw <= 0.0 {
            return Err("total power must be positive".to_string());
        }
        if result.power.vectors != vectors.len() {
            return Err(format!("activity vectors = {}", result.power.vectors));
        }
        if result.ir.total_current_a <= 0.0 {
            return Err("total current must be positive".to_string());
        }
        if result.ir.worst_drop_v <= 0.0 || result.ir.worst_row.is_none() {
            return Err("IR analysis must identify a worst row".to_string());
        }
        if result.mapped_stats.cells == 0 {
            return Err("mapped cell count must be nonzero".to_string());
        }
        Ok(())
    });

    // 电源网格（power mesh）：RV32I 电流负载的 Gauss-Seidel 网格 IR 求解。
    c.add("rv32i_power_mesh_ir_and_em", || {
        let design =
            eda_hdl::compile(&signoff_source(), Some("rv32i_top")).map_err(|e| e.to_string())?;
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &eda_synth::SynthOptions::default(),
            &eda_core::library::CellLibrary::std(),
        )
        .map_err(|e| e.to_string())?
        .netlist;
        let placement = eda_pnr::place(&netlist).map_err(|e| e.to_string())?;
        // 全一翻转活动（zero→ones 默认对的宽字面量形式）。
        let zero: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::zero(p.bits.len()),
                )
            })
            .collect();
        let ones: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::ones(p.bits.len()),
                )
            })
            .collect();
        let power =
            eda_power::analyze(&netlist, &[zero, ones], &eda_power::PowerOptions::default())
                .map_err(|e| e.to_string())?;
        let options = eda_ir::PowerMeshOptions::default();
        let mesh = eda_ir::analyze_mesh(&netlist, &placement, &power, &options)
            .map_err(|e| e.to_string())?;
        if mesh.total_current_a <= 0.0 {
            return Err("mesh total current must be positive".to_string());
        }
        if mesh.worst_drop_v < 0.0 || !mesh.worst_drop_v.is_finite() {
            return Err("mesh worst drop must be finite".to_string());
        }
        if mesh.nodes.is_empty() || mesh.segments.is_empty() {
            return Err("mesh must contain nodes and segments".to_string());
        }
        if mesh.iterations == 0 {
            return Err("mesh solver must iterate".to_string());
        }
        let report = eda_ir::report_mesh(&mesh);
        if !report.to_lowercase().contains("mesh") {
            return Err("mesh report missing".to_string());
        }
        Ok(())
    });

    // 多层电源网格 + EM 寿命：M8/M7 双层网格与 Black 方程电迁移评估。
    c.add("rv32i_multilayer_mesh_em_lifetime", || {
        let design =
            eda_hdl::compile(&signoff_source(), Some("rv32i_top")).map_err(|e| e.to_string())?;
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &eda_synth::SynthOptions::default(),
            &eda_core::library::CellLibrary::std(),
        )
        .map_err(|e| e.to_string())?
        .netlist;
        let placement = eda_pnr::place(&netlist).map_err(|e| e.to_string())?;
        let zero: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::zero(p.bits.len()),
                )
            })
            .collect();
        let ones: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::ones(p.bits.len()),
                )
            })
            .collect();
        let power =
            eda_power::analyze(&netlist, &[zero, ones], &eda_power::PowerOptions::default())
                .map_err(|e| e.to_string())?;
        let options = eda_ir::MultilayerPowerMeshOptions::default();
        let mesh = eda_ir::analyze_multilayer_mesh(&netlist, &placement, &power, &options)
            .map_err(|e| e.to_string())?;
        if mesh.total_current_a <= 0.0 {
            return Err("multilayer mesh current must be positive".to_string());
        }
        if !mesh.worst_em_lifetime_hours.is_finite() || mesh.worst_em_lifetime_hours <= 0.0 {
            return Err(format!(
                "EM lifetime must be positive and finite, got {}",
                mesh.worst_em_lifetime_hours
            ));
        }
        if mesh.nodes.is_empty() || mesh.segments.is_empty() {
            return Err("multilayer mesh must contain nodes and segments".to_string());
        }
        if mesh.iterations == 0 {
            return Err("multilayer mesh solver must iterate".to_string());
        }
        Ok(())
    });

    // UPF 单域 IR + 域多层网格：RV32I 全设计作为 TOP 域。
    c.add("rv32i_upf_domain_ir_and_mesh", || {
        let design =
            eda_hdl::compile(&signoff_source(), Some("rv32i_top")).map_err(|e| e.to_string())?;
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &eda_synth::SynthOptions::default(),
            &eda_core::library::CellLibrary::std(),
        )
        .map_err(|e| e.to_string())?
        .netlist;
        let placement = eda_pnr::place(&netlist).map_err(|e| e.to_string())?;
        let zero: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::zero(p.bits.len()),
                )
            })
            .collect();
        let ones: eda_power::InputVector = netlist
            .ports
            .iter()
            .filter(|p| p.dir == eda_core::netlist::Dir::In)
            .map(|p| {
                (
                    p.name.clone(),
                    eda_core::logic::LogicWord::ones(p.bits.len()),
                )
            })
            .collect();
        let power =
            eda_power::analyze(&netlist, &[zero, ones], &eda_power::PowerOptions::default())
                .map_err(|e| e.to_string())?;
        let upf_text = r#"
upf_version 2.0
set_design_top rv32i_top
create_power_domain TOP -elements {rv32i_top}
create_supply_port VDD -domain TOP
create_supply_port VSS -domain TOP
"#;
        let upf = eda_upf::parse_upf(upf_text).map_err(|e| e.to_string())?;
        let result = eda_ir::analyze_upf(
            &netlist,
            &placement,
            &power,
            &upf,
            &eda_ir::UpfIrOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        if result.total_current_a <= 0.0 {
            return Err("UPF total current must be positive".to_string());
        }
        if result.domains.is_empty() {
            return Err("UPF result must contain the TOP domain".to_string());
        }
        let meshes = eda_ir::analyze_upf_domain_multilayer_meshes(
            &netlist,
            &placement,
            &result,
            &eda_ir::MultilayerPowerMeshOptions::default(),
        )
        .map_err(|e| e.to_string())?;
        if meshes.len() != result.domains.len() {
            return Err("per-domain mesh count mismatch".to_string());
        }
        let cell_counts: std::collections::HashMap<_, _> = result
            .domains
            .iter()
            .map(|d| (d.name.clone(), d.cell_count))
            .collect();
        let top_mesh = meshes.iter().find(|m| m.power_domain == "TOP");
        let Some(top) = top_mesh else {
            return Err("TOP domain mesh missing".to_string());
        };
        let Some(inner) = &top.mesh else {
            return Err("TOP domain must produce a multilayer mesh".to_string());
        };
        if inner.nodes.is_empty() {
            return Err("TOP domain mesh must contain nodes".to_string());
        }
        // 有单元的域必须产出网格；空域（如自动插入的 DEFAULT）允许为空。
        for mesh in &meshes {
            if cell_counts.get(&mesh.power_domain).copied().unwrap_or(0) > 0 && mesh.mesh.is_none()
            {
                return Err(format!(
                    "nonempty domain '{}' must produce a multilayer mesh",
                    mesh.power_domain
                ));
            }
        }
        Ok(())
    });

    // CTS：综合后网表插入时钟树并审计 skew/插入缓冲。
    c.add("rv32i_cts_inserts_and_audits_clock_tree", || {
        let design =
            eda_hdl::compile(&signoff_source(), Some("rv32i_top")).map_err(|e| e.to_string())?;
        let netlist = eda_synth::synthesize_with_library(
            &design,
            &eda_synth::SynthOptions::default(),
            &eda_core::library::CellLibrary::std(),
        )
        .map_err(|e| e.to_string())?
        .netlist;
        let result = eda_cts::synthesize(&netlist, &eda_cts::CtsOptions::default())
            .map_err(|e| e.to_string())?;
        if result.trees.is_empty() {
            return Err("no clock trees synthesized".to_string());
        }
        if result.buffers_inserted == 0 {
            return Err("no clock buffers inserted".to_string());
        }
        if result.netlist.stats().seq < netlist.stats().seq {
            return Err("CTS dropped sequential cells".to_string());
        }
        let report = eda_cts::report(&result);
        if !report.contains("CTS") && !report.contains("clock") {
            return Err("CTS report missing".to_string());
        }
        Ok(())
    });

    // 分段签核：物理签核（模型相关/timing/DRC/物理库）在套件内常开；
    // LEC 作为独立交付项单独跟踪（见 verify/ 与 README）。
    c.add("rv32i_pdk_physical_signoff_mmmc_pass", || {
        let manifest = r#"pdk rv32i-signoff-pdk
version 1.0
layer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14 tracks 2048
corner fast process FF voltage_v 1.9 temperature_c -40 delay_scale 0.8
corner slow process SS voltage_v 1.6 temperature_c 125 delay_scale 1.25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff timing
signoff drc
signoff physical-database
.end
"#;
        let pdk = eda_api::parse_pdk(manifest).map_err(|e| e.to_string())?;
        let result = eda_api::run_pdk_signoff(&signoff_source(), Some("rv32i_top"), &pdk)
            .map_err(|e| e.to_string())?;
        if !result.passed {
            return Err(format!("physical signoff failed: {:?}", result.checks));
        }
        if result.corner_results.len() != 2 {
            return Err("expected two corner results".to_string());
        }
        if result.drc_errors != 0 {
            return Err("physical signoff must be DRC clean".to_string());
        }
        if result.critical_path_delay_ns <= 0.0 {
            return Err("missing critical path delay".to_string());
        }
        let report = eda_api::report_pdk_signoff(&result);
        if !report.contains("PDK corner result fast")
            || !report.contains("PDK corner result slow")
            || !report.contains("Overall signoff: PASS")
        {
            return Err("physical signoff report missing verdicts".to_string());
        }
        Ok(())
    });

    const FULL_SIGNOFF_ENABLED: bool = true;
    if FULL_SIGNOFF_ENABLED {
        c.add("rv32i_pdk_signoff_mmmc_pass", || {
            let manifest = r#"pdk rv32i-signoff-pdk
version 1.0
layer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14 tracks 2048
corner fast process FF voltage_v 1.9 temperature_c -40 delay_scale 0.8
corner slow process SS voltage_v 1.6 temperature_c 125 delay_scale 1.25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#;
            let pdk = eda_api::parse_pdk(manifest).map_err(|e| e.to_string())?;
            let result = eda_api::run_pdk_signoff(&signoff_source(), Some("rv32i_top"), &pdk)
                .map_err(|e| e.to_string())?;
            if !result.passed {
                return Err(format!("signoff failed: {:?}", result.checks));
            }
            if result.corner_results.len() != 2 {
                return Err(format!(
                    "expected 2 corner results, got {}",
                    result.corner_results.len()
                ));
            }
            if !result.equivalent || result.drc_errors != 0 {
                return Err("equivalence or DRC failed".into());
            }
            if result.critical_path_delay_ns <= 0.0 {
                return Err("missing critical path delay".into());
            }
            let report = eda_api::report_pdk_signoff(&result);
            if !report.contains("PDK corner result fast")
                || !report.contains("PDK corner result slow")
                || !report.contains("Overall signoff: PASS")
            {
                return Err("signoff report missing corner/verdict lines".to_string());
            }
            Ok(())
        });
    }
}
