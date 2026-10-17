//! Distributed/block-RAM synthesis and simulation tests.

use super::super::super::*;
use eda_core::ir::{MemoryKind, MemoryTemplate};
use eda_core::logic::LogicWord;
use eda_sim::Simulator;

#[test]
fn block_ram_preserves_one_writable_macro_and_matches_direct_ast_semantics() {
    let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
        return;
    };
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let source_path = std::env::temp_dir().join(format!("eda-synth-block-ram-{unique}.sv"));
    let image_path = std::env::temp_dir().join(format!("eda-synth-block-ram-{unique}.hex"));
    let mut image = String::new();
    for address in 0..4096_u64 {
        let value = (address.wrapping_mul(11).wrapping_add(5) & 0xff) as u8;
        image.push_str(&format!("{value:02x}\n"));
    }
    std::fs::write(&image_path, image).expect("write BlockRam image");
    std::fs::write(
        &source_path,
        format!(
            r#"
module block_ram(
  input logic clk,
  input logic write,
  input logic [11:0] address,
  input logic [7:0] din,
  output logic [7:0] q
);
  logic [7:0] memory [0:4095];
  initial $readmemh("{}", memory);
  always @(posedge clk) if (write) memory[address] <= din;
  assign q = memory[address];
endmodule
"#,
            image_path.display()
        ),
    )
    .expect("write BlockRam source");

    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("block_ram"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("directly lower BlockRam source");
    let result = synthesize(&rtl, &SynthOptions::default())
        .expect("lower large synchronous RAM to one BlockRam macro");
    assert_eq!(result.memory_plans.len(), 1);
    assert_eq!(
        result.memory_plans[0].candidate,
        MemoryMacroCandidate::BlockRam
    );
    assert!(!result
        .warnings
        .iter()
        .any(|warning| warning.contains("no synthesis lowering")));

    let stats = result.netlist.stats();
    assert_eq!(stats.memory_macros, 1);
    assert_eq!(
        result
            .netlist
            .cells()
            .iter()
            .filter(|cell| matches!(cell.kind, CellKind::BlockRamWrite { .. }))
            .count(),
        8
    );
    assert_eq!(
        result
            .netlist
            .cells()
            .iter()
            .filter(|cell| matches!(cell.kind, CellKind::BlockRamRead { .. }))
            .count(),
        8
    );

    let gate = eda_core::to_design::netlist_to_design(&result.netlist);
    assert_eq!(gate.memories.len(), 1);
    assert_eq!(gate.memories[0].flat_length, 4096);
    assert_eq!(gate.memories[0].kind, MemoryKind::Ram);
    assert_eq!(gate.memories[0].template, MemoryTemplate::SyncRam);
    assert_eq!(gate.processes.len(), 1);
    assert_eq!(gate.initials.len(), 1);

    let mut rtl_sim = Simulator::new(&rtl).unwrap();
    let mut gate_sim = Simulator::new(&gate).unwrap();
    let expected = |address: u64| address.wrapping_mul(11).wrapping_add(5) & 0xff;
    for simulator in [&mut rtl_sim, &mut gate_sim] {
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
    }
    for (address, value) in [
        (0_u64, 0xa5_u64),
        (1, 0x3c),
        (255, 0x81),
        (2048, 0x5a),
        (4095, 0xc3),
    ] {
        for simulator in [&mut rtl_sim, &mut gate_sim] {
            simulator
                .set_input("write", &LogicWord::from_u64(1, 1))
                .unwrap();
            simulator
                .set_input("address", &LogicWord::from_u64(12, address))
                .unwrap();
            simulator
                .set_input("din", &LogicWord::from_u64(8, value))
                .unwrap();
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
        }
        assert_eq!(rtl_sim.read("q").unwrap(), gate_sim.read("q").unwrap());
        assert_eq!(gate_sim.read("q").unwrap().to_u64(), Some(value));
    }
    for (address, value) in [
        (0_u64, 0xa5_u64),
        (1, 0x3c),
        (255, 0x81),
        (2048, 0x5a),
        (4095, 0xc3),
    ] {
        for simulator in [&mut rtl_sim, &mut gate_sim] {
            simulator
                .set_input("write", &LogicWord::from_u64(1, 0))
                .unwrap();
            simulator
                .set_input("address", &LogicWord::from_u64(12, address))
                .unwrap();
        }
        assert_eq!(rtl_sim.read("q").unwrap(), gate_sim.read("q").unwrap());
        assert_eq!(gate_sim.read("q").unwrap().to_u64(), Some(value));
    }
    for address in [2_u64, 256, 4094] {
        for simulator in [&mut rtl_sim, &mut gate_sim] {
            simulator
                .set_input("write", &LogicWord::from_u64(1, 0))
                .unwrap();
            simulator
                .set_input("address", &LogicWord::from_u64(12, address))
                .unwrap();
        }
        assert_eq!(rtl_sim.read("q").unwrap(), gate_sim.read("q").unwrap());
        assert_eq!(
            gate_sim.read("q").unwrap().to_u64(),
            Some(expected(address))
        );
    }

    let verilog = crate::writer::write_verilog(&result.netlist, result.library.as_ref());
    assert_eq!(
        verilog
            .matches("  reg [7:0] block_ram_0 [0:4095];\n")
            .count(),
        1
    );
    assert_eq!(verilog.matches("block_ram_1").count(), 0);

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&image_path);
}

#[test]
fn block_ram_partial_writes_are_rejected_explicitly() {
    let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
        return;
    };
    let source_path = std::env::temp_dir().join(format!(
        "eda-synth-block-ram-partial-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &source_path,
        r#"
module block_ram(
  input logic clk,
  input logic write,
  input logic [11:0] address,
  input logic [7:0] din,
  output logic [15:0] q
);
  logic [15:0] memory [0:4095];
  always @(posedge clk) if (write) memory[address][15:8] <= din;
  assign q = memory[address];
endmodule
"#,
    )
    .expect("write partial-write BlockRam source");
    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("block_ram"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("slang accepts partial-write RAM source");
    let error = synthesize(&rtl, &SynthOptions::default())
        .expect_err("partial-write BlockRam must not be silently widened");
    assert!(
        error.contains("current BlockRam model does not support"),
        "unexpected error: {error}"
    );
    let _ = std::fs::remove_file(&source_path);
}

#[test]
fn distributed_ram_synthesizes_and_matches_direct_ast_semantics() {
    let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
        return;
    };
    let source_path = std::env::temp_dir().join(format!(
        "eda-synth-distributed-ram-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &source_path,
        r#"
module distributed_ram(
  input logic clk,
  input logic write,
  input logic [1:0] address,
  input logic [7:0] din,
  output logic [7:0] q
);
  logic [7:0] memory [0:3];
  always @(posedge clk) if (write) memory[address] <= din;
  assign q = memory[address];
endmodule
"#,
    )
    .expect("write distributed RAM source");
    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("distributed_ram"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("directly lower distributed RAM");
    let result = synthesize(&rtl, &SynthOptions::default())
        .expect("lower distributed RAM to registers and multiplexers");
    assert_eq!(result.memory_plans.len(), 1);
    assert_eq!(
        result.memory_plans[0].candidate,
        MemoryMacroCandidate::DistributedRam
    );
    assert!(!result
        .warnings
        .iter()
        .any(|warning| warning.contains("no synthesis lowering")));
    assert_eq!(result.netlist.stats().seq, 32);

    let gate = eda_core::to_design::netlist_to_design(&result.netlist);
    let mut rtl_sim = Simulator::new(&rtl).unwrap();
    let mut gate_sim = Simulator::new(&gate).unwrap();
    for sim in [&mut rtl_sim, &mut gate_sim] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("write", &LogicWord::from_u64(1, 1)).unwrap();
    }
    for (address, value) in [(0_u64, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)] {
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
            sim.set_input("din", &LogicWord::from_u64(8, value))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        assert_eq!(
            rtl_sim.read("q").unwrap(),
            gate_sim.read("q").unwrap(),
            "write address {address}"
        );
    }
    for sim in [&mut rtl_sim, &mut gate_sim] {
        sim.set_input("write", &LogicWord::from_u64(1, 0)).unwrap();
    }
    for address in 0..4_u64 {
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
        }
        assert_eq!(
            rtl_sim.read("q").unwrap(),
            gate_sim.read("q").unwrap(),
            "read address {address}"
        );
    }
}

#[test]
fn distributed_ram_byte_enable_write_preserves_other_lanes() {
    let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
        return;
    };
    let source_path = std::env::temp_dir().join(format!(
        "eda-synth-byte-enable-ram-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &source_path,
        r#"
module byte_enable_ram(
  input logic clk,
  input logic write,
  input logic [1:0] address,
  input logic [7:0] din,
  output logic [31:0] q
);
  logic [31:0] memory [0:3];
  always @(posedge clk) begin
    if (write) memory[address][23:16] <= din;
  end
  assign q = memory[address];
endmodule
"#,
    )
    .expect("write byte-enable RAM source");
    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("byte_enable_ram"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("directly lower byte-enable RAM");
    assert_eq!(rtl.memories[0].write_mask.as_ref().unwrap().bits.len(), 32);
    assert_eq!(
        rtl.memories[0].byte_enable.as_ref().unwrap().lanes,
        [false, false, true, false].to_vec()
    );
    let result =
        synthesize(&rtl, &SynthOptions::default()).expect("lower byte-enable distributed RAM");
    assert_eq!(result.netlist.stats().seq, 128);

    let gate = eda_core::to_design::netlist_to_design(&result.netlist);
    let mut rtl_sim = Simulator::new(&rtl).unwrap();
    let mut gate_sim = Simulator::new(&gate).unwrap();
    for sim in [&mut rtl_sim, &mut gate_sim] {
        sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        sim.set_input("write", &LogicWord::from_u64(1, 1)).unwrap();
    }
    for (address, lane_value) in [(0_u64, 0x12), (1, 0x34), (2, 0x56), (3, 0x78)] {
        for sim in [&mut rtl_sim, &mut gate_sim] {
            sim.set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
            sim.set_input("din", &LogicWord::from_u64(8, lane_value))
                .unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        let rtl_q = rtl_sim.read("q").unwrap();
        let gate_q = gate_sim.read("q").unwrap();
        assert_eq!(rtl_q, gate_q, "write address {address}");
        assert!(gate_q.to_hex_str().len() >= 8);
        assert_eq!(
            (0..8).map(|bit| gate_q.bit(16 + bit)).collect::<Vec<_>>(),
            {
                let expected = LogicWord::from_u64(8, lane_value);
                (0..8).map(|bit| expected.bit(bit)).collect::<Vec<_>>()
            }
        );
    }
}
