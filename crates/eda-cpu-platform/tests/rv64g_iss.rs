//! RV64G user-level ISS conformance tests.
//!
//! The committed `.bin` images are built from `platform/programs/rv64g/src`
//! with LLVM 20 (`platform/programs/rv64g/build.sh`). Each program checks its
//! own results and exits with the failing check id; a clean run exits 0.

use eda_cpu_platform::rv64g::{Rv64Memory, Rv64Stop, Rv64gMachine};

fn run(name: &str, image: &[u8]) {
    let mut machine = Rv64gMachine::load_flat(0, image);
    let stop = machine.run(200_000);
    assert_eq!(
        stop,
        Rv64Stop::Exit(0),
        "{name}: stop={stop:?} pc={:#x} trap={:?} output={:?}",
        machine.pc,
        machine.trap,
        String::from_utf8_lossy(&machine.output)
    );
    assert_eq!(
        machine.trap, None,
        "{name}: unexpected trap after clean exit"
    );
}

#[test]
fn rv64i_integer_suite_passes() {
    run(
        "rv64i",
        include_bytes!("../../../platform/programs/rv64g/rv64i.bin"),
    );
}

#[test]
fn rv64m_multiply_divide_suite_passes() {
    run(
        "rv64m",
        include_bytes!("../../../platform/programs/rv64g/rv64m.bin"),
    );
}

#[test]
fn rv64a_atomic_suite_passes() {
    run(
        "rv64a",
        include_bytes!("../../../platform/programs/rv64g/rv64a.bin"),
    );
}

#[test]
fn rv64f_single_precision_suite_passes() {
    run(
        "rv64f",
        include_bytes!("../../../platform/programs/rv64g/rv64f.bin"),
    );
}

#[test]
fn rv64d_double_precision_suite_passes() {
    run(
        "rv64d",
        include_bytes!("../../../platform/programs/rv64g/rv64d.bin"),
    );
}

#[test]
fn rv64csr_suite_passes() {
    run(
        "rv64csr",
        include_bytes!("../../../platform/programs/rv64g/rv64csr.bin"),
    );
}

#[test]
fn sparse_memory_roundtrips_all_widths() {
    let mut memory = Rv64Memory::new();
    memory.write_le_u64(0x1234_5000, 0x1122_3344_5566_7788);
    assert_eq!(memory.read_le_u64(0x1234_5000), 0x1122_3344_5566_7788);
    memory.write_le_u32(0x1234_5000, 0xdead_beef);
    assert_eq!(memory.read_le_u32(0x1234_5000), 0xdead_beef);
    memory.write_le_u16(0x1234_5000, 0xabcd);
    assert_eq!(memory.read_le_u16(0x1234_5000), 0xabcd);
    memory.write_u8(0x1234_5000, 0x7f);
    assert_eq!(memory.read_u8(0x1234_5000), 0x7f);
}
