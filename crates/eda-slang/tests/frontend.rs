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
fn slang_ast_summary_validates_elaborated_structure_after_lowering() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("ast-summary");
    let source_path = root.join("top.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module top #(parameter WIDTH = 4)
    (input logic [WIDTH-1:0] a, output logic [WIDTH-1:0] y);
  logic [7:0] wide;
  child u(.a(a[0]), .y(y[0]));
  assign y[3:1] = a[3:1];
  assign wide = {a, a};
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
    assert_eq!(
        summary
            .ports
            .iter()
            .map(|port| (port.name.as_str(), port.direction.as_str()))
            .collect::<Vec<_>>(),
        vec![("a", "In"), ("y", "Out")]
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
            .map(|target| (target.name.as_str(), target.source.as_str()))
            .collect::<Vec<_>>(),
        vec![("y", "ContinuousAssign"), ("wide", "ContinuousAssign")]
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
        });
    assert!(dropped_continuous
        .validate_against_design(&design)
        .unwrap_err()
        .contains("dropped continuous assignments for 'wide'"));

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
