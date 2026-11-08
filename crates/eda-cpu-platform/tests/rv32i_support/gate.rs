//! SDF-annotated gate-level regression for the RV32I full core.
//!
//! Same literal `netlist_to_design_with_timing` flow as the i8086 support:
//! synthesize/map, SDF annotate, convert back to the simulator IR, and run the
//! instruction programs with a levelized Design evaluator that tracks SDF
//! timing slack.

use crate::design_timing::DesignTiming;
use eda_core::library::CellLibrary;
use eda_core::to_design::netlist_to_design_with_timing;
use eda_cpu_platform::SparseMemory;
use std::sync::OnceLock;

const RV32I_SOURCE: &str = include_str!("../../../../platform/rtl/rv32i_full_core.sv");

/// Clock budget used to turn SDF arrival times into pass/fail slack.
pub const CLOCK_PERIOD_NS: f64 = 2000.0;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GateState {
    pub halted: bool,
    pub trap: bool,
    pub trap_cause: u32,
    pub pc: u32,
    pub regs: [u32; 32],
    pub memory: SparseMemory,
    pub worst_setup_slack_ns: f64,
    pub worst_hold_slack_ns: f64,
    pub max_arrival_ns: f64,
}

fn design_timed() -> &'static Result<eda_core::ir::Design, String> {
    static CACHE: OnceLock<Result<eda_core::ir::Design, String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let rtl = eda_hdl::compile(RV32I_SOURCE, Some("rv32i_full_core"))?;
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

fn supply_design(model: &mut DesignTiming<'_>, memory: &SparseMemory) {
    let imem_address = model.read_port("imem_addr").unwrap_or(0) as u32;
    let dmem_address = model.read_port("dmem_addr").unwrap_or(0) as u32;
    model.set_port(
        "imem_rdata",
        u64::from(memory.read_le_u32(imem_address & 0xffff_fffc)),
    );
    model.set_port(
        "dmem_rdata",
        u64::from(memory.read_le_u32(dmem_address & 0xffff_fffc)),
    );
}

fn read_design_reg(model: &DesignTiming<'_>, index: usize) -> Option<u32> {
    let mut value = 0u32;
    for bit in 0..32 {
        match model.read_decl(&format!("x{index}[{bit}]")) {
            Some(1) => value |= 1 << bit,
            Some(_) => {}
            None => return None,
        }
    }
    Some(value)
}

/// Run a program on the `Design` produced by `netlist_to_design_with_timing`
/// from the SDF-annotated mapped RV32I full core.
pub fn run_rv32i_design_timed(
    memory: SparseMemory,
    entry: u32,
    max_cycles: u64,
) -> Result<GateState, String> {
    let design = design_timed().as_ref().map_err(|error| error.clone())?;
    let mut model = DesignTiming::new(design, CLOCK_PERIOD_NS)?;
    let mut memory = memory;
    model.set_port("clk", 0);
    model.set_port("rst", 1);
    model.set_port("init_pc", u64::from(entry));
    model.set_port("init_sp", 0);
    supply_design(&mut model, &memory);
    model.eval();
    model.reset_timing();

    for cycle in 0..max_cycles {
        model.set_port("rst", u64::from(cycle < 2));
        model.set_port("clk", 0);
        supply_design(&mut model, &memory);
        model.eval();
        supply_design(&mut model, &memory);
        model.eval();
        model.set_port("clk", 1);
        supply_design(&mut model, &memory);
        model.eval();
        let pending_write = model.read_bit("dmem_we") && model.read_bit("dmem_req");
        let write_address = model.read_port("dmem_addr").unwrap_or(0) as u32;
        let write_data = model.read_port("dmem_wdata").unwrap_or(0) as u32;
        let write_width = model.read_port("dmem_width").unwrap_or(2);
        model.timing_check();
        model.edge();
        model.set_port("clk", 0);
        model.eval();
        if pending_write {
            match write_width {
                0 => memory.write_u8(write_address, write_data as u8),
                1 => memory.write_le_u16(write_address, write_data as u16),
                _ => memory.write_le_u32(write_address, write_data),
            }
        }
        supply_design(&mut model, &memory);
        model.eval();
        if model.read_bit("halted") {
            let mut regs = [0_u32; 32];
            for (index, register) in regs.iter_mut().enumerate().skip(1) {
                *register = read_design_reg(&model, index).unwrap_or(0);
            }
            return Ok(GateState {
                halted: true,
                trap: model.read_bit("trap"),
                trap_cause: model.read_port("trap_cause").unwrap_or(0) as u32,
                pc: model.read_port("dbg_pc").unwrap_or(0) as u32,
                regs,
                memory,
                worst_setup_slack_ns: model.worst_setup_slack(),
                worst_hold_slack_ns: model.worst_hold_slack(),
                max_arrival_ns: model.max_arrival(),
            });
        }
    }
    Err(format!(
        "design-level RV32I program did not halt within {max_cycles} cycles"
    ))
}
