//! Gate-level regression for the CPU platform cores.
//!
//! The instruction matrices in `i8086_opcode_matrix.rs` and `program_fixtures.rs`
//! execute the SystemVerilog RTL.  This file takes the same i8086 core through
//! in-tree synthesis + standard-cell mapping, converts the mapped netlist back
//! into the simulator IR, and executes a representative program at gate level.
//!
//! The RV32I variant (the `rv32i/rtl/rv32i_core.v` signoff vehicle) runs the
//! same synthesis -> mapped netlist -> `netlist_to_design` path.

use eda_core::library::CellLibrary;
use eda_core::logic::LogicWord;
use eda_core::to_design::{netlist_to_design, netlist_to_design_with_timing};
use eda_cpu_platform::SparseMemory;
use eda_sim::Simulator;
use std::sync::OnceLock;

const RV32I_SIGNOFF_CORE: &str = include_str!("../../../rv32i/rtl/rv32i_core.v");
const I8086_SOURCE: &str = concat!(
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_full_core.sv"),
);

fn i8086_gate() -> &'static eda_core::ir::Design {
    static CACHE: OnceLock<eda_core::ir::Design> = OnceLock::new();
    CACHE.get_or_init(|| {
        let rtl =
            eda_hdl::compile(I8086_SOURCE, Some("i8086_full_core")).expect("compile i8086 RTL");
        let netlist = eda_synth::synthesize_with_library(
            &rtl,
            &eda_synth::SynthOptions::default(),
            &CellLibrary::std(),
        )
        .expect("synthesize i8086 RTL")
        .netlist;
        netlist_to_design(&netlist)
    })
}

fn rv32i_signoff_gate() -> &'static eda_core::ir::Design {
    static CACHE: OnceLock<eda_core::ir::Design> = OnceLock::new();
    CACHE.get_or_init(|| {
        let source = format!(
            "{RV32I_SIGNOFF_CORE}\n\
             module top(input clk, input rst, input prog_ld, input [511:0] prog, \
             output halted, output [1:0] trap_cause, output [31:0] dbg_pc, \
             output [31:0] dbg_x10, output [31:0] dbg_x11, \
             output [31:0] dbg_mepc, output [31:0] dbg_mcause);\n\
             rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog), \
             .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10), \
             .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));\n\
             endmodule\n"
        );
        let rtl = eda_hdl::compile(&source, Some("top")).expect("compile RV32I signoff RTL");
        let netlist = eda_synth::synthesize_with_library(
            &rtl,
            &eda_synth::SynthOptions::default(),
            &CellLibrary::std(),
        )
        .expect("synthesize RV32I signoff core")
        .netlist;
        netlist_to_design(&netlist)
    })
}

fn rv32i_signoff_gate_timed() -> &'static eda_core::ir::Design {
    static CACHE: OnceLock<eda_core::ir::Design> = OnceLock::new();
    CACHE.get_or_init(|| {
        let source = format!(
            "{RV32I_SIGNOFF_CORE}\n\
             module top(input clk, input rst, input prog_ld, input [511:0] prog, \
             output halted, output [1:0] trap_cause, output [31:0] dbg_pc, \
             output [31:0] dbg_x10, output [31:0] dbg_x11, \
             output [31:0] dbg_mepc, output [31:0] dbg_mcause);\n\
             rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog), \
             .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10), \
             .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));\n\
             endmodule\n"
        );
        let rtl = eda_hdl::compile(&source, Some("top")).expect("compile RV32I signoff RTL");
        let netlist = eda_synth::synthesize_with_library(
            &rtl,
            &eda_synth::SynthOptions::default(),
            &CellLibrary::std(),
        )
        .expect("synthesize RV32I signoff core")
        .netlist;
        let sdf_text = eda_sdf::write_sdf(&netlist).expect("write SDF");
        let sdf = eda_sdf::parse_sdf(&sdf_text).expect("parse SDF");
        let annotated = eda_sdf::annotate_sdf(&netlist, &sdf).expect("annotate SDF");
        netlist_to_design_with_timing(&annotated)
    })
}

fn rv32i_program_image(words: &[u32]) -> LogicWord {
    let mut bits = Vec::new();
    for index in 0..16 {
        let word = words.get(index).copied().unwrap_or(0);
        bits.extend(LogicWord::from_u64(32, u64::from(word)).bits);
    }
    LogicWord { bits }
}

fn run_rv32i_signoff_gate(words: &[u32], max_cycles: u64) -> Result<(u32, u64), String> {
    let design = rv32i_signoff_gate();
    let mut sim = Simulator::new(design).map_err(|error| error.to_string())?;
    sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
    sim.set_input("rst", &LogicWord::from_u64(1, 1))?;
    sim.set_input("prog_ld", &LogicWord::from_u64(1, 1))?;
    sim.set_input("prog", &rv32i_program_image(words))?;
    sim.set_input("clk", &LogicWord::from_u64(1, 1))?;
    sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
    sim.set_input("rst", &LogicWord::from_u64(1, 0))?;
    sim.set_input("prog_ld", &LogicWord::from_u64(1, 0))?;
    for _ in 0..max_cycles {
        if value(&sim, "halted") == 1 {
            return Ok((value(&sim, "dbg_x10") as u32, value(&sim, "dbg_pc")));
        }
        sim.set_input("clk", &LogicWord::from_u64(1, 1))?;
        sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
    }
    Err(format!(
        "gate-level RV32I signoff core did not halt (x10={})",
        value(&sim, "dbg_x10")
    ))
}

fn value(sim: &Simulator<'_>, name: &str) -> u64 {
    sim.read_port(name)
        .or_else(|| sim.read(name))
        .and_then(|word| word.to_u64())
        .unwrap_or(0)
}

fn supply_i8086_reads(sim: &mut Simulator<'_>, memory: &SparseMemory) -> Result<(), String> {
    let mem_address = value(sim, "mem_addr") as u32;
    let source_address = value(sim, "source_addr") as u32;
    sim.set_input(
        "mem_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(mem_address))),
    )?;
    sim.set_input(
        "source_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(source_address))),
    )?;
    Ok(())
}

struct I8086GateResult {
    halted: bool,
    ax: u16,
    bx: u16,
    ip: u16,
}

fn run_i8086_gate(program: &[u8], max_cycles: u64) -> Result<I8086GateResult, String> {
    run_i8086_design(i8086_gate(), program, max_cycles)
}

fn run_i8086_design(
    design: &eda_core::ir::Design,
    program: &[u8],
    max_cycles: u64,
) -> Result<I8086GateResult, String> {
    let mut sim = Simulator::new(design).map_err(|error| error.to_string())?;
    let mut memory = SparseMemory::new();
    memory.write_slice(0x0100, program);

    sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
    sim.set_input("rst", &LogicWord::from_u64(1, 1))?;
    sim.set_input("init_ip", &LogicWord::from_u64(16, 0x0100))?;
    sim.set_input("init_sp", &LogicWord::from_u64(16, 0xfffe))?;
    for name in ["init_cs", "init_ds", "init_es", "init_ss"] {
        sim.set_input(name, &LogicWord::from_u64(16, 0))?;
    }
    sim.set_input("irq", &LogicWord::from_u64(1, 0))?;
    sim.set_input("nmi", &LogicWord::from_u64(1, 0))?;
    sim.set_input("interrupt_vector", &LogicWord::from_u64(8, 0))?;
    sim.set_input("mem_rdata", &LogicWord::from_u64(16, 0))?;
    sim.set_input("source_rdata", &LogicWord::from_u64(16, 0))?;
    sim.set_input("io_rdata", &LogicWord::from_u64(16, 0))?;

    let mut now = 0_u64;
    for cycle in 0..max_cycles {
        // Low phase: present memory reads and sample the write request.
        sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
        sim.run_until(now)?;
        supply_i8086_reads(&mut sim, &memory)?;
        let pending_write = value(&sim, "mem_we") == 1;
        let write_address = value(&sim, "mem_addr") as u32;
        let write_data = value(&sim, "mem_wdata") as u16;
        let write_byte = value(&sim, "mem_width") == 0;

        // Rising edge; reset is held for the first two cycles.
        sim.set_input("rst", &LogicWord::from_u64(1, u64::from(cycle < 2)))?;
        sim.set_input("clk", &LogicWord::from_u64(1, 1))?;
        sim.run_until(now)?;
        supply_i8086_reads(&mut sim, &memory)?;

        if pending_write {
            if write_byte {
                memory.write_u8(write_address, write_data as u8);
            } else {
                memory.write_le_u16(write_address, write_data);
            }
        }

        if value(&sim, "halted") == 1 {
            return Ok(I8086GateResult {
                halted: true,
                ax: value(&sim, "dbg_ax") as u16,
                bx: value(&sim, "dbg_bx") as u16,
                ip: value(&sim, "dbg_ip") as u16,
            });
        }

        // Falling edge at the next time slot.
        now += 1;
        sim.set_input("clk", &LogicWord::from_u64(1, 0))?;
        sim.run_until(now)?;
    }
    Err(format!(
        "gate-level i8086 program did not halt within {max_cycles} cycles (ax={})",
        value(&sim, "dbg_ax")
    ))
}

#[test]
fn synthesized_dff_toggles() {
    let source = "module t(input clk, input rst, input d, output reg q);\n\
                  always @(posedge clk) begin\n\
                    if (rst) q <= 1'b0;\n\
                    else q <= d;\n\
                  end\n\
                  endmodule\n";
    let rtl = eda_hdl::compile(source, Some("t")).expect("compile");
    let netlist = eda_synth::synthesize_with_library(
        &rtl,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("synthesize")
    .netlist;
    let gate = netlist_to_design(&netlist);
    let mut sim = Simulator::new(&gate).expect("sim");
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("d", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("d", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    assert_eq!(value(&sim, "q"), 1);
}

#[test]
fn i8086_gate_level_immediate_and_halt() {
    let result = run_i8086_gate(&[0xb8, 0x2a, 0x00, 0xf4], 64).expect("gate-level i8086 run");
    assert!(result.halted);
    assert_eq!(result.ax, 42);
    assert_eq!(result.ip, 0x0104);
}

#[test]
fn rv32i_signoff_gate_level_addi_and_ebreak() {
    let (x10, pc) = run_rv32i_signoff_gate(&[0x02a00513, 0x00100073], 200)
        .expect("gate-level RV32I signoff run");
    assert_eq!(x10, 42);
    assert_eq!(pc, 4);
}

#[test]
fn rv32i_signoff_gate_level_load_store_round_trip() {
    let program = [
        0x000010b7, // lui x1, 0x1
        0x23408093, // addi x1, x1, 0x234
        0x00102023, // sw x1, 0(x0)
        0x00002503, // lw x10, 0(x0)
        0x00100073, // ebreak
    ];
    let (x10, _) = run_rv32i_signoff_gate(&program, 300).expect("gate-level RV32I signoff run");
    assert_eq!(x10, 0x1234);
}

#[test]
fn i8086_gate_level_add_and_halt() {
    let program = [
        0xb8, 0x01, 0x00, // mov ax, 1
        0x05, 0x02, 0x00, // add ax, 2
        0xf4, // hlt
    ];
    let result = run_i8086_gate(&program, 64).expect("gate-level i8086 run");
    assert!(result.halted);
    assert_eq!(result.ax, 3);
    assert_eq!(result.ip, 0x0108);
}

#[test]
#[ignore = "slow full-core gate-level fixture; run explicitly for opcode/memory coverage"]
fn i8086_gate_level_alu_and_memory_round_trip() {
    let program = [
        0xb8, 0x34, 0x12, // mov ax, 0x1234
        0xa3, 0x00, 0x02, // mov [0x0200], ax
        0x8b, 0x1e, 0x00, 0x02, // mov bx, [0x0200]
        0x05, 0x01, 0x00, // add ax, 1
        0xf4, // hlt
    ];
    let result = run_i8086_gate(&program, 128).expect("gate-level i8086 run");
    assert!(result.halted);
    assert_eq!(result.ax, 0x1235);
    assert_eq!(result.bx, 0x1234);
    assert_eq!(result.ip, 0x010e);
}

#[test]
fn i8086_rtl_and_gate_netlist_cosimulate_immediate_and_halt() {
    let program = [0xb8_u8, 0x2a, 0x00, 0xf4];
    let rtl = eda_hdl::compile(I8086_SOURCE, Some("i8086_full_core")).expect("compile i8086 RTL");
    let rtl_result = run_i8086_design(&rtl, &program, 64).expect("RTL i8086 run");
    let gate_result = run_i8086_gate(&program, 64).expect("gate i8086 run");
    assert!(rtl_result.halted && gate_result.halted);
    assert_eq!(rtl_result.ax, gate_result.ax);
    assert_eq!(rtl_result.ip, gate_result.ip);
}

/// Literal GL+SDF path: convert the SDF-annotated mapped netlist back to the
/// simulator IR with `netlist_to_design_with_timing`, then execute a real
/// program under an explicit clock period and assert no setup/hold errors.
#[test]
fn rv32i_signoff_gate_level_with_sdf_timing() {
    if cfg!(debug_assertions) {
        eprintln!("skipping: run this timing proof in release");
        return;
    }
    use eda_core::ir::SimulationSeverity;
    const HALF_PERIOD_NS: u64 = 1_000;
    let design = rv32i_signoff_gate_timed();
    let mut sim = Simulator::new(design).expect("sim");
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("prog_ld", &LogicWord::from_u64(1, 1))
        .unwrap();
    sim.set_input("prog", &rv32i_program_image(&[0x02a00513, 0x00100073]))
        .unwrap();
    let mut now = 0_u64;
    // Let the delayed reset/load mux settle before the first clock edge.
    now += HALF_PERIOD_NS;
    sim.run_until(now).unwrap();
    // Load edge.
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    now += HALF_PERIOD_NS;
    sim.run_until(now).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("prog_ld", &LogicWord::from_u64(1, 0))
        .unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    now += HALF_PERIOD_NS;
    sim.run_until(now).unwrap();
    for _ in 0..20 {
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        now += HALF_PERIOD_NS;
        sim.run_until(now).unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        now += HALF_PERIOD_NS;
        sim.run_until(now).unwrap();
    }
    assert_eq!(
        value(&sim, "dbg_x10"),
        42,
        "timed gate run did not execute addi x10, x0, 42"
    );
    let errors = sim.severity_hits(SimulationSeverity::Error);
    assert_eq!(errors, 0, "SDF setup/hold violations reported: {errors}");
}
