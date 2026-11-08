//! Core-only differential: slang vs eda-hdl on i8086 ALU programs.

use eda_core::ir::Design;
use eda_core::logic::LogicWord;
use eda_cpu_platform::SparseMemory;
use eda_sim::Simulator;
use eda_slang::{PreprocessOptions, SlangFrontend};

const SOURCE: &str = concat!(
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_full_core.sv"),
);

fn word(sim: &Simulator<'_>, name: &str) -> u64 {
    sim.read_port(name)
        .or_else(|| sim.read(name))
        .and_then(|value| value.to_u64())
        .unwrap_or(0)
}

fn supply(sim: &mut Simulator<'_>, memory: &SparseMemory) {
    let mem = word(sim, "mem_addr") as u32;
    let source = word(sim, "source_addr") as u32;
    sim.set_input(
        "mem_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(mem))),
    )
    .unwrap();
    sim.set_input(
        "source_rdata",
        &LogicWord::from_u64(16, u64::from(memory.read_le_u16(source))),
    )
    .unwrap();
    sim.set_input("io_rdata", &LogicWord::from_u64(16, 0))
        .unwrap();
}

fn run(design: &Design, program: &[u8]) -> (u16, u16, u16, bool) {
    let mut memory = SparseMemory::new();
    memory.write_slice(0x0100, program);
    let mut sim = Simulator::new(design).expect("sim");
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    sim.set_input("rst", &LogicWord::from_u64(1, 1)).unwrap();
    sim.set_input("init_ip", &LogicWord::from_u64(16, 0x0100))
        .unwrap();
    sim.set_input("init_sp", &LogicWord::from_u64(16, 0xfffe))
        .unwrap();
    for name in ["init_cs", "init_ds", "init_es", "init_ss"] {
        sim.set_input(name, &LogicWord::from_u64(16, 0)).unwrap();
    }
    for name in ["irq", "nmi"] {
        sim.set_input(name, &LogicWord::from_u64(1, 0)).unwrap();
    }
    sim.set_input("interrupt_vector", &LogicWord::from_u64(8, 0))
        .unwrap();
    supply(&mut sim, &memory);
    let mut now = 0_u64;
    for cycle in 0..300 {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        supply(&mut sim, &memory);
        sim.run_until(now).unwrap();
        now += 1;
        sim.set_input("rst", &LogicWord::from_u64(1, u64::from(cycle < 2)))
            .unwrap();
        sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
        supply(&mut sim, &memory);
        sim.run_until(now).unwrap();
        supply(&mut sim, &memory);
        sim.run_until(now).unwrap();
        if word(&sim, "mem_we") == 1 {
            let address = word(&sim, "mem_addr") as u32;
            let data = word(&sim, "mem_wdata") as u16;
            if word(&sim, "mem_width") == 0 {
                memory.write_u8(address, data as u8);
            } else {
                memory.write_le_u16(address, data);
            }
        }
        if word(&sim, "halted") == 1 {
            return (
                word(&sim, "dbg_ax") as u16,
                word(&sim, "dbg_flags") as u16,
                word(&sim, "dbg_ip") as u16,
                true,
            );
        }
        now += 1;
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.run_until(now).unwrap();
    }
    (
        word(&sim, "dbg_ax") as u16,
        word(&sim, "dbg_flags") as u16,
        word(&sim, "dbg_ip") as u16,
        false,
    )
}

fn main() {
    let programs: &[(&str, &[u8])] = &[
        ("add ovf", &[0xb8, 0xff, 0x7f, 0x05, 0x01, 0x00, 0xf4]),
        ("sub ovf", &[0xb8, 0x00, 0x80, 0x2d, 0x01, 0x00, 0xf4]),
        ("inc ovf", &[0xb8, 0xff, 0x7f, 0x40, 0xf4]),
        ("neg ovf", &[0xb8, 0x00, 0x80, 0xf7, 0xd8, 0xf4]),
        ("add no", &[0xb8, 0x01, 0x00, 0x05, 0x02, 0x00, 0xf4]),
        ("xor", &[0xb8, 0xff, 0x00, 0x35, 0x0f, 0x00, 0xf4]),
        ("and", &[0xb8, 0xf0, 0x00, 0x25, 0x3c, 0x00, 0xf4]),
        ("cmp", &[0xb8, 0x05, 0x00, 0x3d, 0x05, 0x00, 0xf4]),
        ("shift", &[0xb8, 0x01, 0x00, 0xd1, 0xe0, 0xf4]),
    ];
    let frontend = SlangFrontend::discover().expect("slang");
    let slang = frontend
        .compile_source(
            SOURCE,
            Some("i8086_full_core"),
            &PreprocessOptions::default(),
        )
        .expect("slang");
    let hdl = eda_hdl::compile(SOURCE, Some("i8086_full_core")).expect("hdl");
    let mut mismatches = 0;
    for (name, program) in programs {
        let a = run(&slang, program);
        let b = run(&hdl, program);
        let ok = a == b;
        if !ok {
            mismatches += 1;
        }
        println!(
            "{name}: slang ax={:#06x} flags={:#06x} ip={:#06x} halt={} | hdl ax={:#06x} flags={:#06x} ip={:#06x} halt={} {}",
            a.0, a.1, a.2, a.3, b.0, b.1, b.2, b.3,
            if ok { "OK" } else { "MISMATCH" }
        );
    }
    println!("mismatches: {mismatches}/{}", programs.len());
}
