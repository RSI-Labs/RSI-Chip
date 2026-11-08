//! SDF-annotated gate-level regression for the i8086 full core.
//!
//! The matrix uses the literal `netlist_to_design_with_timing` path: synthesize
//! and map to standard cells, write/parse/annotate SDF, convert back to the
//! simulator IR, then run each program with a levelized Design evaluator that
//! keeps the SDF arc delays and checks setup/hold slack.

use crate::design_timing::DesignTiming;
use eda_core::library::CellLibrary;
use eda_core::to_design::netlist_to_design_with_timing;
use eda_cpu_platform::SparseMemory;
use std::sync::OnceLock;

const I8086_SOURCE: &str = concat!(
    include_str!("../../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
    include_str!("../../../../platform/rtl/i8086_full_core.sv"),
);

/// Signoff clock budget used to turn SDF arrival times into pass/fail slack.
pub const CLOCK_PERIOD_NS: f64 = 500.0;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GateState {
    pub halted: bool,
    pub trap: bool,
    pub trap_cause: u32,
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
    pub worst_setup_slack_ns: f64,
    pub worst_hold_slack_ns: f64,
    pub max_arrival_ns: f64,
}

fn design_timed() -> &'static Result<eda_core::ir::Design, String> {
    static CACHE: OnceLock<Result<eda_core::ir::Design, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let rtl = eda_hdl::compile(I8086_SOURCE, Some("i8086_full_core"))?;
        let mapped = eda_synth::synthesize_with_library(
            &rtl,
            &eda_synth::SynthOptions::default(),
            &CellLibrary::std(),
        )?
        .netlist;
        let sdf_text = eda_sdf::write_sdf(&mapped)?;
        let sdf = eda_sdf::parse_sdf(&sdf_text)?;
        let annotated = eda_sdf::annotate_sdf(&mapped, &sdf)?;
        Ok(netlist_to_design_with_timing(&annotated))
    })
}

fn supply_design(model: &mut DesignTiming<'_>, memory: &SparseMemory, io_ports: &[u8]) {
    let mem_address = model.read_port("mem_addr").unwrap_or(0) as u32;
    let source_address = model.read_port("source_addr").unwrap_or(0) as u32;
    model.set_port("mem_rdata", u64::from(memory.read_le_u16(mem_address)));
    model.set_port(
        "source_rdata",
        u64::from(memory.read_le_u16(source_address)),
    );
    let io_address = model.read_port("io_addr").unwrap_or(0) as usize & 0xffff;
    let io_value =
        u64::from(io_ports[io_address]) | (u64::from(io_ports[(io_address + 1) & 0xffff]) << 8);
    model.set_port("io_rdata", io_value);
}

/// Run a real-mode program on the `Design` produced by
/// `netlist_to_design_with_timing` from the SDF-annotated mapped netlist.
pub fn run_i8086_design_timed(memory: SparseMemory, max_cycles: u64) -> Result<GateState, String> {
    let design = design_timed().as_ref().map_err(|error| error.clone())?;
    let mut model = DesignTiming::new(design, CLOCK_PERIOD_NS)?;
    let mut memory = memory;
    model.set_port("clk", 0);
    model.set_port("rst", 1);
    model.set_port("init_ip", 0x0100);
    model.set_port("init_sp", 0xfffe);
    model.set_port("init_cs", 0);
    model.set_port("init_ds", 0);
    model.set_port("init_es", 0);
    model.set_port("init_ss", 0);
    model.set_port("irq", 0);
    model.set_port("nmi", 0);
    model.set_port("interrupt_vector", 0);
    model.set_port("io_rdata", 0);
    let mut io_ports = vec![0_u8; 65_536];
    supply_design(&mut model, &memory, &io_ports);
    model.eval();
    model.reset_timing();

    for cycle in 0..max_cycles {
        model.set_port("rst", u64::from(cycle < 2));
        model.set_port("clk", 0);
        supply_design(&mut model, &memory, &io_ports);
        model.eval();
        supply_design(&mut model, &memory, &io_ports);
        model.eval();

        let pending_write = model.read_bit("mem_we");
        let write_address = model.read_port("mem_addr").unwrap_or(0) as u32;
        let write_data = model.read_port("mem_wdata").unwrap_or(0) as u16;
        let write_byte = model.read_port("mem_width") == Some(0);
        let pending_io_write = model.read_bit("io_we");
        let io_write_address = model.read_port("io_addr").unwrap_or(0) as usize & 0xffff;
        let io_write_value = model.read_port("io_wdata").unwrap_or(0) as u16;
        let io_write_byte = model.read_port("io_width") == Some(0);

        model.set_port("clk", 1);
        supply_design(&mut model, &memory, &io_ports);
        model.eval();
        model.timing_check();
        model.edge();
        model.set_port("clk", 0);
        model.eval();

        if pending_write {
            if write_byte {
                memory.write_u8(write_address, write_data as u8);
            } else {
                memory.write_le_u16(write_address, write_data);
            }
        }
        if pending_io_write {
            io_ports[io_write_address] = io_write_value as u8;
            if !io_write_byte {
                io_ports[(io_write_address + 1) & 0xffff] = (io_write_value >> 8) as u8;
            }
        }
        supply_design(&mut model, &memory, &io_ports);
        model.eval();

        if model.read_bit("halted") {
            return Ok(GateState {
                halted: true,
                trap: model.read_bit("trap"),
                trap_cause: 0,
                ax: model.read_port("dbg_ax").unwrap_or(0) as u16,
                cx: model.read_port("dbg_cx").unwrap_or(0) as u16,
                dx: model.read_port("dbg_dx").unwrap_or(0) as u16,
                bx: model.read_port("dbg_bx").unwrap_or(0) as u16,
                sp: model.read_port("dbg_sp").unwrap_or(0) as u16,
                bp: model.read_port("dbg_bp").unwrap_or(0) as u16,
                si: model.read_port("dbg_si").unwrap_or(0) as u16,
                di: model.read_port("dbg_di").unwrap_or(0) as u16,
                es: model.read_port("dbg_es").unwrap_or(0) as u16,
                cs: model.read_port("dbg_cs").unwrap_or(0) as u16,
                ss: model.read_port("dbg_ss").unwrap_or(0) as u16,
                ds: model.read_port("dbg_ds").unwrap_or(0) as u16,
                ip: model.read_port("dbg_ip").unwrap_or(0) as u16,
                flags: model.read_port("dbg_flags").unwrap_or(0) as u16,
                memory,
                worst_setup_slack_ns: model.worst_setup_slack(),
                worst_hold_slack_ns: model.worst_hold_slack(),
                max_arrival_ns: model.max_arrival(),
            });
        }
    }
    Err(format!(
        "design-level i8086 program did not halt within {max_cycles} cycles"
    ))
}
