//! Distributed/block-ROM synthesis and simulation tests.

use super::super::super::*;
use eda_core::ir::MemoryKind;
use eda_core::logic::LogicWord;
use eda_sim::Simulator;

#[test]
fn distributed_rom_synthesizes_and_matches_direct_ast_semantics() {
    let Some(slang) = eda_slang::SlangFrontend::discover_optional() else {
        return;
    };
    let source_path = std::env::temp_dir().join(format!(
        "eda-synth-distributed-rom-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &source_path,
        r#"
module rom(input logic [1:0] address, output logic [7:0] q);
  logic [7:0] memory [0:3];
  initial begin
    memory[0] = 8'h12;
    memory[1] = 8'h34;
    memory[2] = 8'h56;
    memory[3] = 8'h78;
  end
  assign q = memory[address];
endmodule
"#,
    )
    .expect("write distributed ROM source");
    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("rom"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("directly lower distributed ROM");
    let result = synthesize(&rtl, &SynthOptions::default())
        .expect("lower distributed ROM to combinational logic");
    assert_eq!(result.memory_plans.len(), 1);
    assert_eq!(
        result.memory_plans[0].candidate,
        MemoryMacroCandidate::DistributedRom
    );
    assert!(!result
        .warnings
        .iter()
        .any(|warning| warning.contains("no synthesis lowering")));

    let gate = eda_core::to_design::netlist_to_design(&result.netlist);
    let mut rtl_sim = Simulator::new(&rtl).unwrap();
    let mut gate_sim = Simulator::new(&gate).unwrap();
    for address in 0..4_u64 {
        rtl_sim
            .set_input("address", &LogicWord::from_u64(2, address))
            .unwrap();
        gate_sim
            .set_input("address", &LogicWord::from_u64(2, address))
            .unwrap();
        assert_eq!(
            rtl_sim.read("q").unwrap(),
            gate_sim.read("q").unwrap(),
            "address {address}"
        );
    }
}

#[test]
fn block_rom_preserves_one_macro_and_matches_direct_ast_semantics() {
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
    let source_path = std::env::temp_dir().join(format!("eda-synth-block-rom-{unique}.sv"));
    let image_path = std::env::temp_dir().join(format!("eda-synth-block-rom-{unique}.hex"));
    let mut image = String::new();
    for address in 0..4096_u64 {
        let value = (address.wrapping_mul(7).wrapping_add(13) & 0xff) as u8;
        image.push_str(&format!("{value:02x}\n"));
    }
    std::fs::write(&image_path, image).expect("write BlockRom image");
    std::fs::write(
        &source_path,
        format!(
            r#"
module block_rom(
  input logic clk,
  input logic [11:0] address,
  output logic [7:0] q
);
  logic [7:0] memory [0:4095];
  initial $readmemh("{}", memory);
  always @(posedge clk) q <= memory[address];
endmodule
"#,
            image_path.display()
        ),
    )
    .expect("write BlockRom source");

    let rtl = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("block_rom"),
            &eda_slang::PreprocessOptions::default(),
        )
        .expect("directly lower BlockRom source");
    let result = synthesize(&rtl, &SynthOptions::default())
        .expect("lower large synchronous ROM to one BlockRom macro");
    assert_eq!(result.memory_plans.len(), 1);
    assert_eq!(
        result.memory_plans[0].candidate,
        MemoryMacroCandidate::BlockRom
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
            .filter(|cell| matches!(cell.kind, CellKind::BlockRom { .. }))
            .count(),
        8
    );

    let gate = eda_core::to_design::netlist_to_design(&result.netlist);
    assert_eq!(gate.memories.len(), 1);
    assert_eq!(gate.memories[0].flat_length, 4096);
    assert_eq!(gate.memories[0].kind, MemoryKind::Rom);

    let mut rtl_sim = Simulator::new(&rtl).unwrap();
    let mut gate_sim = Simulator::new(&gate).unwrap();
    for address in [0_u64, 1, 255, 2048, 4095] {
        for simulator in [&mut rtl_sim, &mut gate_sim] {
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 0))
                .unwrap();
            simulator
                .set_input("address", &LogicWord::from_u64(12, address))
                .unwrap();
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
        }
        assert_eq!(
            rtl_sim.read("q").unwrap(),
            gate_sim.read("q").unwrap(),
            "address {address}"
        );
        assert_eq!(
            gate_sim.read("q").unwrap().to_u64(),
            Some(address.wrapping_mul(7).wrapping_add(13) & 0xff),
            "address {address}"
        );
    }

    let verilog = crate::writer::write_verilog(&result.netlist, result.library.as_ref());
    assert_eq!(
        verilog
            .matches("  reg [7:0] block_rom_0 [0:4095];\n")
            .count(),
        1
    );
    assert_eq!(verilog.matches("block_rom_1").count(), 0);

    let _ = std::fs::remove_file(&source_path);
    let _ = std::fs::remove_file(&image_path);
}
