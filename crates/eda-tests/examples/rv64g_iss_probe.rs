//! Run a flat RV64G image on the user-level ISS.

use eda_cpu_platform::rv64g::Rv64gMachine;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: rv64g_iss_probe <image.bin>");
    let bytes = std::fs::read(&path).expect("read image");
    let mut machine = Rv64gMachine::load_flat(0, &bytes);
    if std::env::var("TRACE").is_ok() {
        for step in 0..24 {
            let pc = machine.pc;
            machine.step();
            eprintln!(
                "step {step}: pc={pc:#x} t0={:#x} t1={:#x} t2={:#x} t3={:#x}",
                machine.regs[5], machine.regs[6], machine.regs[7], machine.regs[28]
            );
        }
    }
    let stop = machine.run(1_000_000);
    print!("{}", String::from_utf8_lossy(&machine.output));
    eprintln!(
        "rv64g: stop={:?} pc={:#x} instructions={} trap={:?}",
        stop, machine.pc, machine.instructions_retired, machine.trap
    );
    eprintln!("regs={:x?}", &machine.regs[1..]);
}
