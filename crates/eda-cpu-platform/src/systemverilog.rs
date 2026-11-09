//! Host-memory execution of compiled images on the SystemVerilog CPU cores.
//!
//! The RTL cores remain the architectural executors.  A sparse host memory is
//! attached to their external buses by the runtime so real COM and ELF images
//! are not constrained by a small synthesizable directed-test SOC.

use eda_core::ir::Design;
use eda_core::logic::LogicWord;
use eda_sim::Simulator;

use crate::i8086::I8086Machine;
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
    pub es: u16,
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub ip: u16,
    pub flags: u16,
    pub memory: SparseMemory,
}

/// A compiled i8086 RTL runtime that can execute many host-memory cases
/// without repeating HDL elaboration.
pub struct I8086SvRunner {
    design: Design,
}

struct I8086InitialState {
    ip: u16,
    sp: u16,
    cs: u16,
    ds: u16,
    es: u16,
    ss: u16,
}

#[derive(Clone, Copy)]
struct I8086ExternalInterrupt {
    vector: u8,
    after_instructions: u64,
    nmi: bool,
}

impl I8086InitialState {
    fn com() -> Self {
        Self {
            ip: 0x0100,
            sp: 0xfffe,
            cs: 0,
            ds: 0,
            es: 0,
            ss: 0,
        }
    }

    fn mz(machine: &I8086Machine) -> Self {
        Self {
            ip: machine.ip,
            sp: machine.regs[crate::i8086::REG_SP],
            cs: machine.segments[crate::i8086::SEG_CS],
            ds: machine.segments[crate::i8086::SEG_DS],
            es: machine.segments[crate::i8086::SEG_ES],
            ss: machine.segments[crate::i8086::SEG_SS],
        }
    }
}

impl I8086SvRunner {
    /// Wrap an already-elaborated runtime design (for example a slang-lowered
    /// or externally compiled design).
    pub fn from_design(design: Design) -> Self {
        Self { design }
    }

    pub fn new() -> Result<Self, String> {
        let source = format!("{PREFIX}\n{OPCODE}\n{MODRM}\n{I8086_CORE}\n{I8086_TB}");
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
        execute_i8086(
            &self.design,
            I8086InitialState::com(),
            initial_memory,
            simulation_time_budget,
            None,
        )
    }

    pub fn run_memory_with_external_interrupt(
        &self,
        initial_memory: SparseMemory,
        vector: u8,
        after_instructions: u64,
        nmi: bool,
    ) -> Result<I8086SvResult, String> {
        execute_i8086(
            &self.design,
            I8086InitialState::com(),
            initial_memory,
            SIMULATION_TIME_BUDGET,
            Some(I8086ExternalInterrupt {
                vector,
                after_instructions,
                nmi,
            }),
        )
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

pub fn run_i8086_mz(program: &[u8], load_segment: u16) -> Result<I8086SvResult, String> {
    let golden = I8086Machine::mz(program, load_segment)
        .map_err(|error| format!("invalid MZ image: {error}"))?;
    let initial_memory = golden.memory.clone();
    let source = format!("{PREFIX}\n{OPCODE}\n{MODRM}\n{I8086_CORE}\n{I8086_TB}");
    let design =
        eda_hdl::compile(&source, Some("i8086_full_runtime_tb")).map_err(|e| e.to_string())?;
    execute_i8086(
        &design,
        I8086InitialState::mz(&golden),
        initial_memory,
        SIMULATION_TIME_BUDGET,
        None,
    )
}

fn execute_i8086(
    design: &Design,
    initial_state: I8086InitialState,
    mut memory: SparseMemory,
    simulation_time_budget: u64,
    external_interrupt: Option<I8086ExternalInterrupt>,
) -> Result<I8086SvResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    simulator.set_input("io_rdata", &LogicWord::from_u64(16, 0))?;
    simulator.set_input(
        "init_ip",
        &LogicWord::from_u64(16, u64::from(initial_state.ip)),
    )?;
    simulator.set_input(
        "init_sp",
        &LogicWord::from_u64(16, u64::from(initial_state.sp)),
    )?;
    simulator.set_input(
        "init_cs",
        &LogicWord::from_u64(16, u64::from(initial_state.cs)),
    )?;
    simulator.set_input(
        "init_ds",
        &LogicWord::from_u64(16, u64::from(initial_state.ds)),
    )?;
    simulator.set_input(
        "init_es",
        &LogicWord::from_u64(16, u64::from(initial_state.es)),
    )?;
    simulator.set_input(
        "init_ss",
        &LogicWord::from_u64(16, u64::from(initial_state.ss)),
    )?;
    simulator.set_input("irq", &LogicWord::from_u64(1, 0))?;
    simulator.set_input("nmi", &LogicWord::from_u64(1, 0))?;
    simulator.set_input(
        "interrupt_vector",
        &LogicWord::from_u64(
            8,
            u64::from(external_interrupt.map_or(0, |interrupt| interrupt.vector)),
        ),
    )?;

    let mut output = Vec::new();
    let mut io_ports = vec![0_u8; u16::MAX as usize + 1];
    let mut interrupt_delivered = false;
    for time in 0..simulation_time_budget {
        let clock = output_bool(&simulator, "clock")?;
        let retired = bus_u64(&simulator, "retired")?;
        if let Some(interrupt) = external_interrupt {
            if !interrupt_delivered && retired >= interrupt.after_instructions {
                let signal = if interrupt.nmi { "nmi" } else { "irq" };
                simulator.set_input(signal, &LogicWord::from_u64(1, 1))?;
            } else {
                simulator.set_input("irq", &LogicWord::from_u64(1, 0))?;
                simulator.set_input("nmi", &LogicWord::from_u64(1, 0))?;
            }
        }
        let mem_address = bus_u64(&simulator, "mem_addr")? as u32;
        let source_address = bus_u64(&simulator, "source_addr")? as u32;
        let io_address = bus_u64(&simulator, "io_addr")? as u16;
        simulator.set_input(
            "io_rdata",
            &LogicWord::from_u64(
                16,
                u64::from(io_ports[io_address as usize])
                    | (u64::from(io_ports[io_address.wrapping_add(1) as usize]) << 8),
            ),
        )?;
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
        let pending_io_write = bus_u64(&simulator, "io_we")? == 1;
        let io_write_address = io_address;
        let io_write_value = bus_u64(&simulator, "io_wdata")? as u16;
        let io_write_byte = bus_u64(&simulator, "io_width")? == 0;

        simulator.run_until(time + 1)?;
        let clock_now = output_bool(&simulator, "clock")?;
        if clock_now && !clock {
            if output_bool(&simulator, "interrupt_ack")? {
                interrupt_delivered = true;
                simulator.set_input("irq", &LogicWord::from_u64(1, 0))?;
                simulator.set_input("nmi", &LogicWord::from_u64(1, 0))?;
            }
            if pending_write {
                if write_byte {
                    memory.write_u8(write_address, write_value as u8);
                } else {
                    memory.write_le_u16(write_address, write_value);
                }
            }
            if pending_io_write {
                if io_write_byte {
                    io_ports[io_write_address as usize] = io_write_value as u8;
                } else {
                    io_ports[io_write_address as usize] = io_write_value as u8;
                    io_ports[io_write_address.wrapping_add(1) as usize] =
                        (io_write_value >> 8) as u8;
                }
                simulator.set_input(
                    "io_rdata",
                    &LogicWord::from_u64(
                        16,
                        u64::from(io_ports[io_write_address as usize])
                            | (u64::from(io_ports[io_write_address.wrapping_add(1) as usize]) << 8),
                    ),
                )?;
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
                    es: output_u64(&simulator, "es")? as u16,
                    cs: output_u64(&simulator, "cs")? as u16,
                    ss: output_u64(&simulator, "ss")? as u16,
                    ds: output_u64(&simulator, "ds")? as u16,
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
        execute_rv32i(&self.design, entry, 0, initial_memory)
    }
}

pub fn run_rv32i_elf(bytes: &[u8]) -> Result<Rv32iSvResult, String> {
    let golden = crate::rv32i::Rv32iMachine::load_elf(bytes, &[])
        .map_err(|error| format!("invalid RV32I ELF: {error}"))?;
    let entry = golden.pc;
    let source = format!("{RV_CORE}\n{RV_TB}");
    let design =
        eda_hdl::compile(&source, Some("rv32i_full_runtime_tb")).map_err(|e| e.to_string())?;
    execute_rv32i(&design, entry, golden.regs[2], golden.memory)
}

fn execute_rv32i(
    design: &Design,
    entry: u32,
    initial_stack_pointer: u32,
    mut memory: SparseMemory,
) -> Result<Rv32iSvResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    simulator.set_input("init_pc", &LogicWord::from_u64(32, u64::from(entry)))?;
    simulator.set_input(
        "init_sp",
        &LogicWord::from_u64(32, u64::from(initial_stack_pointer)),
    )?;

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

/// Result of running a bare-metal RV32I program on the RTL platform with
/// memory-mapped console and shutdown devices.
#[derive(Clone, Debug)]
pub struct Rv32iPlatformResult {
    pub console: Vec<u8>,
    pub exit_code: Option<u32>,
    pub halted: bool,
    pub trap: bool,
    pub trap_cause: u32,
    pub pc: u32,
    pub regs: [u32; 32],
    pub instructions: u64,
    pub cycles: u64,
    pub vcd: Option<String>,
}

/// RV32I RTL platform: full sparse instruction/data memory plus a byte console
/// at `0x1000_0000`, shutdown at `0x1000_0004`, and TX-ready status at
/// `0x1000_0008`. Arbitrary bare-metal programs (including MiniOS) run here
/// without a testbench `$finish`.
pub struct Rv32iPlatformRunner {
    design: Design,
}

impl Rv32iPlatformRunner {
    pub fn new() -> Result<Self, String> {
        let source = format!("{RV_CORE}\n{RV_TB}");
        let design =
            eda_hdl::compile(&source, Some("rv32i_full_runtime_tb")).map_err(|e| e.to_string())?;
        Ok(Self { design })
    }

    pub fn from_design(design: Design) -> Self {
        Self { design }
    }

    pub fn run_binary(
        &self,
        entry: u32,
        initial_memory: SparseMemory,
        max_cycles: u64,
        capture_vcd: bool,
    ) -> Result<Rv32iPlatformResult, String> {
        execute_rv32i_platform(&self.design, entry, initial_memory, max_cycles, capture_vcd)
    }
}

/// Load a flat binary at address 0 and run it on the RTL platform.
pub fn run_rv32i_platform(bytes: &[u8], max_cycles: u64) -> Result<Rv32iPlatformResult, String> {
    let mut memory = SparseMemory::new();
    memory.write_slice(0, bytes);
    Rv32iPlatformRunner::new()?.run_binary(0, memory, max_cycles, false)
}

const PLATFORM_UART_TX: u32 = 0x1000_0000;
const PLATFORM_SHUTDOWN: u32 = 0x1000_0004;
const PLATFORM_UART_STATUS: u32 = 0x1000_0008;

fn execute_rv32i_platform(
    design: &Design,
    entry: u32,
    mut memory: SparseMemory,
    max_cycles: u64,
    capture_vcd: bool,
) -> Result<Rv32iPlatformResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    // Platform runs are long-lived (OS boots, syscalls, console output). The
    // default 20M-event guard is sized for unit tests; scale it with the
    // requested cycle budget instead.
    simulator.set_event_limit(max_cycles.saturating_mul(4096).max(100_000_000));
    if capture_vcd {
        simulator.enable_vcd();
    }
    simulator.set_input("init_pc", &LogicWord::from_u64(32, u64::from(entry)))?;
    simulator.set_input("init_sp", &LogicWord::from_u64(32, 0))?;

    let mut console = Vec::new();
    let mut exit_code = None;
    let mut cycles = 0u64;

    for time in 0..max_cycles {
        let clock = output_bool(&simulator, "clock")?;
        let instruction_address = bus_u64(&simulator, "imem_addr")? as u32;
        let data_address = bus_u64(&simulator, "dmem_addr")? as u32;
        let dmem_req = bus_u64(&simulator, "dmem_req")? == 1;
        let dmem_we = bus_u64(&simulator, "dmem_we")? == 1;
        let dmem_width = bus_u64(&simulator, "dmem_width")?;
        let dmem_wdata = bus_u64(&simulator, "dmem_wdata")? as u32;
        let dmem_rdata = if data_address == PLATFORM_UART_STATUS {
            1u32
        } else if data_address == PLATFORM_UART_TX || data_address == PLATFORM_SHUTDOWN {
            0
        } else {
            memory.read_le_u32(data_address & 0xffff_fffc)
        };
        simulator.set_input(
            "imem_rdata",
            &LogicWord::from_u64(32, u64::from(memory.read_le_u32(instruction_address))),
        )?;
        simulator.set_input(
            "dmem_rdata",
            &LogicWord::from_u64(32, u64::from(dmem_rdata)),
        )?;

        let write_address = data_address;
        let write_value = dmem_wdata;
        let write_width = dmem_width;
        let pending_write = dmem_req && dmem_we;

        simulator.run_until(time + 1)?;
        let clock_now = output_bool(&simulator, "clock")?;
        if clock_now && !clock {
            cycles += 1;
            if pending_write {
                match write_address {
                    PLATFORM_UART_TX => console.push(write_value as u8),
                    PLATFORM_SHUTDOWN => exit_code = Some(write_value),
                    address => match write_width {
                        0 => memory.write_u8(address, write_value as u8),
                        1 => memory.write_le_u16(address, write_value as u16),
                        _ => memory.write_le_u32(address, write_value),
                    },
                }
            }
            if exit_code.is_some() || output_bool(&simulator, "halted")? {
                break;
            }
            if output_bool(&simulator, "trap")? && output_bool(&simulator, "halted")? {
                break;
            }
        }
    }

    let mut regs = [0u32; 32];
    for (index, register) in regs.iter_mut().enumerate().skip(1) {
        *register = simulator
            .read(&format!("core.x{index}"))
            .and_then(|word| word.to_u64())
            .map(|value| value as u32)
            .ok_or_else(|| format!("missing RTL register x{index}"))?;
    }
    let instructions = simulator
        .read("core.instret_count")
        .and_then(|word| word.to_u64())
        .or_else(|| {
            simulator
                .read("core.cycle_count")
                .and_then(|word| word.to_u64())
        })
        .unwrap_or(0);
    let vcd = if capture_vcd { simulator.vcd() } else { None };
    Ok(Rv32iPlatformResult {
        console,
        exit_code,
        halted: output_bool(&simulator, "halted")?,
        // `trap` in the core is sticky for any exception, including handled
        // ecall traps. A clean MMIO shutdown is not a fatal trap.
        trap: output_bool(&simulator, "trap")? && exit_code.is_none(),
        trap_cause: output_u64(&simulator, "trap_cause")? as u32,
        pc: output_u64(&simulator, "dbg_pc")? as u32,
        regs,
        instructions,
        cycles,
        vcd,
    })
}

/// Result of running an RV64I bare-metal program on the RTL platform.
#[derive(Clone, Debug)]
pub struct Rv64iPlatformResult {
    pub console: Vec<u8>,
    pub exit_code: Option<u64>,
    pub halted: bool,
    pub trap: bool,
    pub trap_cause: u64,
    pub pc: u64,
    pub regs: [u64; 32],
    pub instructions: u64,
    pub cycles: u64,
    pub vcd: Option<String>,
}

const RV64_CORE: &str = include_str!("../../../platform/rtl/rv64i_full_core.sv");
const RV64_TB: &str = include_str!("../../../platform/rtl/rv64i_full_runtime_tb.sv");

const RV64_PLATFORM_UART_TX: u64 = 0x1000_0000;
const RV64_PLATFORM_SHUTDOWN: u64 = 0x1000_0004;
const RV64_PLATFORM_UART_STATUS: u64 = 0x1000_0008;

/// RV64I RTL platform with the same sparse memory map and console/shutdown
/// devices as the RV32I platform.
pub struct Rv64iPlatformRunner {
    design: Design,
}

impl Rv64iPlatformRunner {
    pub fn new() -> Result<Self, String> {
        let source = format!("{RV64_CORE}\n{RV64_TB}");
        let design =
            eda_hdl::compile(&source, Some("rv64i_full_runtime_tb")).map_err(|e| e.to_string())?;
        Ok(Self { design })
    }

    pub fn from_design(design: Design) -> Self {
        Self { design }
    }

    pub fn run_binary(
        &self,
        entry: u64,
        initial_memory: crate::rv64g::Rv64Memory,
        max_cycles: u64,
        capture_vcd: bool,
    ) -> Result<Rv64iPlatformResult, String> {
        execute_rv64i_platform(&self.design, entry, initial_memory, max_cycles, capture_vcd)
    }
}

/// Load a flat binary at address 0 and run it on the RV64I RTL platform.
pub fn run_rv64i_platform(bytes: &[u8], max_cycles: u64) -> Result<Rv64iPlatformResult, String> {
    let mut memory = crate::rv64g::Rv64Memory::new();
    memory.write_slice(0, bytes);
    Rv64iPlatformRunner::new()?.run_binary(0, memory, max_cycles, false)
}

fn execute_rv64i_platform(
    design: &Design,
    entry: u64,
    mut memory: crate::rv64g::Rv64Memory,
    max_cycles: u64,
    capture_vcd: bool,
) -> Result<Rv64iPlatformResult, String> {
    let mut simulator = Simulator::new(design).map_err(|e| e.to_string())?;
    simulator.set_event_limit(max_cycles.saturating_mul(4096).max(100_000_000));
    if capture_vcd {
        simulator.enable_vcd();
    }
    simulator.set_input("init_pc", &LogicWord::from_u64(64, entry))?;

    let mut console = Vec::new();
    let mut exit_code = None;
    let mut cycles = 0u64;

    for time in 0..max_cycles {
        let clock = output_bool(&simulator, "clock")?;
        let instruction_address = bus_u64(&simulator, "imem_addr")?;
        let data_address = bus_u64(&simulator, "dmem_addr")?;
        let dmem_req = bus_u64(&simulator, "dmem_req")? == 1;
        let dmem_we = bus_u64(&simulator, "dmem_we")? == 1;
        let dmem_width = bus_u64(&simulator, "dmem_width")?;
        let dmem_wdata = bus_u64(&simulator, "dmem_wdata")?;
        let dmem_rdata = if data_address == RV64_PLATFORM_UART_STATUS {
            1u64
        } else if data_address == RV64_PLATFORM_UART_TX || data_address == RV64_PLATFORM_SHUTDOWN {
            0
        } else {
            memory.read_le_u64(data_address & !7)
        };
        simulator.set_input(
            "imem_rdata",
            &LogicWord::from_u64(64, u64::from(memory.read_le_u32(instruction_address))),
        )?;
        simulator.set_input("dmem_rdata", &LogicWord::from_u64(64, dmem_rdata))?;

        let write_address = data_address;
        let write_value = dmem_wdata;
        let write_width = dmem_width;
        let pending_write = dmem_req && dmem_we;

        simulator.run_until(time + 1)?;
        let clock_now = output_bool(&simulator, "clock")?;
        if clock_now && !clock {
            cycles += 1;
            if pending_write {
                match write_address {
                    RV64_PLATFORM_UART_TX => console.push(write_value as u8),
                    RV64_PLATFORM_SHUTDOWN => {
                        exit_code = Some(write_value & 0xffff_ffff);
                    }
                    address => {
                        let size = 1u64 << write_width.min(3);
                        for offset in 0..size {
                            memory.write_u8(
                                address.wrapping_add(offset),
                                (write_value >> (8 * offset)) as u8,
                            );
                        }
                    }
                }
            }
            if exit_code.is_some() {
                break;
            }
            if output_bool(&simulator, "halted")? {
                if output_u64(&simulator, "trap_cause")? == 11 {
                    // ECALL with mtvec == 0: use a0 as the process exit code.
                    exit_code = Some(
                        simulator
                            .read("core.x10")
                            .and_then(|word| word.to_u64())
                            .unwrap_or(0)
                            & 0xffff_ffff,
                    );
                }
                break;
            }
        }
    }

    let mut regs = [0u64; 32];
    for (index, register) in regs.iter_mut().enumerate().skip(1) {
        *register = simulator
            .read(&format!("core.x{index}"))
            .and_then(|word| word.to_u64())
            .ok_or_else(|| format!("missing RTL register x{index}"))?;
    }
    let instructions = simulator
        .read("core.instret_count")
        .and_then(|word| word.to_u64())
        .unwrap_or(0);
    let vcd = if capture_vcd { simulator.vcd() } else { None };
    let halted = output_bool(&simulator, "halted")?;
    let trapped = output_bool(&simulator, "trap")?;
    Ok(Rv64iPlatformResult {
        console,
        exit_code,
        halted,
        trap: trapped && exit_code.is_none(),
        trap_cause: output_u64(&simulator, "trap_cause")?,
        pc: output_u64(&simulator, "dbg_pc")?,
        regs,
        instructions,
        cycles,
        vcd,
    })
}
