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
        error.contains("supports only assignment expression statements"),
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
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nested_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect_err("nested hierarchy must fail closed");
    assert!(
        error.contains("does not yet support nested module hierarchy"),
        "unexpected error: {error}"
    );
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
