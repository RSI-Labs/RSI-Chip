//! cpu8086 — 从门级到系统级的完整回归
//!
//! 层次覆盖:
//! 1. 门级行波进位加法器 vs 行为级参考交叉验证（含进位链中间节点）
//! 2. ALU 全标志向量（CF/PF/AF/ZF/SF/OF）
//! 3. 完整程序在 sys8086（CPU+RAM+IO）上运行：算术 / 循环 / 访存 / 溢出
//! 4. VCD 波形导出

use crate::CaseList;
use eda_core::ir::Design;
use eda_core::logic::LogicWord as LW;
use eda_sim::Simulator;
use std::sync::OnceLock;

const PRIM: &str = include_str!("../../../cpu8086/rtl/primitives.v");
const RIPPLE: &str = include_str!("../../../cpu8086/rtl/ripple_adder16.v");
const ALU: &str = include_str!("../../../cpu8086/rtl/alu16.v");
const REGFILE: &str = include_str!("../../../cpu8086/rtl/regfile16.v");
const CPU: &str = include_str!("../../../cpu8086/rtl/cpu8086.v");
const RAM: &str = include_str!("../../../cpu8086/rtl/ram64x16.v");
const SYS: &str = include_str!("../../../cpu8086/rtl/sys8086.v");
const TB_ALU: &str = include_str!("../../../cpu8086/tb/tb_alu16.v");
const EQUIV: &str = include_str!("../../../cpu8086/verify/equiv_adder16.v");

fn sys_src() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| format!("{PRIM}\n{RIPPLE}\n{ALU}\n{REGFILE}\n{CPU}\n{RAM}\n{SYS}\n"))
}

fn alu_src() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| format!("{PRIM}\n{RIPPLE}\n{ALU}\n{TB_ALU}\n"))
}

fn adder_src() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| format!("{PRIM}\n{RIPPLE}\n{EQUIV}\n"))
}

fn cache(src: &'static str, top: &'static str) -> &'static Design {
    static DESIGNS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<(&'static str, &'static str), &'static Design>>,
    > = OnceLock::new();
    let m = DESIGNS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = m.lock().unwrap();
    let key = (src, top);
    g.entry(key).or_insert_with(|| {
        Box::leak(Box::new(
            eda_hdl::compile(src, Some(top))
                .unwrap_or_else(|e| panic!("cpu8086 compile({top}): {e}")),
        ))
    })
}

/// 把 64 个 16 位程序字打包成 1024 位装载映像（word i 位于 prog[16i+15:16i]）。
fn prog_image(words: &[u16]) -> LW {
    let mut bits = Vec::with_capacity(1024);
    for i in 0..64 {
        let w = words.get(i).copied().unwrap_or(0);
        bits.extend(LW::from_u64(16, w as u64).bits);
    }
    LW { bits }
}

fn set16(sim: &mut Simulator, port: &str, v: u64) -> Result<(), String> {
    sim.set_input(port, &LW::from_u64(16, v))
}

fn edge(sim: &mut Simulator) -> Result<(), String> {
    sim.set_input("clk", &LW::from_u64(1, 0))?;
    sim.set_input("clk", &LW::from_u64(1, 1))
}

struct RunResult {
    halted: bool,
    io_valid: bool,
    io_out: u64,
}

/// 复位 + 装载程序 + 运行至停机（或超时）。
fn run_program(
    words: &[u16],
    max_cycles: usize,
) -> Result<(RunResult, Simulator<'static>), String> {
    let d = cache(sys_src(), "sys8086");
    let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
    sim.set_input("clk", &LW::from_u64(1, 0))?;
    sim.set_input("rst", &LW::from_u64(1, 1))?;
    sim.set_input("prog_ld", &LW::from_u64(1, 1))?;
    sim.set_input("prog", &prog_image(words))?;
    edge(&mut sim)?; // 复位并载入程序
    sim.set_input("rst", &LW::from_u64(1, 0))?;
    sim.set_input("prog_ld", &LW::from_u64(1, 0))?;
    let mut halted = false;
    for _ in 0..max_cycles {
        let h = sim
            .read("halted")
            .and_then(|w| w.to_u64())
            .ok_or("halted 未定义")?;
        if h == 1 {
            halted = true;
            break;
        }
        edge(&mut sim)?;
    }
    // 最后一个下降沿，让输出寄存器稳定
    sim.set_input("clk", &LW::from_u64(1, 0))?;
    let io_valid = sim
        .read_port("io_valid")
        .and_then(|w| w.to_u64())
        .ok_or("io_valid 未定义")?;
    let io_out = sim
        .read_port("io_out")
        .and_then(|w| w.to_u64())
        .ok_or("io_out 未定义")?;
    Ok((
        RunResult {
            halted,
            io_valid: io_valid == 1,
            io_out,
        },
        sim,
    ))
}

fn expect_net(sim: &Simulator, name: &str, want: u64) -> Result<(), String> {
    match sim.read(name).and_then(|w| w.to_u64()) {
        Some(got) if got == want => Ok(()),
        Some(got) => Err(format!("net {name} = {got:#x}, want {want:#x}")),
        None => Err(format!(
            "net {name} = X 或不存在（快照含 {} 个声明）",
            sim.snapshot().len()
        )),
    }
}

fn expect_flag(sim: &Simulator, flag: &str, want: bool) -> Result<(), String> {
    let name = format!("u_cpu.{flag}");
    match sim.read(&name).and_then(|w| w.to_u64()) {
        Some(got) if got == (want as u64) => Ok(()),
        Some(got) => Err(format!("flag {flag} = {got}, want {want}")),
        None => Err(format!("flag {flag} 未定义")),
    }
}

struct AluVec {
    a: u64,
    b: u64,
    sel: u64,
    y: u64,
    cf: bool,
    pf: bool,
    af: bool,
    zf: bool,
    sf: bool,
    of: bool,
}

fn run_alu_vec(v: AluVec) -> Result<(), String> {
    let d = cache(alu_src(), "tb_alu_top");
    let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
    set16(&mut sim, "a", v.a)?;
    set16(&mut sim, "b", v.b)?;
    sim.set_input("sel", &LW::from_u64(4, v.sel))?;
    let checks = [
        ("y", v.y),
        ("cf", v.cf as u64),
        ("pf", v.pf as u64),
        ("af", v.af as u64),
        ("zf", v.zf as u64),
        ("sf", v.sf as u64),
        ("of", v.of as u64),
    ];
    for (port, want) in checks {
        let got = sim
            .read_port(port)
            .and_then(|w| w.to_u64())
            .unwrap_or(u64::MAX);
        if got != want {
            return Err(format!(
                "alu a={:#06x} b={:#06x} sel={}: {} = {got}, want {want}",
                v.a, v.b, v.sel, port
            ));
        }
    }
    Ok(())
}

struct AdderVec {
    a: u64,
    b: u64,
    cin: u64,
    sum: u64,
    cout: u64,
    c14: u64,
    c3: u64,
}

fn run_adder_gate(v: AdderVec) -> Result<(), String> {
    let d = cache(adder_src(), "add16_gate");
    let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
    set16(&mut sim, "a", v.a)?;
    set16(&mut sim, "b", v.b)?;
    sim.set_input("cin", &LW::from_u64(1, v.cin))?;
    let checks = [
        ("sum", v.sum),
        ("cout", v.cout),
        ("c14", v.c14),
        ("c3", v.c3),
    ];
    for (port, want) in checks {
        let got = sim
            .read_port(port)
            .and_then(|w| w.to_u64())
            .unwrap_or(u64::MAX);
        if got != want {
            return Err(format!(
                "adder a={:#06x} b={:#06x} cin={}: {} = {got}, want {want}",
                v.a, v.b, v.cin, port
            ));
        }
    }
    Ok(())
}

const PROG_ARITH: [u16; 7] = [
    0x1000, // MVI AX, imm
    0x0005, //   5
    0x1600, // MVI BX, imm
    0x0003, //   3
    0x20C0, // ADD AX, BX
    0xD000, // OUT 0, AX
    0xE000, // HLT
];

const PROG_LOOP: [u16; 10] = [
    0x1200, // MVI CX, 3
    0x0003, 0x1000, // MVI AX, 0
    0x0000, 0x2040, // ADD AX, CX   (loop @4)
    0x8200, // DEC CX
    0xC001, // JNZ 4
    0x0004, 0xD000, // OUT 0, AX
    0xE000, // HLT
];

const PROG_MEM: [u16; 10] = [
    0x1C00, // MVI SI, 10
    0x000A, 0x1000, // MVI AX, 0x1234
    0x1234, 0xF181, // ST AX, [SI] (rs=SI)
    0x1000, // MVI AX, 0
    0x0000, 0xF180, // LD AX, [SI] (rs=SI)
    0xD000, // OUT 0, AX
    0xE000, // HLT
];

const PROG_OVERFLOW: [u16; 5] = [
    0x1000, // MVI AX, 0x7FFF
    0x7FFF, 0x7000, // INC AX
    0xD000, // OUT 0, AX
    0xE000, // HLT
];

pub fn register(c: &mut CaseList) {
    // ---------- 门级加法器交叉验证（含进位链内部节点） ----------
    c.add("cpu8086_adder_zero", || {
        run_adder_gate(AdderVec {
            a: 0,
            b: 0,
            cin: 0,
            sum: 0,
            cout: 0,
            c14: 0,
            c3: 0,
        })
    });
    c.add("cpu8086_adder_carry_chain", || {
        run_adder_gate(AdderVec {
            a: 0xFFFF,
            b: 0x0001,
            cin: 0,
            sum: 0x0000,
            cout: 1,
            c14: 1,
            c3: 1,
        })
    });
    c.add("cpu8086_adder_cin_propagation", || {
        run_adder_gate(AdderVec {
            a: 0xFFFF,
            b: 0xFFFF,
            cin: 1,
            sum: 0xFFFF,
            cout: 1,
            c14: 1,
            c3: 1,
        })
    });
    c.add("cpu8086_adder_c3_aux_carry", || {
        // 8+8: bit3 产生进位（AF 物理来源），bit15 无进位
        run_adder_gate(AdderVec {
            a: 0x0008,
            b: 0x0008,
            cin: 0,
            sum: 0x0010,
            cout: 0,
            c14: 0,
            c3: 1,
        })
    });
    c.add("cpu8086_adder_c14_msb_carry", || {
        // 0x4000+0x4000: 进位恰好进入 bit15（OF 判据物理来源）
        run_adder_gate(AdderVec {
            a: 0x4000,
            b: 0x4000,
            cin: 0,
            sum: 0x8000,
            cout: 0,
            c14: 1,
            c3: 0,
        })
    });
    c.add("cpu8086_adder_asymmetric", || {
        run_adder_gate(AdderVec {
            a: 0x1234,
            b: 0x5678,
            cin: 0,
            sum: 0x68AC,
            cout: 0,
            c14: 0,
            c3: 0,
        })
    });

    // ---------- ALU 全标志向量 ----------
    c.add("cpu8086_alu_add_carry_zero_result", || {
        run_alu_vec(AluVec {
            a: 0xFFFF,
            b: 0x0001,
            sel: 0,
            y: 0x0000,
            cf: true,
            pf: true,
            af: true,
            zf: true,
            sf: false,
            of: false,
        })
    });
    c.add("cpu8086_alu_add_signed_overflow", || {
        run_alu_vec(AluVec {
            a: 0x7FFF,
            b: 0x0001,
            sel: 0,
            y: 0x8000,
            cf: false,
            pf: true,
            af: true,
            zf: false,
            sf: true,
            of: true,
        })
    });
    c.add("cpu8086_alu_sub_borrow", || {
        // 5-3=2: 无借价；低半字节 5-3 无借价
        run_alu_vec(AluVec {
            a: 0x0005,
            b: 0x0003,
            sel: 1,
            y: 0x0002,
            cf: false,
            pf: false,
            af: false,
            zf: false,
            sf: false,
            of: false,
        })
    });
    c.add("cpu8086_alu_sub_negative_overflow", || {
        // 0x8000-1 = 0x7FFF: 负-正→正，有符号溢出 OF=1，无借价 CF=0
        run_alu_vec(AluVec {
            a: 0x8000,
            b: 0x0001,
            sel: 1,
            y: 0x7FFF,
            cf: false,
            pf: true,
            af: true,
            zf: false,
            sf: false,
            of: true,
        })
    });
    c.add("cpu8086_alu_sub_borrow_from_msb", || {
        // 3-5: 借价 CF=1，结果 0xFFFE
        run_alu_vec(AluVec {
            a: 0x0003,
            b: 0x0005,
            sel: 1,
            y: 0xFFFE,
            cf: true,
            pf: false,
            af: true,
            zf: false,
            sf: true,
            of: false,
        })
    });
    c.add("cpu8086_alu_and", || {
        run_alu_vec(AluVec {
            a: 0xFF00,
            b: 0x0F0F,
            sel: 2,
            y: 0x0F00,
            cf: false,
            pf: true,
            af: false,
            zf: false,
            sf: false,
            of: false,
        })
    });
    c.add("cpu8086_alu_or_all_ones", || {
        run_alu_vec(AluVec {
            a: 0x0F0F,
            b: 0xF0F0,
            sel: 3,
            y: 0xFFFF,
            cf: false,
            pf: true,
            af: false,
            zf: false,
            sf: true,
            of: false,
        })
    });
    c.add("cpu8086_alu_xor_self_zero", || {
        run_alu_vec(AluVec {
            a: 0xABCD,
            b: 0xABCD,
            sel: 4,
            y: 0x0000,
            cf: false,
            pf: true,
            af: false,
            zf: true,
            sf: false,
            of: false,
        })
    });
    c.add("cpu8086_alu_shl_carry_out", || {
        run_alu_vec(AluVec {
            a: 0x8000,
            b: 0,
            sel: 5,
            y: 0x0000,
            cf: true,
            pf: true,
            af: false,
            zf: true,
            sf: false,
            of: true,
        })
    });
    c.add("cpu8086_alu_shr_lsb", || {
        run_alu_vec(AluVec {
            a: 0x0001,
            b: 0,
            sel: 6,
            y: 0x0000,
            cf: true,
            pf: true,
            af: false,
            zf: true,
            sf: false,
            of: false,
        })
    });
    c.add("cpu8086_alu_rol_wrap", || {
        run_alu_vec(AluVec {
            a: 0x8001,
            b: 0,
            sel: 7,
            y: 0x0003,
            cf: true,
            pf: true,
            af: false,
            zf: false,
            sf: false,
            of: true,
        })
    });
    c.add("cpu8086_alu_ror_wrap", || {
        run_alu_vec(AluVec {
            a: 0x0003,
            b: 0,
            sel: 8,
            y: 0x8001,
            cf: true,
            pf: false,
            af: false,
            zf: false,
            sf: true,
            of: false,
        })
    });

    // ---------- 完整程序仿真 ----------
    c.add("cpu8086_prog_arith", || {
        let (r, sim) = run_program(&PROG_ARITH, 100)?;
        if !r.halted {
            return Err("程序未在超时前停机".into());
        }
        if !r.io_valid || r.io_out != 8 {
            return Err(format!("io = {:#x} valid={}，期望 8", r.io_out, r.io_valid));
        }
        expect_net(&sim, "u_cpu.u_regs.r0", 8)?; // AX
        expect_net(&sim, "u_cpu.u_regs.r3", 3)?; // BX
        expect_flag(&sim, "zf", false)?;
        expect_flag(&sim, "sf", false)?;
        expect_flag(&sim, "cf", false)?;
        expect_flag(&sim, "of", false)?;
        Ok(())
    });

    c.add("cpu8086_prog_loop", || {
        let (r, sim) = run_program(&PROG_LOOP, 200)?;
        if !r.halted {
            return Err("循环程序未在超时前停机".into());
        }
        if !r.io_valid || r.io_out != 6 {
            return Err(format!("io = {:#x}，期望 6 (3+2+1)", r.io_out));
        }
        expect_net(&sim, "u_cpu.u_regs.r0", 6)?; // AX = 3+2+1
        expect_net(&sim, "u_cpu.u_regs.r1", 0)?; // CX 计数归零
        Ok(())
    });

    c.add("cpu8086_prog_mem", || {
        let (r, sim) = run_program(&PROG_MEM, 200)?;
        if !r.halted {
            return Err("访存程序未在超时前停机".into());
        }
        if !r.io_valid || r.io_out != 0x1234 {
            return Err(format!("io = {:#x}，期望 0x1234", r.io_out));
        }
        expect_net(&sim, "u_ram.m10", 0x1234)?; // ST 写入
        expect_net(&sim, "u_cpu.u_regs.r0", 0x1234)?; // LD 读回
        Ok(())
    });

    c.add("cpu8086_prog_inc_overflow", || {
        let (r, sim) = run_program(&PROG_OVERFLOW, 100)?;
        if !r.halted {
            return Err("溢出程序未在超时前停机".into());
        }
        if !r.io_valid || r.io_out != 0x8000 {
            return Err(format!("io = {:#x}，期望 0x8000", r.io_out));
        }
        expect_flag(&sim, "of", true)?; // 正 + 1 → 负
        expect_flag(&sim, "sf", true)?;
        expect_flag(&sim, "cf", false)?; // INC 不影响 CF
        Ok(())
    });

    // ---------- 波形 ----------
    c.add("cpu8086_waveform_export", || {
        let d = cache(sys_src(), "sys8086");
        let mut sim = Simulator::new(d).map_err(|e| e.to_string())?;
        sim.enable_vcd();
        sim.set_input("clk", &LW::from_u64(1, 0))?;
        sim.set_input("rst", &LW::from_u64(1, 1))?;
        sim.set_input("prog_ld", &LW::from_u64(1, 1))?;
        sim.set_input("prog", &prog_image(&PROG_ARITH))?;
        for _ in 0..20 {
            edge(&mut sim)?;
        }
        let vcd = sim.vcd().ok_or("未生成 VCD")?;
        if !vcd.contains("$var") || !vcd.contains("u_cpu") {
            return Err("VCD 缺少变量或层次名".into());
        }
        Ok(())
    });
}
