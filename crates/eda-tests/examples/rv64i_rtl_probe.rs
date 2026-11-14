//! Run RV64I conformance images on the RTL platform and on the ISS.

use eda_cpu_platform::rv64g::Rv64gMachine;
use eda_cpu_platform::systemverilog::run_rv64i_platform;

fn main() {
    for (name, image) in [
        (
            "rv64i",
            &include_bytes!("../../../platform/programs/rv64g/rv64i.bin")[..],
        ),
        (
            "rv64csr",
            &include_bytes!("../../../platform/programs/rv64g/rv64csr.bin")[..],
        ),
        (
            "rv64m",
            &include_bytes!("../../../platform/programs/rv64g/rv64m.bin")[..],
        ),
        (
            "rv64a",
            &include_bytes!("../../../platform/programs/rv64g/rv64a.bin")[..],
        ),
        (
            "rv64f_ops",
            &include_bytes!("../../../platform/programs/rv64g/rv64f_ops.bin")[..],
        ),
    ] {
        let rtl = run_rv64i_platform(image, 2_000_000).expect("RTL run");
        let mut iss = Rv64gMachine::load_flat(0, image);
        let stop = iss.run(200_000);
        println!(
            "{name}: rtl exit={:?} halted={} trap={} cause={} pc={:#x} inst={} cycles={} console={:?}",
            rtl.exit_code,
            rtl.halted,
            rtl.trap,
            rtl.trap_cause,
            rtl.pc,
            rtl.instructions,
            rtl.cycles,
            String::from_utf8_lossy(&rtl.console)
        );
        println!(
            "{name}: iss stop={:?} pc={:#x} inst={} trap={:?}",
            stop, iss.pc, iss.instructions_retired, iss.trap
        );
        println!("{name}: rtl regs={:x?}", &rtl.regs[1..]);
        println!("{name}: iss regs={:x?}", &iss.regs[1..]);
    }
}
