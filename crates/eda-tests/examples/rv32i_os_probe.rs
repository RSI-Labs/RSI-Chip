//! Run MiniOS-RV32I on the event-driven RTL simulator platform.

fn main() {
    let image = include_bytes!("../../../platform/os/rv32i/minios.bin");
    let result = eda_cpu_platform::systemverilog::run_rv32i_platform(image, 5_000_000)
        .expect("run MiniOS on RV32I RTL");
    print!("{}", String::from_utf8_lossy(&result.console));
    eprintln!(
        "platform: exit={:?} halted={} trap={} trap_cause={} pc={:#010x} instructions={} cycles={}",
        result.exit_code,
        result.halted,
        result.trap,
        result.trap_cause,
        result.pc,
        result.instructions,
        result.cycles
    );
}
