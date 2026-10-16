//! Host-memory execution of compiled images on the SystemVerilog CPU cores.
//!
//! The RTL cores remain the architectural executors.  A sparse host memory is
//! attached to their external buses by the runtime so real COM and ELF images
//! are not constrained by a small synthesizable directed-test SOC.

use eda_core::ir::Design;
use eda_core::logic::LogicWord;
use eda_sim::Simulator;

use crate::SparseMemory;

const PREFIX: &str = include_str!("../../../platform/rtl/i8086_prefix_decoder.sv");
const OPCODE: &str = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");
const MODRM: &str = include_str!("../../../platform/rtl/i8086_modrm_decoder.sv");
const I8086_CORE: &str = include_str!("../../../platform/rtl/i8086_full_core.sv");
const I8086_TB: &str = include_str!("../../../platform/rtl/i8086_full_runtime_tb.sv");
const RV_CORE: &str = include_str!("../../../platform/rtl/rv32i_full_core.sv");
const RV_TB: &str = include_str!("../../../platform/rtl/rv32i_full_runtime_tb.sv");

const SIMULATION_TIME_BUDGET: u64 = 200_000;
const I8086_COM_LOAD_ADDRESS: u32 = 0x0100;

#[derive(Clone, Debug)]
pub struct I8086SvResult {
    pub output: Vec<u8>,
    pub halted: bool,
    pub trap: bool,
    pub instructions: u64,
    pub ax: u16,
    pub cx: u16,
    pub dx: u16,
    pub bx: u16,
    pub sp: u16,
    pub bp: u16,
    pub si: u16,
    pub di: u16,
    pub ip: u16,
    pub flags: u16,
    pub memory: SparseMemory,
}

/// A compiled i8086 RTL runtime that can execute many host-memory cases
/// without repeating HDL elaboration.
pub struct I8086SvRunner {
    design: Design,
}

impl I8086SvRunner {
    pub fn new() -> Result<Self, String> {
        let testbench = I8086_TB.replace(
            "parameter INIT_IP = 16'd0;",
            "parameter INIT_IP = 16'h0100;",
        );
        let source = format!("{PREFIX}\n{OPCODE}\n{MODRM}\n{I8086_CORE}\n{testbench}");
        let design =
            eda_hdl::compile(&source, Some("i8086_full_runtime_tb")).map_err(|e| e.to_string())?;
        Ok(Self { design })
    }

    pub fn run_memory(&self, initial_memory: SparseMemory) -> Result<I8086SvResult, String> {
        self.run_memory_with_budget(initial_memory, SIMULATION_TIME_BUDGET)
    }

    pub fn run_memory_with_budget(
        &self,
        initial_memory: SparseMemory,
        simulation_time_budget: u64,
    ) -> Result<I8086SvResult, String> {
        execute_i8086(&self.design, initial_memory, simulation_time_budget)
    }
}

#[derive(Clone, Debug)]
pub struct Rv32iSvResult {
    pub halted: bool,
    pub trap: bool,
    pub trap_cause: u32,
    pub pc: u32,
    pub a0: u32,
    pub instructions: u64,
    pub regs: [u32; 32],
    pub memory: SparseMemory,
}

pub fn run_i8086_com(program: &[u8]) -> Result<I8086SvResult, String> {
    const MAXIMUM_COM_SIZE: usize = 0x10000 - I8086_COM_LOAD_ADDRESS as usize;
    if program.len() > MAXIMUM_COM_SIZE {
        return Err(format!(
            "the real-mode SV runtime supports COM images up to {MAXIMUM_COM_SIZE} bytes"
        ));
    }

    let mut memory = SparseMemory::new();
    memory.write_slice(I8086_COM_LOAD_ADDRESS, program);
    I8086SvRunner::new()?.run_memory(memory)
}

fn execute_i8086(
    design: &Design,
    mut memory: SparseMemory,
    simulation_time_budget: u64,
) -> Result<I8086SvResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    simulator.set_input("io_rdata", &LogicWord::from_u64(16, 0))?;

    let mut output = Vec::new();
    for time in 0..simulation_time_budget {
        let clock = output_bool(&simulator, "clock")?;
        let mem_address = bus_u64(&simulator, "mem_addr")? as u32;
        let source_address = bus_u64(&simulator, "source_addr")? as u32;
        simulator.set_input(
            "mem_rdata",
            &LogicWord::from_u64(16, u64::from(memory.read_le_u16(mem_address))),
        )?;
        simulator.set_input(
            "source_rdata",
            &LogicWord::from_u64(16, u64::from(memory.read_le_u16(source_address))),
        )?;

        let pending_write = bus_u64(&simulator, "mem_we")? == 1;
        let write_address = mem_address;
        let write_value = bus_u64(&simulator, "mem_wdata")? as u16;
        let write_byte = bus_u64(&simulator, "mem_width")? == 0;

        simulator.run_until(time + 1)?;
        let clock_now = output_bool(&simulator, "clock")?;
        if clock_now && !clock {
            if pending_write {
                if write_byte {
                    memory.write_u8(write_address, write_value as u8);
                } else {
                    memory.write_le_u16(write_address, write_value);
                }
            }
            supply_i8086_reads(&mut simulator, &memory)?;
            if output_bool(&simulator, "console_write")? {
                output.push(output_u64(&simulator, "console_char")? as u8);
            }
            if output_bool(&simulator, "halted")? {
                return Ok(I8086SvResult {
                    output,
                    halted: true,
                    trap: output_bool(&simulator, "trap")?,
                    instructions: output_u64(&simulator, "retired")?,
                    ax: output_u64(&simulator, "ax")? as u16,
                    cx: output_u64(&simulator, "cx")? as u16,
                    dx: output_u64(&simulator, "dx")? as u16,
                    bx: output_u64(&simulator, "bx")? as u16,
                    sp: output_u64(&simulator, "sp")? as u16,
                    bp: output_u64(&simulator, "bp")? as u16,
                    si: output_u64(&simulator, "si")? as u16,
                    di: output_u64(&simulator, "di")? as u16,
                    ip: output_u64(&simulator, "ip")? as u16,
                    flags: output_u64(&simulator, "flags")? as u16,
                    memory,
                });
            }
        }
    }
    Err("SystemVerilog i8086 program did not halt within the simulation budget".into())
}

fn supply_i8086_reads(simulator: &mut Simulator<'_>, memory: &SparseMemory) -> Result<(), String> {
    let mem_address = bus_u64(simulator, "mem_addr")? as u32;
    let source_address = bus_u64(simulator, "source_addr")? as u32;
    simulator.set_input(
        "mem_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(mem_address))),
    )?;
    simulator.set_input(
        "source_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(source_address))),
    )
}

/// A compiled RV32I RTL runtime that can execute many sparse-memory cases
/// without repeating HDL elaboration.
pub struct Rv32iSvRunner {
    design: Design,
}

impl Rv32iSvRunner {
    pub fn new() -> Result<Self, String> {
        let source = format!("{RV_CORE}\n{RV_TB}");
        let design =
            eda_hdl::compile(&source, Some("rv32i_full_runtime_tb")).map_err(|e| e.to_string())?;
        Ok(Self { design })
    }

    pub fn run_memory(
        &self,
        entry: u32,
        initial_memory: SparseMemory,
    ) -> Result<Rv32iSvResult, String> {
        execute_rv32i(&self.design, entry, initial_memory)
    }
}

pub fn run_rv32i_elf(bytes: &[u8]) -> Result<Rv32iSvResult, String> {
    let golden = crate::rv32i::Rv32iMachine::load_elf(bytes, &[])
        .map_err(|error| format!("invalid RV32I ELF: {error}"))?;
    let entry = golden.pc;
    let source = format!("{RV_CORE}\n{RV_TB}");
    let design =
        eda_hdl::compile(&source, Some("rv32i_full_runtime_tb")).map_err(|e| e.to_string())?;
    execute_rv32i(&design, entry, golden.memory)
}

fn execute_rv32i(
    design: &Design,
    entry: u32,
    mut memory: SparseMemory,
) -> Result<Rv32iSvResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    simulator.set_input("init_pc", &LogicWord::from_u64(32, u64::from(entry)))?;

    for time in 0..SIMULATION_TIME_BUDGET {
        let clock = output_bool(&simulator, "clock")?;
        let instruction_address = bus_u64(&simulator, "imem_addr")? as u32;
        let data_address = bus_u64(&simulator, "dmem_addr")? as u32;
        simulator.set_input(
            "imem_rdata",
            &LogicWord::from_u64(32, u64::from(memory.read_le_u32(instruction_address))),
        )?;
        simulator.set_input(
            "dmem_rdata",
            &LogicWord::from_u64(
                32,
                u64::from(memory.read_le_u32(data_address & 0xffff_fffc)),
            ),
        )?;

        let pending_write = bus_u64(&simulator, "dmem_we")? == 1;
        let write_address = data_address;
        let write_value = bus_u64(&simulator, "dmem_wdata")? as u32;
        let write_width = bus_u64(&simulator, "dmem_width")?;

        simulator.run_until(time + 1)?;
        let clock_now = output_bool(&simulator, "clock")?;
        if clock_now && !clock {
            if pending_write {
                match write_width {
                    0 => memory.write_u8(write_address, write_value as u8),
                    1 => memory.write_le_u16(write_address, write_value as u16),
                    _ => memory.write_le_u32(write_address, write_value),
                }
            }
            if output_bool(&simulator, "halted")? {
                let mut regs = [0_u32; 32];
                for (index, register) in regs.iter_mut().enumerate().skip(1) {
                    *register = simulator
                        .read(&format!("core.x{index}"))
                        .and_then(|word| word.to_u64())
                        .map(|value| value as u32)
                        .ok_or_else(|| format!("missing RTL register x{index}"))?;
                }
                return Ok(Rv32iSvResult {
                    halted: true,
                    trap: output_bool(&simulator, "trap")?,
                    trap_cause: output_u64(&simulator, "trap_cause")? as u32,
                    pc: output_u64(&simulator, "dbg_pc")? as u32,
                    a0: output_u64(&simulator, "dbg_x10")? as u32,
                    instructions: simulator
                        .read("core.cycle_count")
                        .and_then(|word| word.to_u64())
                        .ok_or("missing RV32I cycle count")?,
                    regs,
                    memory,
                });
            }
        }
    }
    Err("SystemVerilog RV32I program did not halt within the simulation budget".into())
}

fn output_u64(simulator: &Simulator<'_>, name: &str) -> Result<u64, String> {
    simulator
        .read_port(name)
        .and_then(|word| word.to_u64())
        .ok_or_else(|| format!("missing or unknown top output '{name}'"))
}

fn output_bool(simulator: &Simulator<'_>, name: &str) -> Result<bool, String> {
    Ok(output_u64(simulator, name)? == 1)
}

fn bus_u64(simulator: &Simulator<'_>, name: &str) -> Result<u64, String> {
    Ok(simulator
        .read_port(name)
        .and_then(|word| word.to_u64())
        .unwrap_or(0))
}
