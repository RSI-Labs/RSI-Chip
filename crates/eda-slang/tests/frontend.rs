use eda_core::logic::LogicWord;
use eda_sim::Simulator;
use eda_slang::{PreprocessOptions, SlangFrontend};
use std::path::PathBuf;

fn temporary_directory(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "eda-slang-test-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("create temporary test directory");
    path
}

#[test]
fn slang_preprocesses_macros_and_includes_before_rtl_lowering() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("macros");
    let include_directory = root.join("include");
    std::fs::create_dir_all(&include_directory).expect("create include directory");
    std::fs::write(include_directory.join("width.svh"), "`define BUS_WIDTH 8\n")
        .expect("write include");
    let source = r#"
`include "width.svh"
`ifndef BUS_WIDTH
`define BUS_WIDTH 4
`endif
module top(input [`BUS_WIDTH-1:0] a, output [`BUS_WIDTH-1:0] y);
  assign y = ~a;
endmodule
"#;
    let source_path = root.join("top.sv");
    std::fs::write(&source_path, source).expect("write source");

    let options = PreprocessOptions {
        include_directories: vec![include_directory],
        ..Default::default()
    };
    let design = slang
        .compile_files(&[source_path], Some("top"), &options)
        .expect("slang frontend compiles preprocessed RTL");
    assert_eq!(design.name, "top");
    assert_eq!(design.n_nets, 16);

    let mut simulator = Simulator::new(&design).expect("create simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xa5))
        .expect("set input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn slang_ast_direct_lowering_compiles_and_simulates_simple_modules() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast");
    let source_path = root.join("simple.sv");
    std::fs::write(
        &source_path,
        r#"
module simple #(parameter SHIFT = 1)(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic [3:0] sum,
  output logic [3:0] mask,
  output logic [3:0] shifted,
  output logic [3:0] inverted,
  output logic selected,
  output logic [7:0] concatenated,
  output logic [2:0] sliced,
  output logic [7:0] literal
);
  logic [3:0] initialized = 4'h3;
  assign sum = a + b;
  assign mask = a & b;
  assign shifted = a << SHIFT;
  assign inverted = ~a;
  assign selected = a[0] ? b[0] : a[1];
  assign concatenated = {a, b};
  assign sliced = a[3:1];
  assign literal = 8'hA5;
endmodule
"#,
    )
    .expect("write direct-lowering source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("simple"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower slang AST");
    let preprocessed_reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("simple"),
            &PreprocessOptions::default(),
        )
        .expect("lower the same module through the preprocessed path");
    assert_eq!(design.name, "simple");
    assert_eq!(design.n_nets, 48);
    assert_eq!(design.assigns.len(), 9);
    assert_eq!(preprocessed_reference.name, design.name);
    assert_eq!(preprocessed_reference.n_nets, design.n_nets);
    assert_eq!(preprocessed_reference.assigns.len(), design.assigns.len());
    assert_eq!(
        format!("{:?}", preprocessed_reference.ports),
        format!("{:?}", design.ports)
    );
    assert_eq!(
        format!("{:?}", preprocessed_reference.net_decls),
        format!("{:?}", design.net_decls)
    );

    let mut simulator = Simulator::new(&design).expect("create direct-AST simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1101))
        .expect("set a");
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0b0101))
        .expect("set b");
    let output = |name: &str| {
        simulator
            .read(name)
            .unwrap_or_else(|| panic!("missing output {name}"))
            .to_u64()
            .unwrap_or_else(|| panic!("{name} was unknown"))
    };
    assert_eq!(output("sum"), 0b0010);
    assert_eq!(output("mask"), 0b0101);
    assert_eq!(output("shifted"), 0b1010);
    assert_eq!(output("inverted"), 0b0010);
    assert_eq!(output("selected"), 1);
    assert_eq!(output("concatenated"), 0b11010101);
    assert_eq!(output("sliced"), 0b110);
    assert_eq!(output("literal"), 0xa5);
    assert_eq!(output("initialized"), 3);

    let unsupported_path = root.join("unsupported.sv");
    std::fs::write(
        &unsupported_path,
        "module unsupported(output logic y); initial $display(\"unsupported\"); endmodule\n",
    )
    .expect("write unsupported source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&unsupported_path),
            Some("unsupported"),
            &PreprocessOptions::default(),
        )
        .expect_err("direct path must reject unsupported constructs");
    assert!(
        error.contains("does not support system call"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_compiles_procedural_control_flow() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-procedural");
    let source_path = root.join("procedural.sv");
    std::fs::write(
        &source_path,
        r#"
module procedural(
  input logic a,
  input logic clk,
  input logic [1:0] sel,
  output logic y,
  output logic z,
  output logic q
);
  initial q = 1'b0;
  always_comb begin
    if (a) begin
      y = 1'b1;
    end else begin
      y = 1'b0;
    end
    case (sel)
      2'b00: z = 1'b0;
      2'b01: z = 1'b1;
      default: z = a;
    endcase
  end
  always @(posedge clk) q <= a;
endmodule
"#,
    )
    .expect("write procedural direct-lowering source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("procedural"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower procedural slang AST");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("procedural"),
            &PreprocessOptions::default(),
        )
        .expect("lower procedural module through the preprocessed path");
    assert_eq!(design.n_nets, 7);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.processes.len(), 2);
    assert_eq!(
        format!("{:?}", reference.initials),
        format!("{:?}", design.initials)
    );
    assert_eq!(
        format!("{:?}", reference.processes),
        format!("{:?}", design.processes)
    );

    let mut simulator = Simulator::new(&design).expect("create procedural simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk low");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set a");
    simulator
        .set_input("sel", &LogicWord::from_u64(2, 2))
        .expect("set default case selector");
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create posedge");
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));

    let mixed_event_path = root.join("mixed-event.sv");
    std::fs::write(
        &mixed_event_path,
        r#"
module mixed_event(input logic a, input logic clk, output logic y);
  always @(posedge clk or a) y = a;
endmodule
"#,
    )
    .expect("write mixed-event source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mixed_event_path),
            Some("mixed_event"),
            &PreprocessOptions::default(),
        )
        .expect_err("mixed edge/level sensitivity must fail closed");
    assert!(
        error.contains("single signal event sensitivity"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_flattens_simple_hierarchy() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-hierarchy");
    let source_path = root.join("hierarchy.sv");
    std::fs::write(
        &source_path,
        r#"
module child #(parameter W = 4)
    (input logic [W-1:0] a, output logic [W-1:0] y, output logic held);
  assign y = ~a;
  initial held = 1'b1;
endmodule

module top(input logic [3:0] a, output logic [3:0] y, output logic held);
  child #(.W(4)) u (.a(a), .y(y), .held(held));
endmodule
"#,
    )
    .expect("write hierarchy source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple hierarchy");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower hierarchy through the preprocessed path");
    assert_eq!(
        design.n_nets, 18,
        "direct declarations: {:?}; reference declarations: {:?}",
        design.net_decls, reference.net_decls
    );
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.hierarchy.len(), 2);
    assert_eq!(
        format!("{:?}", reference.net_decls),
        format!("{:?}", design.net_decls)
    );
    assert_eq!(
        format!("{:?}", reference.assigns),
        format!("{:?}", design.assigns)
    );
    assert_eq!(
        format!("{:?}", reference.initials),
        format!("{:?}", design.initials)
    );
    assert_eq!(
        format!("{:?}", reference.hierarchy),
        format!("{:?}", design.hierarchy)
    );

    let mut simulator = Simulator::new(&design).expect("create hierarchy simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1010))
        .expect("set hierarchy input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0b0101));
    assert_eq!(simulator.read("held").unwrap().to_u64(), Some(1));

    let nested_path = root.join("nested.sv");
    std::fs::write(
        &nested_path,
        r#"
module leaf(input logic a, output logic y); assign y = a; endmodule
module middle(input logic a, output logic y); leaf u (.a(a), .y(y)); endmodule
module top(input logic a, output logic y); middle u (.a(a), .y(y)); endmodule
"#,
    )
    .expect("write nested hierarchy source");
    let nested_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nested_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested hierarchy");
    let nested_reference = slang
        .compile_files(
            std::slice::from_ref(&nested_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower nested hierarchy through the preprocessed path");
    assert_eq!(nested_design.n_nets, 6);
    assert_eq!(nested_design.assigns.len(), 5);
    assert_eq!(nested_design.hierarchy.len(), 3);
    assert!(
        nested_design
            .hierarchy
            .iter()
            .any(|instance| instance.path == "top.u.u"),
        "unexpected hierarchy: {:?}",
        nested_design.hierarchy
    );
    assert_eq!(
        format!("{:?}", nested_reference.net_decls),
        format!("{:?}", nested_design.net_decls)
    );
    assert_eq!(
        format!("{:?}", nested_reference.assigns),
        format!("{:?}", nested_design.assigns)
    );
    assert_eq!(
        format!("{:?}", nested_reference.hierarchy),
        format!("{:?}", nested_design.hierarchy)
    );

    let mut nested_simulator = Simulator::new(&nested_design).expect("create nested simulator");
    nested_simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set nested input");
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(1));
    nested_simulator
        .set_input("a", &LogicWord::from_u64(1, 0))
        .expect("update nested input");
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_uses_sparse_memory_operations() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory");
    let source_path = root.join("memory.sv");
    std::fs::write(
        &source_path,
        r#"
module memory(
  input logic clk,
  input logic write,
  input logic [1:0] addr,
  input logic [7:0] wdata,
  output logic [7:0] rdata
);
  logic [7:0] mem [0:3];
  initial begin
    mem[0] = 8'h11;
    mem[1] = 8'h22;
  end
  always_comb rdata = mem[addr];
  always @(posedge clk) begin
    if (write) mem[addr] <= wdata;
  end
endmodule
"#,
    )
    .expect("write sparse-memory source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse memory");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse memory through the preprocessed path");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].name, "mem");
    assert_eq!(design.memories[0].element_width, 8);
    assert_eq!(design.memories[0].flat_length, 4);
    assert!(reference.memories.is_empty());
    assert_eq!(
        format!("{:?}", reference.ports),
        format!("{:?}", design.ports)
    );

    let mut simulator = Simulator::new(&design).expect("create sparse-memory simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk");
    simulator
        .set_input("write", &LogicWord::from_u64(1, 0))
        .expect("initialize write");
    simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("select initialized element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0x11));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select write element");
    simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0xaa))
        .expect("set write data");
    simulator
        .set_input("write", &LogicWord::from_u64(1, 1))
        .expect("request write");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create sparse-memory posedge");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0xaa));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("reset clk");
    simulator
        .set_input("addr", &LogicWord::from_u64(2, 1))
        .expect("select another initialized element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0x22));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select overwritten element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), Some(0xaa));

    simulator
        .set_input("addr", &LogicWord::from_u64(2, 3))
        .expect("select out-of-range element");
    assert_eq!(simulator.read("rdata").unwrap().to_u64(), None);

    let mut reference_simulator =
        Simulator::new(&reference).expect("create legacy-array simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy clk");
    reference_simulator
        .set_input("write", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy write");
    reference_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("select initialized legacy element");
    assert_eq!(
        reference_simulator.read("rdata").unwrap().to_u64(),
        Some(0x11)
    );
    reference_simulator
        .set_input("addr", &LogicWord::from_u64(2, 2))
        .expect("select legacy write element");
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0xaa))
        .expect("set legacy write data");
    reference_simulator
        .set_input("write", &LogicWord::from_u64(1, 1))
        .expect("request legacy write");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create legacy posedge");
    assert_eq!(
        reference_simulator.read("rdata").unwrap().to_u64(),
        Some(0xaa)
    );
}

#[test]
fn slang_ast_direct_lowering_uses_sparse_memory_part_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-parts");
    let source_path = root.join("memory-parts.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_parts(
  input logic clk,
  input logic [2:0] start,
  input logic [7:0] data,
  input logic [1:0] raddr,
  output logic [3:0] simple,
  output logic [3:0] up,
  output logic [3:0] down,
  output logic [3:0] updated
);
  logic [7:0] mem [0:3];
  initial begin
    mem[0] = 8'h9c;
    mem[1] = 8'h22;
  end
  assign simple = mem[0][0 +: 4];
  assign up = mem[0][2 +: 4];
  assign down = mem[0][6 -: 4];
  assign updated = mem[raddr][start +: 4];
  always @(posedge clk) begin
    mem[1][start +: 4] <= data[3:0];
  end
endmodule
"#,
    )
    .expect("write sparse-memory part-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory part selects");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse-memory part selects through the preprocessed path");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create part-select simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize clk");
    simulator
        .set_input("start", &LogicWord::from_u64(3, 2))
        .expect("set part-select start");
    simulator
        .set_input("raddr", &LogicWord::from_u64(2, 1))
        .expect("set read element");
    assert_eq!(simulator.read("simple").unwrap().to_u64(), Some(0b1100));
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0b0111));
    assert_eq!(simulator.read("down").unwrap().to_u64(), Some(0b0011));
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(0b1000));

    simulator
        .set_input("data", &LogicWord::from_u64(8, 0b1011))
        .expect("set part-write data");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create part-write posedge");
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(0b1011));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create legacy part-select simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize legacy clk");
    reference_simulator
        .set_input("start", &LogicWord::from_u64(3, 2))
        .expect("set legacy part-select start");
    reference_simulator
        .set_input("raddr", &LogicWord::from_u64(2, 1))
        .expect("set legacy read element");
    assert_eq!(
        reference_simulator.read("simple").unwrap().to_u64(),
        Some(0b1100)
    );
    assert_eq!(
        reference_simulator.read("up").unwrap().to_u64(),
        Some(0b0111)
    );
    assert_eq!(
        reference_simulator.read("down").unwrap().to_u64(),
        Some(0b0011)
    );
    assert_eq!(
        reference_simulator.read("updated").unwrap().to_u64(),
        Some(0b1000)
    );
    reference_simulator
        .set_input("data", &LogicWord::from_u64(8, 0b1011))
        .expect("set legacy part-write data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create legacy part-write posedge");
    assert_eq!(
        reference_simulator.read("updated").unwrap().to_u64(),
        Some(0b1011)
    );

    let simple_path = root.join("simple-part.sv");
    std::fs::write(
        &simple_path,
        r#"
module simple_part(output logic [3:0] y);
  logic [7:0] mem [0:0];
  initial begin
    mem[0] = 8'h9c;
  end
  assign y = mem[0][3:0];
endmodule
"#,
    )
    .expect("write simple memory part-select source");
    let simple_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&simple_path),
            Some("simple_part"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple memory part select");
    let mut simple_simulator =
        Simulator::new(&simple_design).expect("create simple part-select simulator");
    simple_simulator
        .run_until(0)
        .expect("settle simple part-select outputs");
    assert_eq!(simple_simulator.read("y").unwrap().to_u64(), Some(0b1100));
}

#[test]
fn slang_and_bounded_paths_lower_array_element_simple_ranges() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("array-simple-range");
    let source_path = root.join("simple-range.sv");
    std::fs::write(
        &source_path,
        r#"
module simple_range(input logic [1:0] addr, output logic [3:0] y);
  logic [7:0] mem [0:0];
  initial begin
    mem[0] = 8'h9c;
    mem[0][2:1] = 2'b01;
  end
  assign y = mem[addr][3:0];
endmodule
"#,
    )
    .expect("write array-element simple-range source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower array-element simple range");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &PreprocessOptions::default(),
        )
        .expect("lower array-element simple range through bounded path");
    assert_eq!(direct.memories.len(), 1);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct simulator");
    direct_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("set direct address");
    assert_eq!(direct_simulator.read("y").unwrap().to_u64(), Some(0b1010));

    let mut bounded_simulator = Simulator::new(&bounded).expect("create bounded simulator");
    bounded_simulator
        .set_input("addr", &LogicWord::from_u64(2, 0))
        .expect("set bounded address");
    assert_eq!(bounded_simulator.read("y").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_uses_multidimensional_sparse_memory() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory2d");
    let source_path = root.join("memory2d.sv");
    std::fs::write(
        &source_path,
        r#"
module memory2d(
  input logic clk,
  input logic [1:0] x,
  input logic [2:0] y,
  input logic [7:0] wdata,
  output logic [7:0] q,
  output logic [3:0] p
);
  logic [7:0] mem [0:1][2:3];
  initial begin
    mem[0][2] = 8'h12;
    mem[1][3] = 8'h34;
  end
  always_comb q = mem[x][y];
  assign p = mem[x][y][3:0];
  always @(posedge clk) mem[x][y] <= wdata;
endmodule
"#,
    )
    .expect("write multidimensional sparse-memory source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional sparse memory");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional memory through the bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(direct.memories[0].dimensions.len(), 2);
    assert_eq!(direct.memories[0].flat_length, 4);
    assert_eq!(direct.n_nets, 27);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct 2D simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 0))
        .expect("set first direct index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 2))
        .expect("set second direct index");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x12));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x2));
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set direct 2D write data");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("update first direct index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("update second direct index");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x4));
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct 2D posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0xa));

    let mut reference_simulator = Simulator::new(&reference).expect("create bounded 2D simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 0))
        .expect("set first bounded index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 2))
        .expect("set second bounded index");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x12));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x2));
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("update first bounded index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("update second bounded index");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x4));
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set bounded 2D write data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded 2D posedge");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0xa));
}

#[test]
fn slang_ast_direct_lowering_uses_multidimensional_indexed_part_writes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory2d-parts");
    let source_path = root.join("memory2d-parts.sv");
    std::fs::write(
        &source_path,
        r#"
module memory2d_parts(
  input logic clk,
  input logic [1:0] x,
  input logic [2:0] y,
  input logic [2:0] start,
  input logic [7:0] wdata,
  output logic [7:0] q,
  output logic [3:0] p
);
  logic [7:0] mem [0:1][2:3];
  initial begin
    mem[1][3] = 8'h34;
  end
  assign q = mem[x][y];
  assign p = mem[x][y][start +: 4];
  always @(posedge clk) mem[x][y][start +: 4] <= wdata[3:0];
endmodule
"#,
    )
    .expect("write multidimensional indexed part-write source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional indexed part write");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional indexed part write through bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(direct.memories[0].dimensions.len(), 2);

    let mut direct_simulator = Simulator::new(&direct).expect("create direct part-write simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("set direct first index");
    direct_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("set direct second index");
    direct_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set direct part start");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x3));
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set direct part data");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0xa));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded part-write simulator");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    reference_simulator
        .set_input("x", &LogicWord::from_u64(2, 1))
        .expect("set bounded first index");
    reference_simulator
        .set_input("y", &LogicWord::from_u64(3, 3))
        .expect("set bounded second index");
    reference_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set bounded part start");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0x3));
    reference_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set bounded part data");
    reference_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded posedge");
    assert_eq!(reference_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(reference_simulator.read("p").unwrap().to_u64(), Some(0xa));
}

#[test]
fn slang_ast_direct_lowering_shares_child_sparse_memory_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-port");
    let source_path = root.join("memory-port.sv");
    std::fs::write(
        &source_path,
        r#"
module child(
  inout wire logic [7:0] memory [0:4095],
  output logic [7:0] q
);
  assign q = memory[2];
endmodule

module top(
  input logic [7:0] din,
  output logic [7:0] child_q,
  output logic [7:0] parent_q
);
  wire logic [7:0] memory [0:4095];
  child u (
    .memory(memory),
    .q(child_q)
  );
  assign memory[2] = din;
  assign parent_q = memory[2];
endmodule
"#,
    )
    .expect("write shared sparse-memory port source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower shared sparse-memory port");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower shared sparse-memory port through bounded path");
    assert_eq!(direct.memories.len(), 2);
    assert_eq!(direct.memory_connections.len(), 1);
    assert_eq!(direct.memory_connections[0].child, 1);
    assert_eq!(direct.memory_connections[0].parent, 0);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.memory_connections.len(), 1);
    let mut direct_simulator =
        Simulator::new(&direct).expect("create direct shared-memory simulator");
    direct_simulator
        .set_input("din", &LogicWord::from_u64(8, 0xa7))
        .expect("set direct shared data");
    assert_eq!(
        direct_simulator.read("child_q").unwrap().to_u64(),
        Some(0xa7)
    );
    assert_eq!(
        direct_simulator.read("parent_q").unwrap().to_u64(),
        Some(0xa7)
    );

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded shared-memory simulator");
    reference_simulator
        .set_input("din", &LogicWord::from_u64(8, 0xa7))
        .expect("set bounded shared data");
    assert_eq!(
        reference_simulator.read("child_q").unwrap().to_u64(),
        Some(0xa7)
    );
    assert_eq!(
        reference_simulator.read("parent_q").unwrap().to_u64(),
        Some(0xa7)
    );
}

#[test]
fn slang_ast_direct_lowering_initializes_multidimensional_sparse_memory() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-init");
    let source_path = root.join("memory-init.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_init(
  output logic [7:0] first,
  output logic [7:0] middle,
  output logic [7:0] last
);
  logic [7:0] mem [0:1][0:2] = '{
    '{8'h01, 8'h02, 8'h03},
    '{8'h04, 8'h05, 8'h06}
  };
  assign first = mem[0][0];
  assign middle = mem[0][2];
  assign last = mem[1][2];
endmodule
"#,
    )
    .expect("write sparse-memory initializer source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_init"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory initializer");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].dimensions.len(), 2);
    assert_eq!(design.memories[0].flat_length, 6);
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create initializer simulator");
    simulator
        .run_until(0)
        .expect("settle sparse-memory initializers");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("middle").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(6));
}

#[test]
fn slang_ast_direct_lowering_reads_sparse_memory_files() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-readmem");
    let source_path = root.join("readmem.sv");
    std::fs::write(
        &source_path,
        r#"
module readmem(
  output logic [7:0] first,
  output logic [7:0] second,
  output logic [7:0] skipped,
  output logic [7:0] last,
  output logic [1:0] binary
);
  logic [7:0] mem [0:4096];
  logic [1:0] bits [0:2048];
  initial begin
    $readmemh("memory.hex", mem);
    $readmemb("bits.bin", bits);
  end
  assign first = mem[0];
  assign second = mem[1];
  assign skipped = mem[2];
  assign last = mem[3];
  assign binary = bits[1];
endmodule
"#,
    )
    .expect("write memory-file source");
    std::fs::write(
        root.join("memory.hex"),
        r#"
// leading comment
01 02
/* block
   comment with ff ff */
@3 04
"#,
    )
    .expect("write hexadecimal memory file");
    std::fs::write(root.join("bits.bin"), "01\n10\n").expect("write binary memory file");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("readmem"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory file loads");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("readmem"),
            &PreprocessOptions::default(),
        )
        .expect("lower memory files through the bounded path");
    assert_eq!(design.memories.len(), 2);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create memory-file simulator");
    simulator
        .run_until(0)
        .expect("settle memory-file initializers");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("skipped").unwrap().to_u64(), None);
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("binary").unwrap().to_u64(), Some(2));

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded memory-file simulator");
    reference_simulator
        .run_until(0)
        .expect("settle bounded memory-file initializers");
    assert_eq!(reference_simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(
        reference_simulator.read("second").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(reference_simulator.read("skipped").unwrap().to_u64(), None);
    assert_eq!(reference_simulator.read("last").unwrap().to_u64(), Some(4));
    assert_eq!(
        reference_simulator.read("binary").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_and_bounded_memory_file_ranges_load_the_same_elements() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-readmem-range");
    let source_path = root.join("readmem-range.sv");
    std::fs::write(
        &source_path,
        r#"
module readmem_range(
  output logic [7:0] first,
  output logic [7:0] second,
  output logic [7:0] third,
  output logic [7:0] after
);
  logic [7:0] mem [0:4096];
  initial $readmemh("range.hex", mem, 2, 4);
  assign first = mem[2];
  assign second = mem[3];
  assign third = mem[4];
  assign after = mem[5];
endmodule
"#,
    )
    .expect("write ranged memory-file source");
    std::fs::write(root.join("range.hex"), "01 02 03 04 05\n").expect("write range data");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("readmem_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower ranged memory file");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("readmem_range"),
            &PreprocessOptions::default(),
        )
        .expect("lower ranged memory file through bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(bounded.memories.len(), 1);
    assert_eq!(direct.initials.len(), 1);
    assert_eq!(bounded.initials.len(), 1);

    for design in [&direct, &bounded] {
        let mut simulator = Simulator::new(design).expect("create ranged memory simulator");
        simulator
            .run_until(0)
            .expect("settle ranged memory initializer");
        assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("second").unwrap().to_u64(), Some(2));
        assert_eq!(simulator.read("third").unwrap().to_u64(), Some(3));
        assert_eq!(simulator.read("after").unwrap().to_u64(), None);
    }
}

#[test]
fn slang_and_bounded_paths_write_sparse_memory_files() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("memory-write-files");
    let source_path = root.join("writemem.sv");
    std::fs::write(
        &source_path,
        r#"
module writemem;
  logic [7:0] mem [0:4096];
  logic [1:0] bits [0:2048];
  initial begin
    mem[2] = 8'h01;
    mem[3] = 8'h02;
    bits[1] = 2'b10;
    $writememh("writemem.hex", mem, 2, 4);
    $writememb("writemem.bin", bits, 1, 2);
  end
endmodule
"#,
    )
    .expect("write sparse-memory dump source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("writemem"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory dump tasks");
    let bounded = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("writemem"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse-memory dump tasks through bounded path");
    assert_eq!(direct.memories.len(), 2);
    assert_eq!(bounded.memories.len(), 2);
    assert_eq!(direct.initials.len(), 1);
    assert_eq!(bounded.initials.len(), 1);

    for design in [&direct, &bounded] {
        let mut simulator = Simulator::new(design).expect("create sparse-memory dump simulator");
        simulator
            .run_until(0)
            .expect("execute sparse-memory dump tasks");
        assert_eq!(
            std::fs::read_to_string(root.join("writemem.hex")).unwrap(),
            "1\n2\nxx\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("writemem.bin")).unwrap(),
            "10\nxx\n"
        );
    }
}

#[test]
fn slang_ast_summary_validates_elaborated_structure_after_lowering() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("ast-summary");
    let source_path = root.join("top.sv");
    std::fs::write(
        &source_path,
        r#"
module child #(parameter CHILD_WIDTH = 1)
    (input logic a, output logic y);
  assign y = ~a;
endmodule

module top #(parameter WIDTH = 4)
    (input logic [WIDTH-1:0] a, output logic [WIDTH-1:0] y,
     output logic [7:0] lit);
  logic [7:0] wide;
  logic held;
  logic [WIDTH-1:0] sum;
  child #(.CHILD_WIDTH(1)) u(.a(a[0]), .y(y[0]));
  initial held = a[0];
  assign y[3:1] = a[3:1];
  assign wide = {a, a};
  assign sum = a + a;
  assign lit = 8'hA5;
endmodule
"#,
    )
    .expect("write hierarchy source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("compile and structurally validate hierarchy");
    let json = slang
        .ast_json(&[source_path], Some("top"), &["top".to_string()])
        .expect("serialize slang AST");
    let summary = slang
        .summarize_ast_json(&json)
        .expect("parse slang AST summary");

    assert_eq!(summary.top_name, "top");
    for (key, expected_line) in [
        ("top", 7),
        ("parameter:WIDTH", 7),
        ("port:a", 8),
        ("declaration:wide", 10),
        ("instance:u", 13),
        ("procedural:0", 14),
        ("continuous:wide:1", 16),
        ("continuous:sum:2", 17),
        ("continuous:lit:3", 18),
    ] {
        let span = summary
            .source_spans
            .get(key)
            .unwrap_or_else(|| panic!("missing source span {key}"));
        assert!(span.file.ends_with("top.sv"), "{key} file: {}", span.file);
        assert_eq!(span.start_line, expected_line, "{key}");
        assert!(span.start_column > 0, "{key}");
    }
    assert_eq!(
        summary
            .ports
            .iter()
            .map(|port| (port.name.as_str(), port.direction.as_str()))
            .collect::<Vec<_>>(),
        vec![("a", "In"), ("y", "Out"), ("lit", "Out")]
    );
    assert!(summary.declarations.iter().any(|declaration| {
        declaration.name == "wide"
            && declaration.kind == "Variable"
            && declaration.type_name == "logic[7:0]"
            && declaration.width == Some(8)
    }));
    assert_eq!(
        summary
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.as_str(),
                    parameter.type_name.as_str(),
                    parameter.value.as_str(),
                    parameter.is_local,
                    parameter.is_port,
                )
            })
            .collect::<Vec<_>>(),
        vec![("WIDTH", "logic signed[31:0]", "4", false, true)]
    );
    assert_eq!(
        summary
            .continuous_targets
            .iter()
            .map(|target| {
                (
                    target.name.as_str(),
                    target.source.as_str(),
                    target.rhs.kind.as_str(),
                    target.rhs.operator.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("y", "ContinuousAssign", "RangeSelect", ""),
            ("wide", "ContinuousAssign", "Concatenation", ""),
            ("sum", "ContinuousAssign", "BinaryOp", "Add"),
            ("lit", "ContinuousAssign", "Conversion", ""),
        ]
    );
    let sum_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs;
    assert_eq!(sum_rhs.operator, "Add");
    assert_eq!(sum_rhs.type_width, Some(4));
    assert_eq!(sum_rhs.type_signed, Some(false));
    assert_eq!(
        sum_rhs
            .operands
            .iter()
            .map(|operand| (operand.kind.as_str(), operand.symbol.as_deref()))
            .collect::<Vec<_>>(),
        vec![("NamedValue", Some("a")), ("NamedValue", Some("a"))]
    );
    let child_instance = summary
        .instances
        .iter()
        .find(|instance| instance.instance_name == "u")
        .expect("child instance parameter summary");
    assert_eq!(
        child_instance
            .parameters
            .iter()
            .map(|parameter| {
                (
                    parameter.name.as_str(),
                    parameter.value.as_str(),
                    parameter.is_local,
                    parameter.is_port,
                )
            })
            .collect::<Vec<_>>(),
        vec![("CHILD_WIDTH", "1", false, true)]
    );
    let wide_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "wide")
        .expect("wide assignment summary")
        .rhs;
    assert_eq!(
        wide_rhs
            .operands
            .iter()
            .map(|operand| operand.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["NamedValue", "NamedValue"]
    );
    let literal_rhs = &summary
        .continuous_targets
        .iter()
        .find(|target| target.name == "lit")
        .expect("literal assignment summary")
        .rhs;
    assert_eq!(literal_rhs.type_width, Some(8));
    assert_eq!(literal_rhs.type_signed, Some(false));
    assert_eq!(literal_rhs.operands.len(), 1);
    assert_eq!(
        literal_rhs
            .operands
            .first()
            .and_then(|operand| operand.literal.as_deref()),
        Some("8:10100101")
    );
    assert_eq!(
        summary
            .procedural_blocks
            .iter()
            .map(|block| {
                (
                    block.procedure_kind.clone(),
                    block.assignment_targets.clone(),
                    block
                        .assignments
                        .iter()
                        .map(|assignment| {
                            (
                                assignment.target_names.clone(),
                                assignment.rhs.kind.clone(),
                                assignment.rhs.operator.clone(),
                                assignment.nonblocking,
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            "Initial".to_string(),
            vec!["held".to_string()],
            vec![(
                vec!["held".to_string()],
                "ElementSelect".to_string(),
                String::new(),
                false
            )]
        )]
    );
    assert_eq!(
        summary
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.instance_name.as_str(),
                    instance.definition_name.as_str(),
                )
            })
            .collect::<Vec<_>>(),
        vec![("u", "child")]
    );
    assert!(summary.kinds.contains("InstanceBody"));
    assert!(summary.kinds.contains("ContinuousAssign"));
    summary
        .validate_against_design(&design)
        .expect("slang and bounded design inventories agree");

    let mut dropped_port = summary.clone();
    dropped_port.ports.pop();
    assert!(dropped_port
        .validate_against_design(&design)
        .unwrap_err()
        .contains("do not match slang"));

    let mut dropped_declaration = summary.clone();
    dropped_declaration
        .declarations
        .push(eda_slang::SlangDeclarationSummary {
            name: "missing".into(),
            kind: "Variable".into(),
            type_name: "logic".into(),
            width: Some(1),
        });
    assert!(dropped_declaration
        .validate_against_design(&design)
        .unwrap_err()
        .contains("dropped slang declaration 'missing'"));

    let mut changed_width = summary.clone();
    changed_width
        .declarations
        .iter_mut()
        .find(|declaration| declaration.name == "wide")
        .expect("wide declaration summary")
        .width = Some(7);
    assert!(changed_width
        .validate_against_design(&design)
        .unwrap_err()
        .contains("width 8 does not match slang width 7"));

    let mut dropped_continuous = summary.clone();
    dropped_continuous
        .continuous_targets
        .push(eda_slang::SlangContinuousTarget {
            name: "wide".into(),
            source: "ContinuousAssign".into(),
            rhs: eda_slang::SlangExpressionSummary {
                kind: "BinaryOp".into(),
                operator: "Add".into(),
                type_width: None,
                type_signed: None,
                symbol: None,
                literal: None,
                operands: Vec::new(),
            },
        });
    assert!(dropped_continuous
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'wide' does not match slang"));

    let mut changed_operand_shape = summary.clone();
    let sum_rhs = changed_operand_shape
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    sum_rhs.kind = "BinaryOp".into();
    sum_rhs.operator = "Add".into();
    sum_rhs.symbol = None;
    assert!(changed_operand_shape
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_symbol = summary.clone();
    let operand = changed_operand_symbol
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.symbol = Some("wrong".into());
    assert!(changed_operand_symbol
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_width = changed_operand_symbol;
    let operand = changed_operand_width
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.type_width = Some(3);
    assert!(changed_operand_width
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut changed_operand_signedness = changed_operand_width;
    let operand = changed_operand_signedness
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "sum")
        .expect("sum assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("sum binary operand");
    operand.type_width = Some(4);
    operand.type_signed = Some(true);
    assert!(changed_operand_signedness
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'sum' does not match slang"));

    let mut dropped_procedural = summary.clone();
    dropped_procedural
        .procedural_blocks
        .push(eda_slang::SlangProceduralBlockSummary {
            procedure_kind: "Initial".into(),
            assignment_targets: vec!["held".into()],
            assignments: Vec::new(),
        });
    assert!(dropped_procedural
        .validate_against_design(&design)
        .unwrap_err()
        .contains("dropped initial blocks"));

    let mut changed_procedural_rhs = summary.clone();
    let assignment = changed_procedural_rhs
        .procedural_blocks
        .iter_mut()
        .find(|block| block.procedure_kind == "Initial")
        .expect("initial block summary")
        .assignments
        .first_mut()
        .expect("initial assignment summary");
    assignment.rhs.kind = "BinaryOp".into();
    assignment.rhs.operator = "Add".into();
    assert!(changed_procedural_rhs
        .validate_against_design(&design)
        .unwrap_err()
        .contains("procedural assignment shape for 'held' does not match slang"));

    let mut changed_literal = summary.clone();
    let literal_rhs = changed_literal
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "lit")
        .expect("literal assignment summary")
        .rhs
        .operands
        .first_mut()
        .expect("integer literal under conversion")
        .literal
        .as_mut()
        .expect("slang literal fingerprint");
    *literal_rhs = "8:10100100".into();
    assert!(changed_literal
        .validate_against_design(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'lit' does not match slang"));

    let mut dropped_instance = summary;
    dropped_instance.instances.pop();
    assert!(dropped_instance
        .validate_against_design(&design)
        .unwrap_err()
        .contains("direct instances"));
}

#[test]
fn slang_front_end_rejects_invalid_rtl() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("invalid");
    let source_path = root.join("broken.sv");
    std::fs::write(&source_path, "module broken(output y; assign y = 1'b0;\n")
        .expect("write broken source");
    let error = slang
        .check_files(&[source_path], Some("broken"))
        .expect_err("slang must reject malformed RTL");
    let diagnostic = format!("{error}\n{}\n{}", error.stdout, error.stderr);
    assert!(
        diagnostic.contains("error"),
        "unexpected failure: {diagnostic}"
    );
}

#[test]
fn slang_structural_validation_accepts_generate_instances() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let design = slang
        .compile_source(
            r#"
module child(input logic a, output logic y);
  assign y = a;
endmodule

module top(input logic a, output logic y);
  generate
    if (1) begin : selected
      child u(.a(a), .y(y));
    end
  endgenerate
endmodule
"#,
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("compile generated instance through slang boundary");
    assert_eq!(design.name, "top");
    assert!(design.instance("top.u").is_some());
}

#[test]
fn slang_ast_direct_lowering_accepts_generated_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let source = r#"
module child(input logic a, output logic y);
  assign y = a;
endmodule

module top(input logic a, output logic y, output logic z);
  generate
    if (1) begin : selected
      child u(.a(a), .y(y));
      assign z = ~a;
    end
  endgenerate
endmodule
"#;
    let root = temporary_directory("direct-ast-generate");
    let source_path = root.join("generate.sv");
    std::fs::write(&source_path, source).expect("write generate source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower generated members");
    let reference = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated members through the preprocessed path");
    assert_eq!(design.n_nets, 5);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(
        format!("{:?}", reference.net_decls),
        format!("{:?}", design.net_decls)
    );
    assert_eq!(
        format!("{:?}", reference.assigns),
        format!("{:?}", design.assigns)
    );
    assert_eq!(
        format!("{:?}", reference.hierarchy),
        format!("{:?}", design.hierarchy)
    );

    let mut simulator = Simulator::new(&design).expect("create generate simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set generate input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_json_contains_elaborated_module_symbols() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let design = slang
        .compile_source(
            "module json_probe(input [3:0] a, output [3:0] y); assign y = a + 4'd1; endmodule",
            Some("json_probe"),
            &PreprocessOptions::default(),
        )
        .expect("compile AST probe");
    assert_eq!(design.n_nets, 8);

    let path = std::env::temp_dir().join(format!(
        "eda-slang-json-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "module json_probe(input [3:0] a, output [3:0] y); assign y = a + 4'd1; endmodule",
    )
    .expect("write AST source");
    let json = slang
        .ast_json(&[path], Some("json_probe"), &["json_probe".to_string()])
        .expect("serialize slang AST");
    assert!(json.starts_with('{'));
    assert!(json.contains("\"kind\": \"Instance\""));
    assert!(json.contains("\"name\": \"json_probe\""));
}

#[test]
fn slang_accepts_the_official_accellera_uvm_reference_source() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some(uvm_home) = std::env::var_os("UVM_HOME") else {
        return;
    };
    let root = PathBuf::from(uvm_home);
    let source = root.join("src").join("uvm.sv");
    if !source.is_file() {
        return;
    }
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    slang
        .check_files_with_options(&[source], None, &options)
        .expect("slang elaborates the official UVM reference library");
}

#[test]
fn slang_validates_the_repository_cpu_rtl_compilation_units() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf();
    let rtl = root.join("platform").join("rtl");
    let i8086 = [
        rtl.join("i8086_prefix_decoder.sv"),
        rtl.join("i8086_opcode_decoder.sv"),
        rtl.join("i8086_modrm_decoder.sv"),
        rtl.join("i8086_full_core.sv"),
        rtl.join("i8086_full_runtime_tb.sv"),
    ];
    slang
        .check_files(&i8086, Some("i8086_full_runtime_tb"))
        .expect("slang validates the i8086 RTL compilation unit");

    let rv32i = [
        rtl.join("rv32i_full_core.sv"),
        rtl.join("rv32i_full_runtime_tb.sv"),
    ];
    slang
        .check_files(&rv32i, Some("rv32i_full_runtime_tb"))
        .expect("slang validates the RV32I RTL compilation unit");

    let rv32i_directed = [
        rtl.join("rv32i_full_core.sv"),
        rtl.join("rv32i_full_soc_16.sv"),
        rtl.join("rv32i_full_tb_16.sv"),
    ];
    slang
        .check_files(&rv32i_directed, Some("rv32i_full_tb_16"))
        .expect("slang validates the directed RV32I RTL compilation unit");
}
