//! Memory macro contract and timing planning tests.

use super::super::super::*;
use eda_core::ir::{MemoryDecl, MemoryStorage};
use eda_hdl::compile;

#[test]
fn sparse_memory_macro_plans_preserve_timing_and_write_contracts() {
    let rom = compile(
        r#"
module rom(input clk, input [11:0] address, output reg [7:0] q);
  logic [7:0] memory [0:4095];
  initial memory[0] = 8'h1;
  always @(posedge clk) q <= memory[address];
endmodule
"#,
        None,
    )
    .unwrap();
    let rom_plans = plan_memories(&rom);
    assert_eq!(rom_plans.len(), 1);
    assert_eq!(rom_plans[0].candidate, MemoryMacroCandidate::BlockRom);
    assert_eq!(rom_plans[0].bank_count, 1);
    assert_eq!(rom_plans[0].bank_depth, 4096);
    assert_eq!(rom_plans[0].write_mask, None);
    assert_eq!(rom_plans[0].byte_enable, None);

    let ram = compile(
        r#"
module ram(
  input logic clk,
  input logic write,
  input logic [11:0] address,
  input logic [7:0] din,
  output logic [7:0] q
);
  logic [7:0] memory [0:4095];
  always @(posedge clk) if (write) memory[address] <= din;
  assign q = memory[address];
endmodule
"#,
        None,
    )
    .unwrap();
    let ram_plans = plan_memories(&ram);
    assert_eq!(ram_plans.len(), 1);
    assert_eq!(ram_plans[0].candidate, MemoryMacroCandidate::BlockRam);
    assert_eq!(ram_plans[0].bank_count, 1);
    assert_eq!(ram_plans[0].bank_depth, 4096);
    assert_eq!(ram_plans[0].total_bits, 32768);
    assert!(ram_plans[0]
        .write_mask
        .as_ref()
        .expect("RAM write mask")
        .is_full());
    assert!(ram_plans[0]
        .byte_enable
        .as_ref()
        .expect("full byte-enable mask")
        .is_full());
}

#[test]
fn ordinary_arrays_do_not_become_sparse_memory_macro_plans() {
    let mut ordinary = Design::default();
    ordinary.memories.push(MemoryDecl {
        name: "values".into(),
        element_width: 8,
        element_msb: 7,
        element_lsb: 0,
        dimensions: Vec::new(),
        flat_length: 4,
        dynamic: false,
        queue: false,
        storage: MemoryStorage::Ordinary,
        kind: Default::default(),
        template: Default::default(),
        write_mask: None,
        byte_enable: None,
        banking: Default::default(),
    });
    assert_eq!(plan_memories(&ordinary), Vec::new());
}
