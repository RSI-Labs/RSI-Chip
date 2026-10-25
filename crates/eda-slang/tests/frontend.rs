use eda_core::logic::{LogicValue, LogicWord};
use eda_sim::Simulator;
use eda_slang::{PreprocessOptions, SlangFrontend};
use std::path::{Path, PathBuf};

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

fn validate_official_uvm_root(root: &Path) -> Result<PathBuf, String> {
    let source = root.join("src").join("uvm.sv");
    if !source.is_file() {
        return Err(format!(
            "UVM_HOME is set to {}, but the official UVM source is missing {}. Clone Accellera-official/uvm-core tag 2020.3.1 before running the required UVM gate",
            root.display(),
            source.display()
        ));
    }

    let version_path = root.join("src").join("base").join("uvm_version.svh");
    let version = std::fs::read_to_string(&version_path).map_err(|error| {
        format!(
            "cannot read official UVM version marker {}: {error}",
            version_path.display()
        )
    })?;
    if !version.contains("Accellera:1800.2:UVM:2020.3.1") {
        return Err(format!(
            "UVM_HOME {} does not contain the required official UVM 2020.3.1 source",
            root.display()
        ));
    }

    Ok(source)
}

fn official_uvm_sources() -> Option<(PathBuf, PathBuf)> {
    let uvm_home = std::env::var_os("UVM_HOME")?;
    let root = PathBuf::from(uvm_home);
    let source = validate_official_uvm_root(&root).expect("valid official UVM 2020.3.1 source");
    Some((root, source))
}

#[test]
fn official_uvm_validation_fails_closed_on_missing_source() {
    let root = temporary_directory("uvm-missing-source");
    let error = validate_official_uvm_root(&root)
        .expect_err("missing official UVM source must be an error, not a skip");
    assert!(error.contains("official UVM source is missing"), "{error}");
}

#[test]
fn official_uvm_validation_rejects_the_wrong_version() {
    let root = temporary_directory("uvm-wrong-version");
    let source_directory = root.join("src");
    let base_directory = source_directory.join("base");
    std::fs::create_dir_all(&base_directory).expect("create UVM source directories");
    std::fs::write(source_directory.join("uvm.sv"), "module uvm;\nendmodule\n")
        .expect("write UVM source stub");
    std::fs::write(
        base_directory.join("uvm_version.svh"),
        r#"parameter string UVM_VERSION_STRING = "Accellera:1800.2:UVM:2020.2.0";"#,
    )
    .expect("write wrong UVM version marker");

    let error = validate_official_uvm_root(&root)
        .expect_err("wrong official UVM version must be an error, not a skip");
    assert!(error.contains("required official UVM 2020.3.1"), "{error}");
}

#[test]
fn slang_compiles_macros_and_includes_through_the_direct_default_path() {
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
fn slang_ast_direct_lowering_supports_simple_class_objects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-simple-class");
    let source_path = root.join("simple_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Counter;
  int value;

  function new(int start);
    value = start;
  endfunction

  function void inc();
    value = value + 1;
  endfunction

  function int get();
    return value;
  endfunction

endclass

module class_top(
  output int observed0,
  output int observed1,
  output int observed2
);
  Counter counter;
  Counter second;
  initial begin
    counter = new(41);
    second = new(9);
    counter.inc();
    observed0 = counter.get();
    counter.inc();
    observed1 = counter.get();
    second.inc();
    observed2 = second.value;
  end
endmodule
"#,
    )
    .expect("write simple-class source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple class object");
    for field in ["counter.value", "second.value"] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == field && declaration.width == 32),
            "missing class field {field}"
        );
    }
    let class_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("class_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize simple-class AST");
    let mut class_summary = slang
        .summarize_ast_json(&class_json)
        .expect("summarize simple-class AST");
    class_summary
        .validate_against_design(&design)
        .expect("exact direct class-property fingerprints should match");
    if let Some(assignment) = class_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "observed2")
            })
        })
    {
        assignment.rhs.symbol = Some("second.wrong".into());
    }
    let property_error = class_summary
        .validate_against_design(&design)
        .expect_err("wrong direct class property must fail");
    assert!(
        property_error.contains("Ref:32:s[second.wrong]")
            && property_error.contains("Ref:32:s[second.value]"),
        "unexpected direct class-property error: {property_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create class simulator");
    simulator.run_until(0).expect("run class initial block");
    assert_eq!(simulator.read("observed0").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("observed1").unwrap().to_u64(), Some(43));
    assert_eq!(simulator.read("observed2").unwrap().to_u64(), Some(10));
}

#[test]
fn slang_ast_direct_lowering_supports_class_property_initializers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-property-initializers");
    let source_path = root.join("property_initializers.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  int value = 41;
  string label = "ready";
endclass

class Derived extends Base;
  int extra = 1;

  function new();
  endfunction

  function int total();
    return value + extra;
  endfunction
endclass

module property_initializer_top(
  output int total,
  output string label
);
  Derived object;
  initial begin
    object = new();
    total = object.total();
    label = object.label;
  end
endmodule
"#,
    )
    .expect("write property-initializer source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("property_initializer_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower class property initializers");
    let mut simulator = Simulator::new(&design).expect("create property-initializer simulator");
    simulator
        .run_until(0)
        .expect("run property-initializer constructor");
    assert_eq!(simulator.read("total").unwrap().to_u64(), Some(42));
    assert_eq!(
        simulator
            .read("label")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "ready"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_successful_class_casts() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-cast");
    let source_path = root.join("class_cast.sv");
    std::fs::write(
        &source_path,
        r#"
class CastBase;
  function new();
  endfunction

  virtual function int kind();
    return 1;
  endfunction
endclass

class CastDerived extends CastBase;
  function new();
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

module class_cast_top(output int observed);
  CastDerived object;
  CastBase source;
  CastDerived destination;

  initial begin
    object = new();
    source = object;
    $cast(destination, source);
    observed = destination.kind();
  end
endmodule
"#,
    )
    .expect("write class-cast source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("class_cast_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower a successful class cast");
    let cast_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("class_cast_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize class-cast AST");
    let mut cast_summary = slang
        .summarize_ast_json(&cast_json)
        .expect("summarize class-cast AST");
    cast_summary
        .validate_against_design(&design)
        .expect("exact field-less class-handle copy fingerprint should match");
    let expected_source_key = cast_summary
        .procedural_blocks
        .iter()
        .find_map(|block| {
            block.assignments.iter().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "source")
            })
        })
        .and_then(|assignment| assignment.class_handle_source_key.clone())
        .expect("field-less class-handle source key");
    let expected_source_key = expected_source_key
        .rsplit_once(' ')
        .map(|(_, name)| name)
        .expect("stable field-less class-handle source name");
    assert_eq!(expected_source_key, "object");
    if let Some(assignment) = cast_summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .target_names
                .first()
                .is_some_and(|name| name == "source")
        })
    }) {
        assignment.class_handle_source_key = Some("wrong_fieldless_source".into());
    }
    let fieldless_error = cast_summary
        .validate_against_design(&design)
        .expect_err("wrong field-less class-handle source must fail");
    assert!(
        fieldless_error.contains("ClassHandleCopy:wrong_fieldless_source")
            && fieldless_error.contains(&format!("ClassHandleCopy:{expected_source_key}")),
        "unexpected field-less class-handle error: {fieldless_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create class-cast simulator");
    simulator
        .run_until(0)
        .expect("run the class cast and dispatched method");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_ast_direct_lowering_supports_cast_conditions() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-cast-condition");
    let source_path = root.join("class_cast_condition.sv");
    std::fs::write(
        &source_path,
        r#"
class CastConditionBase;
  function new();
  endfunction

  virtual function int kind();
    return 3;
  endfunction
endclass

class CastConditionDerived extends CastConditionBase;
  function new();
  endfunction

  virtual function int kind();
    return 4;
  endfunction
endclass

module class_cast_condition_top(output int observed);
  CastConditionDerived object;
  CastConditionBase source;
  CastConditionDerived destination;

  initial begin
    object = new();
    source = object;
    if (!$cast(destination, source)) begin
      observed = 0;
    end else begin
      observed = destination.kind();
    end
  end
endmodule
"#,
    )
    .expect("write class-cast condition source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("class_cast_condition_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower a class-cast condition");
    let cast_condition_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("class_cast_condition_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize class-cast-condition AST");
    let mut cast_condition_summary = slang
        .summarize_ast_json(&cast_condition_json)
        .expect("summarize class-cast-condition AST");
    cast_condition_summary
        .validate_against_design(&design)
        .expect("exact guarded-cast method fingerprint should match");
    let expected_cast_result = format!("32:{:032b}", 4_u64);
    if let Some(assignment) = cast_condition_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "observed")
                    && assignment
                        .rhs
                        .literal
                        .as_deref()
                        .is_some_and(|literal| literal == expected_cast_result)
            })
        })
    {
        assignment.rhs.literal = Some(format!("32:{:032b}", 5_u64));
    }
    let guarded_cast_error = cast_condition_summary
        .validate_against_design(&design)
        .expect_err("wrong guarded-cast method result must fail");
    assert!(
        guarded_cast_error.contains(&format!("Const:32:u[32:{:032b}]", 4_u64))
            && guarded_cast_error.contains(&format!("Const:32:u[32:{:032b}]", 5_u64)),
        "unexpected guarded-cast method error: {guarded_cast_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create class-cast condition simulator");
    simulator
        .run_until(0)
        .expect("run the conditional class cast");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(4));
}

#[test]
fn slang_ast_direct_lowering_supports_constant_indexed_class_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-array");
    let source_path = root.join("class_array.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction

  function void set(int v);
    value = v;
  endfunction
endclass

module class_array_top(
  output int first,
  output int second,
  output int third,
  output int updated
);
  Item items[0:2];

  initial begin
    #1;
    items[0] = new(10);
    items[1] = new(20);
    items[2] = new(30);
    first = items[0].get();
    second = items[1].get();
    third = items[2].get();
    items[2].value = 77;
    updated = items[2].get();
  end
endmodule
"#,
    )
    .expect("write class-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("class_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower constant-indexed class arrays");
    let mut simulator = Simulator::new(&design).expect("create class-array simulator");
    simulator
        .run_until(1)
        .expect("run class-array initial block");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(20));
    assert_eq!(simulator.read("third").unwrap().to_u64(), Some(30));
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(77));
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_class_array_method_reads() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-class-array");
    let source_path = root.join("dynamic_class_array.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction

  function void set(int v);
    value = v;
  endfunction
endclass

module dynamic_class_array_top(
  input int selector,
  output int result,
  output int selected_value
);
  Item items[0:1];

  initial begin
    #1;
    items[0] = new(10);
    items[1] = new(20);
    result = items[selector].get();
    items[selector].set(77);
    selected_value = items[selector].value;
  end
endmodule
"#,
    )
    .expect("write dynamic class-array source");
    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_class_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic class-array method reads");
    let mut simulator = Simulator::new(&design).expect("create dynamic class-array simulator");
    simulator
        .set_input("selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run dynamic class-array initial block");
    assert_eq!(simulator.read("result").unwrap().to_u64(), Some(20));
    assert_eq!(simulator.read("selected_value").unwrap().to_u64(), Some(77));

    let mut out_of_range =
        Simulator::new(&design).expect("create out-of-range class-array simulator");
    out_of_range
        .set_input("selector", &LogicWord::from_u64(32, 2))
        .unwrap();
    out_of_range
        .run_until(1)
        .expect("run out-of-range class-array initial block");
    assert_eq!(out_of_range.read("result").unwrap().to_u64(), None);
    assert_eq!(out_of_range.read("selected_value").unwrap().to_u64(), None);
}

#[test]
fn slang_ast_direct_lowering_rejects_excessive_class_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-excessive-class-array");
    let source_path = root.join("excessive_class_array.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;
endclass

module excessive_class_array_top;
  Item items[0:4096];
endmodule
"#,
    )
    .expect("write excessive class-array source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("excessive_class_array_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("excessive class arrays must fail closed");
    assert!(
        error.contains("the direct path supports at most 4096"),
        "unexpected excessive class-array error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_class_array_writes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-class-array-write");
    let source_path = root.join("dynamic_class_array_write.sv");
    std::fs::write(
        &source_path,
        r#"
class Factory;
  static int calls = 0;

  static function int next(int value);
    calls = calls + 1;
    return value;
  endfunction
endclass

class Item;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction
endclass

module dynamic_class_array_write_top(
  input int selector,
  input int v,
  output int constructor_result,
  output int property_result,
  output int call_count
);
  Item items[0:1];

  initial begin
    #1;
    items[selector] = new(Factory::next(v));
    items[selector].value = v + 23;
    constructor_result = items[selector].get();
    property_result = items[selector].value;
    call_count = Factory::calls;
  end
endmodule
"#,
    )
    .expect("write dynamic class-array write source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_class_array_write_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic class-array writes");
    let mut simulator =
        Simulator::new(&design).expect("create dynamic class-array-write simulator");
    simulator
        .set_input("selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("v", &LogicWord::from_u64(32, 19))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run dynamic class-array-write initial block");
    assert_eq!(
        simulator.read("constructor_result").unwrap().to_u64(),
        Some(42)
    );
    assert_eq!(
        simulator.read("property_result").unwrap().to_u64(),
        Some(42)
    );

    let mut out_of_range =
        Simulator::new(&design).expect("create out-of-range dynamic class-array-write simulator");
    out_of_range
        .set_input("selector", &LogicWord::from_u64(32, 2))
        .unwrap();
    out_of_range
        .set_input("v", &LogicWord::from_u64(32, 19))
        .unwrap();
    out_of_range
        .run_until(1)
        .expect("run out-of-range dynamic class-array-write initial block");
    assert_eq!(
        out_of_range.read("constructor_result").unwrap().to_u64(),
        None
    );
    assert_eq!(out_of_range.read("property_result").unwrap().to_u64(), None);
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_class_array_handle_assignment() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-class-handle");
    let source_path = root.join("dynamic_class_handle.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction
endclass

module dynamic_class_handle_top(
  input int target_selector,
  input int source_selector,
  output int scalar_to_array,
  output int array_to_scalar,
  output int array_to_array
);
  Item destination[0:1];
  Item source[0:1];
  Item scalar;
  Item selected;

  initial begin
    #1;
    destination[0] = new(10);
    destination[1] = new(20);
    source[0] = new(30);
    source[1] = new(40);
    scalar = new(50);

    destination[target_selector] = scalar;
    scalar_to_array = destination[target_selector].get();

    selected = source[source_selector];
    source[source_selector].value = 61;
    array_to_scalar = selected.get();

    destination[target_selector] = source[source_selector];
    source[source_selector].value = 70;
    array_to_array = destination[target_selector].get();
  end
endmodule
"#,
    )
    .expect("write dynamic class-handle source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_class_handle_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic class-array handle assignment");
    let dynamic_handle_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("dynamic_class_handle_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize dynamic class-handle AST");
    let dynamic_handle_summary = slang
        .summarize_ast_json(&dynamic_handle_json)
        .expect("summarize dynamic class-handle AST");
    dynamic_handle_summary
        .validate_against_design(&design)
        .expect("exact dynamic class-array source fingerprints should match");

    let mut wrong_dynamic_target = dynamic_handle_summary.clone();
    if let Some(assignment) = wrong_dynamic_target
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "destination")
                    && !assignment.class_handle_dynamic_targets.is_empty()
                    && assignment.class_handle_dynamic_sources.is_empty()
                    && assignment.class_handle_source.is_some()
            })
        })
    {
        assignment.class_handle_source = Some("wrong_scalar".into());
    }
    let dynamic_target_error = wrong_dynamic_target
        .validate_against_design(&design)
        .expect_err("wrong dynamic class-array target source must fail");
    assert!(
        dynamic_target_error.contains("ClassHandleCopy:wrong_scalar")
            && dynamic_target_error.contains("ClassHandleCopy:scalar")
            && dynamic_target_error.contains("destination[0]")
            && dynamic_target_error.contains("ClassHandleCopy:source[0]"),
        "unexpected dynamic class-array target error: {dynamic_target_error}"
    );

    let mut wrong_array_to_array = dynamic_handle_summary.clone();
    if let Some(assignment) = wrong_array_to_array
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "destination")
                    && !assignment.class_handle_dynamic_targets.is_empty()
                    && !assignment.class_handle_dynamic_sources.is_empty()
            })
        })
    {
        assignment.class_handle_dynamic_sources[0] = "wrong_source".into();
    }
    let dynamic_source_error = wrong_array_to_array
        .validate_against_design(&design)
        .expect_err("wrong array-to-array dynamic source must fail");
    assert!(
        dynamic_source_error.contains("ClassHandleCopy:wrong_source")
            && dynamic_source_error.contains("ClassHandleCopy:source[0]")
            && dynamic_source_error.contains("ClassHandleCopy:source[1]"),
        "unexpected dynamic class-array source error: {dynamic_source_error}"
    );

    let mut wrong_selected = dynamic_handle_summary;
    if let Some(assignment) = wrong_selected
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "selected")
                    && !assignment.class_handle_dynamic_sources.is_empty()
            })
        })
    {
        assignment.class_handle_dynamic_sources[0] = "wrong_selected_source".into();
    }
    let selected_source_error = wrong_selected
        .validate_against_design(&design)
        .expect_err("wrong selected dynamic class-array source must fail");
    assert!(
        selected_source_error.contains("ClassHandleCopy:wrong_selected_source")
            && selected_source_error.contains("ClassHandleCopy:source[0]")
            && selected_source_error.contains("ClassHandleCopy:source[1]"),
        "unexpected selected dynamic class-array error: {selected_source_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create dynamic class-handle simulator");
    simulator
        .set_input("target_selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("source_selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run dynamic class-handle initial block");
    assert_eq!(
        simulator.read("scalar_to_array").unwrap().to_u64(),
        Some(50)
    );
    assert_eq!(
        simulator.read("array_to_scalar").unwrap().to_u64(),
        Some(61)
    );
    assert_eq!(simulator.read("array_to_array").unwrap().to_u64(), Some(70));
}

#[test]
fn slang_ast_direct_lowering_supports_null_class_handles() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-null-class-handles");
    let source_path = root.join("null_class_handles.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;

  function new(int v);
    value = v;
  endfunction
endclass

module null_class_handle_top(
  input int selector,
  output int default_null,
  output int is_null,
  output int not_null,
  output int same,
  output int different,
  output int after_null,
  output int detached,
  output int array_null
);
  Item a;
  Item b;
  Item c;
  Item items[0:1];

  initial begin
    #1;
    default_null = (c == null);
    a = new(10);
    b = a;
    is_null = (a == null);
    not_null = (a != null);
    same = (a == b);
    different = (a != b);

    a = null;
    after_null = (a == null);
    detached = (b == null);

    items[0] = new(20);
    items[1] = new(30);
    items[selector] = null;
    array_null = (items[selector] == null);
  end
endmodule
"#,
    )
    .expect("write null-handle source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("null_class_handle_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower null class handles");
    let mut simulator = Simulator::new(&design).expect("create null-handle simulator");
    simulator
        .set_input("selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run null-handle initial block");
    assert_eq!(simulator.read("default_null").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("is_null").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("not_null").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("same").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("different").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("after_null").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("detached").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("array_null").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_class_composition() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-composition");
    let source_path = root.join("class_composition.sv");
    std::fs::write(
        &source_path,
        r#"
class Inner;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction
endclass

class Outer;
  Inner inner;

  function new(int v);
    inner = new(v);
  endfunction

  function int get();
    return inner.get();
  endfunction

  function void set(int v);
    inner.value = v;
  endfunction
endclass

module class_composition_top(
  input int v,
  output int method_result,
  output int property_result,
  output int updated_result,
  output int method_set_result,
  output int source_after_write,
  output int named_handle_result,
  output int identity_result
);
  Outer outer;
  Inner source;
  Inner direct;
  Inner named_copy;

  initial begin
    #1;
    outer = new(v);
    source = outer.inner;
    direct = new(51);
    named_copy = direct;
    method_result = outer.get();
    property_result = outer.inner.value;
    outer.inner.value = v + 5;
    updated_result = outer.inner.get();
    outer.set(v + 6);
    method_set_result = outer.inner.get();
    source_after_write = source.get();
    named_handle_result = named_copy.get();
    identity_result = (outer.inner == source);
  end
endmodule
"#,
    )
    .expect("write class-composition source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("class_composition_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower class composition");
    let composition_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("class_composition_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize class-composition AST");
    let mut composition_summary = slang
        .summarize_ast_json(&composition_json)
        .expect("summarize class-composition AST");
    composition_summary
        .validate_against_design(&design)
        .expect("exact nested class-property fingerprint should match");
    let mut wrong_handle_source = composition_summary.clone();
    if let Some(assignment) = wrong_handle_source
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "source")
            })
        })
    {
        assignment.class_handle_source = Some("wrong_source".into());
    }
    let handle_source_error = wrong_handle_source
        .validate_against_design(&design)
        .expect_err("wrong whole class-handle source must fail");
    assert!(
        handle_source_error.contains("ClassHandleCopy:wrong_source")
            && handle_source_error.contains("ClassHandleCopy:outer.inner"),
        "unexpected whole class-handle source error: {handle_source_error}"
    );
    let mut wrong_named_handle_source = composition_summary.clone();
    if let Some(assignment) = wrong_named_handle_source
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "named_copy")
            })
        })
    {
        assignment.class_handle_source = Some("wrong_named_source".into());
    }
    let named_handle_error = wrong_named_handle_source
        .validate_against_design(&design)
        .expect_err("wrong named class-handle source must fail");
    assert!(
        named_handle_error.contains("ClassHandleCopy:wrong_named_source")
            && named_handle_error.contains("ClassHandleCopy:direct"),
        "unexpected named class-handle source error: {named_handle_error}"
    );
    let mut wrong_composition_method = composition_summary.clone();
    if let Some(assignment) = wrong_composition_method
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "method_result")
            })
        })
    {
        assignment.rhs.symbol = Some("outer.inner.wrong".into());
    }
    let composition_method_error = wrong_composition_method
        .validate_against_design(&design)
        .expect_err("wrong composed class-method property must fail");
    assert!(
        composition_method_error.contains("Ref:32:s[outer.inner.wrong]")
            && composition_method_error.contains("Ref:32:s[outer.inner.value]"),
        "unexpected composed class-method error: {composition_method_error}"
    );
    if let Some(assignment) = composition_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "property_result")
            })
        })
    {
        assignment.rhs.symbol = Some("outer.inner.wrong".into());
    }
    let composition_property_error = composition_summary
        .validate_against_design(&design)
        .expect_err("wrong nested class property must fail");
    assert!(
        composition_property_error.contains("Ref:32:s[outer.inner.wrong]")
            && composition_property_error.contains("Ref:32:s[outer.inner.value]"),
        "unexpected nested class-property error: {composition_property_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create class-composition simulator");
    simulator
        .set_input("v", &LogicWord::from_u64(32, 37))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run class-composition initial block");
    assert_eq!(simulator.read("method_result").unwrap().to_u64(), Some(37));
    assert_eq!(
        simulator.read("property_result").unwrap().to_u64(),
        Some(37)
    );
    assert_eq!(simulator.read("updated_result").unwrap().to_u64(), Some(42));
    assert_eq!(
        simulator.read("method_set_result").unwrap().to_u64(),
        Some(43)
    );
    assert_eq!(
        simulator.read("source_after_write").unwrap().to_u64(),
        Some(43)
    );
    assert_eq!(
        simulator.read("named_handle_result").unwrap().to_u64(),
        Some(51)
    );
    assert_eq!(simulator.read("identity_result").unwrap().to_u64(), Some(1));

    let recursive_path = root.join("recursive_class_composition.sv");
    std::fs::write(
        &recursive_path,
        r#"
class Recursive;
  Recursive child;
endclass

module recursive_composition_top;
  Recursive object;
endmodule
"#,
    )
    .expect("write recursive class-composition source");
    let recursive_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&recursive_path),
            Some("recursive_composition_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("recursive class composition must fail closed");
    assert!(
        recursive_error.contains("recursive class composition"),
        "unexpected recursive-composition error: {recursive_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_class_valued_array_properties() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-composition-array");
    let source_path = root.join("composition_array.sv");
    std::fs::write(
        &source_path,
        r#"
class Item;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction
endclass

class Group;
  Item items[0:2];

  function new(int a, int b, int c);
    items[0] = new(a);
    items[1] = new(b);
    items[2] = new(c);
  endfunction

  function int sum();
    return items[0].get() + items[1].get() + items[2].get();
  endfunction
endclass

module composition_array_top(
  input int a,
  input int b,
  input int c,
  input int selector,
  output int first,
  output int last,
  output int selected,
  output int group_sum,
  output int updated
);
  Group group;

  initial begin
    #1;
    group = new(a, b, c);
    first = group.items[0].get();
    last = group.items[2].get();
    selected = group.items[selector].get();
    group_sum = group.sum();
    group.items[selector].value = 77;
    updated = group.items[selector].get();
  end
endmodule
"#,
    )
    .expect("write class-valued array-property source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("composition_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower class-valued array properties");
    let mut simulator = Simulator::new(&design).expect("create composition-array simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 10))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(32, 20))
        .unwrap();
    simulator
        .set_input("c", &LogicWord::from_u64(32, 30))
        .unwrap();
    simulator
        .set_input("selector", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run composition-array initial block");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(30));
    assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(20));
    assert_eq!(simulator.read("group_sum").unwrap().to_u64(), Some(60));
    assert_eq!(simulator.read("updated").unwrap().to_u64(), Some(77));
}

#[test]
fn slang_ast_direct_lowering_rejects_unsupported_class_forms() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-invalid");
    let source_path = root.join("static_class.sv");
    std::fs::write(
        &source_path,
        r#"
interface class Interface;
  pure virtual function int get();
endclass

class StaticCounter implements Interface;
  virtual function int get();
    return 1;
  endfunction
endclass

module class_invalid_top;
  StaticCounter counter;
endmodule
"#,
    )
    .expect("write unsupported class source");
    let error = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("class_invalid_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("implemented class interfaces must fail closed");
    assert!(
        error.contains("class 'StaticCounter' interfaces are not supported"),
        "unexpected class-interface error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_class_inheritance() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-inheritance");
    let source_path = root.join("class_inheritance.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  int base_value;

  function new(int b);
    base_value = b;
  endfunction

  function int get();
    return base_value;
  endfunction

  virtual function int kind();
    return 1;
  endfunction

endclass

class Derived extends Base;
  int derived_value;

  function new(int b, int d);
    super.new(b);
    derived_value = d;
  endfunction

  virtual function int kind();
    return 2;
  endfunction

  function int sum();
    return base_value + derived_value;
  endfunction
endclass

module inheritance_top(
  input int b,
  input int d,
  output int base_get,
  output int derived_get,
  output int derived_sum,
  output int derived_kind,
  output int base_kind
);
  Base base;
  Derived derived;
  initial begin
    #1;
    base = new(b);
    derived = new(b, d);
    base_get = base.get();
    derived_get = derived.get();
    derived_sum = derived.sum();
    derived_kind = derived.kind();
    base_kind = base.kind();
  end
endmodule
"#,
    )
    .expect("write class-inheritance source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("inheritance_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower class inheritance");
    for field in [
        "base.base_value",
        "derived.base_value",
        "derived.derived_value",
    ] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == field && declaration.width == 32),
            "missing inherited class field {field}"
        );
    }

    let mut simulator = Simulator::new(&design).expect("create class-inheritance simulator");
    simulator
        .set_input("b", &LogicWord::from_u64(32, 7))
        .unwrap();
    simulator
        .set_input("d", &LogicWord::from_u64(32, 35))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run inheritance initial block");
    assert_eq!(simulator.read("base_get").unwrap().to_u64(), Some(7));
    assert_eq!(simulator.read("derived_get").unwrap().to_u64(), Some(7));
    assert_eq!(simulator.read("derived_sum").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("derived_kind").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("base_kind").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_class_handles() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-class");
    let source_path = root.join("dynamic_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  int value;

  function new(int v);
    value = v;
  endfunction

  function int get();
    return value;
  endfunction

  function void inc(int amount);
    value = value + amount;
  endfunction

  virtual function int kind();
    return 1;
  endfunction

  virtual function int total();
    return value;
  endfunction
endclass

class Derived extends Base;
  int extra;

  function new(int v, int e);
    super.new(v);
    extra = e;
  endfunction

  virtual function int kind();
    return 2;
  endfunction

  virtual function int total();
    return value + extra;
  endfunction
endclass

module dynamic_class_top(
  input int first_value_in,
  input int first_extra_in,
  input int second_value_in,
  input int second_extra_in,
  output int base_kind,
  output int derived_kind,
  output int dynamic_kind,
  output int dynamic_get,
  output int dynamic_total,
  output int derived_write_visible,
  output int base_write_visible,
  output int method_write_visible,
  output int transitive_write_visible,
  output int second_total,
  output int first_still_total
);
  Base base;
  Base mirror;
  Derived first;
  Derived second;

  initial begin
    #1;
    base = new(first_value_in);
    first = new(first_value_in, first_extra_in);
    second = new(second_value_in, second_extra_in);

    base_kind = base.kind();
    derived_kind = first.kind();

    base = first;
    mirror = base;
    dynamic_kind = base.kind();
    dynamic_get = base.get();
    dynamic_total = base.total();

    first.value = first_value_in + 10;
    derived_write_visible = base.get();

    base.value = first_value_in + 20;
    base_write_visible = first.get();

    first.inc(5);
    method_write_visible = base.get();
    transitive_write_visible = mirror.get();

    first_still_total = first.total();
    base = second;
    second_total = base.total();
  end
endmodule
"#,
    )
    .expect("write dynamic-class source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("dynamic_class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic class handles");
    let mut simulator = Simulator::new(&design).expect("create dynamic-class simulator");
    simulator
        .set_input("first_value_in", &LogicWord::from_u64(32, 7))
        .unwrap();
    simulator
        .set_input("first_extra_in", &LogicWord::from_u64(32, 35))
        .unwrap();
    simulator
        .set_input("second_value_in", &LogicWord::from_u64(32, 11))
        .unwrap();
    simulator
        .set_input("second_extra_in", &LogicWord::from_u64(32, 22))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run dynamic-class initial block");
    assert_eq!(simulator.read("base_kind").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("derived_kind").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("dynamic_kind").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("dynamic_get").unwrap().to_u64(), Some(7));
    assert_eq!(simulator.read("dynamic_total").unwrap().to_u64(), Some(42));
    assert_eq!(
        simulator.read("derived_write_visible").unwrap().to_u64(),
        Some(17)
    );
    assert_eq!(
        simulator.read("base_write_visible").unwrap().to_u64(),
        Some(27)
    );
    assert_eq!(
        simulator.read("method_write_visible").unwrap().to_u64(),
        Some(32)
    );
    assert_eq!(
        simulator.read("transitive_write_visible").unwrap().to_u64(),
        Some(32)
    );
    assert_eq!(
        simulator.read("first_still_total").unwrap().to_u64(),
        Some(67)
    );
    assert_eq!(simulator.read("second_total").unwrap().to_u64(), Some(33));
}

#[test]
fn slang_ast_direct_lowering_supports_static_class_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-static-class");
    let source_path = root.join("static_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Registry;
  static int shared = 3;

  static function void set(int value);
    shared = value;
  endfunction

  static function int get();
    return shared;
  endfunction
endclass

class Item;
  int id;
  static int count = 0;

  function new(int initial_id);
    id = initial_id;
    count = count + 1;
  endfunction

  function int identity();
    return id;
  endfunction
endclass

module static_class_top(
  input int replacement,
  output int initial_shared,
  output int method_shared,
  output int direct_shared,
  output int item_count,
  output int first_id,
  output int second_id
);
  Item first;
  Item second;

  initial begin
    #1;
    initial_shared = Registry::get();

    first = new(10);
    second = new(20);
    item_count = Item::count;
    first_id = first.identity();
    second_id = second.identity();

    Registry::set(replacement);
    method_shared = Registry::get();
    Registry::shared = replacement + 5;
    direct_shared = Registry::shared;
  end
endmodule
"#,
    )
    .expect("write static-class source");

    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("static_class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower static class members");
    let static_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("static_class_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize authoritative static-class AST");
    let mut static_summary = slang
        .summarize_ast_json(&static_json)
        .expect("summarize authoritative static-class AST");
    static_summary
        .validate_against_design(&design)
        .expect("exact class-static property fingerprint should match");
    let mut wrong_instance_method = static_summary.clone();
    if let Some(assignment) = wrong_instance_method
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "first_id")
            })
        })
    {
        assignment.rhs.symbol = Some("first.wrong".into());
    }
    let instance_method_error = wrong_instance_method
        .validate_against_design(&design)
        .expect_err("wrong simple instance-method receiver/property must fail");
    assert!(
        instance_method_error.contains("Ref:32:s[first.wrong]")
            && instance_method_error.contains("Ref:32:s[first.id]"),
        "unexpected simple instance-method error: {instance_method_error}"
    );
    if let Some(assignment) = static_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "initial_shared")
            })
        })
    {
        assignment.rhs.symbol = Some("Registry_wrong".into());
    }
    let error = static_summary
        .validate_against_design(&design)
        .expect_err("wrong inlined static function return must fail");
    assert!(
        error.contains("Ref:32:s[Registry_wrong]"),
        "unexpected structural error: {error}"
    );
    if let Some(assignment) = static_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "initial_shared")
            })
        })
    {
        assignment.rhs.symbol = Some("Registry_shared".into());
    }
    if let Some(assignment) = static_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "direct_shared")
            })
        })
    {
        assignment.rhs.symbol = Some("Registry_wrong".into());
    }
    let error = static_summary
        .validate_against_design(&design)
        .expect_err("wrong class-static property must fail structural validation");
    assert!(
        error.contains("Ref:32:s[Registry_wrong]"),
        "unexpected structural error: {error}"
    );
    let mut simulator = Simulator::new(&design).expect("create static-class simulator");
    simulator
        .set_input("replacement", &LogicWord::from_u64(32, 37))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run static-class initial block");

    assert_eq!(simulator.read("initial_shared").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("method_shared").unwrap().to_u64(), Some(37));
    assert_eq!(simulator.read("direct_shared").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("item_count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("first_id").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("second_id").unwrap().to_u64(), Some(20));
}

#[test]
fn slang_ast_direct_lowering_supports_parameterized_classes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-parameterized-class");
    let source_path = root.join("parameterized_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Box #(parameter int WIDTH = 8, parameter int OFFSET = 3);
  logic [WIDTH-1:0] value;

  function new(logic [WIDTH-1:0] v);
    value = v;
  endfunction

  function logic [WIDTH-1:0] get();
    return value;
  endfunction

  virtual function int kind();
    return WIDTH + OFFSET;
  endfunction
endclass

class DerivedBox #(parameter int WIDTH = 8) extends Box #(WIDTH);
  logic [WIDTH-1:0] extra;

  function new(logic [WIDTH-1:0] v, logic [WIDTH-1:0] e);
    super.new(v);
    extra = e;
  endfunction

  function logic [WIDTH:0] sum();
    return value + extra;
  endfunction

  virtual function int kind();
    return WIDTH + OFFSET + 1;
  endfunction
endclass

module parameterized_class_top(
  input logic [7:0] value8,
  input logic [15:0] value16,
  input logic [7:0] extra8,
  output logic [7:0] observed8,
  output logic [15:0] observed16,
  output logic [8:0] observed_sum,
  output int direct_kind8,
  output int direct_kind16,
  output int dynamic_kind
);
  Box #(.WIDTH(8)) box8;
  Box #(.WIDTH(16)) box16;
  DerivedBox #(.WIDTH(8)) derived_box;
  Box #(.WIDTH(8)) base_box;

  initial begin
    #1;
    box8 = new(value8);
    box16 = new(value16);
    derived_box = new(value8, extra8);
    observed8 = box8.get();
    observed16 = box16.get();
    observed_sum = derived_box.sum();
    direct_kind8 = box8.kind();
    direct_kind16 = box16.kind();
    base_box = derived_box;
    dynamic_kind = base_box.kind();
  end
endmodule
"#,
    )
    .expect("write parameterized-class source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("parameterized_class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower parameterized classes");
    let parameterized_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("parameterized_class_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize parameterized-class AST");
    let parameterized_summary = slang
        .summarize_ast_json(&parameterized_json)
        .expect("summarize parameterized-class AST");
    parameterized_summary
        .validate_against_design(&design)
        .expect("exact parameterized class-property method fingerprints should match");
    for (target, wrong_symbol) in [("observed8", "box8.wrong"), ("observed16", "box16.wrong")] {
        let mut wrong_summary = parameterized_summary.clone();
        if let Some(assignment) = wrong_summary
            .procedural_blocks
            .iter_mut()
            .find_map(|block| {
                block.assignments.iter_mut().find(|assignment| {
                    assignment
                        .target_names
                        .first()
                        .is_some_and(|name| name == target)
                })
            })
        {
            assignment.rhs.symbol = Some(wrong_symbol.into());
        }
        let error = wrong_summary
            .validate_against_design(&design)
            .expect_err("wrong parameterized class-property method must fail");
        let expected_symbol = wrong_symbol.replace(".wrong", ".value");
        assert!(
            error.contains(&format!("Ref:8:u[{wrong_symbol}]"))
                || error.contains(&format!("Ref:16:u[{wrong_symbol}]")),
            "unexpected wrong parameterized symbol: {error}"
        );
        assert!(
            error.contains(&format!("Ref:8:u[{expected_symbol}]"))
                || error.contains(&format!("Ref:16:u[{expected_symbol}]")),
            "unexpected parameterized symbol: {error}"
        );
    }
    for (target, wrong_value, expected_value) in [
        ("direct_kind8", 12_u64, 8_u64),
        ("direct_kind16", 13_u64, 16_u64),
    ] {
        let mut wrong_kind = parameterized_summary.clone();
        if let Some(assignment) = wrong_kind.procedural_blocks.iter_mut().find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == target)
            })
        }) {
            assignment.rhs.operands[0].literal = Some(format!("32:{wrong_value:032b}"));
        }
        let error = wrong_kind
            .validate_against_design(&design)
            .expect_err("wrong parameterized constant method must fail");
        assert!(
            error.contains(&format!("Const:32:u[32:{wrong_value:032b}]"))
                && error.contains(&format!("Const:32:u[32:{expected_value:032b}]"))
                && error.contains("Add:32:s["),
            "unexpected parameterized constant-method error: {error}"
        );
    }
    let mut wrong_dynamic_dispatch = parameterized_summary.clone();
    if let Some(assignment) = wrong_dynamic_dispatch
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "dynamic_kind")
            })
        })
    {
        assignment.rhs.operands[1].operands[1].literal = Some(format!("32:{:032b}", 2_u64));
    }
    let dynamic_dispatch_error = wrong_dynamic_dispatch
        .validate_against_design(&design)
        .expect_err("wrong runtime virtual-dispatch branch must fail");
    assert!(
        dynamic_dispatch_error.contains("Cond:32:s[")
            && dynamic_dispatch_error.contains("Ref:32:u[__class_tag_base_box]")
            && dynamic_dispatch_error.contains(&format!("Const:32:u[32:{:032b}]", 1))
            && dynamic_dispatch_error.contains(&format!("Const:32:u[32:{:032b}]", 2)),
        "unexpected runtime virtual-dispatch error: {dynamic_dispatch_error}"
    );
    let mut wrong_binary_summary = parameterized_summary.clone();
    if let Some(assignment) = wrong_binary_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "observed_sum")
            })
        })
    {
        assignment.rhs.operands[0].operands[0].symbol = Some("derived_box.wrong".into());
    }
    let binary_error = wrong_binary_summary
        .validate_against_design(&design)
        .expect_err("wrong parameterized binary-property method must fail");
    assert!(
        binary_error.contains("Ref:8:u[derived_box.wrong]")
            && binary_error.contains("Ref:8:u[derived_box.value]")
            && binary_error.contains("Ref:8:u[derived_box.extra]")
            && binary_error.contains("Add:9:u["),
        "unexpected parameterized binary-property error: {binary_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create parameterized-class simulator");
    simulator
        .set_input("value8", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    simulator
        .set_input("value16", &LogicWord::from_u64(16, 0x1234))
        .unwrap();
    simulator
        .set_input("extra8", &LogicWord::from_u64(8, 0x5b))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run parameterized-class initial block");
    assert_eq!(simulator.read("observed8").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("observed16").unwrap().to_u64(), Some(0x1234));
    assert_eq!(
        simulator.read("observed_sum").unwrap().to_u64(),
        Some(0x100)
    );
    assert_eq!(simulator.read("direct_kind8").unwrap().to_u64(), Some(11));
    assert_eq!(simulator.read("direct_kind16").unwrap().to_u64(), Some(19));
    assert_eq!(simulator.read("dynamic_kind").unwrap().to_u64(), Some(12));
}

#[test]
fn slang_ast_direct_lowering_shares_hierarchy_global_static_class_storage() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-child-static-class");
    let source_path = root.join("child_static_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Registry;
  static int shared = 1;

  static function void set(int value);
    shared = value;
  endfunction

  static function int get();
    return shared;
  endfunction
endclass

module static_class_first_writer;
  initial begin
    #1;
    Registry::set(20);
  end
endmodule

module static_class_second_writer;
  initial begin
    #2;
    Registry::set(41);
  end
endmodule

module static_class_leaf_writer;
  initial begin
    #3;
    Registry::set(55);
  end
endmodule

module static_class_branch_writer;
  static_class_leaf_writer leaf();
endmodule

module static_class_hierarchy_top(
  output int initial_result,
  output int final_result
);
  static_class_first_writer first_writer();
  static_class_second_writer second_writer();
  static_class_branch_writer branch_writer();

  initial begin
    initial_result = Registry::get();
    #4;
    final_result = Registry::get();
  end
endmodule
"#,
    )
    .expect("write child static-class source");
    let design = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("static_class_hierarchy_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower hierarchy-global class statics");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("static_class_hierarchy_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize hierarchy-global class-static AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize hierarchy-global class-static AST");
    summary
        .validate_against_design(&design)
        .expect("exact hierarchy-global class-static fingerprint should match");
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .target_names
                .first()
                .is_some_and(|name| name == "initial_result")
        })
    }) {
        assignment.rhs.symbol = Some("Registry_wrong_static".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong hierarchy-global class-static source must fail");
    assert!(
        error.contains("Ref:32:s[Registry_wrong_static]")
            && error.contains("Ref:32:s[Registry_shared]"),
        "unexpected class-static structural error: {error}"
    );
    assert_eq!(
        design
            .net_decls
            .iter()
            .filter(|declaration| {
                declaration.name.starts_with("__class_static_")
                    && declaration.name.ends_with("_shared")
            })
            .count(),
        1,
        "class static storage must have one hierarchy-global allocation"
    );
    let mut simulator = Simulator::new(&design).expect("simulate hierarchy-global class statics");
    simulator
        .run_until(4)
        .expect("run parent and child static-class initials");
    assert!(
        simulator
            .read("initial_result")
            .unwrap()
            .to_u64()
            .is_some_and(|value| value == 1),
        "class static initializer was not visible before the child write"
    );
    assert_eq!(
        simulator.read("final_result").unwrap().to_u64(),
        Some(55),
        "nested child static write was not visible in the parent scope"
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_null_class_access() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-null-class");
    let source_path = root.join("null_class.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  function int kind();
    return 1;
  endfunction
endclass

module null_class_top(output int result);
  Base base;
  initial begin
    result = base.kind();
  end
endmodule
"#,
    )
    .expect("write null-class source");
    let error = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("null_class_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("null class access must fail closed");
    assert!(
        error.contains("may be null; null object access is not supported"),
        "unexpected null-class error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_null_class_handle_propagation() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-null-class-propagation");
    let source_path = root.join("null_class_propagation.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  function int kind();
    return 1;
  endfunction
endclass

module null_class_propagation_top(output int result);
  Base source;
  Base target;
  initial begin
    #1;
    target = source;
    result = target.kind();
  end
endmodule
"#,
    )
    .expect("write null-class propagation source");
    let error = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("null_class_propagation_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("null class handle propagation must fail closed");
    assert!(
        error.contains("null handle propagation is not supported"),
        "unexpected null-propagation error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_class_property_shadowing() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-class-shadowing");
    let source_path = root.join("class_shadowing.sv");
    std::fs::write(
        &source_path,
        r#"
class Base;
  int value;
endclass

class Derived extends Base;
  int value;
endclass

module class_shadowing_top;
  Derived object;
endmodule
"#,
    )
    .expect("write class-shadowing source");
    let error = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("class_shadowing_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("class property shadowing must fail closed");
    assert!(
        error.contains("class property shadowing"),
        "unexpected shadowing error: {error}"
    );
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
        .compile_files_bounded(
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
        "module unsupported(output logic y); initial $fopen(\"unsupported\"); endmodule\n",
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
fn slang_ast_direct_lowering_supports_display_tasks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-display");
    let source_path = root.join("display.sv");
    std::fs::write(
        &source_path,
        r#"
module displays(
  input logic [3:0] a,
  input logic [3:0] b
);
  initial begin
    #1;
    $display("a=%b b=%h text%%", a, b);
    $displayb(a);
    $displayh(a);
    $display("decimal=%d", a);
    $display(a, b);
    $write("plain");
  end
endmodule
"#,
    )
    .expect("write display source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("displays"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower display tasks");
    let mut simulator = Simulator::new(&design).expect("create display simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1011))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0b0010))
        .unwrap();
    simulator.run_until(1).expect("run display initial block");
    assert_eq!(
        simulator.take_displays(),
        vec![
            "a=1011 b=2 text%\n",
            "1011\n",
            "b\n",
            "decimal=11\n",
            "11 2\n",
            "plain",
        ]
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_invalid_display_formats() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-display-invalid");
    let source_path = root.join("display_invalid.sv");
    std::fs::write(
        &source_path,
        r#"
module display_invalid(input logic [3:0] a);
  initial $display("missing=%b", a, a);
endmodule
"#,
    )
    .expect("write invalid display source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("display_invalid"),
            &PreprocessOptions::default(),
        )
        .expect_err("display argument mismatches must fail closed");
    assert!(
        error.contains("display has 1 format specifiers but 2 arguments"),
        "unexpected display error: {error}"
    );

    let unsupported_path = root.join("display_unsupported.sv");
    std::fs::write(
        &unsupported_path,
        r#"
module display_unsupported(input logic [3:0] a);
  initial $display("octal=%o", a);
endmodule
"#,
    )
    .expect("write unsupported display source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&unsupported_path),
            Some("display_unsupported"),
            &PreprocessOptions::default(),
        )
        .expect_err("unsupported display specifiers must fail closed");
    assert!(
        error.contains("does not support display specifier '%o'"),
        "unexpected display specifier error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_monitor_tasks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-monitor");
    let source_path = root.join("monitor.sv");
    std::fs::write(
        &source_path,
        r#"
module monitor_tb;
  reg a;
  initial a = 0;
  initial #10 a = 1;
  initial $monitor("a=%b", a);
endmodule
"#,
    )
    .expect("write monitor source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("monitor_tb"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower monitor task");
    let mut simulator = Simulator::new(&design).expect("create monitor simulator");
    simulator.run_until(20).expect("run monitor timeline");
    let displays = simulator.take_displays();
    assert!(displays.iter().any(|line| line == "a=0"));
    assert!(displays.iter().any(|line| line == "a=1"));
}

#[test]
fn slang_ast_direct_lowering_supports_simulation_time_functions() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-simulation-time");
    let source_path = root.join("simulation_time.sv");
    std::fs::write(
        &source_path,
        r#"
module simulation_time(
  output logic [63:0] now_time,
  output logic [31:0] now_stime,
  output logic [63:0] added
);
  initial begin
    #7;
    now_time = $time;
    now_stime = $stime;
    added = $time + 64'd3;
  end
endmodule
"#,
    )
    .expect("write simulation-time source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("simulation_time"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simulation time functions");
    let mut simulator = Simulator::new(&design).expect("create simulation-time simulator");
    simulator.run_until(7).expect("run to time seven");
    assert_eq!(simulator.read("now_time").unwrap().to_u64(), Some(7));
    assert_eq!(simulator.read("now_stime").unwrap().to_u64(), Some(7));
    assert_eq!(simulator.read("added").unwrap().to_u64(), Some(10));
}

#[test]
fn slang_ast_direct_lowering_supports_timeformat_tasks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-timeformat");
    let source_path = root.join("timeformat.sv");
    std::fs::write(
        &source_path,
        r#"
module timeformat(
  input logic [3:0] a
);
  initial begin
    #7;
    $timeformat(-9, 3, " ns", 10);
    $display("t=%t a=%h", $time, a);
    $timeformat(-6, 3, " us", 0);
    $display("t=%t a=%h", $time, a);
  end
endmodule
"#,
    )
    .expect("write timeformat source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("timeformat"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower timeformat task");
    let mut simulator = Simulator::new(&design).expect("create timeformat simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0xb))
        .unwrap();
    simulator.run_until(7).expect("run timeformat timeline");
    assert_eq!(
        simulator.take_displays(),
        vec!["t=  7.000 ns a=b\n", "t=0.007 us a=b\n"]
    );

    let invalid_path = root.join("timeformat_invalid.sv");
    std::fs::write(
        &invalid_path,
        r#"
module timeformat_invalid;
  initial $timeformat(-9, 10, " ns", 0);
endmodule
"#,
    )
    .expect("write invalid timeformat source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&invalid_path),
            Some("timeformat_invalid"),
            &PreprocessOptions::default(),
        )
        .expect_err("unsupported timeformat precision must fail closed");
    assert!(
        error.contains("$timeformat precision 10 is outside 0..=9"),
        "unexpected timeformat error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_module_timescales() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-module-timescales");
    let top_path = root.join("timescale_top.sv");
    let child_path = root.join("timescale_child.sv");
    std::fs::write(
        &top_path,
        r#"
`timescale 100ns/1ps
module timescale_top(
  input logic a,
  output logic y,
  output logic [63:0] top_time,
  output logic [63:0] child_time
);
  time_probe child(.child_time(child_time));
  assign #2 y = ~a;
  initial #7 top_time = $time;
endmodule
"#,
    )
    .expect("write top timescale source");
    std::fs::write(
        &child_path,
        r#"
`timescale 1ps/1ps
module time_probe(
  output logic [63:0] child_time
);
  initial #3 child_time = $time;
endmodule
"#,
    )
    .expect("write child timescale source");
    let files = [top_path, child_path];
    let design = slang
        .compile_files_via_ast(&files, Some("timescale_top"), &PreprocessOptions::default())
        .expect("directly lower module timescales");
    assert_eq!(design.time_scale.precision_exponent, -12);
    let timescale_json = slang
        .ast_json_with_options(
            &files,
            Some("timescale_top"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize module-timescale AST");
    let mut timescale_summary = slang
        .summarize_ast_json(&timescale_json)
        .expect("summarize module-timescale AST");
    timescale_summary
        .validate_against_design(&design)
        .expect("exact module simulation-time fingerprint should match");
    if let Some(assignment) = timescale_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "top_time")
            })
        })
    {
        assignment.rhs.simulation_time_unit = Some(-6);
    }
    let timescale_error = timescale_summary
        .validate_against_design(&design)
        .expect_err("wrong module simulation-time unit must fail");
    assert!(
        timescale_error.contains("SimulationTime:64:s[Unit-7]")
            && timescale_error.contains("SimulationTime:64:s[Unit-6]"),
        "unexpected simulation-time error: {timescale_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create timescale simulator");
    simulator.enable_vcd();
    simulator
        .set_input("a", &LogicWord::from_u64(1, 0))
        .unwrap();
    simulator.run_until(199_999).expect("run before top delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), None);
    simulator.run_until(200_000).expect("run after top delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("child_time").unwrap().to_u64(), Some(3));
    simulator
        .run_until(700_000)
        .expect("run after top time capture");
    assert_eq!(simulator.read("top_time").unwrap().to_u64(), Some(7));
    assert!(simulator
        .vcd()
        .expect("timescale VCD output")
        .contains("$timescale 1ps $end"));
}

#[test]
fn slang_ast_direct_lowering_supports_timeunit_declarations() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-timeunit");
    let source_path = root.join("timeunit.sv");
    std::fs::write(
        &source_path,
        r#"
module timeunit_probe(
  output logic [63:0] elapsed
);
  timeunit 100ns;
  timeprecision 1ps;
  initial begin
    #2;
    elapsed = $time;
    $printtimescale;
  end
endmodule
"#,
    )
    .expect("write timeunit source");
    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("timeunit_probe"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower timeunit declarations");
    assert_eq!(design.time_scale.precision_exponent, -12);

    let mut simulator = Simulator::new(&design).expect("create timeunit simulator");
    simulator.run_until(200_000).expect("run timeunit timeline");
    assert_eq!(simulator.read("elapsed").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator.take_displays(),
        vec!["time unit = 100ns, time precision = 1ps\n"]
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_realtime_until_real_types_exist() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-realtime");
    let source_path = root.join("realtime.sv");
    std::fs::write(
        &source_path,
        r#"
module realtime_probe(output logic [63:0] now_realtime);
  initial now_realtime = $realtime;
endmodule
"#,
    )
    .expect("write realtime source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("realtime_probe"),
            &PreprocessOptions::default(),
        )
        .expect_err("realtime requires a real type model and must fail closed");
    assert!(
        error.contains("unknown function '$realtime'") || error.contains("cannot resolve"),
        "unexpected realtime error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_package_imports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-import");
    let source_path = root.join("package_import.sv");
    std::fs::write(
        &source_path,
        r#"
package math_pkg;
  parameter int WIDTH = 8;
  localparam int OFFSET = 42;

  function automatic int double(input int value);
    double = 2 * value;
  endfunction
endpackage

module package_import
  import math_pkg::*;
#(
  parameter int EXTRA = 1
)(
  input logic [WIDTH-1:0] a,
  output logic [WIDTH-1:0] y,
  output int doubled_offset
);
  assign y = a + OFFSET;
  assign doubled_offset = double(OFFSET) + EXTRA;
endmodule
"#,
    )
    .expect("write package-import source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_import"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wildcard package import");
    assert_eq!(design.assigns.len(), 2);
    let mut simulator = Simulator::new(&design).expect("create package-import simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 3))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(45));
    assert_eq!(simulator.read("doubled_offset").unwrap().to_u64(), Some(85));

    let package_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("package_import"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize package-import AST");
    let package_summary = slang
        .summarize_ast_json(&package_json)
        .expect("summarize package-import AST");
    package_summary
        .validate_against_design(&design)
        .expect("exact inlined package-function fingerprints should match");

    let mut wrong_function_body = package_summary.clone();
    if let Some(target) = wrong_function_body
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "doubled_offset")
    {
        target.rhs.operands[0].operands[0].operands[0].literal = Some(format!("32:{:032b}", 3));
    }
    let function_body_error = wrong_function_body
        .validate_against_design(&design)
        .expect_err("wrong inlined package-function body must fail");
    assert!(
        function_body_error.contains("Mul:32:s[")
            && function_body_error.contains(&format!("Const:32:u[32:{:032b}]", 2))
            && function_body_error.contains(&format!("Const:32:u[32:{:032b}]", 3)),
        "unexpected inlined package-function body error: {function_body_error}"
    );

    let mut wrong_function_argument = package_summary;
    if let Some(target) = wrong_function_argument
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "doubled_offset")
    {
        target.rhs.operands[0].operands[0].operands[1].operands[0].symbol =
            Some("wrong_OFFSET".into());
    }
    let function_argument_error = wrong_function_argument
        .validate_against_design(&design)
        .expect_err("wrong inlined package-function argument must fail");
    assert!(
        function_argument_error.contains("Ref:32:s[wrong_OFFSET]")
            && function_argument_error.contains("Const:32:u[")
            && function_argument_error.contains("Mul:32:s["),
        "unexpected inlined package-function argument error: {function_argument_error}"
    );

    let explicit_path = root.join("explicit_import.sv");
    std::fs::write(
        &explicit_path,
        r#"
package scoped_pkg;
  parameter int WIDTH = 8;
  localparam int OFFSET = 21;

  function automatic int double(input int value);
    double = 2 * value;
  endfunction
endpackage

module explicit_import
  import scoped_pkg::WIDTH,
          scoped_pkg::OFFSET,
          scoped_pkg::double;
(
  input logic [WIDTH-1:0] a,
  output logic [WIDTH-1:0] imported,
  output logic [WIDTH-1:0] scoped,
  output int imported_function,
  output int scoped_function
);
  assign imported = a + OFFSET;
  assign scoped = a + scoped_pkg::OFFSET;
  assign imported_function = double(OFFSET);
  assign scoped_function = scoped_pkg::double(OFFSET);
endmodule
"#,
    )
    .expect("write explicit-import source");
    let explicit = slang
        .compile_files_via_ast(
            std::slice::from_ref(&explicit_path),
            Some("explicit_import"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower explicit package imports");
    assert_eq!(explicit.assigns.len(), 4);
    let mut explicit_simulator =
        Simulator::new(&explicit).expect("create explicit-import simulator");
    explicit_simulator
        .set_input("a", &LogicWord::from_u64(8, 3))
        .unwrap();
    assert_eq!(
        explicit_simulator.read("imported").unwrap().to_u64(),
        Some(24)
    );
    assert_eq!(
        explicit_simulator.read("scoped").unwrap().to_u64(),
        Some(24)
    );
    assert_eq!(
        explicit_simulator
            .read("imported_function")
            .unwrap()
            .to_u64(),
        Some(42)
    );
    assert_eq!(
        explicit_simulator.read("scoped_function").unwrap().to_u64(),
        Some(42)
    );

    let collision_path = root.join("package_collision.sv");
    std::fs::write(
        &collision_path,
        r#"
package left_pkg;
  parameter int SAME = 1;
endpackage

package right_pkg;
  parameter int SAME = 2;
endpackage

module package_collision
  import left_pkg::*,
          right_pkg::*;
(
  output int y
);
  assign y = 1;
endmodule
"#,
    )
    .expect("write package-collision source");
    let collision_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&collision_path),
            Some("package_collision"),
            &PreprocessOptions::default(),
        )
        .expect_err("ambiguous imported package names must fail closed");
    assert!(
        collision_error.contains("imported package symbol collision for 'SAME'"),
        "unexpected collision error: {collision_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_package_classes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-classes");
    let source_path = root.join("package_classes.sv");
    std::fs::write(
        &source_path,
        r#"
package class_pkg;
  class Item;
    int value;

    function new(int v);
      value = v;
    endfunction

    function int get();
      return value;
    endfunction
  endclass

  class Registry extends Item;
    int extra;

    function new(int v, int e);
      super.new(v);
      extra = e;
    endfunction

    function int total();
      return value + extra;
    endfunction
  endclass
endpackage

module package_class_top(
  input int v,
  input int e,
  output int item_value,
  output int registry_total,
  output int registry_extra
);
  import class_pkg::Item,
          class_pkg::Registry;

  Item item;
  Registry registry;

  initial begin
    #1;
    item = new(v);
    registry = new(v, e);
    item_value = item.get();
    registry_total = registry.total();
    registry_extra = registry.extra;
  end
endmodule
"#,
    )
    .expect("write package-class source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package classes");
    let mut simulator = Simulator::new(&design).expect("create package-class simulator");
    simulator
        .set_input("v", &LogicWord::from_u64(32, 19))
        .unwrap();
    simulator
        .set_input("e", &LogicWord::from_u64(32, 23))
        .unwrap();
    simulator
        .run_until(1)
        .expect("run package-class initial block");
    assert_eq!(simulator.read("item_value").unwrap().to_u64(), Some(19));
    assert_eq!(simulator.read("registry_total").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("registry_extra").unwrap().to_u64(), Some(23));

    let scoped_path = root.join("scoped_package_class.sv");
    std::fs::write(
        &scoped_path,
        r#"
package scoped_class_pkg;
  class Value;
    int value;

    function new(int v);
      value = v;
    endfunction

    function int get();
      return value;
    endfunction
  endclass
endpackage

module scoped_package_class_top(
  input int v,
  output int result
);
  scoped_class_pkg::Value object;

  initial begin
    #1;
    object = new(v);
    result = object.get();
  end
endmodule
"#,
    )
    .expect("write scoped package-class source");
    let scoped = slang
        .compile_files_via_ast(
            std::slice::from_ref(&scoped_path),
            Some("scoped_package_class_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package-scoped class references");
    let mut scoped_simulator =
        Simulator::new(&scoped).expect("create scoped package-class simulator");
    scoped_simulator
        .set_input("v", &LogicWord::from_u64(32, 55))
        .unwrap();
    scoped_simulator
        .run_until(1)
        .expect("run scoped package-class initial block");
    assert_eq!(scoped_simulator.read("result").unwrap().to_u64(), Some(55));

    let collision_path = root.join("package_class_collision.sv");
    std::fs::write(
        &collision_path,
        r#"
package left_class_pkg;
  class Same;
    int value;

    function new(int v);
      value = v;
    endfunction

    function int get();
      return value;
    endfunction
  endclass
endpackage

package right_class_pkg;
  class Same;
    int value;
  endclass
endpackage

module package_class_collision_top(
  input int v,
  output int result
);
  left_class_pkg::Same object;

  initial begin
    #1;
    object = new(v);
    result = object.get();
  end
endmodule
"#,
    )
    .expect("write duplicate package-class-name source");
    let duplicate_name = slang
        .compile_files_via_ast(
            std::slice::from_ref(&collision_path),
            Some("package_class_collision_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package-qualified duplicate class names");
    let mut duplicate_name_simulator = Simulator::new(&duplicate_name)
        .expect("create package-qualified duplicate-class simulator");
    duplicate_name_simulator
        .set_input("v", &LogicWord::from_u64(32, 77))
        .unwrap();
    duplicate_name_simulator
        .run_until(1)
        .expect("run package-qualified duplicate-class initial block");
    assert!(
        duplicate_name_simulator
            .read("result")
            .unwrap()
            .to_u64()
            .is_some_and(|value| value == 77),
        "package-qualified duplicate class selected the wrong definition"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_simple_type_aliases_and_string_parameters() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-simple-type-aliases");
    let source_path = root.join("simple_type_aliases.sv");
    std::fs::write(
        &source_path,
        r#"
package types_pkg;
  typedef logic [15:0] wide_t;
  typedef int count_t;
  parameter string MESSAGE = "UVM";
endpackage

module type_alias_top
  import types_pkg::*;
(
  output wide_t wide,
  output count_t count,
  output string message
);
  assign wide = 16'h1234;
  assign count = 42;
  assign message = MESSAGE;
endmodule
"#,
    )
    .expect("write simple-type-alias source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("type_alias_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower simple type aliases");
    let mut simulator = Simulator::new(&design).expect("create type-alias simulator");
    simulator.run_until(0).expect("run type-alias assignments");
    assert_eq!(simulator.read("wide").unwrap().to_u64(), Some(0x1234));
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(42));
    assert_eq!(
        simulator
            .read("message")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "UVM"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_package_typedefs_and_enums() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-types");
    let source_path = root.join("package_types.sv");
    std::fs::write(
        &source_path,
        r#"
package types_pkg;
  typedef enum logic [1:0] {
    A = 2'd1,
    B = 2'd2
  } state_t;

  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  localparam state_t INIT = A;
endpackage

module package_types
  import types_pkg::*;
(
  input logic [1:0] selector,
  input logic [7:0] data,
  output state_t state,
  output pair_t pair,
  output state_t next_state,
  output state_t previous_state,
  output state_t first_state,
  output state_t last_state,
  output int enum_count,
  output string state_name,
  output logic initialized,
  output logic selected_a
);
  assign state = selector[0] ? B : A;
  assign pair = data;
  assign next_state = state.next();
  assign previous_state = state.prev();
  assign first_state = state.first();
  assign last_state = state.last();
  assign enum_count = state.num();
  assign state_name = state.name();
  assign initialized = (INIT == A);
  assign selected_a = (state == A);
endmodule
"#,
    )
    .expect("write package-typedef source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_types"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package typedefs and enums");
    assert_eq!(design.assigns.len(), 10);
    let mut simulator = Simulator::new(&design).expect("create package-type simulator");
    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    simulator
        .set_input("data", &LogicWord::from_u64(8, 0x5A))
        .unwrap();
    assert_eq!(simulator.read("state").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("pair").unwrap().to_u64(), Some(0x5A));
    assert_eq!(simulator.read("next_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("previous_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("first_state").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("last_state").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("enum_count").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator
            .read("state_name")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "A"
    );
    assert_eq!(simulator.read("initialized").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("selected_a").unwrap().to_u64(), Some(1));
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
        .compile_files_bounded(
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
    let mixed_event = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mixed_event_path),
            Some("mixed_event"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower mixed edge/level sensitivity");
    assert!(
        matches!(
            &mixed_event.processes[0].sens,
            eda_core::ir::Sens::List(list)
                if matches!(list.as_slice(), [eda_core::ir::Sens::Posedge(_), eda_core::ir::Sens::Any(_)])
        ),
        "unexpected sensitivity: {:?}",
        mixed_event.processes[0].sens
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
        .compile_files_bounded(
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
        .compile_files_bounded(
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse memory through the preprocessed path");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Sparse
    );
    assert_eq!(design.memories[0].name, "mem");
    assert_eq!(design.memories[0].element_width, 8);
    assert_eq!(design.memories[0].flat_length, 4);
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::SyncRam
    );
    assert!(design.memories[0]
        .write_mask
        .as_ref()
        .expect("runtime memory write mask")
        .is_full());
    assert_eq!(
        design.memories[0].byte_enable,
        Some(eda_core::ir::MemoryByteEnable::full(8))
    );
    assert!(reference.memories.is_empty());
    assert_eq!(
        format!("{:?}", reference.ports),
        format!("{:?}", design.ports)
    );
    let reference_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize legacy-array AST");
    let mut reference_summary = slang
        .summarize_ast_json(&reference_json)
        .expect("summarize legacy-array AST");
    reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect("exact dynamic array lvalue fingerprint should match");
    if let Some(assignment) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .dynamic_array_lvalue_indexes
                    .first()
                    .is_some_and(|selector| selector.symbol.as_deref() == Some("addr"))
            })
        })
    {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("wrong_addr".into());
    }
    let selector_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic array selector must fail");
    assert!(
        selector_error.contains("Ref:2:u[wrong_addr]") && selector_error.contains("Ref:2:u[addr]"),
        "unexpected dynamic array selector error: {selector_error}"
    );
    if let Some(assignment) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .dynamic_array_lvalue_indexes
                    .first()
                    .is_some_and(|selector| selector.symbol.as_deref() == Some("wrong_addr"))
            })
        })
    {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("addr".into());
        assignment.rhs.symbol = Some("wrong_data".into());
    }
    let value_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic array value must fail");
    assert!(
        value_error.contains("Ref:8:u[wrong_data]") && value_error.contains("Ref:8:u[wdata]"),
        "unexpected dynamic array value error: {value_error}"
    );
    if let Some(assignment) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .dynamic_array_lvalue_indexes
                    .first()
                    .is_some_and(|selector| selector.symbol.as_deref() == Some("addr"))
            })
        })
    {
        assignment.rhs.symbol = Some("wdata".into());
    }
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
fn slang_ast_summary_rejects_wrong_sparse_memory_read_and_part_shapes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-summary");
    let source_path = root.join("memory_summary.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_summary(
  input logic [1:0] address,
  input logic [1:0] start,
  output logic [7:0] data,
  output logic [3:0] part
);
  logic [7:0] memory [0:3];
  assign data = memory[address];
  assign part = memory[address][start +: 4];
endmodule
"#,
    )
    .expect("write sparse-memory summary source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_summary"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory summary source");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory_summary"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize authoritative sparse-memory AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize authoritative sparse-memory AST");
    summary
        .validate_against_design(&design)
        .expect("exact sparse-memory read fingerprint should match");

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "data")
    {
        target.rhs.operands[1].symbol = Some("0 wrong_address".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong sparse-memory index must fail structural validation");
    assert!(
        error.contains("MemoryRead:") && error.contains("wrong_address"),
        "unexpected structural error: {error}"
    );

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "data")
    {
        target.rhs.operands[1].symbol = Some("address".into());
    }
    summary
        .validate_against_design(&design)
        .expect("restored whole-element read should match");
    let part = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "part")
        .expect("sparse-memory part-read target");
    part.rhs.operands[1].symbol = Some("0 wrong_start".into());
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong sparse-memory part start must fail structural validation");
    assert!(
        error.contains("MemoryReadPart:") && error.contains("wrong_start"),
        "unexpected structural error: {error}"
    );
}

#[test]
fn slang_ast_summary_rejects_wrong_packed_select_shapes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-summary");
    let source_path = root.join("packed_summary.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_summary(
  input logic [7:0] data,
  input logic [0:7] ascending,
  input logic [2:0] selector,
  input logic [3:0][7:0] groups,
  input logic [0:3][0:7] ascending_groups,
  input logic signed [2:0] signed_selector,
  input logic [-4:3] negative_range,
  output logic bit_one,
  output logic [3:0] high,
  output logic [3:0] up,
  output logic [3:0] down,
  output logic [3:0] ascending_up,
  output logic dynamic_bit,
  output logic [7:0] dynamic_group,
  output logic [7:0] ascending_dynamic_group,
  output logic negative_dynamic
);
  assign bit_one = data[1];
  assign high = data[7:4];
  assign up = data[3 +: 4];
  assign down = data[6 -: 4];
  assign ascending_up = ascending[0 +: 4];
  assign dynamic_bit = data[selector];
  assign dynamic_group = groups[selector];
  assign ascending_dynamic_group = ascending_groups[selector];
  assign negative_dynamic = negative_range[signed_selector];
endmodule
"#,
    )
    .expect("write packed-select summary source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_summary"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower packed-select summary source");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("packed_summary"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize authoritative packed-select AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize authoritative packed-select AST");
    summary
        .validate_against_design(&design)
        .expect("constant packed-select fingerprints should match");

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "bit_one")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000010".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong constant packed bit index must fail structural validation");
    assert!(
        error.contains("PackedSelect:") && error.contains("Range:2:2"),
        "unexpected structural error: {error}"
    );

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "bit_one")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000001".into());
    }
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "high")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000111".into());
    }
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "up")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000010".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong constant indexed packed select start must fail");
    assert!(
        error.contains("PackedSelect:") && error.contains("Range:5:2"),
        "unexpected structural error: {error}"
    );

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "up")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000011".into());
    }
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "down")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000101".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong constant descending packed select start must fail");
    assert!(
        error.contains("PackedSelect:") && error.contains("Range:5:2"),
        "unexpected structural error: {error}"
    );

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "down")
    {
        target.rhs.operands[1].literal = Some("32:00000000000000000000000000000110".into());
    }

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "dynamic_bit")
    {
        target.rhs.operands[1].symbol = Some("wrong_selector".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong dynamic packed selector must fail structural validation");
    assert!(
        error.contains("Eq:1:u[Ref:3:u[wrong_selector],"),
        "unexpected structural error: {error}"
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower sparse-memory part selects through the preprocessed path");
    let reference_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory_parts"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize bounded part-select AST");
    let mut reference_summary = slang
        .summarize_ast_json(&reference_json)
        .expect("summarize bounded part-select AST");
    reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect("exact bounded array part-write fingerprint should match");
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.indexes[0].literal = Some("32:00000000000000000000000000000000".into());
    }
    let index_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong array part element index must fail");
    assert!(
        index_error.contains("Offset:0") && index_error.contains("Offset:8"),
        "unexpected array part index error: {index_error}"
    );
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.indexes[0].literal = Some("32:00000000000000000000000000000001".into());
        part.start.symbol = Some("wrong_start".into());
    }
    let start_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong array part start must fail");
    assert!(
        start_error.contains("Ref:3:u[wrong_start]") && start_error.contains("Ref:3:u[start]"),
        "unexpected array part start error: {start_error}"
    );
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.start.symbol = Some("start".into());
        part.width = 3;
    }
    let width_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong array part width must fail closed");
    assert!(
        width_error.contains("could not fingerprint array part lvalue 'mem'"),
        "unexpected array part width error: {width_error}"
    );
    if let Some(assignment) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find(|assignment| assignment.array_part_lvalue.is_some())
        })
    {
        assignment.array_part_lvalue.as_mut().unwrap().width = 4;
        assignment.rhs.operands[0].symbol = Some("wrong_data".into());
    }
    let value_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong array part value must fail");
    assert!(
        value_error.contains("PackedSelect:4:u[Ref:8:u[wrong_data]")
            && value_error.contains("PackedSelect:4:u[Ref:8:u[data]"),
        "unexpected array part value error: {value_error}"
    );
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.processes.len(), 1);
    assert!(design.memories[0]
        .write_mask
        .as_ref()
        .expect("dynamic part writes require a conservative mask")
        .is_full());

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
fn slang_ast_summary_uses_reversed_dynamic_array_fingerprints() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("reversed-dynamic-array");
    let source_path = root.join("reversed_dynamic_array.sv");
    std::fs::write(
        &source_path,
        r#"
module reversed_dynamic_array(
  input logic clk,
  input logic [1:0] addr,
  input logic [7:0] wdata,
  output logic [7:0] q
);
  logic [0:7] mem [0:3];
  initial begin
    mem[0] = 8'h12;
    mem[3] = 8'h34;
  end
  always @(posedge clk) mem[addr] <= wdata;
  assign q = mem[addr];
endmodule
"#,
    )
    .expect("write reversed dynamic array source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("reversed_dynamic_array"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower reversed dynamic array");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("reversed_dynamic_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower reversed dynamic array through bounded path");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("reversed_dynamic_array"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize reversed dynamic array AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize reversed dynamic array AST");
    summary
        .validate_against_design(&direct)
        .expect("exact direct reversed dynamic fingerprint should match");
    summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact bounded reversed dynamic fingerprint should match");
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "q")
    {
        target.rhs.operands[1].symbol = Some("wrong_addr".into());
    }
    let continuous_read_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong reversed continuous read selector must fail");
    assert!(
        continuous_read_error.contains("Ref:2:u[wrong_addr]")
            && continuous_read_error.contains("Ref:2:u[addr]")
            && continuous_read_error.contains("Cond:8:u[")
            && continuous_read_error.contains("Concat:8:u["),
        "unexpected reversed continuous read error: {continuous_read_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "q")
    {
        target.rhs.operands[1].symbol = Some("addr".into());
    }
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| !assignment.dynamic_array_lvalue_indexes.is_empty())
    }) {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("wrong_addr".into());
    }
    let selector_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong reversed dynamic selector must fail");
    assert!(
        selector_error.contains("Ref:2:u[wrong_addr]")
            && selector_error.contains("Ref:2:u[addr]")
            && selector_error.contains("GuardedArrayPartWrite:8:u[")
            && selector_error.contains("Reversed:true"),
        "unexpected reversed selector error: {selector_error}"
    );
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| !assignment.dynamic_array_lvalue_indexes.is_empty())
    }) {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("addr".into());
        assignment.rhs.symbol = Some("wrong_data".into());
    }
    let value_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong reversed dynamic value must fail");
    assert!(
        value_error.contains("Ref:8:u[wrong_data]")
            && value_error.contains("Ref:8:u[wdata]")
            && value_error.contains("GuardedArrayPartWrite:8:u["),
        "unexpected reversed value error: {value_error}"
    );
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| !assignment.dynamic_array_lvalue_indexes.is_empty())
    }) {
        assignment.rhs.symbol = Some("wdata".into());
    }
    assert_eq!(direct.memories.len(), 1);
    assert!(bounded.memories.is_empty());

    let mut direct_simulator =
        Simulator::new(&direct).expect("create direct reversed-array simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("addr", &LogicWord::from_u64(2, 3))
        .expect("select direct reversed element");
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set direct reversed data");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct reversed posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x5a));
    // Reversed-element direct/bounded runtime parity is intentionally not
    // claimed here: the legacy path currently applies its historical logical
    // bit order to whole-element reads. The exact assignment fingerprints
    // above still prove selector, offset, reversal flag, and value identity.
}

#[test]
fn slang_ast_summary_uses_wide_dynamic_array_fingerprints() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("wide-dynamic-array-parts");
    let source_path = root.join("wide_dynamic_array.sv");
    std::fs::write(
        &source_path,
        r#"
module wide_dynamic_array(
  input logic clk,
  input logic [2:0] addr,
  input logic [2:0] start,
  input logic [7:0] wdata,
  output logic [7:0] whole,
  output logic [7:0] indexed_whole,
  output logic [3:0] part
);
  logic [7:0] mem [0:4];
  initial begin
    mem[0] = 8'h12;
    mem[4] = 8'h34;
  end
  always @(posedge clk) begin
    mem[addr] <= wdata;
    mem[addr][start +: 4] <= wdata[3:0];
  end
  assign whole = mem[addr];
  assign indexed_whole = mem[addr][0 +: 8];
  assign part = mem[addr][start +: 4];
endmodule
"#,
    )
    .expect("write wide dynamic array source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("wide_dynamic_array"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wide dynamic array");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("wide_dynamic_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower wide dynamic array through bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert!(bounded.memories.is_empty());
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("wide_dynamic_array"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize wide dynamic array AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize wide dynamic array AST");
    summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact wide dynamic array fingerprints should match");

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "whole")
    {
        target.rhs.operands[1].symbol = Some("wrong_addr".into());
    }
    let continuous_read_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong continuous wide array read selector must fail");
    assert!(
        continuous_read_error.contains("Ref:3:u[wrong_addr]")
            && continuous_read_error.contains("Ref:3:u[addr]")
            && continuous_read_error.contains("Cond:40:u["),
        "unexpected continuous read error: {continuous_read_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "whole")
    {
        target.rhs.operands[1].symbol = Some("addr".into());
    }

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "indexed_whole")
    {
        target.rhs.operands[0].operands[1].symbol = Some("wrong_addr".into());
    }
    let indexed_whole_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong canonical indexed whole-element selector must fail");
    assert!(
        indexed_whole_error.contains("Ref:3:u[wrong_addr]")
            && indexed_whole_error.contains("Ref:3:u[addr]")
            && indexed_whole_error.contains("Cond:40:u["),
        "unexpected indexed whole-element error: {indexed_whole_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "indexed_whole")
    {
        target.rhs.operands[0].operands[1].symbol = Some("addr".into());
    }

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "part")
    {
        target.rhs.operands[0].operands[1].symbol = Some("wrong_addr".into());
    }
    let part_read_index_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong continuous wide array part-read selector must fail");
    assert!(
        part_read_index_error.contains("Ref:3:u[wrong_addr]")
            && part_read_index_error.contains("Ref:3:u[addr]")
            && part_read_index_error.contains("Cond:8:u["),
        "unexpected continuous part-read index error: {part_read_index_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "part")
    {
        target.rhs.operands[0].operands[1].symbol = Some("addr".into());
        target.rhs.operands[1].symbol = Some("wrong_start".into());
    }
    let part_read_start_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong continuous wide array part-read start must fail");
    assert!(
        part_read_start_error.contains("Ref:3:u[wrong_start]")
            && part_read_start_error.contains("Ref:3:u[start]")
            && part_read_start_error.contains("Cond:8:u["),
        "unexpected continuous part-read start error: {part_read_start_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "part")
    {
        target.rhs.operands[1].symbol = Some("start".into());
        target.rhs.type_width = Some(3);
        target.rhs.operands[2].literal = Some("32:011".into());
    }
    let part_read_width_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong continuous wide array part-read width must fail");
    assert!(
        part_read_width_error.contains("Const:3:u[3:111]")
            && part_read_width_error.contains("Const:4:u[4:1111]"),
        "unexpected continuous part-read width error: {part_read_width_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "part")
    {
        target.rhs.type_width = Some(4);
        target.rhs.operands[2].literal = Some("32:00000000000000000000000000000100".into());
    }

    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| assignment.dynamic_array_lvalue_indexes.len() == 1)
    }) {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("wrong_addr".into());
    }
    let whole_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong wide whole-element selector must fail");
    assert!(
        whole_error.contains("Ref:3:u[wrong_addr]")
            && whole_error.contains("Ref:3:u[addr]")
            && whole_error.contains("Cond:40:u["),
        "unexpected wide whole-element error: {whole_error}"
    );
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| assignment.dynamic_array_lvalue_indexes.len() == 1)
    }) {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("addr".into());
        assignment.rhs.symbol = Some("wrong_data".into());
    }
    let whole_value_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong wide whole-element value must fail");
    assert!(
        whole_value_error.contains("Ref:8:u[wrong_data]")
            && whole_value_error.contains("Ref:8:u[wdata]")
            && whole_value_error.contains("Cond:40:u["),
        "unexpected wide whole-value error: {whole_value_error}"
    );
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| assignment.array_part_lvalue.is_some())
    }) {
        assignment.rhs.symbol = Some("wdata".into());
        assignment.array_part_lvalue.as_mut().unwrap().start.symbol = Some("wrong_start".into());
    }
    let part_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong wide part start must fail");
    assert!(
        part_error.contains("Ref:3:u[wrong_start]")
            && part_error.contains("Ref:3:u[start]")
            && part_error.contains("GuardedArrayPartWrite:4:u["),
        "unexpected wide part error: {part_error}"
    );
    if let Some(part) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find_map(|assignment| assignment.array_part_lvalue.as_mut())
    }) {
        part.start.symbol = Some("start".into());
        part.width = 3;
    }
    let width_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong wide part width must fail closed");
    assert!(
        width_error.contains("could not fingerprint array part lvalue 'mem'"),
        "unexpected wide part width error: {width_error}"
    );
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| assignment.array_part_lvalue.is_some())
    }) {
        assignment.array_part_lvalue.as_mut().unwrap().width = 4;
        assignment.rhs.operands[0].symbol = Some("wrong_data".into());
    }
    let part_value_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong wide part value must fail");
    assert!(
        part_value_error.contains("PackedSelect:4:u[Ref:8:u[wrong_data]")
            && part_value_error.contains("PackedSelect:4:u[Ref:8:u[wdata]"),
        "unexpected wide part value error: {part_value_error}"
    );

    let mut direct_simulator = Simulator::new(&direct).expect("create direct wide-array simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("addr", &LogicWord::from_u64(3, 4))
        .expect("select direct element");
    direct_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set direct part start");
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set direct data");
    assert_eq!(direct_simulator.read("whole").unwrap().to_u64(), Some(0x34));
    assert_eq!(
        direct_simulator.read("indexed_whole").unwrap().to_u64(),
        Some(0x34)
    );
    assert_eq!(direct_simulator.read("part").unwrap().to_u64(), Some(0x3));
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct posedge");
    assert_eq!(direct_simulator.read("whole").unwrap().to_u64(), Some(0xaa));
    assert_eq!(
        direct_simulator.read("indexed_whole").unwrap().to_u64(),
        Some(0xaa)
    );
    assert_eq!(direct_simulator.read("part").unwrap().to_u64(), Some(0xa));

    let mut bounded_simulator =
        Simulator::new(&bounded).expect("create bounded wide-array simulator");
    bounded_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    bounded_simulator
        .set_input("addr", &LogicWord::from_u64(3, 4))
        .expect("select bounded element");
    bounded_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set bounded part start");
    bounded_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x5a))
        .expect("set bounded data");
    assert_eq!(
        bounded_simulator.read("whole").unwrap().to_u64(),
        Some(0x34)
    );
    assert_eq!(
        bounded_simulator.read("indexed_whole").unwrap().to_u64(),
        Some(0x34)
    );
    assert_eq!(bounded_simulator.read("part").unwrap().to_u64(), Some(0x3));
    bounded_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded posedge");
    assert_eq!(
        bounded_simulator.read("whole").unwrap().to_u64(),
        Some(0xaa)
    );
    assert_eq!(
        bounded_simulator.read("indexed_whole").unwrap().to_u64(),
        Some(0xaa)
    );
    assert_eq!(bounded_simulator.read("part").unwrap().to_u64(), Some(0xa));
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &PreprocessOptions::default(),
        )
        .expect("lower array-element simple range through bounded path");
    let bounded_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("simple_range"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize bounded array-element AST");
    let mut bounded_summary = slang
        .summarize_ast_json(&bounded_json)
        .expect("summarize bounded array-element AST");
    bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact constant-index array fingerprints should match");
    if let Some(target) = bounded_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "y")
    {
        target.rhs.operands[0].operands[1].symbol = Some("wrong_addr".into());
    }
    let simple_range_index_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong dynamic array simple-range selector must fail");
    assert!(
        simple_range_index_error.contains("Ref:2:u[wrong_addr]")
            && simple_range_index_error.contains("Ref:2:u[addr]")
            && simple_range_index_error.contains("Cond:8:u["),
        "unexpected simple-range selector error: {simple_range_index_error}"
    );
    if let Some(target) = bounded_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "y")
    {
        target.rhs.operands[0].operands[1].symbol = Some("addr".into());
        target.rhs.type_width = Some(3);
        target.rhs.operands[1].literal = Some("32:010".into());
    }
    let simple_range_width_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong dynamic array simple-range width must fail");
    assert!(
        simple_range_width_error.contains("Const:3:u[3:111]")
            && simple_range_width_error.contains("Const:4:u[4:1111]"),
        "unexpected simple-range width error: {simple_range_width_error}"
    );
    if let Some(target) = bounded_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "y")
    {
        target.rhs.type_width = Some(4);
        target.rhs.operands[1].literal = Some("32:011".into());
    }
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
    {
        assignment.rhs.operands[0].literal = Some("8:10100100".into());
    }
    let whole_element_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant-index whole-element RHS must fail");
    assert!(
        whole_element_error.contains("Const:8:u[8:10100100]")
            && whole_element_error.contains("Const:8:u[8:10011100]"),
        "unexpected whole-element error: {whole_element_error}"
    );
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
    {
        assignment.rhs.operands[0].literal = Some("8:10011100".into());
    }
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.get_mut(1))
    {
        assignment.rhs.operands[0].literal = Some("2:10".into());
    }
    let range_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant-index array range RHS must fail");
    assert!(
        range_error.contains("Const:2:u[2:10]") && range_error.contains("Const:2:u[2:01]"),
        "unexpected array-range error: {range_error}"
    );
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional memory through the bounded path");
    assert_eq!(direct.memories.len(), 1);
    assert_eq!(direct.memories[0].dimensions.len(), 2);
    assert_eq!(direct.memories[0].flat_length, 4);
    assert_eq!(
        direct.memories[0].banking,
        eda_core::ir::MemoryBanking { count: 2, depth: 2 }
    );
    assert_eq!(direct.n_nets, 27);
    let direct_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory2d"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize multidimensional-memory AST");
    let direct_summary = slang
        .summarize_ast_json(&direct_json)
        .expect("summarize multidimensional-memory AST");
    direct_summary
        .validate_against_design(&direct)
        .expect("exact multidimensional memory fingerprints should match");
    let mut bounded_summary = direct_summary.clone();
    bounded_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect("exact dynamic multidimensional whole-element fingerprint should match");
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find(|assignment| assignment.dynamic_array_lvalue_indexes.len() == 2)
        })
    {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("wrong_x".into());
    }
    let bounded_first_index_error = bounded_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong bounded first multidimensional index must fail");
    assert!(
        bounded_first_index_error.contains("Ref:2:u[wrong_x]")
            && bounded_first_index_error.contains("Ref:2:u[x]")
            && bounded_first_index_error.contains("Cond:32:u["),
        "unexpected bounded first index error: {bounded_first_index_error}"
    );
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find(|assignment| assignment.dynamic_array_lvalue_indexes.len() == 2)
        })
    {
        assignment.dynamic_array_lvalue_indexes[0].symbol = Some("x".into());
        assignment.dynamic_array_lvalue_indexes[1].symbol = Some("wrong_y".into());
    }
    let bounded_second_index_error = bounded_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong bounded second multidimensional index must fail");
    assert!(
        bounded_second_index_error.contains("Ref:3:u[wrong_y]")
            && bounded_second_index_error.contains("Ref:3:u[y]")
            && bounded_second_index_error.contains("Cond:32:u["),
        "unexpected bounded second index error: {bounded_second_index_error}"
    );
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find(|assignment| assignment.dynamic_array_lvalue_indexes.len() == 2)
        })
    {
        assignment.dynamic_array_lvalue_indexes[1].symbol = Some("y".into());
        assignment.rhs.symbol = Some("wrong_data".into());
    }
    let bounded_value_error = bounded_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong bounded multidimensional value must fail");
    assert!(
        bounded_value_error.contains("Ref:8:u[wrong_data]")
            && bounded_value_error.contains("Ref:8:u[wdata]")
            && bounded_value_error.contains("Cond:32:u["),
        "unexpected bounded value error: {bounded_value_error}"
    );

    let mut wrong_first_index = direct_summary.clone();
    if let Some(assignment) = wrong_first_index
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        assignment.rhs.operands[0].operands[0].operands[1].symbol = Some("wrong_x".into());
    }
    let first_index_error = wrong_first_index
        .validate_against_design(&direct)
        .expect_err("wrong first multidimensional-memory index must fail");
    assert!(
        first_index_error.contains("Ref:2:u[wrong_x]")
            && first_index_error.contains("Ref:2:u[x]")
            && first_index_error.contains("MemoryReadPart:4:u[mem,"),
        "unexpected first multidimensional-memory index error: {first_index_error}"
    );

    let mut wrong_second_index = direct_summary;
    if let Some(target) = wrong_second_index
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        target.rhs.operands[0].operands[1].symbol = Some("wrong_y".into());
    }
    let second_index_error = wrong_second_index
        .validate_against_design(&direct)
        .expect_err("wrong second multidimensional-memory index must fail");
    assert!(
        second_index_error.contains("Ref:3:u[wrong_y]")
            && second_index_error.contains("Ref:3:u[y]")
            && second_index_error.contains("MemoryReadPart:4:u[mem,"),
        "unexpected second multidimensional-memory index error: {second_index_error}"
    );

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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional indexed part write through bounded path");
    let reference_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory2d_parts"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize multidimensional indexed part AST");
    let mut reference_summary = slang
        .summarize_ast_json(&reference_json)
        .expect("summarize multidimensional indexed part AST");
    reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect("exact dynamic multidimensional part fingerprint should match");
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.indexes[0].symbol = Some("wrong_x".into());
    }
    let first_index_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic first multidimensional index must fail");
    assert!(
        first_index_error.contains("Ref:2:u[wrong_x]")
            && first_index_error.contains("Ref:2:u[x]")
            && first_index_error.contains("GuardedArrayPartWrite:4:u["),
        "unexpected dynamic first index error: {first_index_error}"
    );
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.indexes[0].symbol = Some("x".into());
        part.indexes[1].symbol = Some("wrong_y".into());
    }
    let second_index_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic second multidimensional index must fail");
    assert!(
        second_index_error.contains("Ref:3:u[wrong_y]")
            && second_index_error.contains("Ref:3:u[y]")
            && second_index_error.contains("GuardedArrayPartWrite:4:u["),
        "unexpected dynamic second index error: {second_index_error}"
    );
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.indexes[1].symbol = Some("y".into());
        part.start.symbol = Some("wrong_start".into());
    }
    let start_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic multidimensional part start must fail");
    assert!(
        start_error.contains("Ref:3:u[wrong_start]") && start_error.contains("Ref:3:u[start]"),
        "unexpected dynamic part start error: {start_error}"
    );
    if let Some(part) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find_map(|assignment| assignment.array_part_lvalue.as_mut())
        })
    {
        part.start.symbol = Some("start".into());
        part.width = 3;
    }
    let width_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic multidimensional part width must fail closed");
    assert!(
        width_error.contains("could not fingerprint array part lvalue 'mem'"),
        "unexpected dynamic part width error: {width_error}"
    );
    if let Some(assignment) = reference_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block
                .assignments
                .iter_mut()
                .find(|assignment| assignment.array_part_lvalue.is_some())
        })
    {
        assignment.array_part_lvalue.as_mut().unwrap().width = 4;
        assignment.rhs.operands[0].symbol = Some("wrong_data".into());
    }
    let value_error = reference_summary
        .validate_against_design_legacy_compatibility(&reference)
        .expect_err("wrong dynamic multidimensional part value must fail");
    assert!(
        value_error.contains("PackedSelect:4:u[Ref:8:u[wrong_data]")
            && value_error.contains("PackedSelect:4:u[Ref:8:u[wdata]"),
        "unexpected dynamic part value error: {value_error}"
    );
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
fn slang_ast_summary_uses_multidimensional_array_part_layouts() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory2d-constant-parts");
    let source_path = root.join("memory2d_constant_parts.sv");
    std::fs::write(
        &source_path,
        r#"
module memory2d_constant_parts(
  input logic clk,
  input logic [2:0] start,
  input logic [7:0] wdata,
  output logic [7:0] q,
  output logic [3:0] p,
  output logic [3:0] r
);
  logic [7:0] mem [0:1][2:3];
  initial begin
    mem[1][3] = 8'h34;
  end
  always @(posedge clk) mem[1][3][start +: 4] <= wdata[3:0];
  assign q = mem[1][3];
  assign p = mem[1][3][start +: 4];
  assign r = mem[1][3][5:2];
endmodule
"#,
    )
    .expect("write multidimensional constant part-write source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory2d_constant_parts"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional constant part write");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("memory2d_constant_parts"),
            &PreprocessOptions::default(),
        )
        .expect("lower multidimensional constant part write through bounded path");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory2d_constant_parts"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize multidimensional constant part AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize multidimensional constant part AST");
    summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact multidimensional constant part fingerprint should match");

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "q")
    {
        target.rhs.operands[0].operands[1].literal =
            Some("32:00000000000000000000000000000000".into());
    }
    let constant_read_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant multidimensional array read index must fail");
    assert!(
        constant_read_error.contains("Range:15:8") && constant_read_error.contains("Range:31:24"),
        "unexpected constant read error: {constant_read_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "q")
    {
        target.rhs.operands[0].operands[1].literal =
            Some("32:00000000000000000000000000000001".into());
    }

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        target.rhs.operands[1].symbol = Some("wrong_start".into());
    }
    let constant_part_start_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant multidimensional array part-read start must fail");
    assert!(
        constant_part_start_error.contains("Ref:3:u[wrong_start]")
            && constant_part_start_error.contains("Ref:3:u[start]")
            && constant_part_start_error.contains("Cond:8:u["),
        "unexpected constant part-read start error: {constant_part_start_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        target.rhs.operands[1].symbol = Some("start".into());
        target.rhs.operands[0].operands[1].literal =
            Some("32:00000000000000000000000000000010".into());
    }
    let constant_part_index_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant multidimensional array part-read index must fail");
    assert!(
        constant_part_index_error.contains("Range:23:16")
            && constant_part_index_error.contains("Range:31:24"),
        "unexpected constant part-read index error: {constant_part_index_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        target.rhs.operands[0].operands[1].literal =
            Some("32:00000000000000000000000000000011".into());
        target.rhs.type_width = Some(3);
        target.rhs.operands[2].literal = Some("32:011".into());
    }
    let constant_part_width_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant multidimensional array part-read width must fail");
    assert!(
        constant_part_width_error.contains("Const:3:u[3:111]")
            && constant_part_width_error.contains("Const:4:u[4:1111]"),
        "unexpected constant part-read width error: {constant_part_width_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "p")
    {
        target.rhs.type_width = Some(4);
        target.rhs.operands[2].literal = Some("32:00000000000000000000000000000100".into());
    }

    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "r")
    {
        target.rhs.type_width = Some(3);
        target.rhs.operands[1].literal = Some("32:100".into());
    }
    let constant_simple_range_width_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong constant multidimensional array simple-range width must fail");
    assert!(
        constant_simple_range_width_error.contains("Const:3:u[3:111]")
            && constant_simple_range_width_error.contains("Const:4:u[4:1111]"),
        "unexpected constant simple-range width error: {constant_simple_range_width_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "r")
    {
        target.rhs.type_width = Some(4);
        target.rhs.operands[1].literal = Some("32:101".into());
    }

    if let Some(part) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find_map(|assignment| assignment.array_part_lvalue.as_mut())
    }) {
        part.indexes[0].literal = Some("32:00000000000000000000000000000000".into());
    }
    let first_index_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong first multidimensional part index must fail");
    assert!(
        first_index_error.contains("Offset:8") && first_index_error.contains("Offset:24"),
        "unexpected first index error: {first_index_error}"
    );

    if let Some(part) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find_map(|assignment| assignment.array_part_lvalue.as_mut())
    }) {
        part.indexes[0].literal = Some("32:00000000000000000000000000000001".into());
        part.indexes[1].literal = Some("32:00000000000000000000000000000010".into());
    }
    let second_index_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong second multidimensional part index must fail");
    assert!(
        second_index_error.contains("Offset:16") && second_index_error.contains("Offset:24"),
        "unexpected second index error: {second_index_error}"
    );

    if let Some(part) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find_map(|assignment| assignment.array_part_lvalue.as_mut())
    }) {
        part.indexes[1].literal = Some("32:00000000000000000000000000000011".into());
        part.start.symbol = Some("wrong_start".into());
    }
    let start_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong multidimensional part start must fail");
    assert!(
        start_error.contains("Ref:3:u[wrong_start]") && start_error.contains("Ref:3:u[start]"),
        "unexpected multidimensional part start error: {start_error}"
    );

    if let Some(part) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find_map(|assignment| assignment.array_part_lvalue.as_mut())
    }) {
        part.start.symbol = Some("start".into());
        part.width = 3;
    }
    let width_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong multidimensional part width must fail closed");
    assert!(
        width_error.contains("could not fingerprint array part lvalue 'mem'"),
        "unexpected multidimensional part width error: {width_error}"
    );

    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block
            .assignments
            .iter_mut()
            .find(|assignment| assignment.array_part_lvalue.is_some())
    }) {
        assignment.array_part_lvalue.as_mut().unwrap().width = 4;
        assignment.rhs.operands[0].symbol = Some("wrong_data".into());
    }
    let value_error = summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong multidimensional part value must fail");
    assert!(
        value_error.contains("PackedSelect:4:u[Ref:8:u[wrong_data]")
            && value_error.contains("PackedSelect:4:u[Ref:8:u[wdata]"),
        "unexpected multidimensional part value error: {value_error}"
    );

    let mut direct_simulator =
        Simulator::new(&direct).expect("create direct multidimensional part simulator");
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize direct clock");
    direct_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set direct part start");
    direct_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set direct part data");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0x3));
    assert_eq!(direct_simulator.read("r").unwrap().to_u64(), Some(0xd));
    direct_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create direct posedge");
    assert_eq!(direct_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(direct_simulator.read("p").unwrap().to_u64(), Some(0xa));
    assert_eq!(direct_simulator.read("r").unwrap().to_u64(), Some(0x9));

    let mut bounded_simulator =
        Simulator::new(&bounded).expect("create bounded multidimensional part simulator");
    bounded_simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .expect("initialize bounded clock");
    bounded_simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .expect("set bounded part start");
    bounded_simulator
        .set_input("wdata", &LogicWord::from_u64(8, 0x0a))
        .expect("set bounded part data");
    assert_eq!(bounded_simulator.read("q").unwrap().to_u64(), Some(0x34));
    assert_eq!(bounded_simulator.read("p").unwrap().to_u64(), Some(0x3));
    assert_eq!(bounded_simulator.read("r").unwrap().to_u64(), Some(0xd));
    bounded_simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .expect("create bounded posedge");
    assert_eq!(bounded_simulator.read("q").unwrap().to_u64(), Some(0xa4));
    assert_eq!(bounded_simulator.read("p").unwrap().to_u64(), Some(0xa));
    assert_eq!(bounded_simulator.read("r").unwrap().to_u64(), Some(0x9));
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower shared sparse-memory port through bounded path");
    assert_eq!(direct.memories.len(), 2);
    assert_eq!(direct.memory_connections.len(), 1);
    assert_eq!(direct.array_connections.len(), 0);
    assert_eq!(direct.memory_connections[0].child, 1);
    assert_eq!(direct.memory_connections[0].parent, 0);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.memory_connections.len(), 1);
    assert_eq!(direct.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(direct.memories[1].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(reference.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(reference.memories[1].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(
        direct.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        direct.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        reference.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
    assert_eq!(
        reference.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRam
    );
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
fn slang_ast_direct_and_bounded_paths_share_nested_ordinary_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nested-ordinary-array-ports");
    let source_path = root.join("nested_ordinary_array_ports.sv");
    std::fs::write(
        &source_path,
        r#"
module leaf(
  input wire logic [7:0] source [0:3],
  output logic [7:0] destination [0:3],
  output logic [7:0] source_observed
);
  initial begin
    destination[2] = 8'h5a;
  end
  assign source_observed = source[2];
endmodule

module middle(
  input wire logic [7:0] pass_source [0:3],
  output logic [7:0] pass_destination [0:3],
  output logic [7:0] middle_observed
);
  leaf u (
    .source(pass_source),
    .destination(pass_destination),
    .source_observed(middle_observed)
  );
endmodule

module top(
  output logic [7:0] source_observed,
  output logic [7:0] destination_observed
);
  logic [7:0] source [0:3];
  logic [7:0] destination [0:3];

  initial begin
    source[2] = 8'h3c;
  end

  middle u (
    .pass_source(source),
    .pass_destination(destination),
    .middle_observed(source_observed)
  );
  assign destination_observed = destination[2];
endmodule
"#,
    )
    .expect("write nested ordinary-array port source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested ordinary-array ports");
    let reference = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower nested ordinary-array ports through bounded path");

    assert_eq!(direct.memories.len(), 6);
    assert_eq!(direct.memory_connections.len(), 0);
    assert_eq!(direct.array_connections.len(), 4);
    assert!(direct
        .array_connections
        .contains(&eda_core::ir::ArrayConnection {
            child: 4,
            parent: 2
        }));
    assert!(direct
        .array_connections
        .contains(&eda_core::ir::ArrayConnection {
            child: 5,
            parent: 3
        }));
    assert!(direct
        .array_connections
        .contains(&eda_core::ir::ArrayConnection {
            child: 2,
            parent: 0
        }));
    assert!(direct
        .array_connections
        .contains(&eda_core::ir::ArrayConnection {
            child: 3,
            parent: 1
        }));
    assert!(direct
        .memories
        .iter()
        .all(|memory| memory.storage == eda_core::ir::MemoryStorage::Ordinary));

    let mut direct_simulator =
        Simulator::new(&direct).expect("create direct nested ordinary-array simulator");
    direct_simulator
        .run_until(0)
        .expect("settle direct nested ordinary arrays");
    assert_eq!(
        direct_simulator.read("source_observed").unwrap().to_u64(),
        Some(0x3c),
        "parent initial value must flow through two child input aliases"
    );
    assert_eq!(
        direct_simulator
            .read("destination_observed")
            .unwrap()
            .to_u64(),
        Some(0x5a),
        "leaf initial value must flow through two child output aliases"
    );

    let mut reference_simulator =
        Simulator::new(&reference).expect("create bounded nested ordinary-array simulator");
    reference_simulator
        .run_until(0)
        .expect("settle bounded nested ordinary arrays");
    assert_eq!(
        reference_simulator
            .read("source_observed")
            .unwrap()
            .to_u64(),
        Some(0x3c)
    );
    assert_eq!(
        reference_simulator
            .read("destination_observed")
            .unwrap()
            .to_u64(),
        Some(0x5a)
    );
}

#[test]
fn slang_ast_direct_lowering_initializes_multidimensional_ordinary_array() {
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
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(design.memories[0].dimensions.len(), 2);
    assert_eq!(design.memories[0].flat_length, 6);
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Ram);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::Unmapped
    );
    assert_eq!(design.memories[0].write_mask, None);
    assert_eq!(design.memories[0].byte_enable, None);
    assert_eq!(
        design.memories[0].banking,
        eda_core::ir::MemoryBanking::default()
    );
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
fn slang_ast_direct_lowering_supports_directional_sparse_memory_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-directional-memory-port");
    let source_path = root.join("directional_memory_port.sv");
    std::fs::write(
        &source_path,
        r#"
module child(
  input wire logic [7:0] in_mem [0:3],
  output wire logic [7:0] out_mem [0:3],
  output logic [7:0] q
);
  assign q = in_mem[2];
  assign out_mem[2] = 8'h5a;
endmodule

module top(
  input logic [7:0] din,
  output logic [7:0] child_q,
  output logic [7:0] parent_q
);
  wire logic [7:0] source [0:3];
  wire logic [7:0] destination [0:3];
  assign source[2] = din;
  child u (
    .in_mem(source),
    .out_mem(destination),
    .q(child_q)
  );
  assign parent_q = destination[2];
endmodule
"#,
    )
    .expect("write directional sparse-memory port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower directional sparse-memory ports");
    assert_eq!(design.memories.len(), 4);
    assert_eq!(design.memory_connections.len(), 2);
    assert_eq!(design.memory_connections[0].child, 2);
    assert_eq!(design.memory_connections[0].parent, 0);
    assert_eq!(design.memory_connections[1].child, 3);
    assert_eq!(design.memory_connections[1].parent, 1);

    let mut simulator = Simulator::new(&design).expect("create directional-memory simulator");
    simulator
        .set_input("din", &LogicWord::from_u64(8, 0x3c))
        .unwrap();
    assert_eq!(simulator.read("child_q").unwrap().to_u64(), Some(0x3c));
    assert_eq!(simulator.read("parent_q").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn slang_ast_direct_lowering_supports_directional_ordinary_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-ordinary-array-ports");
    let source_path = root.join("ordinary_array_ports.sv");
    std::fs::write(
        &source_path,
        r#"
module ordinary_array_ports(
  input wire logic [7:0] source [0:3],
  output logic [7:0] destination [0:3],
  inout wire logic [7:0] bidirectional [0:3],
  output logic [7:0] observed,
  output logic [7:0] observed_bidirectional
);
  initial begin
    destination[0] = 8'h10;
    destination[1] = 8'h11;
    destination[2] = 8'h12;
    destination[3] = 8'h13;
  end
  assign observed = source[2];
  assign observed_bidirectional = bidirectional[1];
endmodule
"#,
    )
    .expect("write directional ordinary-array port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("ordinary_array_ports"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower directional ordinary-array ports");
    assert_eq!(design.memory_ports.len(), 0);
    assert_eq!(design.array_ports.len(), 3);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(
        design.memories[1].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::Unmapped
    );
    assert_eq!(
        design.array_ports[0],
        eda_core::ir::ArrayPortDecl {
            name: "source".to_string(),
            direction: eda_core::ir::MemoryPortDirection::In,
            array: 0,
        }
    );
    assert_eq!(
        design.array_ports[1],
        eda_core::ir::ArrayPortDecl {
            name: "destination".to_string(),
            direction: eda_core::ir::MemoryPortDirection::Out,
            array: 1,
        }
    );
    assert_eq!(
        design.array_ports[2],
        eda_core::ir::ArrayPortDecl {
            name: "bidirectional".to_string(),
            direction: eda_core::ir::MemoryPortDirection::InOut,
            array: 2,
        }
    );
    assert_eq!(design.ports.len(), 5);

    let mut simulator = Simulator::new(&design).expect("create ordinary-array-port simulator");
    let error = simulator
        .set_memory_input("source", &[2], &LogicWord::from_u64(8, 0x42))
        .expect_err("sparse-memory APIs must reject ordinary arrays");
    assert!(
        error.contains("sparse-memory port 'source' is ordinary"),
        "unexpected error: {error}"
    );
    simulator
        .run_until(0)
        .expect("run ordinary-array initializers");
    simulator
        .set_ordinary_array_input("source", &[2], &LogicWord::from_u64(8, 0x42))
        .expect("set one ordinary-array input element");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0x42));
    simulator
        .set_ordinary_array_input("bidirectional", &[1], &LogicWord::from_u64(8, 0x5a))
        .expect("set one ordinary-array inout element");
    assert_eq!(
        simulator
            .read_ordinary_array_output("bidirectional", &[1])
            .expect("read one ordinary-array inout element"),
        LogicWord::from_u64(8, 0x5a)
    );
    assert_eq!(
        simulator.read("observed_bidirectional").unwrap().to_u64(),
        Some(0x5a)
    );
    assert_eq!(
        simulator
            .read_ordinary_array_output("destination", &[2])
            .expect("read one ordinary-array output element"),
        LogicWord::from_u64(8, 0x12)
    );

    let stimulus = vec![
        (vec![0], LogicWord::from_u64(8, 0x10)),
        (vec![3], LogicWord::from_u64(8, 0x13)),
    ];
    simulator
        .set_ordinary_array_input_bulk("source", &stimulus)
        .expect("bulk-set ordinary-array input");
    let values = simulator
        .read_ordinary_array_output_bulk("destination", &[vec![3], vec![2], vec![0]])
        .expect("bulk-read ordinary-array output");
    assert_eq!(
        values,
        vec![
            LogicWord::from_u64(8, 0x13),
            LogicWord::from_u64(8, 0x12),
            LogicWord::from_u64(8, 0x10),
        ]
    );

    simulator
        .set_ordinary_array_input_range(
            "source",
            &[(1, 2)],
            &[LogicWord::from_u64(8, 0x21), LogicWord::from_u64(8, 0x22)],
        )
        .expect("range-set ordinary-array input");
    assert_eq!(
        simulator
            .read_ordinary_array_output_range("destination", &[(2, 3)])
            .expect("range-read ordinary-array output"),
        vec![LogicWord::from_u64(8, 0x12), LogicWord::from_u64(8, 0x13),]
    );
}

#[test]
fn slang_ast_direct_lowering_supports_top_level_sparse_memory_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-top-memory-port");
    let source_path = root.join("top_memory_port.sv");
    std::fs::write(
        &source_path,
        r#"
module top(
  input wire logic [7:0] source [0:4096],
  output wire logic [7:0] destination [0:4096],
  output logic [7:0] observed
);
  assign observed = source[2];
  assign destination[3] = source[1];
endmodule
"#,
    )
    .expect("write top-level sparse-memory port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower top-level sparse-memory ports");
    assert_eq!(design.memory_ports.len(), 2);
    assert_eq!(design.array_ports.len(), 0);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Sparse
    );
    assert_eq!(
        design.memory_ports[0],
        eda_core::ir::MemoryPortDecl {
            name: "source".to_string(),
            direction: eda_core::ir::MemoryPortDirection::In,
            memory: 0,
        }
    );
    assert_eq!(
        design.memory_ports[1],
        eda_core::ir::MemoryPortDecl {
            name: "destination".to_string(),
            direction: eda_core::ir::MemoryPortDirection::Out,
            memory: 1,
        }
    );

    let mut simulator = Simulator::new(&design).expect("create top-memory simulator");
    let error = simulator
        .set_ordinary_array_input("source", &[2], &LogicWord::from_u64(8, 0x42))
        .expect_err("ordinary-array APIs must reject sparse memories");
    assert!(
        error.contains("array port 'source' is sparse"),
        "unexpected error: {error}"
    );
    simulator
        .set_memory_input("source", &[2], &LogicWord::from_u64(8, 0x42))
        .expect("set top memory input");
    simulator
        .set_memory_input("source", &[1], &LogicWord::from_u64(8, 0x7c))
        .expect("set second top memory input");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0x42));
    assert_eq!(
        simulator
            .read_memory_output("destination", &[3])
            .expect("read top memory output"),
        LogicWord::from_u64(8, 0x7c)
    );

    let error = simulator
        .read_memory_output("source", &[0])
        .expect_err("input memory ports cannot be read through the output API");
    assert!(error.contains("is not an output"));
}

#[test]
fn slang_ast_direct_lowering_supports_fixed_array_copies_and_size() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fixed-array-copy-size");
    let source_path = root.join("fixed_array_copy_size.sv");
    std::fs::write(
        &source_path,
        r#"
module fixed_array_copy_size(
  output int first_dimension,
  output int second_dimension,
  output int nonblocking_size,
  output logic [7:0] copied_first,
  output logic [7:0] copied_last,
  output logic [7:0] nonblocking_first,
  output logic [7:0] nonblocking_last
);
  logic [7:0] source [0:1][0:2];
  logic [7:0] destination [0:1][0:2];
  logic [7:0] nonblocking_source [0:1][0:2];
  logic [7:0] nonblocking_destination [0:1][0:2];

  initial begin
    source[0][0] = 8'h11;
    source[1][2] = 8'h66;
    destination = source;

    nonblocking_source[0][0] = 8'h7c;
    nonblocking_source[1][2] = 8'ha5;
    nonblocking_destination <= nonblocking_source;

    first_dimension = $size(source);
    second_dimension = $size(source, 2);
    nonblocking_size = $size(nonblocking_destination);
  end

  assign copied_first = destination[0][0];
  assign copied_last = destination[1][2];
  assign nonblocking_first = nonblocking_destination[0][0];
  assign nonblocking_last = nonblocking_destination[1][2];
endmodule
"#,
    )
    .expect("write fixed-array copy and size source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("fixed_array_copy_size"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fixed-array copies and size");
    assert_eq!(design.memories.len(), 4);
    assert!(design
        .memories
        .iter()
        .all(|memory| memory.storage == eda_core::ir::MemoryStorage::Ordinary));
    let fixed_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("fixed_array_copy_size"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize fixed-array AST");
    let mut fixed_summary = slang
        .summarize_ast_json(&fixed_json)
        .expect("summarize fixed-array AST");
    fixed_summary
        .validate_against_design(&design)
        .expect("exact fixed-array copy and size fingerprints should match");
    if let Some(assignment) = fixed_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "second_dimension")
            })
        })
    {
        assignment.rhs.operands[0].operator = "size".into();
    }
    let size_error = fixed_summary
        .validate_against_design(&design)
        .expect_err("wrong fixed-array dimension size must fail");
    assert!(
        size_error.contains("MemorySize:32:s[source]")
            && size_error.contains("Const:32:u[32:00000000000000000000000000000011]"),
        "unexpected fixed-array size error: {size_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create fixed-array simulator");
    simulator.run_until(0).expect("settle fixed-array copies");
    assert_eq!(simulator.read("first_dimension").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator.read("second_dimension").unwrap().to_u64(),
        Some(3)
    );
    assert_eq!(
        simulator.read("nonblocking_size").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(simulator.read("copied_first").unwrap().to_u64(), Some(0x11));
    assert_eq!(simulator.read("copied_last").unwrap().to_u64(), Some(0x66));
    assert_eq!(
        simulator.read("nonblocking_first").unwrap().to_u64(),
        Some(0x7c)
    );
    assert_eq!(
        simulator.read("nonblocking_last").unwrap().to_u64(),
        Some(0xa5)
    );

    let mismatch_path = root.join("array_copy_mismatch.sv");
    std::fs::write(
        &mismatch_path,
        r#"
module array_copy_mismatch;
  logic [7:0] source [0:3];
  logic [7:0] destination [0:4];
  initial destination = source;
endmodule
"#,
    )
    .expect("write array-copy shape mismatch source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mismatch_path),
            Some("array_copy_mismatch"),
            &PreprocessOptions::default(),
        )
        .expect_err("whole-array shape mismatches must fail closed");
    assert!(
        error.contains("cannot be assigned"),
        "unexpected error: {error}"
    );

    let differential_path = root.join("array_copy_differential.sv");
    std::fs::write(
        &differential_path,
        r#"
module array_copy_differential(
  output logic [7:0] first,
  output logic [7:0] second
);
  logic [7:0] source [0:3];
  logic [7:0] destination [0:3];
  initial begin
    source[0] = 8'h34;
    source[1] = 8'h12;
    destination = source;
  end
  assign first = destination[0];
  assign second = destination[1];
endmodule
"#,
    )
    .expect("write array-copy differential source");
    let direct_copy = slang
        .compile_files_via_ast(
            std::slice::from_ref(&differential_path),
            Some("array_copy_differential"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower array-copy differential");
    let reference_copy = slang
        .compile_files_bounded(
            std::slice::from_ref(&differential_path),
            Some("array_copy_differential"),
            &PreprocessOptions::default(),
        )
        .expect("lower array-copy differential through bounded path");
    let mut direct_copy_simulator =
        Simulator::new(&direct_copy).expect("create direct array-copy simulator");
    direct_copy_simulator
        .run_until(0)
        .expect("settle direct array copy");
    let mut reference_copy_simulator =
        Simulator::new(&reference_copy).expect("create bounded array-copy simulator");
    reference_copy_simulator
        .run_until(0)
        .expect("settle bounded array copy");
    assert_eq!(
        direct_copy_simulator.read("first").unwrap().to_u64(),
        reference_copy_simulator.read("first").unwrap().to_u64()
    );
    assert_eq!(
        direct_copy_simulator.read("second").unwrap().to_u64(),
        reference_copy_simulator.read("second").unwrap().to_u64()
    );
}

#[test]
fn slang_ast_direct_lowering_supports_fixed_array_bounds() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fixed-array-bounds");
    let source_path = root.join("fixed_array_bounds.sv");
    std::fs::write(
        &source_path,
        r#"
module fixed_array_bounds(
  output int first_left,
  output int first_right,
  output int first_low,
  output int first_high,
  output int first_increment,
  output int second_left,
  output int second_right,
  output int second_low,
  output int second_high,
  output int second_increment
);
  logic [7:0] values [2:0][1:4];

  initial begin
    first_left = $left(values);
    first_right = $right(values);
    first_low = $low(values);
    first_high = $high(values);
    first_increment = $increment(values);

    second_left = $left(values, 2);
    second_right = $right(values, 2);
    second_low = $low(values, 2);
    second_high = $high(values, 2);
    second_increment = $increment(values, 2);
  end
endmodule
"#,
    )
    .expect("write fixed-array bounds source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("fixed_array_bounds"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fixed-array bounds");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].dimensions.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create fixed-array-bounds simulator");
    simulator.run_until(0).expect("settle fixed-array bounds");
    let expected = [
        ("first_left", 2),
        ("first_right", 0),
        ("first_low", 0),
        ("first_high", 2),
        ("first_increment", u32::MAX as u64),
        ("second_left", 1),
        ("second_right", 4),
        ("second_low", 1),
        ("second_high", 4),
        ("second_increment", 1),
    ];
    for (name, value) in expected {
        assert_eq!(
            simulator.read(name).unwrap().to_u64(),
            Some(value),
            "wrong value for {name}"
        );
    }

    let invalid_path = root.join("invalid_array_dimension.sv");
    std::fs::write(
        &invalid_path,
        r#"
module invalid_array_dimension;
  logic [7:0] values [0:3];
  initial begin
    values[0] = 8'h00;
    values[1] = 8'h00;
    values[2] = 8'h00;
    values[3] = 8'h00;
    values[$left(values, 2)] = 8'hff;
  end
endmodule
"#,
    )
    .expect("write invalid fixed-array dimension source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&invalid_path),
            Some("invalid_array_dimension"),
            &PreprocessOptions::default(),
        )
        .expect_err("invalid fixed-array dimensions must fail closed");
    assert!(
        error.contains("cannot refer to element"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_array_dimension_and_bit_queries() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-array-dimension-bits");
    let source_path = root.join("array_dimension_bits.sv");
    std::fs::write(
        &source_path,
        r#"
module array_dimension_bits(
  output int array_dimensions,
  output int array_unpacked_dimensions,
  output int array_bits,
  output int packed_dimensions,
  output int packed_unpacked_dimensions,
  output int packed_bits,
  output int scalar_dimensions,
  output int scalar_unpacked_dimensions,
  output int scalar_bits
);
  logic [7:0][3:0] values [2:0][1:4];
  logic [15:0] packed_vector;
  logic scalar;

  initial begin
    array_dimensions = $dimensions(values);
    array_unpacked_dimensions = $unpacked_dimensions(values);
    array_bits = $bits(values);

    packed_dimensions = $dimensions(packed_vector);
    packed_unpacked_dimensions = $unpacked_dimensions(packed_vector);
    packed_bits = $bits(packed_vector);

    scalar_dimensions = $dimensions(scalar);
    scalar_unpacked_dimensions = $unpacked_dimensions(scalar);
    scalar_bits = $bits(scalar);
  end
endmodule
"#,
    )
    .expect("write array dimension/bit-query source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("array_dimension_bits"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower array dimension/bit queries");
    assert_eq!(design.memories.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create dimension/bit-query simulator");
    simulator
        .run_until(0)
        .expect("settle dimension/bit queries");
    let expected = [
        ("array_dimensions", 4),
        ("array_unpacked_dimensions", 2),
        ("array_bits", 384),
        ("packed_dimensions", 1),
        ("packed_unpacked_dimensions", 0),
        ("packed_bits", 16),
        ("scalar_dimensions", 0),
        ("scalar_unpacked_dimensions", 0),
        ("scalar_bits", 1),
    ];
    for (name, value) in expected {
        assert_eq!(
            simulator.read(name).unwrap().to_u64(),
            Some(value),
            "wrong value for {name}"
        );
    }

    let mismatch_path = root.join("dynamic_container_copy_mismatch.sv");
    std::fs::write(
        &mismatch_path,
        r#"
module dynamic_container_copy_mismatch;
  logic [7:0] source[];
  logic [9:0] destination[];
  initial destination = source;
endmodule
"#,
    )
    .expect("write dynamic-array copy mismatch source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mismatch_path),
            Some("dynamic_container_copy_mismatch"),
            &PreprocessOptions::default(),
        )
        .expect_err("dynamic-array element mismatches must fail closed");
    assert!(
        error.contains("slang failed")
            || error.contains(
                "whole dynamic-array assignment source and destination shapes do not match"
            ),
        "unexpected dynamic-array mismatch error: {error}"
    );

    let queue_mismatch_path = root.join("queue_copy_mismatch.sv");
    std::fs::write(
        &queue_mismatch_path,
        r#"
module queue_copy_mismatch;
  logic [7:0] source[$];
  int destination[$];
  initial destination = source;
endmodule
"#,
    )
    .expect("write queue copy mismatch source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&queue_mismatch_path),
            Some("queue_copy_mismatch"),
            &PreprocessOptions::default(),
        )
        .expect_err("queue element mismatches must fail closed");
    assert!(
        error.contains("slang failed")
            || error.contains(
                "whole queue assignment source and destination element shapes do not match"
            ),
        "unexpected queue mismatch error: {error}"
    );

    let associative_mismatch_path = root.join("associative_copy_mismatch.sv");
    std::fs::write(
        &associative_mismatch_path,
        r#"
module associative_copy_mismatch;
  logic [7:0] source[int];
  logic [7:0] destination[byte];
  initial destination = source;
endmodule
"#,
    )
    .expect("write associative-array copy mismatch source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&associative_mismatch_path),
            Some("associative_copy_mismatch"),
            &PreprocessOptions::default(),
        )
        .expect_err("associative index mismatches must fail closed");
    assert!(
        error.contains("slang failed")
            || error.contains(
                "whole associative-array assignment source and destination shapes do not match"
            ),
        "unexpected associative mismatch error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_unpacked_assignment_patterns() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-unpacked-assignment-pattern");
    let source_path = root.join("unpacked_assignment_pattern.sv");
    std::fs::write(
        &source_path,
        r#"
module unpacked_assignment_pattern(
  input logic [7:0] a,
  input logic [7:0] b,
  output logic [7:0] blocking_first,
  output logic [7:0] blocking_input,
  output logic [7:0] blocking_last,
  output logic [7:0] nonblocking_first,
  output logic [7:0] nonblocking_input,
  output logic [7:0] nonblocking_last,
  output logic [7:0] multidimensional_first,
  output logic [7:0] multidimensional_input,
  output logic [7:0] multidimensional_last,
  output logic [7:0] structured_zero,
  output logic [7:0] structured_one,
  output logic [7:0] structured_input,
  output logic [7:0] structured_last
);
  logic [7:0] blocking_values [0:2];
  logic [7:0] nonblocking_values [0:2];
  logic [7:0] multidimensional_values [2:0][1:2];
  logic [7:0] structured_values [0:3];

  initial begin
    #1;
    blocking_values = '{8'h10, a, 8'h30};
    nonblocking_values <= '{8'h40, a + 8'd1, 8'h60};
    multidimensional_values = '{
      '{8'h01, 8'h02},
      '{8'h03, a},
      '{b, 8'h06}
    };
    structured_values = '{0: 8'h71, 2: a, default: 8'h5a};
  end

  assign blocking_first = blocking_values[0];
  assign blocking_input = blocking_values[1];
  assign blocking_last = blocking_values[2];
  assign nonblocking_first = nonblocking_values[0];
  assign nonblocking_input = nonblocking_values[1];
  assign nonblocking_last = nonblocking_values[2];
  assign multidimensional_first = multidimensional_values[0][1];
  assign multidimensional_input = multidimensional_values[1][2];
  assign multidimensional_last = multidimensional_values[2][1];
  assign structured_zero = structured_values[0];
  assign structured_one = structured_values[1];
  assign structured_input = structured_values[2];
  assign structured_last = structured_values[3];
endmodule
"#,
    )
    .expect("write unpacked assignment-pattern source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("unpacked_assignment_pattern"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unpacked assignment patterns");
    assert_eq!(design.memories.len(), 4);
    assert!(design
        .memories
        .iter()
        .all(|memory| memory.storage == eda_core::ir::MemoryStorage::Ordinary));

    let mut simulator = Simulator::new(&design).expect("create assignment-pattern simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0x22))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(8, 0x44))
        .unwrap();
    simulator.run_until(1).expect("settle assignment patterns");

    let expected = [
        ("blocking_first", 0x10),
        ("blocking_input", 0x22),
        ("blocking_last", 0x30),
        ("nonblocking_first", 0x40),
        ("nonblocking_input", 0x23),
        ("nonblocking_last", 0x60),
        ("multidimensional_first", 0x44),
        ("multidimensional_input", 0x22),
        ("multidimensional_last", 0x01),
        ("structured_zero", 0x71),
        ("structured_one", 0x5a),
        ("structured_input", 0x22),
        ("structured_last", 0x5a),
    ];
    for (name, value) in expected {
        assert_eq!(
            simulator.read(name).unwrap().to_u64(),
            Some(value),
            "wrong value for {name}"
        );
    }

    let nonblocking_path = root.join("nonblocking_traversal.sv");
    std::fs::write(
        &nonblocking_path,
        r#"
module nonblocking_traversal;
  logic [7:0] values[int];
  int key;
  int status;
  initial status <= values.first(key);
endmodule
"#,
    )
    .expect("write nonblocking traversal source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nonblocking_path),
            Some("nonblocking_traversal"),
            &PreprocessOptions::default(),
        )
        .expect_err("associative traversal must reject nonblocking assignment");
    assert!(
        error.contains("associative-array first is not supported as a nonblocking assignment"),
        "unexpected nonblocking error: {error}"
    );

    let mismatch_path = root.join("traversal_key_mismatch.sv");
    std::fs::write(
        &mismatch_path,
        r#"
module traversal_key_mismatch;
  logic [7:0] values[int];
  byte key;
  int status;
  initial status = values.first(key);
endmodule
"#,
    )
    .expect("write traversal key mismatch source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&mismatch_path),
            Some("traversal_key_mismatch"),
            &PreprocessOptions::default(),
        )
        .expect_err("associative traversal key width mismatches must fail closed");
    assert!(
        error.contains("slang failed")
            || error.contains("associative-array first key width 8 does not match index width 32"),
        "unexpected key mismatch error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_associative_array_traversal() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-associative-traversal");
    let source_path = root.join("associative_traversal.sv");
    std::fs::write(
        &source_path,
        r#"
module associative_traversal(
  output int first_status,
  output int next_status,
  output int next_last_status,
  output int next_failure_status,
  output int prev_status,
  output int last_status,
  output int empty_status,
  output int cursor_final,
  output int empty_cursor_final,
  output int cursor_after_first,
  output int cursor_after_next,
  output int cursor_after_prev
);
  logic [7:0] values[int];
  logic [7:0] empty[int];
  int cursor;
  int empty_cursor;

  initial begin
    #1;
    values[-3] = 8'h10;
    values[2] = 8'h21;
    values[7] = 8'h32;

    first_status = values.first(cursor);
    cursor_after_first = cursor;
    next_status = values.next(cursor);
    cursor_after_next = cursor;
    next_last_status = values.next(cursor);
    next_failure_status = values.next(cursor);
    prev_status = values.prev(cursor);
    cursor_after_prev = cursor;
    last_status = values.last(cursor);
    cursor_final = cursor;

    empty_cursor = 0;
    empty_status = empty.first(empty_cursor);
    empty_cursor_final = empty_cursor;
  end
endmodule
"#,
    )
    .expect("write associative traversal source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("associative_traversal"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower associative traversal");
    assert_eq!(design.memories.len(), 2);
    let traversal_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("associative_traversal"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize associative-traversal AST");
    let mut traversal_summary = slang
        .summarize_ast_json(&traversal_json)
        .expect("summarize associative-traversal AST");
    traversal_summary
        .validate_against_design(&design)
        .expect("exact associative-traversal fingerprint should match");
    if let Some(assignment) = traversal_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "first_status")
            })
        })
    {
        let original = assignment.rhs.operator.clone();
        assignment.rhs.operator = original
            .split_once(' ')
            .map(|(receiver, _)| format!("{receiver} last"))
            .unwrap_or_else(|| "last".into());
    }
    let traversal_error = traversal_summary
        .validate_against_design(&design)
        .expect_err("wrong associative-traversal direction must fail");
    assert!(
        traversal_error.contains("AssocTraversal:Last:u[values,"),
        "unexpected traversal structural error: {traversal_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create associative-traversal simulator");
    simulator
        .run_until(1)
        .expect("execute associative traversal");
    let expected = [
        ("first_status", 1),
        ("next_status", 1),
        ("next_last_status", 1),
        ("next_failure_status", 0),
        ("prev_status", 1),
        ("last_status", 1),
        ("empty_status", 0),
        ("cursor_final", 7),
        ("empty_cursor_final", 0),
        ("cursor_after_first", u32::MAX as u64 - 2),
        ("cursor_after_next", 2),
        ("cursor_after_prev", 2),
    ];
    for (name, value) in expected {
        assert_eq!(
            simulator.read(name).unwrap().to_u64(),
            Some(value),
            "wrong value for {name}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_container_copies() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-container-copies");
    let source_path = root.join("dynamic_container_copies.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_container_copies(
  output int dynamic_size,
  output int nonblocking_dynamic_size,
  output int queue_size,
  output int bounded_queue_size,
  output int associative_count,
  output int nonblocking_associative_count,
  output logic [7:0] dynamic_first,
  output logic [7:0] dynamic_last,
  output logic [7:0] nonblocking_dynamic_first,
  output logic [7:0] nonblocking_dynamic_last,
  output logic [7:0] queue_first,
  output logic [7:0] queue_last,
  output logic [7:0] bounded_queue_first,
  output logic [7:0] bounded_queue_last,
  output logic [7:0] associative_first,
  output logic [7:0] associative_second,
  output logic [7:0] nonblocking_associative_first,
  output logic [7:0] nonblocking_associative_second
);
  logic [7:0] dynamic_source[];
  logic [7:0] dynamic_destination[];
  logic [7:0] nonblocking_dynamic_source[];
  logic [7:0] nonblocking_dynamic_destination[];
  logic [7:0] queue_source[$];
  logic [7:0] queue_destination[$];
  logic [7:0] bounded_queue_destination[$:1];
  logic [7:0] associative_source[int];
  logic [7:0] associative_destination[int];
  logic [7:0] nonblocking_associative_source[int];
  logic [7:0] nonblocking_associative_destination[int];

  initial begin
    #1;
    dynamic_source = new[3];
    dynamic_source[0] = 8'h10;
    dynamic_source[1] = 8'h21;
    dynamic_source[2] = 8'h32;
    dynamic_destination = dynamic_source;

    nonblocking_dynamic_source = new[2];
    nonblocking_dynamic_source[0] = 8'h40;
    nonblocking_dynamic_source[1] = 8'd81;
    nonblocking_dynamic_destination <= nonblocking_dynamic_source;

    queue_source.push_back(8'h50);
    queue_source.push_back(8'd91);
    queue_source.push_back(8'ha2);
    queue_destination = queue_source;
    bounded_queue_destination = queue_source;

    associative_source[41] = 8'h60;
    associative_source[42] = 8'ha1;
    associative_destination = associative_source;

    nonblocking_associative_source[43] = 8'h70;
    nonblocking_associative_source[44] = 8'b1;
    nonblocking_associative_destination <= nonblocking_associative_source;
  end

  initial begin
    #2;
    dynamic_size = dynamic_destination.size();
    nonblocking_dynamic_size = nonblocking_dynamic_destination.size();
    queue_size = queue_destination.size();
    bounded_queue_size = bounded_queue_destination.size();
    associative_count = associative_destination.num();
    nonblocking_associative_count = nonblocking_associative_destination.num();
    dynamic_first = dynamic_destination[0];
    dynamic_last = dynamic_destination[2];
    nonblocking_dynamic_first = nonblocking_dynamic_destination[0];
    nonblocking_dynamic_last = nonblocking_dynamic_destination[1];
    queue_first = queue_destination[0];
    queue_last = queue_destination[2];
    bounded_queue_first = bounded_queue_destination[0];
    bounded_queue_last = bounded_queue_destination[1];
    associative_first = associative_destination[41];
    associative_second = associative_destination[42];
    nonblocking_associative_first = nonblocking_associative_destination[43];
    nonblocking_associative_second = nonblocking_associative_destination[44];
  end
endmodule
"#,
    )
    .expect("write dynamic-container copy source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_container_copies"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic-container copies");
    assert_eq!(design.memories.len(), 11);
    let copy_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("dynamic_container_copies"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize dynamic-container copy AST");
    let mut copy_summary = slang
        .summarize_ast_json(&copy_json)
        .expect("summarize dynamic-container copy AST");
    copy_summary
        .validate_against_design(&design)
        .expect("exact dynamic-container copy fingerprint should match");
    if let Some(assignment) = copy_summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .target_names
                .first()
                .is_some_and(|name| name == "dynamic_destination")
        })
    }) {
        assignment.rhs.symbol = Some("wrong_dynamic_source".into());
    }
    let copy_error = copy_summary
        .validate_against_design(&design)
        .expect_err("wrong dynamic-container copy source must fail");
    assert!(
        copy_error.contains("MemoryCopy:wrong_dynamic_source")
            && copy_error.contains("MemoryCopy:dynamic_source"),
        "unexpected dynamic-container copy error: {copy_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create dynamic-container-copy simulator");
    simulator
        .run_until(2)
        .expect("execute dynamic-container copies");
    let expected = [
        ("dynamic_size", 3),
        ("nonblocking_dynamic_size", 2),
        ("queue_size", 3),
        ("bounded_queue_size", 2),
        ("associative_count", 2),
        ("nonblocking_associative_count", 2),
        ("dynamic_first", 0x10),
        ("dynamic_last", 0x32),
        ("nonblocking_dynamic_first", 0x40),
        ("nonblocking_dynamic_last", 81),
        ("queue_first", 0x50),
        ("queue_last", 0xa2),
        ("bounded_queue_first", 0x50),
        ("bounded_queue_last", 91),
        ("associative_first", 0x60),
        ("associative_second", 0xa1),
        ("nonblocking_associative_first", 0x70),
        ("nonblocking_associative_second", 1),
    ];
    for (name, value) in expected {
        assert_eq!(
            simulator.read(name).unwrap().to_u64(),
            Some(value),
            "wrong value for {name}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_array_lifecycle() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-array-lifecycle");
    let source_path = root.join("dynamic_array_lifecycle.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_array_lifecycle(
  input int requested_size,
  input logic [7:0] data,
  output int blocking_size,
  output int system_size,
  output int array_bits,
  output int nonblocking_size,
  output int deleted_size,
  output logic [7:0] first,
  output logic [7:0] last,
  output logic [7:0] out_of_bounds
);
  logic [7:0] blocking_values[];
  logic [7:0] nonblocking_values[];

  initial begin
    #1;
    blocking_values = new[requested_size];
    blocking_size = blocking_values.size();
    system_size = $size(blocking_values);
    array_bits = $bits(blocking_values);
    blocking_values[0] = data + 8'd1;
    blocking_values[requested_size - 1] = data + 8'd2;
    first = blocking_values[0];
    last = blocking_values[requested_size - 1];
    out_of_bounds = blocking_values[requested_size];

    nonblocking_values <= new[3];
    #1;
    nonblocking_size = nonblocking_values.size();

    blocking_values.delete();
    deleted_size = blocking_values.size();
  end
endmodule
"#,
    )
    .expect("write dynamic-array lifecycle source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_array_lifecycle"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower dynamic-array lifecycle");
    assert_eq!(design.memories.len(), 2);
    assert!(design.memories.iter().all(|memory| memory.dynamic));
    assert_eq!(design.memories[0].flat_length, 256);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::Unmapped
    );
    let dynamic_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("dynamic_array_lifecycle"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize dynamic-array lifecycle AST");
    let mut dynamic_summary = slang
        .summarize_ast_json(&dynamic_json)
        .expect("summarize dynamic-array lifecycle AST");
    dynamic_summary
        .validate_against_design(&design)
        .expect("exact dynamic-array allocation fingerprint should match");
    if let Some(assignment) = dynamic_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "blocking_values")
                    && assignment.rhs.kind == "NewArray"
            })
        })
    {
        assignment.rhs.operands[0].symbol = Some("wrong_requested_size".into());
    }
    let allocation_error = dynamic_summary
        .validate_against_design(&design)
        .expect_err("wrong dynamic-array allocation size must fail");
    assert!(
        allocation_error.contains("MemoryNew:Ref:32:s[wrong_requested_size]")
            && allocation_error.contains("MemoryNew:Ref:32:s[requested_size]"),
        "unexpected allocation error: {allocation_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create dynamic-array simulator");
    simulator
        .set_input("requested_size", &LogicWord::from_u64(32, 4))
        .expect("set dynamic-array size");
    simulator
        .set_input("data", &LogicWord::from_u64(8, 0x20))
        .expect("set dynamic-array data");
    simulator.run_until(2).expect("run dynamic-array lifecycle");

    assert_eq!(simulator.read("blocking_size").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("system_size").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("array_bits").unwrap().to_u64(), Some(32));
    assert_eq!(
        simulator.read("nonblocking_size").unwrap().to_u64(),
        Some(3)
    );
    assert_eq!(simulator.read("deleted_size").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(0x21));
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(0x22));
    assert_eq!(simulator.read("out_of_bounds").unwrap().to_u64(), None);
}

#[test]
fn slang_ast_direct_lowering_supports_queue_lifecycle() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-queue-lifecycle");
    let source_path = root.join("queue_lifecycle.sv");
    std::fs::write(
        &source_path,
        r#"
module queue_lifecycle(
  input logic [7:0] data,
  output int initial_size,
  output int pushed_size,
  output int popped_size,
  output int deleted_size,
  output int bounded_size,
  output int queue_bits,
  output logic [7:0] first,
  output logic [7:0] second,
  output logic [7:0] front,
  output logic [7:0] popped,
  output logic [7:0] back,
  output int back_size,
  output logic [7:0] empty_pop,
  output int inserted_size,
  output logic [7:0] inserted_first,
  output logic [7:0] inserted_second,
  output logic [7:0] inserted_third,
  output logic [7:0] deleted_first,
  output logic [7:0] deleted_second,
  output int invalid_insert_size,
  output int invalid_delete_size
);
  logic [7:0] values[$];
  logic [7:0] bounded[$:3];
  logic [7:0] empty[$];
  logic [7:0] methods[$];

  initial begin
    #1;
    initial_size = values.size();
    values.push_back(data);
    values.push_back(data + 8'd1);
    values.push_front(data + 8'd2);
    pushed_size = values.size();
    queue_bits = $bits(values);
    first = values[0];
    second = values[1];
    front = values.pop_front();
    popped_size = values.size();
    popped = values.pop_front();
    back = values.pop_back();
    back_size = values.size();
    values.delete();
    deleted_size = values.size();
    empty_pop = empty.pop_front();

    bounded.push_back(data);
    bounded.push_back(data + 8'd3);
    bounded_size = bounded.size();

    methods.push_back(data);
    methods.push_back(data + 8'd1);
    methods.insert(1, data + 8'd2);
    inserted_size = methods.size();
    inserted_first = methods[0];
    inserted_second = methods[1];
    inserted_third = methods[2];
    methods.delete(0);
    deleted_size = methods.size();
    deleted_first = methods[0];
    deleted_second = methods[1];
    methods.insert(-1, data + 8'd3);
    invalid_insert_size = methods.size();
    methods.delete(2);
    invalid_delete_size = methods.size();
  end
endmodule
"#,
    )
    .expect("write queue lifecycle source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("queue_lifecycle"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower queue lifecycle");
    assert_eq!(design.memories.len(), 4);
    assert!(design.memories.iter().all(|memory| memory.queue));
    assert!(design.memories.iter().all(|memory| memory.dynamic));
    assert_eq!(design.memories[0].flat_length, 256);
    assert_eq!(design.memories[1].flat_length, 4);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::Unmapped
    );
    let queue_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("queue_lifecycle"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize queue lifecycle AST");
    let mut queue_summary = slang
        .summarize_ast_json(&queue_json)
        .expect("summarize queue lifecycle AST");
    queue_summary
        .validate_against_design(&design)
        .expect("exact queue method fingerprint should match");
    let mut pop_summary = queue_summary.clone();
    if let Some(assignment) = queue_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "back_size")
            })
        })
    {
        assignment.rhs.operands[0].symbol = Some("0 bounded".into());
    }
    let queue_method_error = queue_summary
        .validate_against_design(&design)
        .expect_err("wrong queue size receiver must fail");
    assert!(
        queue_method_error.contains("MemorySize:32:s[bounded]")
            && queue_method_error.contains("MemorySize:32:s[values]"),
        "unexpected queue method error: {queue_method_error}"
    );
    if let Some(assignment) = pop_summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .target_names
                .first()
                .is_some_and(|name| name == "front")
        })
    }) {
        assignment.rhs.operands[0].symbol = Some("0 bounded".into());
    }
    let pop_error = pop_summary
        .validate_against_design(&design)
        .expect_err("wrong queue pop receiver must fail");
    assert!(
        pop_error.contains("QueuePop:true:bounded") && pop_error.contains("QueuePop:true:values"),
        "unexpected queue pop error: {pop_error}"
    );
    let mut simulator = Simulator::new(&design).expect("create queue simulator");
    simulator
        .set_input("data", &LogicWord::from_u64(8, 0x20))
        .expect("set queue data");
    simulator.run_until(1).expect("run queue lifecycle");

    assert_eq!(simulator.read("initial_size").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("pushed_size").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("queue_bits").unwrap().to_u64(), Some(24));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(0x22));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0x20));
    assert_eq!(simulator.read("front").unwrap().to_u64(), Some(0x22));
    assert_eq!(simulator.read("popped_size").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("popped").unwrap().to_u64(), Some(0x20));
    assert_eq!(simulator.read("back").unwrap().to_u64(), Some(0x21));
    assert_eq!(simulator.read("back_size").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("empty_pop").unwrap().to_u64(), None);
    assert_eq!(simulator.read("bounded_size").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("inserted_size").unwrap().to_u64(), Some(3));
    assert_eq!(
        simulator.read("inserted_first").unwrap().to_u64(),
        Some(0x20)
    );
    assert_eq!(
        simulator.read("inserted_second").unwrap().to_u64(),
        Some(0x22)
    );
    assert_eq!(
        simulator.read("inserted_third").unwrap().to_u64(),
        Some(0x21)
    );
    assert_eq!(simulator.read("deleted_size").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator.read("deleted_first").unwrap().to_u64(),
        Some(0x22)
    );
    assert_eq!(
        simulator.read("deleted_second").unwrap().to_u64(),
        Some(0x21)
    );
    assert_eq!(
        simulator.read("invalid_insert_size").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(
        simulator.read("invalid_delete_size").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_ast_direct_lowering_discards_bounded_queue_overflow() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-queue-overflow");
    let source_path = root.join("queue_overflow.sv");
    std::fs::write(
        &source_path,
        r#"
module queue_overflow(
  input logic [7:0] data,
  output int back_overflow_size,
  output int front_overflow_size,
  output int inserted_overflow_size,
  output logic [7:0] inserted_first,
  output logic [7:0] inserted_last,
  output logic [7:0] first,
  output logic [7:0] last
);
  logic [7:0] values[$:1];

  initial begin
    #1;
    values.push_back(data);
    values.push_back(data + 8'd1);
    back_overflow_size = values.size();
    values.insert(1, data + 8'd3);
    inserted_overflow_size = values.size();
    inserted_first = values[0];
    inserted_last = values[1];
    values.push_front(data + 8'd4);
    front_overflow_size = values.size();
    first = values[0];
    last = values[1];
  end
endmodule
"#,
    )
    .expect("write bounded queue overflow source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("queue_overflow"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower bounded queue overflow");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].flat_length, 2);

    let mut simulator = Simulator::new(&design).expect("create queue overflow simulator");
    simulator
        .set_input("data", &LogicWord::from_u64(8, 0x20))
        .expect("set queue overflow data");
    simulator
        .run_until(1)
        .expect("bounded queue overflow follows discard semantics");
    assert_eq!(
        simulator.read("back_overflow_size").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(
        simulator.read("front_overflow_size").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(
        simulator.read("inserted_overflow_size").unwrap().to_u64(),
        Some(2)
    );
    assert_eq!(
        simulator.read("inserted_first").unwrap().to_u64(),
        Some(0x20)
    );
    assert_eq!(
        simulator.read("inserted_last").unwrap().to_u64(),
        Some(0x23)
    );
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(0x24));
    assert_eq!(simulator.read("last").unwrap().to_u64(), Some(0x20));
}

#[test]
fn slang_ast_direct_lowering_supports_associative_array_lifecycle() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-associative-array-lifecycle");
    let source_path = root.join("associative_array_lifecycle.sv");
    std::fs::write(
        &source_path,
        r#"
module associative_array_lifecycle(
  input int key,
  input logic [7:0] value,
  output int initial_count,
  output int written_count,
  output int system_size,
  output int array_bits,
  output int exists_before,
  output int exists_after,
  output int deleted_count,
  output int cleared_count,
  output logic [7:0] hit,
  output logic [7:0] missing,
  output logic [7:0] unused
);
  logic [7:0] values [int];

  initial begin
    #1;
    initial_count = values.num();
    values[key] = value;
    values[key + 1] = value + 8'd1;
    written_count = values.num();
    system_size = $size(values);
    array_bits = $bits(values);
    exists_before = values.exists(key);
    hit = values[key];
    missing = values[key + 99];

    values.delete(key);
    exists_after = values.exists(key);
    deleted_count = values.num();
    values.delete();
    cleared_count = values.num();
  end
endmodule
"#,
    )
    .expect("write associative-array lifecycle source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("associative_array_lifecycle"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower associative-array lifecycle");
    assert_eq!(design.memories.len(), 1);
    assert!(design
        .memories
        .iter()
        .all(|memory| memory.associative && memory.dynamic && !memory.queue));
    assert_eq!(design.memories[0].flat_length, 256);
    assert_eq!(design.memories[0].index_width, 32);
    assert!(design.memories[0].index_signed);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Ordinary
    );
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::Unmapped
    );
    let associative_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("associative_array_lifecycle"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize associative lifecycle AST");
    let mut associative_summary = slang
        .summarize_ast_json(&associative_json)
        .expect("summarize associative lifecycle AST");
    associative_summary
        .validate_against_design(&design)
        .expect("exact associative method fingerprint should match");
    if let Some(assignment) = associative_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "exists_before")
            })
        })
    {
        assignment.rhs.operands[1].symbol = Some("wrong_key".into());
    }
    let associative_method_error = associative_summary
        .validate_against_design(&design)
        .expect_err("wrong associative exists key must fail");
    assert!(
        associative_method_error.contains("Ref:32:s[wrong_key]")
            && associative_method_error.contains("Ref:32:s[key]"),
        "unexpected associative method error: {associative_method_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create associative simulator");
    simulator
        .set_input("key", &LogicWord::from_u64(32, 10))
        .expect("set associative key");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0x30))
        .expect("set associative value");
    simulator.run_until(1).expect("run associative lifecycle");

    assert_eq!(simulator.read("initial_count").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("written_count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("system_size").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("array_bits").unwrap().to_u64(), Some(16));
    assert_eq!(simulator.read("exists_before").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("hit").unwrap().to_u64(), Some(0x30));
    assert_eq!(simulator.read("missing").unwrap().to_u64(), None);
    assert_eq!(simulator.read("exists_after").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("deleted_count").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("cleared_count").unwrap().to_u64(), Some(0));

    let full_path = root.join("associative_array_full.sv");
    std::fs::write(
        &full_path,
        r#"
module associative_array_full;
  logic [7:0] values [int];
  initial begin
    for (int index = 0; index < 257; index = index + 1) begin
      values[index] = 8'h00;
    end
  end
endmodule
"#,
    )
    .expect("write full associative-array source");
    let full = slang
        .compile_files_via_ast(
            std::slice::from_ref(&full_path),
            Some("associative_array_full"),
            &PreprocessOptions::default(),
        )
        .expect("lower full associative-array source");
    let mut full_simulator = Simulator::new(&full).expect("create full associative simulator");
    let error = full_simulator
        .run_until(0)
        .expect_err("inserting 257 keys must fail closed");
    assert!(
        error.contains("associative array 'values' is full; capacity is 256"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_top_level_inout_memory_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-top-inout-memory-port");
    let source_path = root.join("top_inout_memory_port.sv");
    std::fs::write(
        &source_path,
        r#"
module top(
  inout wire logic [7:0] bus [0:3],
  output logic [7:0] observed
);
  assign observed = bus[2];
  assign bus[3] = bus[1];
endmodule
"#,
    )
    .expect("write top-level inout sparse-memory port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower top-level inout sparse-memory port");
    assert_eq!(design.memory_ports.len(), 1);
    assert_eq!(
        design.memory_ports[0],
        eda_core::ir::MemoryPortDecl {
            name: "bus".to_string(),
            direction: eda_core::ir::MemoryPortDirection::InOut,
            memory: 0,
        }
    );

    let mut simulator = Simulator::new(&design).expect("create top-inout-memory simulator");
    let invalid_bulk = vec![
        (vec![0], LogicWord::from_u64(8, 0xaa)),
        (vec![99], LogicWord::from_u64(8, 0xbb)),
    ];
    let error = simulator
        .set_memory_input_bulk("bus", &invalid_bulk)
        .expect_err("bulk memory stimulus must validate every index before writing");
    assert!(error.contains("index is out of range"));
    assert_eq!(
        simulator
            .read_memory_output("bus", &[0])
            .expect("failed bulk update must not write the valid prefix"),
        LogicWord::xes(8)
    );

    let stimulus = vec![
        (vec![2], LogicWord::from_u64(8, 0x42)),
        (vec![1], LogicWord::from_u64(8, 0x7c)),
    ];
    simulator
        .set_memory_input_bulk("bus", &stimulus)
        .expect("bulk-set top-level inout memory");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0x42));
    let readback = simulator
        .read_memory_output_bulk("bus", &[vec![3], vec![2]])
        .expect("bulk-read top-level inout memory");
    assert_eq!(
        readback,
        vec![LogicWord::from_u64(8, 0x7c), LogicWord::from_u64(8, 0x42)]
    );
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_memory_ranges() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-multidimensional-memory-range");
    let source_path = root.join("multidimensional_memory_range.sv");
    std::fs::write(
        &source_path,
        r#"
module top(
  inout wire logic [7:0] bus [0:1][0:2],
  output logic [7:0] observed
);
  assign observed = bus[1][2];
  assign bus[1][1] = bus[0][1];
endmodule
"#,
    )
    .expect("write multidimensional memory-range source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional memory range");
    assert_eq!(design.memory_ports.len(), 1);
    assert_eq!(design.memories[0].dimensions.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create multidimensional-memory simulator");
    let error = simulator
        .set_memory_input_range(
            "bus",
            &[(0, 1), (1, 99)],
            &[
                LogicWord::from_u64(8, 1),
                LogicWord::from_u64(8, 2),
                LogicWord::from_u64(8, 3),
                LogicWord::from_u64(8, 4),
            ],
        )
        .expect_err("multidimensional ranges must validate every bound before writing");
    assert!(error.contains("range is out of bounds"));
    assert_eq!(
        simulator
            .read_memory_output("bus", &[0, 1])
            .expect("failed multidimensional range must not write"),
        LogicWord::xes(8)
    );

    simulator
        .set_memory_input_range(
            "bus",
            &[(0, 1), (1, 2)],
            &[
                LogicWord::from_u64(8, 0x11),
                LogicWord::from_u64(8, 0x12),
                LogicWord::from_u64(8, 0x14),
                LogicWord::from_u64(8, 0x15),
            ],
        )
        .expect("set multidimensional memory range");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0x15));
    let values = simulator
        .read_memory_output_range("bus", &[(0, 1), (1, 2)])
        .expect("read multidimensional memory range");
    let expected = [0x11, 0x12, 0x11, 0x15]
        .iter()
        .map(|value| LogicWord::from_u64(8, *value))
        .collect::<Vec<_>>();
    assert_eq!(values, expected);
    assert_eq!(
        simulator
            .read_memory_output("bus", &[1, 1])
            .expect("read aliased multidimensional element"),
        LogicWord::from_u64(8, 0x11)
    );
    assert_eq!(
        simulator
            .read_memory_output_range("bus", &[(1, 1), (2, 2)])
            .expect("read singleton multidimensional range"),
        vec![LogicWord::from_u64(8, 0x15)]
    );
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("readmem"),
            &PreprocessOptions::default(),
        )
        .expect("lower memory files through the bounded path");
    assert_eq!(design.memories.len(), 2);
    assert_eq!(
        design.memories[0].storage,
        eda_core::ir::MemoryStorage::Sparse
    );
    assert_eq!(
        design.memories[1].storage,
        eda_core::ir::MemoryStorage::Sparse
    );
    assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(design.memories[1].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(
        design.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        design.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        design.memories[0].banking,
        eda_core::ir::MemoryBanking {
            count: 1,
            depth: 4097
        }
    );
    assert_eq!(design.initials.len(), 1);
    assert_eq!(reference.memories.len(), 2);
    assert_eq!(reference.memories[0].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(reference.memories[1].kind, eda_core::ir::MemoryKind::Rom);
    assert_eq!(
        reference.memories[0].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        reference.memories[1].template,
        eda_core::ir::MemoryTemplate::AsyncRom
    );
    assert_eq!(
        reference.memories[1].banking,
        eda_core::ir::MemoryBanking {
            count: 1,
            depth: 2049
        }
    );
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
        .compile_files_bounded(
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
fn slang_and_bounded_paths_infer_fixed_partial_write_masks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("fixed-write-mask");
    let source_path = root.join("write-mask.sv");
    std::fs::write(
        &source_path,
        r#"
module write_mask(
  input logic clk,
  input logic [3:0] din,
  output logic [15:0] q
);
  logic [15:0] mem [0:4095];
  always @(posedge clk) mem[0][3:0] <= din;
  assign q = mem[0];
endmodule
"#,
    )
    .expect("write fixed memory mask source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("write_mask"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fixed memory mask");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("write_mask"),
            &PreprocessOptions::default(),
        )
        .expect("lower fixed memory mask through bounded path");
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Ram);
        assert_eq!(
            design.memories[0].template,
            eda_core::ir::MemoryTemplate::SyncRam
        );
        assert_eq!(
            design.memories[0].write_mask,
            Some(eda_core::ir::MemoryWriteMask {
                width: 16,
                bits: [
                    true, true, true, true, false, false, false, false, false, false, false, false,
                    false, false, false, false
                ]
                .to_vec()
            })
        );
        assert_eq!(design.memories[0].byte_enable, None);
    }
}

#[test]
fn slang_and_bounded_paths_infer_aligned_byte_enables() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("aligned-byte-enable");
    let source_path = root.join("byte-enable.sv");
    std::fs::write(
        &source_path,
        r#"
module byte_enable(
  input logic clk,
  input logic [7:0] din,
  output logic [31:0] q
);
  logic [31:0] mem [0:4095];
  always @(posedge clk) mem[0][23:16] <= din;
  assign q = mem[0];
endmodule
"#,
    )
    .expect("write aligned byte-enable source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("byte_enable"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower aligned byte enable");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("byte_enable"),
            &PreprocessOptions::default(),
        )
        .expect("lower aligned byte enable through bounded path");
    let expected = eda_core::ir::MemoryByteEnable {
        lane_width: 8,
        lanes: [false, false, true, false].to_vec(),
    };
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].byte_enable, Some(expected.clone()));
    }
}

#[test]
fn slang_and_bounded_paths_infer_synchronous_rom_template() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("synchronous-rom-template");
    let source_path = root.join("sync-rom.sv");
    std::fs::write(
        &source_path,
        r#"
module sync_rom(
  input logic clk,
  input logic [11:0] address,
  output logic [7:0] q
);
  logic [7:0] rom [0:4095];
  initial $readmemh("rom.hex", rom);
  always @(posedge clk) q <= rom[address];
endmodule
"#,
    )
    .expect("write synchronous ROM source");
    std::fs::write(root.join("rom.hex"), "01 22 44 88\n").expect("write ROM data");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("sync_rom"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower synchronous ROM");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("sync_rom"),
            &PreprocessOptions::default(),
        )
        .expect("lower synchronous ROM through bounded path");
    for design in [&direct, &bounded] {
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].kind, eda_core::ir::MemoryKind::Rom);
        assert_eq!(
            design.memories[0].template,
            eda_core::ir::MemoryTemplate::SyncRom
        );

        let mut simulator = Simulator::new(design).expect("create synchronous ROM simulator");
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .expect("initialize ROM clock");
        simulator
            .set_input("address", &LogicWord::from_u64(12, 2))
            .expect("select initialized ROM word");
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .expect("create synchronous ROM read edge");
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0x44));
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
        .compile_files_bounded(
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
        .compile_files_bounded(
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
        .validate_against_design_legacy_compatibility(&design)
        .expect("slang and bounded design inventories agree");

    let mut dropped_port = summary.clone();
    dropped_port.ports.pop();
    assert!(dropped_port
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
                type_name: String::new(),
                operator: "Add".into(),
                selection_kind: String::new(),
                type_width: None,
                type_signed: None,
                symbol: None,
                literal: None,
                repeat_count: None,
                simulation_time_unit: None,
                enum_definition: None,
                operands: Vec::new(),
            },
        });
    assert!(dropped_continuous
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
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
        .validate_against_design_legacy_compatibility(&design)
        .unwrap_err()
        .contains("continuous assignment shape for 'lit' does not match slang"));

    let mut dropped_instance = summary;
    dropped_instance.instances.pop();
    assert!(dropped_instance
        .validate_against_design_legacy_compatibility(&design)
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
    assert!(design.instance("top.selected.u").is_some());
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
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated members through the preprocessed path");
    assert_eq!(design.n_nets, 5);
    assert_eq!(design.assigns.len(), 4);
    assert_eq!(reference.n_nets, design.n_nets);
    assert_eq!(reference.assigns.len(), design.assigns.len());
    assert!(design
        .hierarchy
        .iter()
        .any(|instance| instance.path == "top.selected.u"));
    // The bounded compatibility path intentionally flattens generate labels;
    // compare behavior rather than requiring identical hierarchy names.
    let mut reference_simulator = Simulator::new(&reference).expect("create reference simulator");
    reference_simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set reference generate input");

    let mut simulator = Simulator::new(&design).expect("create generate simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set generate input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(0));
    assert_eq!(reference_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(reference_simulator.read("z").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_accepts_generate_for_assignments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generate-for");
    let source_path = root.join("generate_for.sv");
    std::fs::write(
        &source_path,
        r#"
module generate_for(
  input logic a,
  output logic [3:0] forward,
  output logic [3:0] reverse
);
  for (genvar i = 0; i < 4; i = i + 1) begin : iteration
    assign forward[i] = a ^ i[0];
    assign reverse[3 - i] = a ^ i[1];
  end
endmodule
"#,
    )
    .expect("write generate-for source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generate_for"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated assignments");
    assert_eq!(design.assigns.len(), 8);
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("generate_for"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize generated assignment AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize generated assignment AST");
    summary
        .validate_against_design(&design)
        .expect("exact generated constant bit-select fingerprints should match");
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "forward")
    {
        target.rhs.operands[1].operands[0].literal =
            Some("32:00000000000000000000000000000001".into());
    }
    let wrong_constant_error = summary
        .validate_against_design(&design)
        .expect_err("wrong generated genvar constant must fail");
    assert!(
        wrong_constant_error.contains("Const:32:u[32:00000000000000000000000000000000]")
            && wrong_constant_error.contains("Const:32:u[32:00000000000000000000000000000001]"),
        "unexpected generated genvar constant error: {wrong_constant_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "forward")
    {
        target.rhs.operands[1].operands[0].literal =
            Some("32:00000000000000000000000000000000".into());
        target.rhs.operands[1].operands[1].literal =
            Some("32:00000000000000000000000000000001".into());
    }
    let wrong_selector_error = summary
        .validate_against_design(&design)
        .expect_err("wrong generated constant bit selector must fail");
    assert!(
        wrong_selector_error.contains("Range:0:0") && wrong_selector_error.contains("Range:1:1"),
        "unexpected generated bit selector error: {wrong_selector_error}"
    );
    if let Some(target) = summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "forward")
    {
        target.rhs.operands[1].operands[1].literal =
            Some("32:00000000000000000000000000000000".into());
    }
    let mut simulator = Simulator::new(&design).expect("create generate simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("forward").unwrap().to_u64(), Some(0b0101));
    assert_eq!(simulator.read("reverse").unwrap().to_u64(), Some(0b1100));
}

#[test]
fn slang_ast_direct_lowering_accepts_generate_for_instances() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generate-instances");
    let source_path = root.join("generate_instances.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module generate_instances(input logic a, output logic [3:0] y);
  for (genvar i = 0; i < 4; i = i + 1) begin : iteration
    child u(.a(a), .y(y[i]));
  end
endmodule
"#,
    )
    .expect("write generated-instance source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generate_instances"),
            &PreprocessOptions::default(),
        )
        .expect("lower generated instances");
    assert_eq!(design.assigns.len(), 12);
    assert_eq!(design.hierarchy.len(), 5);
    let child_paths = design
        .hierarchy
        .iter()
        .filter(|instance| instance.depth == 1)
        .map(|instance| instance.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        child_paths,
        vec![
            "generate_instances.iteration[0].u",
            "generate_instances.iteration[1].u",
            "generate_instances.iteration[2].u",
            "generate_instances.iteration[3].u",
        ]
    );

    let mut simulator = Simulator::new(&design).expect("create generated-instance simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_preserves_continuous_assignment_delay() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-delay");
    let source_path = root.join("delay.sv");
    std::fs::write(
        &source_path,
        r#"
module delay(input logic a, output logic y);
  assign #5 y = a;
endmodule
"#,
    )
    .expect("write delayed assignment source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("delay"),
            &PreprocessOptions::default(),
        )
        .expect("lower delayed continuous assignment");
    assert_eq!(design.assigns.len(), 1);
    assert_eq!(design.assigns[0].delay, 5);

    let mut simulator = Simulator::new(&design).expect("create delayed simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .expect("set delayed input");
    simulator.run_until(4).expect("run before delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), None);
    simulator.run_until(5).expect("run to delay");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_fork_join_all() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fork-join");
    let source_path = root.join("fork_join.sv");
    std::fs::write(
        &source_path,
        r#"
module fork_join(output logic x, output logic y, output logic z);
  initial begin
    fork
      #5 x = 1'b1;
      #2 y = 1'b1;
    join
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write fork/join source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("fork_join"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower fork/join");
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create fork/join simulator");
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), None);
    simulator.run_until(5).unwrap();
    assert_eq!(simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));

    let nested_path = root.join("nested-fork.sv");
    std::fs::write(
        &nested_path,
        r#"
module nested_fork(output logic x, output logic y, output logic w, output logic z);
  initial begin
    fork
      #1 x = 1'b1;
      fork
        #2 y = 1'b1;
        #3 w = 1'b1;
      join
    join
    #4 z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write nested fork/join source");
    let nested = slang
        .compile_files_via_ast(
            std::slice::from_ref(&nested_path),
            Some("nested_fork"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested fork/join");
    let mut nested_simulator = Simulator::new(&nested).expect("create nested fork simulator");
    nested_simulator.run_until(1).unwrap();
    assert_eq!(nested_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("w").unwrap().to_u64(), None);
    nested_simulator.run_until(2).unwrap();
    assert_eq!(nested_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), None);
    nested_simulator.run_until(3).unwrap();
    assert_eq!(nested_simulator.read("w").unwrap().to_u64(), Some(1));
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), None);
    nested_simulator.run_until(7).unwrap();
    assert_eq!(nested_simulator.read("z").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_fork_join_any_and_none() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-fork-modes");

    let any_path = root.join("join_any.sv");
    std::fs::write(
        &any_path,
        r#"
module fork_any(output logic x, output logic y, output logic z);
  initial begin
    fork
      #5 x = 1'b1;
      #2 y = 1'b1;
    join_any
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write join_any source");
    let any = slang
        .compile_files_via_ast(
            std::slice::from_ref(&any_path),
            Some("fork_any"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower join_any");
    let mut any_simulator = Simulator::new(&any).expect("create join_any simulator");
    any_simulator.run_until(2).unwrap();
    assert_eq!(any_simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(any_simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(any_simulator.read("z").unwrap().to_u64(), Some(1));
    any_simulator.run_until(5).unwrap();
    assert_eq!(any_simulator.read("x").unwrap().to_u64(), Some(1));

    let none_path = root.join("join_none.sv");
    std::fs::write(
        &none_path,
        r#"
module fork_none(
  output logic x,
  output logic y,
  output logic z,
  output logic parent_started
);
  initial begin
    parent_started = 1'b1;
    fork
      #2 x = 1'b1;
      #3 y = 1'b1;
    join_none
    z = 1'b1;
  end
endmodule
"#,
    )
    .expect("write join_none source");
    let none = slang
        .compile_files_via_ast(
            std::slice::from_ref(&none_path),
            Some("fork_none"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower join_none");
    let mut none_simulator = Simulator::new(&none).expect("create join_none simulator");
    none_simulator.run_until(0).unwrap();
    assert_eq!(
        none_simulator.read("parent_started").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(none_simulator.read("z").unwrap().to_u64(), Some(1));
    assert_eq!(none_simulator.read("x").unwrap().to_u64(), None);
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), None);
    none_simulator.run_until(2).unwrap();
    assert_eq!(none_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), None);
    none_simulator.run_until(3).unwrap();
    assert_eq!(none_simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_flattens_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-instance-array");
    let source_path = root.join("instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module top #(parameter N = 2) (
  input logic [N-1:0] a,
  output logic [N-1:0] y
);
  child u[N] (.a(a), .y(y));
endmodule
"#,
    )
    .expect("write instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower instance array");
    assert_eq!(design.hierarchy.len(), 3);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec!["top", "top.u[0]", "top.u[1]"]
    );
    let mut simulator = Simulator::new(&design).expect("create instance-array simulator");
    for value in 0..4_u64 {
        simulator
            .set_input("a", &LogicWord::from_u64(2, value))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(!value & 0b11));
    }

    let multidimensional_path = root.join("instance_array_2d.sv");
    std::fs::write(
        &multidimensional_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module top (
  input logic [3:0] a,
  output logic [3:0] y
);
  child u[2][2] (.a(a), .y(y));
endmodule
"#,
    )
    .expect("write multidimensional instance-array source");
    let multidimensional = slang
        .compile_files_via_ast(
            std::slice::from_ref(&multidimensional_path),
            Some("top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional instance array");
    assert_eq!(multidimensional.hierarchy.len(), 5);
    assert_eq!(
        multidimensional
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "top",
            "top.u[0][0]",
            "top.u[0][1]",
            "top.u[1][0]",
            "top.u[1][1]"
        ]
    );
    let mut multidimensional_simulator =
        Simulator::new(&multidimensional).expect("create multidimensional array simulator");
    multidimensional_simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(
        multidimensional_simulator.read("y").unwrap().to_u64(),
        Some(0b0101)
    );
}

#[test]
fn slang_ast_direct_lowering_flattens_generated_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-generated-instance-array");
    let source_path = root.join("generated_instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
module child(input logic a, output logic y);
  assign y = ~a;
endmodule

module generated_instance_arrays(
  input logic [3:0] a,
  output logic [3:0] y
);
  generate
    if (1) begin : generated_scope
      child cells[4] (.a(a), .y(y));
    end
  endgenerate
endmodule
"#,
    )
    .expect("write generated instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("generated_instance_arrays"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower generated instance array");
    assert_eq!(design.hierarchy.len(), 5);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "generated_instance_arrays",
            "generated_instance_arrays.generated_scope.cells[0]",
            "generated_instance_arrays.generated_scope.cells[1]",
            "generated_instance_arrays.generated_scope.cells[2]",
            "generated_instance_arrays.generated_scope.cells[3]"
        ]
    );
    let mut simulator = Simulator::new(&design).expect("create generated array simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0b1001))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0b0110));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_modports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-modport");
    let source_path = root.join("interface_modport.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  logic ready = 1'b1;
  modport src(input clk, ready, output valid);
endinterface

module producer(
  bus_if.src bus,
  input logic go,
  output logic done
);
  assign bus.valid = go;
  assign done = bus.ready;
endmodule

module interface_top(
  input logic clk,
  input logic go,
  output logic valid,
  output logic done
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .go(go), .done(done));
endmodule
"#,
    )
    .expect("write interface-modport source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface modports");
    assert_eq!(design.hierarchy.len(), 3);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_top",
            "interface_top.bus",
            "interface_top.producer_u"
        ]
    );
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.valid"));
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.ready"));

    let mut simulator = Simulator::new(&design).expect("create interface simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("bus.valid").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));

    let mut ast = serde_json::from_str::<serde_json::Value>(
        &slang
            .ast_json_with_options(
                std::slice::from_ref(&source_path),
                Some("interface_top"),
                &[],
                &PreprocessOptions::default(),
            )
            .expect("serialize authoritative interface AST"),
    )
    .expect("parse authoritative interface AST");
    fn make_valid_input(value: &mut serde_json::Value) {
        if let Some(fields) = value.as_object_mut() {
            if fields.get("kind").and_then(serde_json::Value::as_str) == Some("Modport")
                && fields.get("name").and_then(serde_json::Value::as_str) == Some("src")
            {
                if let Some(members) = fields
                    .get_mut("members")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for member in members {
                        if member.get("name").and_then(serde_json::Value::as_str) == Some("valid") {
                            member["direction"] = serde_json::Value::String("In".into());
                        }
                    }
                }
            }
            for nested in fields.values_mut() {
                make_valid_input(nested);
            }
        } else if let Some(values) = value.as_array_mut() {
            for nested in values {
                make_valid_input(nested);
            }
        }
    }
    make_valid_input(&mut ast);
    let error = slang
        .lower_ast_json(&ast.to_string())
        .expect_err("modport input writes must fail closed");
    assert!(
        error.contains("modport input variable cannot be assigned directly"),
        "unexpected modport direction error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_interface_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-vector");
    let source_path = root.join("interface_vector.sv");
    std::fs::write(
        &source_path,
        r#"
interface vector_if(input logic clk);
  logic [7:0] outgoing;
  logic [7:0] incoming = 8'h3c;
  modport producer(input clk, incoming, output outgoing);
endinterface

module vector_producer(
  vector_if.producer bus,
  input logic increment,
  output logic [7:0] captured
);
  assign bus.outgoing = bus.incoming + (increment ? 8'h10 : 8'h00);
  assign captured = bus.incoming;
endmodule

module vector_interface_top(
  input logic clk,
  input logic increment,
  output logic [7:0] outgoing,
  output logic [7:0] captured
);
  vector_if bus(.clk(clk));
  vector_producer producer_u(
    .bus(bus),
    .increment(increment),
    .captured(captured)
  );
  assign outgoing = bus.outgoing;
endmodule
"#,
    )
    .expect("write packed-interface-vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("vector_interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower packed interface vectors");
    let outgoing = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "bus.outgoing")
        .expect("interface outgoing vector");
    assert_eq!(outgoing.width, 8);

    let mut simulator = Simulator::new(&design).expect("create interface-vector simulator");
    simulator
        .set_input("increment", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("bus.outgoing").unwrap().to_u64(), Some(0x4c));
    assert_eq!(simulator.read("captured").unwrap().to_u64(), Some(0x3c));
    assert_eq!(simulator.read("outgoing").unwrap().to_u64(), Some(0x4c));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_packed_struct_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-packed-struct");
    let source_path = root.join("interface_packed_struct.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef struct packed {
    logic [3:0] value;
    logic [1:0] tag;
  } packet_t;
  packet_t packet;
  modport producer(output packet);
endinterface

module producer(
  bus_if.producer bus,
  input logic [3:0] value,
  input logic [1:0] tag
);
  assign bus.packet.value = value;
  assign bus.packet.tag = tag;
endmodule

module interface_struct_top(
  input logic clk,
  input logic [3:0] value,
  input logic [1:0] tag,
  output logic [5:0] observed
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .value(value), .tag(tag));
  assign observed = bus.packet;
endmodule
"#,
    )
    .expect("write interface packed-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_struct_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface packed struct members");
    let mut simulator = Simulator::new(&design).expect("create interface struct simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0b1011))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    assert_eq!(
        simulator.read("observed").unwrap().to_u64(),
        Some(0b10_1110)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_interface_unpacked_struct_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-unpacked-struct");
    let source_path = root.join("interface_unpacked_struct.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef struct {
    logic [7:0] value;
  } payload_t;

  typedef struct {
    logic [3:0] tag;
    logic valid;
  } metadata_t;

  typedef struct {
    payload_t payload;
    metadata_t metadata;
  } packet_t;

  packet_t packet;
  modport producer(output packet);
endinterface

module producer(
  bus_if.producer bus,
  input logic [7:0] value,
  input logic [3:0] tag
);
  assign bus.packet.payload.value = value;
  assign bus.packet.metadata.tag = tag;
  assign bus.packet.metadata.valid = 1'b1;
endmodule

module interface_unpacked_struct_top(
  input logic clk,
  input logic [7:0] value,
  input logic [3:0] tag,
  output logic [7:0] observed_value,
  output logic [3:0] observed_tag,
  output logic observed_valid
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .value(value), .tag(tag));
  assign observed_value = bus.packet.payload.value;
  assign observed_tag = bus.packet.metadata.tag;
  assign observed_valid = bus.packet.metadata.valid;
endmodule
"#,
    )
    .expect("write interface unpacked-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_unpacked_struct_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface unpacked struct members");
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.packet" && declaration.width == 13));
    let mut simulator = Simulator::new(&design).expect("create unpacked-struct simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0x9c))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(4, 0b0110))
        .unwrap();
    assert_eq!(
        simulator.read("observed_value").unwrap().to_u64(),
        Some(0x9c)
    );
    assert_eq!(
        simulator.read("observed_tag").unwrap().to_u64(),
        Some(0b0110)
    );
    assert_eq!(simulator.read("observed_valid").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_parameterized_interface_specializations() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-parameterized-interface");
    let source_path = root.join("parameterized_interface.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if #(parameter int WIDTH = 8) (input logic clk);
  logic [WIDTH-1:0] data;
  modport producer(output data);
endinterface

module producer #(parameter WIDTH = 4) (
  bus_if.producer bus,
  input logic [WIDTH-1:0] value,
  output logic [WIDTH-1:0] observed
);
  assign bus.data = value;
  assign observed = bus.data;
endmodule

module parameterized_interface_top(
  input logic clk,
  input logic [3:0] value4,
  input logic [7:0] value8,
  output logic [3:0] observed4,
  output logic [7:0] observed8
);
  bus_if #(.WIDTH(4)) bus4(.clk(clk));
  bus_if #(.WIDTH(8)) bus8(.clk(clk));
  producer #(.WIDTH(4)) p4(.bus(bus4), .value(value4), .observed(observed4));
  producer #(.WIDTH(8)) p8(.bus(bus8), .value(value8), .observed(observed8));
endmodule
"#,
    )
    .expect("write parameterized-interface source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("parameterized_interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower parameterized interface specializations");
    let mut simulator = Simulator::new(&design).expect("create parameterized-interface simulator");
    simulator
        .set_input("value4", &LogicWord::from_u64(4, 0b0101))
        .unwrap();
    simulator
        .set_input("value8", &LogicWord::from_u64(8, 0b1010_0101))
        .unwrap();
    assert_eq!(simulator.read("observed4").unwrap().to_u64(), Some(0b0101));
    assert_eq!(
        simulator.read("observed8").unwrap().to_u64(),
        Some(0b1010_0101)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_parameterized_interface_aggregate_specializations() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-parameterized-interface-aggregate");
    let source_path = root.join("parameterized_interface_aggregate.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if #(parameter int WIDTH = 8) (input logic clk);
  typedef struct packed {
    logic [WIDTH-1:0] value;
  } payload_t;
  typedef struct packed {
    payload_t payload;
    logic [1:0] tag;
  } packet_t;

  packet_t packet;
  modport producer(output packet);
endinterface

module producer #(parameter WIDTH = 4) (
  bus_if.producer bus,
  input logic [WIDTH-1:0] value,
  input logic [1:0] tag,
  output logic [WIDTH-1:0] observed
);
  assign bus.packet.payload.value = value;
  assign bus.packet.tag = tag;
  assign observed = bus.packet.payload.value;
endmodule

module parameterized_interface_aggregate_top(
  input logic clk,
  input logic [3:0] value4,
  input logic [7:0] value8,
  input logic [1:0] tag,
  output logic [3:0] observed4,
  output logic [7:0] observed8
);
  bus_if #(.WIDTH(4)) bus4(.clk(clk));
  bus_if #(.WIDTH(8)) bus8(.clk(clk));
  producer #(.WIDTH(4)) p4(
    .bus(bus4),
    .value(value4),
    .tag(tag),
    .observed(observed4)
  );
  producer #(.WIDTH(8)) p8(
    .bus(bus8),
    .value(value8),
    .tag(tag),
    .observed(observed8)
  );
endmodule
"#,
    )
    .expect("write parameterized-interface aggregate source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("parameterized_interface_aggregate_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower parameterized interface aggregate specializations");
    let mut simulator = Simulator::new(&design).expect("create parameterized aggregate simulator");
    simulator
        .set_input("value4", &LogicWord::from_u64(4, 0b0101))
        .unwrap();
    simulator
        .set_input("value8", &LogicWord::from_u64(8, 0b1010_0101))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(2, 0b11))
        .unwrap();
    assert_eq!(simulator.read("observed4").unwrap().to_u64(), Some(0b0101));
    assert_eq!(
        simulator.read("observed8").unwrap().to_u64(),
        Some(0b1010_0101)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_interface_packed_union_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-packed-union");
    let source_path = root.join("interface_packed_union.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef union packed {
    logic [5:0] first;
    logic [5:0] second;
  } packet_t;
  packet_t packet;
  modport producer(output packet);
endinterface

module producer(
  bus_if.producer bus,
  input logic [5:0] value
);
  assign bus.packet.first = value;
endmodule

module interface_union_top(
  input logic clk,
  input logic [5:0] value,
  output logic [5:0] observed
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .value(value));
  assign observed = bus.packet.second;
endmodule
"#,
    )
    .expect("write interface packed-union source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_union_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface packed union members");
    let mut simulator = Simulator::new(&design).expect("create interface union simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(6, 0b011011))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b011011));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_unpacked_union_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-unpacked-union");
    let source_path = root.join("interface_unpacked_union.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef union {
    logic [7:0] wide;
    logic [3:0] narrow;
  } packet_t;

  packet_t packet;
  modport producer(output packet);
endinterface

module producer(
  bus_if.producer bus,
  input logic [7:0] value
);
  assign bus.packet.wide = value;
endmodule

module interface_unpacked_union_top(
  input logic clk,
  input logic [7:0] value,
  output logic [3:0] observed
);
  bus_if bus(.clk(clk));
  producer producer_u(.bus(bus), .value(value));
  assign observed = bus.packet.narrow;
endmodule
"#,
    )
    .expect("write interface unpacked-union source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_unpacked_union_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface unpacked union members");
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "bus.packet" && declaration.width == 8));
    let mut simulator = Simulator::new(&design).expect("create unpacked-union simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0xb7))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0x7));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_nested_struct_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-nested-struct");
    let source_path = root.join("interface_nested_struct.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef struct packed {
    logic [1:0] low;
    logic [3:0] high;
  } inner_t;
  typedef struct packed {
    inner_t inner;
    logic [1:0] tag;
  } packet_t;
  packet_t packet;
  modport producer(output packet);
endinterface

module producer(
  bus_if.producer bus,
  input logic [1:0] low,
  input logic [3:0] high,
  input logic [1:0] tag
);
  assign bus.packet.inner.low = low;
  assign bus.packet.inner.high = high;
  assign bus.packet.tag = tag;
endmodule

module interface_nested_struct_top(
  input logic clk,
  input logic [1:0] low,
  input logic [3:0] high,
  input logic [1:0] tag,
  output logic [1:0] observed_low,
  output logic [3:0] observed_high,
  output logic [1:0] observed_tag
);
  bus_if bus(.clk(clk));
  producer producer_u(
    .bus(bus),
    .low(low),
    .high(high),
    .tag(tag)
  );
  assign observed_low = bus.packet.inner.low;
  assign observed_high = bus.packet.inner.high;
  assign observed_tag = bus.packet.tag;
endmodule
"#,
    )
    .expect("write interface nested-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_nested_struct_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface nested struct members");
    let mut simulator = Simulator::new(&design).expect("create interface nested-struct simulator");
    simulator
        .set_input("low", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    simulator
        .set_input("high", &LogicWord::from_u64(4, 0b0101))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(2, 0b11))
        .unwrap();
    assert_eq!(simulator.read("observed_low").unwrap().to_u64(), Some(0b10));
    assert_eq!(
        simulator.read("observed_high").unwrap().to_u64(),
        Some(0b0101)
    );
    assert_eq!(simulator.read("observed_tag").unwrap().to_u64(), Some(0b11));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_array_port_struct_members() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-struct");
    let source_path = root.join("interface_array_port_struct.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  typedef struct packed {
    logic [3:0] value;
    logic [1:0] tag;
  } packet_t;
  packet_t packet;
  modport producer(output packet);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [7:0] value,
  input logic [3:0] tag
);
  assign buses[0].packet.value = value[3:0];
  assign buses[1].packet.value = value[7:4];
  assign buses[0].packet.tag = tag[1:0];
  assign buses[1].packet.tag = tag[3:2];
endmodule

module interface_array_port_struct_top(
  input logic clk,
  input logic [7:0] value,
  input logic [3:0] tag,
  output logic [7:0] observed_value,
  output logic [3:0] observed_tag
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value), .tag(tag));
  assign observed_value[3:0] = buses[0].packet.value;
  assign observed_value[7:4] = buses[1].packet.value;
  assign observed_tag[1:0] = buses[0].packet.tag;
  assign observed_tag[3:2] = buses[1].packet.tag;
endmodule
"#,
    )
    .expect("write interface-array-port struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_struct_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface array port struct members");
    let mut simulator = Simulator::new(&design).expect("create interface-array struct simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0b1100_1011))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(4, 0b01_10))
        .unwrap();
    assert_eq!(
        simulator.read("observed_value").unwrap().to_u64(),
        Some(0b1100_1011)
    );
    assert_eq!(
        simulator.read("observed_tag").unwrap().to_u64(),
        Some(0b01_10)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-vector-multi");
    let source_path = root.join("interface_vector_multi.sv");
    std::fs::write(
        &source_path,
        r#"
interface multi_vector_if(input logic clk);
  logic [3:0][7:0] outgoing;
  logic [3:0][7:0] incoming = 32'h12345678;
  modport producer(input clk, incoming, output outgoing);
endinterface

module multi_vector_producer(
  multi_vector_if.producer bus,
  input logic [1:0] index,
  output logic [7:0] captured
);
  assign bus.outgoing = bus.incoming;
  assign captured = bus.incoming[index];
endmodule

module multidim_interface_top(
  input logic clk,
  input logic [1:0] index,
  output logic [31:0] outgoing,
  output logic [7:0] captured
);
  multi_vector_if bus(.clk(clk));
  multi_vector_producer producer_u(
    .bus(bus),
    .index(index),
    .captured(captured)
  );
  assign outgoing = bus.outgoing;
endmodule
"#,
    )
    .expect("write multidimensional interface-vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multidim_interface_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface vectors");
    let outgoing = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "bus.outgoing")
        .expect("multidimensional interface vector");
    assert_eq!(outgoing.width, 32);

    let mut simulator =
        Simulator::new(&design).expect("create multidimensional interface simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(2, 2))
        .unwrap();
    assert_eq!(
        simulator.read("bus.outgoing").unwrap().to_u64(),
        Some(0x12345678)
    );
    assert_eq!(simulator.read("captured").unwrap().to_u64(), Some(0x34));
    assert_eq!(
        simulator.read("outgoing").unwrap().to_u64(),
        Some(0x12345678)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_unpacked_interface_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-member");
    let source_path = root.join("interface_array_member.sv");
    std::fs::write(
        &source_path,
        r#"
interface memory_if(input logic clk);
  logic [7:0] data[0:3];
  modport producer(output data);
endinterface

module array_producer(
  memory_if.producer bus,
  input logic [1:0] index,
  input logic [7:0] value
);
  always_comb begin
    bus.data[index] = value;
  end
endmodule

module interface_array_member_top(
  input logic clk,
  input logic [1:0] index,
  input logic [7:0] value,
  output logic [7:0] observed
);
  memory_if bus(.clk(clk));
  array_producer producer_u(
    .bus(bus),
    .index(index),
    .value(value)
  );
  assign observed = bus.data[index];
endmodule
"#,
    )
    .expect("write unpacked-interface-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_member_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unpacked interface arrays");
    assert_eq!(design.hierarchy.len(), 3);
    assert!(design
        .memories
        .iter()
        .any(|memory| memory.name == "bus.data"));
    assert_eq!(design.memory_connections.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create interface-array simulator");
    for (index, value) in [(0_u64, 0xA5), (1, 0x3C), (2, 0x5A), (3, 0x96)] {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(value));
    }

    let mut ast = serde_json::from_str::<serde_json::Value>(
        &slang
            .ast_json_with_options(
                std::slice::from_ref(&source_path),
                Some("interface_array_member_top"),
                &[],
                &PreprocessOptions::default(),
            )
            .expect("serialize authoritative interface-array AST"),
    )
    .expect("parse authoritative interface-array AST");
    fn make_memory_input(value: &mut serde_json::Value) {
        if let Some(fields) = value.as_object_mut() {
            if fields.get("kind").and_then(serde_json::Value::as_str) == Some("Modport")
                && fields.get("name").and_then(serde_json::Value::as_str) == Some("producer")
            {
                if let Some(members) = fields
                    .get_mut("members")
                    .and_then(serde_json::Value::as_array_mut)
                {
                    for member in members {
                        if member.get("name").and_then(serde_json::Value::as_str) == Some("data") {
                            member["direction"] = serde_json::Value::String("In".into());
                        }
                    }
                }
            }
            for nested in fields.values_mut() {
                make_memory_input(nested);
            }
        } else if let Some(values) = value.as_array_mut() {
            for nested in values {
                make_memory_input(nested);
            }
        }
    }
    make_memory_input(&mut ast);
    let error = slang
        .lower_ast_json(&ast.to_string())
        .expect_err("modport input memory writes must fail closed");
    assert!(
        error.contains("modport input variable cannot be assigned directly"),
        "unexpected interface-memory direction error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_unpacked_interface_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-member-2d");
    let source_path = root.join("interface_array_member_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface multi_memory_if(input logic clk);
  logic [7:0] data[0:1][0:3];
  modport producer(output data);
endinterface

module multidim_array_producer(
  multi_memory_if.producer bus,
  input logic row,
  input logic [1:0] column,
  input logic [7:0] value
);
  always_comb begin
    bus.data[row][column] = value;
  end
endmodule

module multidim_interface_array_top(
  input logic clk,
  input logic row,
  input logic [1:0] column,
  input logic [7:0] value,
  output logic [7:0] observed
);
  multi_memory_if bus(.clk(clk));
  multidim_array_producer producer_u(
    .bus(bus),
    .row(row),
    .column(column),
    .value(value)
  );
  assign observed = bus.data[row][column];
endmodule
"#,
    )
    .expect("write multidimensional unpacked-interface-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multidim_interface_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional unpacked interface arrays");
    assert_eq!(design.hierarchy.len(), 3);
    let interface_memory = design
        .memories
        .iter()
        .find(|memory| memory.name == "bus.data")
        .expect("interface multidimensional memory");
    assert_eq!(interface_memory.dimensions.len(), 2);
    assert_eq!(interface_memory.flat_length, 8);
    assert_eq!(design.memory_connections.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create 2D interface-array simulator");
    for (row, column, value) in [
        (0_u64, 0_u64, 0xA5),
        (0, 3, 0x3C),
        (1, 1, 0x5A),
        (1, 2, 0x96),
    ] {
        simulator
            .set_input("row", &LogicWord::from_u64(1, row))
            .unwrap();
        simulator
            .set_input("column", &LogicWord::from_u64(2, column))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(value));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_interface_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-instance-array");
    let source_path = root.join("interface_instance_array.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module array_producer(
  bus_if.producer bus,
  input logic value
);
  assign bus.valid = value;
endmodule

module interface_instance_array_top(
  input logic clk,
  input logic [1:0] value,
  output logic [1:0] observed
);
  bus_if buses[2] (.clk(clk));
  array_producer producers[2] (
    .bus(buses),
    .value(value)
  );
  assign observed[0] = buses[0].valid;
  assign observed[1] = buses[1].valid;
endmodule
"#,
    )
    .expect("write interface-instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_instance_array_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface instance arrays");
    assert_eq!(design.hierarchy.len(), 5);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_instance_array_top",
            "interface_instance_array_top.buses[0]",
            "interface_instance_array_top.buses[1]",
            "interface_instance_array_top.producers[0]",
            "interface_instance_array_top.producers[1]"
        ]
    );
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "buses[0].valid"));
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "buses[1].valid"));

    let mut simulator = Simulator::new(&design).expect("create interface-instance-array simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b01));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_instance_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-instance-array-2d");
    let source_path = root.join("interface_instance_array_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module array_producer(bus_if.producer bus, input logic value);
  assign bus.valid = value;
endmodule

module interface_instance_array_2d_top(
  input logic clk,
  input logic [3:0] value,
  output logic [3:0] observed
);
  bus_if buses[2][2] (.clk(clk));
  array_producer p00 (.bus(buses[0][0]), .value(value[0]));
  array_producer p01 (.bus(buses[0][1]), .value(value[1]));
  array_producer p10 (.bus(buses[1][0]), .value(value[2]));
  array_producer p11 (.bus(buses[1][1]), .value(value[3]));
  assign observed[0] = buses[0][0].valid;
  assign observed[1] = buses[0][1].valid;
  assign observed[2] = buses[1][0].valid;
  assign observed[3] = buses[1][1].valid;
endmodule
"#,
    )
    .expect("write multidimensional interface-instance-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_instance_array_2d_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface instance arrays");
    assert_eq!(design.hierarchy.len(), 9);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_instance_array_2d_top",
            "interface_instance_array_2d_top.buses[0][0]",
            "interface_instance_array_2d_top.buses[0][1]",
            "interface_instance_array_2d_top.buses[1][0]",
            "interface_instance_array_2d_top.buses[1][1]",
            "interface_instance_array_2d_top.p00",
            "interface_instance_array_2d_top.p01",
            "interface_instance_array_2d_top.p10",
            "interface_instance_array_2d_top.p11"
        ]
    );
    for path in [
        "buses[0][0].valid",
        "buses[0][1].valid",
        "buses[1][0].valid",
        "buses[1][1].valid",
    ] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == path),
            "missing interface signal {path}"
        );
    }

    let mut simulator = Simulator::new(&design).expect("create 2D interface-array simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port");
    let source_path = root.join("interface_array_port.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [1:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
endmodule

module interface_array_port_top(
  input logic clk,
  input logic [1:0] value,
  output logic [1:0] observed
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[0] = buses[0].valid;
  assign observed[1] = buses[1].valid;
endmodule
"#,
    )
    .expect("write interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface array ports");
    assert_eq!(design.hierarchy.len(), 4);
    assert_eq!(
        design
            .hierarchy
            .iter()
            .map(|instance| instance.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "interface_array_port_top",
            "interface_array_port_top.buses[0]",
            "interface_array_port_top.buses[1]",
            "interface_array_port_top.producers"
        ]
    );
    for path in ["buses[0].valid", "buses[1].valid"] {
        assert!(
            design
                .net_decls
                .iter()
                .any(|declaration| declaration.name == path),
            "missing interface signal {path}"
        );
    }

    let mut simulator = Simulator::new(&design).expect("create interface-array-port simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b10));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_interface_array_ports() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-2d");
    let source_path = root.join("interface_array_port_2d.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[2][2],
  input logic [3:0] value
);
  assign buses[0][0].valid = value[0];
  assign buses[0][1].valid = value[1];
  assign buses[1][0].valid = value[2];
  assign buses[1][1].valid = value[3];
endmodule

module interface_array_port_2d_top(
  input logic clk,
  input logic [3:0] value,
  output logic [3:0] observed
);
  bus_if buses[2][2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[0] = buses[0][0].valid;
  assign observed[1] = buses[0][1].valid;
  assign observed[2] = buses[1][0].valid;
  assign observed[3] = buses[1][1].valid;
endmodule
"#,
    )
    .expect("write multidimensional interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_2d_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional interface array ports");
    assert_eq!(design.hierarchy.len(), 6);
    let mut simulator = Simulator::new(&design).expect("create 2D interface-array-port simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0b1010))
        .unwrap();
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(0b1010));
}

#[test]
fn slang_ast_direct_lowering_supports_interface_array_port_vectors() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-vector");
    let source_path = root.join("interface_array_port_vector.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic [3:0] data;
  modport producer(output data);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [7:0] value
);
  assign buses[0].data = value[3:0];
  assign buses[1].data = value[7:4];
endmodule

module interface_array_port_vector_top(
  input logic clk,
  input logic [7:0] value,
  output logic [7:0] observed
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
  assign observed[3:0] = buses[0].data;
  assign observed[7:4] = buses[1].data;
endmodule
"#,
    )
    .expect("write interface-array-port vector source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_vector_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower interface array port vectors");
    let mut simulator =
        Simulator::new(&design).expect("create interface-array-port vector simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0b1011_0110))
        .unwrap();
    assert_eq!(
        simulator.read("observed").unwrap().to_u64(),
        Some(0b1011_0110)
    );
}

#[test]
fn slang_ast_direct_lowering_shares_interface_array_ports_between_consumers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-shared");
    let source_path = root.join("interface_array_port_shared.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  logic ready;
  modport producer(output valid, input ready);
  modport consumer(input valid, output ready);
endinterface

module producer_array(
  bus_if.producer buses[2],
  input logic [1:0] drive,
  output logic [1:0] observed_ready
);
  assign buses[0].valid = drive[0];
  assign buses[1].valid = drive[1];
  assign observed_ready[0] = buses[0].ready;
  assign observed_ready[1] = buses[1].ready;
endmodule

module consumer_array(
  bus_if.consumer buses[2],
  input logic [1:0] enable,
  output logic [1:0] observed_valid
);
  assign buses[0].ready = enable[0];
  assign buses[1].ready = enable[1];
  assign observed_valid[0] = buses[0].valid;
  assign observed_valid[1] = buses[1].valid;
endmodule

module interface_array_port_shared_top(
  input logic clk,
  input logic [1:0] drive,
  input logic [1:0] enable,
  output logic [1:0] observed_valid,
  output logic [1:0] observed_ready
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .drive(drive), .observed_ready(observed_ready));
  consumer_array consumers (.buses(buses), .enable(enable), .observed_valid(observed_valid));
endmodule
"#,
    )
    .expect("write shared interface-array-port source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_shared_top"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower shared interface array ports");
    let mut simulator = Simulator::new(&design).expect("create shared interface-array simulator");
    simulator
        .set_input("drive", &LogicWord::from_u64(2, 0b10))
        .unwrap();
    simulator
        .set_input("enable", &LogicWord::from_u64(2, 0b01))
        .unwrap();
    assert_eq!(
        simulator.read("observed_valid").unwrap().to_u64(),
        Some(0b10)
    );
    assert_eq!(
        simulator.read("observed_ready").unwrap().to_u64(),
        Some(0b01)
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_interface_array_port_input_writes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-input");
    let source_path = root.join("interface_array_port_input.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport consumer(input valid);
endinterface

module consumer_array(
  bus_if.consumer buses[2],
  input logic [1:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
endmodule

module interface_array_port_input_top(
  input logic clk,
  input logic [1:0] value
);
  bus_if buses[2] (.clk(clk));
  consumer_array consumers (.buses(buses), .value(value));
endmodule
"#,
    )
    .expect("write interface-array-port input source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_input_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("interface array port input writes must fail closed");
    assert!(
        error.contains("cannot assign to input port 'valid'"),
        "unexpected interface-array port input error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_interface_array_port_shape_mismatches() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-interface-array-port-shape");
    let source_path = root.join("interface_array_port_shape.sv");
    std::fs::write(
        &source_path,
        r#"
interface bus_if(input logic clk);
  logic valid;
  modport producer(output valid);
endinterface

module producer_array(
  bus_if.producer buses[3],
  input logic [2:0] value
);
  assign buses[0].valid = value[0];
  assign buses[1].valid = value[1];
  assign buses[2].valid = value[2];
endmodule

module interface_array_port_shape_top(
  input logic clk,
  input logic [2:0] value
);
  bus_if buses[2] (.clk(clk));
  producer_array producers (.buses(buses), .value(value));
endmodule
"#,
    )
    .expect("write interface-array-port shape source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("interface_array_port_shape_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("interface array port shape mismatches must fail closed");
    assert!(
        error.contains("dimensions of connection for port 'buses' do not match"),
        "unexpected interface-array port shape error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_multi_packed_width() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-multi-packed");
    let source_path = root.join("multi_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module multi_packed(
  input logic [3:0][7:0] a,
  output logic [31:0] y
);
  assign y = a;
endmodule
"#,
    )
    .expect("write multi-packed source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("multi_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower multi-packed declaration");
    assert_eq!(design.n_nets, 64);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "a")
            .expect("multi-packed input")
            .width,
        32
    );

    let mut simulator = Simulator::new(&design).expect("create multi-packed simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 0x1234_5678))
        .expect("set multi-packed input");
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0x1234_5678));

    let selected_path = root.join("multi_packed_select.sv");
    std::fs::write(
        &selected_path,
        r#"
module multi_packed_select(
  input logic [3:0][7:0] a,
  input logic [1:0] selector,
  output logic [7:0] y,
  output logic [7:0] dynamic_y,
  output logic nested_bit
);
  assign y = a[3];
  assign dynamic_y = a[selector];
  assign nested_bit = a[3][7];
endmodule
"#,
    )
    .expect("write multi-packed select source");
    let selected = slang
        .compile_files_via_ast(
            std::slice::from_ref(&selected_path),
            Some("multi_packed_select"),
            &PreprocessOptions::default(),
        )
        .expect("lower constant, dynamic, and nested packed selects");

    let mut simulator = Simulator::new(&selected).expect("create packed-select simulator");
    for selector in 0..4_u64 {
        simulator
            .set_input("a", &LogicWord::from_u64(32, 0xB234_5678))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0xB2));
        let expected = match selector {
            0 => 0x78,
            1 => 0x56,
            2 => 0x34,
            _ => 0xB2,
        };
        assert_eq!(
            simulator.read("dynamic_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(simulator.read("nested_bit").unwrap().to_u64(), Some(1));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_unpacked_aggregate_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-unpacked-aggregate");
    let source_path = root.join("unpacked_aggregate.sv");
    std::fs::write(
        &source_path,
        r#"
module unpacked_aggregate(
  input logic [7:0] value,
  input logic [3:0] tag,
  output logic [7:0] observed_value,
  output logic [3:0] observed_tag,
  output logic observed_valid,
  output logic [3:0] observed_union
);
  typedef struct {
    logic [7:0] value;
    logic [3:0] tag;
    logic valid;
  } packet_t;

  typedef union {
    logic [7:0] wide;
    logic [3:0] narrow;
  } raw_t;

  packet_t packet;
  raw_t raw;

  assign packet.value = value;
  assign packet.tag = tag;
  assign packet.valid = 1'b1;
  assign observed_value = packet.value;
  assign observed_tag = packet.tag;
  assign observed_valid = packet.valid;

  assign raw.wide = value;
  assign observed_union = raw.narrow;
endmodule
"#,
    )
    .expect("write unpacked-aggregate source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("unpacked_aggregate"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unpacked aggregates");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|decl| decl.name == "packet")
            .map(|decl| decl.width),
        Some(13)
    );
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|decl| decl.name == "raw")
            .map(|decl| decl.width),
        Some(8)
    );

    let mut simulator = Simulator::new(&design).expect("create unpacked-aggregate simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0xa7))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(4, 0b0101))
        .unwrap();
    assert_eq!(
        simulator.read("observed_value").unwrap().to_u64(),
        Some(0xa7)
    );
    assert_eq!(
        simulator.read("observed_tag").unwrap().to_u64(),
        Some(0b0101)
    );
    assert_eq!(simulator.read("observed_valid").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("observed_union").unwrap().to_u64(),
        Some(0x7)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_nested_unpacked_aggregates() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nested-unpacked");
    let source_path = root.join("nested_unpacked.sv");
    std::fs::write(
        &source_path,
        r#"
module nested_unpacked(
  input logic [7:0] value,
  input logic [3:0] tag,
  output logic [7:0] observed_value,
  output logic [3:0] observed_tag,
  output logic [1:0] observed_mode,
  output logic [3:0] observed_union
);
  typedef struct {
    logic [7:0] value;
    logic [3:0] tag;
  } inner_t;

  typedef struct {
    inner_t inner;
    logic [1:0] mode;
  } outer_t;

  typedef union {
    logic [7:0] wide;
    logic [3:0] narrow;
  } raw_t;

  typedef struct {
    raw_t raw;
    logic [3:0] checksum;
  } wrapper_t;

  outer_t packet;
  outer_t copied;
  wrapper_t wrapper;

  assign packet.inner.value = value;
  assign packet.inner.tag = tag;
  assign packet.mode = 2'b10;
  assign wrapper.raw.wide = value;
  assign wrapper.checksum = tag;
  assign copied = packet;

  assign observed_value = copied.inner.value;
  assign observed_tag = copied.inner.tag;
  assign observed_mode = copied.mode;
  assign observed_union = wrapper.raw.narrow;
endmodule
"#,
    )
    .expect("write nested-unpacked source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("nested_unpacked"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nested unpacked aggregates");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|decl| decl.name == "packet")
            .map(|decl| decl.width),
        Some(14)
    );
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|decl| decl.name == "wrapper")
            .map(|decl| decl.width),
        Some(12)
    );

    let mut simulator = Simulator::new(&design).expect("create nested-unpacked simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(8, 0x5d))
        .unwrap();
    simulator
        .set_input("tag", &LogicWord::from_u64(4, 0b0110))
        .unwrap();
    assert_eq!(
        simulator.read("observed_value").unwrap().to_u64(),
        Some(0x5d)
    );
    assert_eq!(
        simulator.read("observed_tag").unwrap().to_u64(),
        Some(0b0110)
    );
    assert_eq!(
        simulator.read("observed_mode").unwrap().to_u64(),
        Some(0b10)
    );
    assert_eq!(
        simulator.read("observed_union").unwrap().to_u64(),
        Some(0xd)
    );
}

#[test]
fn slang_ast_direct_lowering_rejects_unsupported_unpacked_aggregate_forms() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-unpacked-aggregate-invalid");
    let pattern_path = root.join("unpacked_pattern.sv");
    std::fs::write(
        &pattern_path,
        r#"
module unpacked_pattern;
  typedef struct {
    logic [7:0] wide;
    logic [3:0] narrow;
  } packet_t;

  packet_t packet;
  initial packet = '{8'h12, 4'h3};
endmodule
"#,
    )
    .expect("write unpacked-pattern source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&pattern_path),
            Some("unpacked_pattern"),
            &PreprocessOptions::default(),
        )
        .expect_err("unpacked aggregate assignment patterns must fail closed");
    assert!(
        error.contains("unpacked aggregate assignment patterns are not supported"),
        "unexpected unpacked pattern error: {error}"
    );

    let tagged_path = root.join("tagged_union.sv");
    std::fs::write(
        &tagged_path,
        r#"
module tagged_union_probe;
  typedef union tagged {
    logic [7:0] wide;
    logic [1:0] narrow;
  } packet_t;

  packet_t packet;
  initial packet.wide = 8'h12;
endmodule
"#,
    )
    .expect("write tagged-union source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&tagged_path),
            Some("tagged_union_probe"),
            &PreprocessOptions::default(),
        )
        .expect_err("tagged unions must fail closed");
    assert!(
        error.contains("does not support tagged union 'packet_t'"),
        "unexpected tagged-union error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-struct");
    let source_path = root.join("packed_struct.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_struct(
  input logic [3:0] high,
  input logic [3:0] low,
  output logic [7:0] whole,
  output logic [3:0] selected_low
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t pair;
  assign pair.high = high;
  assign pair.low = low;
  assign whole = pair;
  assign selected_low = pair.low;
endmodule
"#,
    )
    .expect("write packed-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_struct"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct");
    assert_eq!(design.assigns.len(), 4);

    let mut simulator = Simulator::new(&design).expect("create packed-struct simulator");
    simulator
        .set_input("high", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("low", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("selected_low").unwrap().to_u64(), Some(0x5));
}

#[test]
fn slang_ast_direct_lowering_supports_nested_packed_struct_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nested-packed-struct");
    let source_path = root.join("nested_packed_struct.sv");
    std::fs::write(
        &source_path,
        r#"
module nested_packed_struct(
  output logic [7:0] whole,
  output logic [1:0] selected
);
  typedef struct packed {
    logic [1:0] a;
    logic [1:0] b;
  } inner_t;

  typedef struct packed {
    inner_t left;
    logic [3:0] right;
  } outer_t;

  outer_t value;
  assign value.left.a = 2'b10;
  assign value.left.b = 2'b01;
  assign value.right = 4'h3;
  assign whole = value;
  assign selected = value.left.a;
endmodule
"#,
    )
    .expect("write nested packed-struct source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("nested_packed_struct"),
            &PreprocessOptions::default(),
        )
        .expect("lower nested packed struct");
    assert_eq!(design.assigns.len(), 5);

    let mut simulator = Simulator::new(&design).expect("create nested packed-struct simulator");
    simulator.run_until(0).expect("settle nested packed struct");
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x93));
    assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(0b10));
}

#[test]
fn slang_ast_direct_lowering_supports_packed_union_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-union");
    let source_path = root.join("packed_union.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_union(
  input logic [7:0] byte_value,
  input logic [15:0] word_value,
  input logic select_word,
  output logic [15:0] word_view,
  output logic [7:0] byte_view,
  output logic [15:0] whole
);
  typedef union packed {
    logic [15:0] word;
    logic [15:0] raw;
  } value_t;

  value_t value;
  always_comb begin
    if (select_word) value.word = word_value;
    else value.raw = {8'h00, byte_value};
  end
  assign word_view = value.word;
  assign byte_view = value.raw[7:0];
  assign whole = value;
endmodule
"#,
    )
    .expect("write packed-union source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_union"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed union");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "value")
            .expect("packed union declaration")
            .width,
        16
    );

    let mut simulator = Simulator::new(&design).expect("create packed-union simulator");
    simulator
        .set_input("byte_value", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    simulator
        .set_input("select_word", &LogicWord::from_u64(1, 0))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x00a5));
    assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(0x00a5));
    assert_eq!(simulator.read("byte_view").unwrap().to_u64(), Some(0xa5));

    simulator
        .set_input("word_value", &LogicWord::from_u64(16, 0x1234))
        .unwrap();
    simulator
        .set_input("select_word", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("whole").unwrap().to_u64(), Some(0x1234));
    assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(0x1234));
    assert_eq!(simulator.read("byte_view").unwrap().to_u64(), Some(0x34));
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-struct-array");
    let source_path = root.join("packed_struct_array.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_struct_array(
  input logic [1:0] index,
  input logic [3:0] high,
  input logic [3:0] low,
  output logic [3:0] selected_high,
  output logic [3:0] selected_low,
  output logic [31:0] all_values
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t pairs [0:3];
  always_comb begin
    pairs[0].high = 4'h1;
    pairs[0].low = 4'h2;
    pairs[1].high = 4'h3;
    pairs[1].low = 4'h4;
    pairs[2].high = 4'h5;
    pairs[2].low = 4'h6;
    pairs[3].high = high;
    pairs[3].low = low;
  end

  assign selected_high = pairs[index].high;
  assign selected_low = pairs[index].low;
  assign all_values = {pairs[3], pairs[2], pairs[1], pairs[0]};
endmodule
"#,
    )
    .expect("write packed struct array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_struct_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct array");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].element_width, 8);
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);

    let mut simulator = Simulator::new(&design).expect("create packed struct array simulator");
    simulator
        .set_input("high", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("low", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0xa556_3412)
    );
    for (index, selected_high, selected_low) in
        [(0, 0x1, 0x2), (1, 0x3, 0x4), (2, 0x5, 0x6), (3, 0xa, 0x5)]
    {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        assert_eq!(
            simulator.read("selected_high").unwrap().to_u64(),
            Some(selected_high)
        );
        assert_eq!(
            simulator.read("selected_low").unwrap().to_u64(),
            Some(selected_low)
        );
    }
}

#[test]
fn slang_ast_summary_rejects_wrong_memory_member_lvalue_shapes() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-memory-member-summary");
    let source_path = root.join("memory_member_summary.sv");
    std::fs::write(
        &source_path,
        r#"
module memory_member_summary(
  input logic [1:0] address,
  input logic [3:0] value,
  input logic [1:0] nested_value,
  output logic [7:0] observed,
  output logic [7:0] nested_observed
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t pairs [0:3];
  typedef struct packed {
    logic [1:0] a;
    logic [1:0] b;
  } inner_t;

  typedef struct packed {
    inner_t left;
    logic [3:0] right;
  } nested_t;

  nested_t rows [0:3];
  always_comb begin
    pairs[address].high = value;
    rows[address].right = 4'b0;
    rows[address].left.b = 2'b0;
    rows[address].left.a = nested_value;
  end
  assign observed = pairs[address];
  assign nested_observed = rows[address];
endmodule
"#,
    )
    .expect("write sparse-memory member summary source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("memory_member_summary"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower sparse-memory member source");
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("memory_member_summary"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize authoritative sparse-memory member AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize authoritative sparse-memory member AST");
    summary
        .validate_against_design(&design)
        .expect("exact sparse-memory member fingerprint should match");
    let mut wrong_whole_element = summary.clone();
    if let Some(target) = wrong_whole_element
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "observed")
    {
        target.rhs.operands[1].symbol = Some("wrong_address".into());
    }
    let whole_element_error = wrong_whole_element
        .validate_against_design(&design)
        .expect_err("wrong whole sparse-memory element index must fail");
    assert!(
        whole_element_error.contains("Ref:2:u[wrong_address]")
            && whole_element_error.contains("Ref:2:u[address]")
            && whole_element_error.contains("MemoryRead:8:u[pairs,"),
        "unexpected whole sparse-memory element error: {whole_element_error}"
    );
    let assignment = summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
        .expect("sparse-memory member assignment");
    assignment
        .memory_member_lvalue
        .as_mut()
        .expect("sparse-memory member lvalue")
        .indexes[0]
        .literal = Some("32:00000000000000000000000000000001".into());
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong sparse-memory member index must fail");
    assert!(
        error.contains("MemoryRead:8:u[pairs,")
            && error.contains("32:00000000000000000000000000000001"),
        "unexpected structural error: {error}"
    );

    let assignment = summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
        .expect("sparse-memory member assignment");
    assignment
        .memory_member_lvalue
        .as_mut()
        .expect("sparse-memory member lvalue")
        .indexes[0]
        .literal = None;
    assignment
        .memory_member_lvalue
        .as_mut()
        .expect("sparse-memory member lvalue")
        .members = vec!["low".into()];
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong sparse-memory member field must fail");
    assert!(
        error.contains("Range:3:0") && error.contains("Range:7:4"),
        "unexpected structural error: {error}"
    );

    let assignment = summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
        .expect("sparse-memory member assignment");
    assignment
        .memory_member_lvalue
        .as_mut()
        .expect("sparse-memory member lvalue")
        .members = vec!["high".into()];

    if let Some(nested_assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .memory_member_lvalue
                .as_ref()
                .is_some_and(|member| member.members.last().is_some_and(|name| name == "a"))
        })
    }) {
        nested_assignment
            .memory_member_lvalue
            .as_mut()
            .expect("nested sparse-memory member lvalue")
            .members[1] = "b".into();
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong nested sparse-memory member field must fail");
    assert!(
        error.contains("Range:5:0") && error.contains("Range:7:6") && error.contains("Range:3:0"),
        "unexpected structural error: {error}"
    );
    if let Some(nested_assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .memory_member_lvalue
                .as_ref()
                .is_some_and(|member| member.members.last().is_some_and(|name| name == "b"))
        })
    }) {
        nested_assignment
            .memory_member_lvalue
            .as_mut()
            .expect("nested sparse-memory member lvalue")
            .members[1] = "a".into();
    }

    let assignment = summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
        .expect("sparse-memory member assignment");
    assignment
        .memory_member_lvalue
        .as_mut()
        .expect("sparse-memory member lvalue")
        .members = vec!["high".into()];
    assignment.rhs.symbol = Some("wrong_value".into());
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong sparse-memory member RHS must fail");
    assert!(
        error.contains("Ref:4:u[wrong_value]"),
        "unexpected structural error: {error}"
    );

    let mut simulator = Simulator::new(&design).expect("create sparse-memory member simulator");
    simulator
        .set_input("address", &LogicWord::from_u64(2, 1))
        .unwrap();
    simulator
        .set_input("value", &LogicWord::from_u64(4, 0))
        .unwrap();
    simulator
        .set_input("nested_value", &LogicWord::from_u64(2, 3))
        .unwrap();
    simulator.run_until(0).unwrap();
    assert_eq!(
        simulator.read("nested_observed").unwrap().to_u64(),
        Some(0b1100_0000),
        "nested memory member write should update the physically offset field"
    );

    let forward_path = root.join("forward_memory_member.sv");
    std::fs::write(
        &forward_path,
        r#"
module forward_memory_member(
  input logic [1:0] address,
  input logic [1:0] value,
  output logic [7:0] observed
);
  typedef inner_t;
  typedef struct packed {
    inner_t left;
    logic [3:0] right;
  } outer_t;
  typedef struct packed {
    logic [1:0] a;
    logic [1:0] b;
  } inner_t;

  outer_t rows [0:3];
  always_comb begin
    rows[address].right = 4'b0;
    rows[address].left.b = 2'b0;
    rows[address].left.a = value;
  end
  assign observed = rows[address];
endmodule
"#,
    )
    .expect("write forward nested sparse-memory member source");
    let forward_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&forward_path),
            Some("forward_memory_member"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower forward nested sparse-memory member");
    let forward_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&forward_path),
            Some("forward_memory_member"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize forward nested sparse-memory AST");
    let mut forward_summary = slang
        .summarize_ast_json(&forward_json)
        .expect("summarize forward nested sparse-memory AST");
    forward_summary
        .validate_against_design(&forward_design)
        .expect("forward nested packed layouts should resolve exactly");
    if let Some(assignment) = forward_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .memory_member_lvalue
                    .as_ref()
                    .is_some_and(|member| member.members.last().is_some_and(|name| name == "a"))
            })
        })
    {
        assignment
            .memory_member_lvalue
            .as_mut()
            .expect("forward nested sparse-memory member lvalue")
            .members[1] = "b".into();
    }
    let error = forward_summary
        .validate_against_design(&forward_design)
        .expect_err("wrong forward nested sparse-memory field must fail");
    assert!(
        error.contains("Range:5:0") && error.contains("Range:7:6") && error.contains("Range:3:0"),
        "unexpected structural error: {error}"
    );

    let mut forward_simulator =
        Simulator::new(&forward_design).expect("create forward sparse-memory simulator");
    forward_simulator
        .set_input("address", &LogicWord::from_u64(2, 2))
        .unwrap();
    forward_simulator
        .set_input("value", &LogicWord::from_u64(2, 3))
        .unwrap();
    forward_simulator.run_until(0).unwrap();
    assert_eq!(
        forward_simulator.read("observed").unwrap().to_u64(),
        Some(0b1100_0000),
        "forward nested packed layout should preserve physical field offset"
    );
}

#[test]
fn slang_ast_summary_uses_package_memory_member_layouts() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-package-memory-member");
    let source_path = root.join("package_memory_member.sv");
    std::fs::write(
        &source_path,
        r#"
package shapes_pkg;
  typedef struct packed {
    logic [1:0] a;
    logic [1:0] b;
  } inner_t;

  typedef struct packed {
    inner_t left;
    logic [3:0] right;
  } outer_t;
endpackage

module package_memory_member(
  input logic [1:0] address,
  input logic [1:0] value,
  output logic [7:0] observed
);
  import shapes_pkg::*;
  typedef struct packed {
    logic [7:0] byte_value;
  } outer_t;

  shapes_pkg::outer_t rows [0:3];
  always_comb begin
    rows[address].right = 4'b0;
    rows[address].left.b = 2'b0;
    rows[address].left.a = value;
  end
  assign observed = rows[address];
endmodule
"#,
    )
    .expect("write package sparse-memory member source");

    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("package_memory_member"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize package sparse-memory member AST");
    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("package_memory_member"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower package sparse-memory member");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize package sparse-memory member AST");
    summary
        .validate_against_design(&design)
        .expect("package sparse-memory member fingerprint should match");
    if let Some(assignment) = summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .memory_member_lvalue
                .as_ref()
                .is_some_and(|member| member.members.last().is_some_and(|name| name == "a"))
        })
    }) {
        assignment
            .memory_member_lvalue
            .as_mut()
            .expect("package sparse-memory member lvalue")
            .members[1] = "b".into();
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong package sparse-memory member field must fail");
    assert!(
        error.contains("Range:5:0") && error.contains("Range:7:6") && error.contains("Range:3:0"),
        "unexpected structural error: {error}"
    );

    let mut simulator = Simulator::new(&design).expect("create package sparse-memory simulator");
    simulator
        .set_input("address", &LogicWord::from_u64(2, 1))
        .unwrap();
    simulator
        .set_input("value", &LogicWord::from_u64(2, 3))
        .unwrap();
    simulator.run_until(0).unwrap();
    assert_eq!(
        simulator.read("observed").unwrap().to_u64(),
        Some(0b1100_0000),
        "package nested sparse-memory member write should preserve physical field offsets"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_union_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-packed-union-array");
    let source_path = root.join("packed_union_array.sv");
    std::fs::write(
        &source_path,
        r#"
module packed_union_array(
  input logic clk,
  input logic [1:0] index,
  input logic [15:0] word_value,
  input logic [15:0] raw_value,
  input logic use_word,
  output logic [15:0] word_view,
  output logic [15:0] raw_view,
  output logic [63:0] all_values
);
  typedef union packed {
    logic [15:0] word;
    logic [15:0] raw;
  } value_t;

  value_t values [0:3];
  initial begin
    values[0].raw = 16'h1111;
    values[1].raw = 16'h2222;
    values[2].raw = 16'h3333;
    values[3].raw = 16'h4444;
  end

  always @(posedge clk) begin
    if (use_word) values[index].word = word_value;
    else values[index].raw = raw_value;
  end

  assign word_view = values[index].word;
  assign raw_view = values[index].raw;
  assign all_values = {values[3], values[2], values[1], values[0]};
endmodule
"#,
    )
    .expect("write packed union array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("packed_union_array"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed union array");
    assert_eq!(design.memories.len(), 1);
    assert_eq!(design.memories[0].element_width, 16);
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);

    let mut simulator = Simulator::new(&design).expect("create packed union array simulator");
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 0))
        .unwrap();
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0x4444_3333_2222_1111)
    );

    for (index, word, raw) in [(0, 0x5555, 0x6666), (1, 0x7777, 0x8888)] {
        simulator
            .set_input("index", &LogicWord::from_u64(2, index))
            .unwrap();
        simulator
            .set_input("word_value", &LogicWord::from_u64(16, word))
            .unwrap();
        simulator
            .set_input("raw_value", &LogicWord::from_u64(16, raw))
            .unwrap();
        simulator
            .set_input("use_word", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(word));
        assert_eq!(simulator.read("raw_view").unwrap().to_u64(), Some(word));

        simulator
            .set_input("use_word", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("word_view").unwrap().to_u64(), Some(raw));
        assert_eq!(simulator.read("raw_view").unwrap().to_u64(), Some(raw));
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
    }
    assert_eq!(
        simulator.read("all_values").unwrap().to_u64(),
        Some(0x4444_3333_8888_6666)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_packed_struct_assignment_patterns() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-struct-patterns");
    let source_path = root.join("struct_patterns.sv");
    std::fs::write(
        &source_path,
        r#"
module struct_patterns(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic [7:0] positional,
  output logic [7:0] named
);
  typedef struct packed {
    logic [3:0] high;
    logic [3:0] low;
  } pair_t;

  pair_t positional_value;
  pair_t named_value;
  always_comb begin
    positional_value = '{a, b};
    named_value = '{high: b, low: a};
  end
  assign positional = positional_value;
  assign named = named_value;
endmodule
"#,
    )
    .expect("write packed struct assignment-pattern source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("struct_patterns"),
            &PreprocessOptions::default(),
        )
        .expect("lower packed struct assignment patterns");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create assignment-pattern simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(4, 0xa))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0x5))
        .unwrap();
    assert_eq!(simulator.read("positional").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("named").unwrap().to_u64(), Some(0x5a));
}

#[test]
fn slang_ast_direct_lowering_supports_enum_types() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-enum");
    let source_path = root.join("enum.sv");
    std::fs::write(
        &source_path,
        r#"
module enum_module(
  input logic [1:0] selector,
  output logic [1:0] raw,
  output logic is_b,
  output logic [1:0] cast_raw
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  state_t state;
  always_comb begin
    case (selector)
      A: state = A;
      B: state = B;
      default: state = C;
    endcase
  end
  assign raw = state;
  assign is_b = state == B;
  assign cast_raw = state_t'(2'b11);
endmodule
"#,
    )
    .expect("write enum source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("enum_module"),
            &PreprocessOptions::default(),
        )
        .expect("lower enum type");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 3);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "state")
            .expect("enum state declaration")
            .width,
        2
    );

    let mut simulator = Simulator::new(&design).expect("create enum simulator");
    for (selector, expected_raw, expected_is_b) in
        [(0b00, 0b00, false), (0b01, 0b01, true), (0b10, 0b10, false)]
    {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(simulator.read("raw").unwrap().to_u64(), Some(expected_raw));
        assert_eq!(
            simulator.read("is_b").unwrap().to_u64(),
            Some(expected_is_b as u64)
        );
    }
    assert_eq!(simulator.read("cast_raw").unwrap().to_u64(), Some(0b11));
}

#[test]
fn slang_ast_direct_lowering_supports_enum_next_and_prev() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-enum-next-prev");
    let source_path = root.join("enum_next_prev.sv");
    std::fs::write(
        &source_path,
        r#"
module enum_next_prev(
  input logic [1:0] selector,
  output logic [1:0] next_state,
  output logic [1:0] prev_state,
  output logic [1:0] next_by_two,
  output logic [1:0] constant_next,
  output logic [1:0] constant_prev,
  output logic [1:0] invalid_next,
  output logic [1:0] first_value,
  output logic [1:0] last_value,
  output int count
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  state_t state;
  always_comb state = state_t'(selector);
  assign next_state = state.next();
  assign prev_state = state.prev();
  assign next_by_two = state.next(2);
  assign constant_next = C.next();
  assign constant_prev = A.prev();
  assign invalid_next = state_t'(2'b11).next();
  assign first_value = state.first();
  assign last_value = state.last();
  assign count = state.num();
endmodule
"#,
    )
    .expect("write enum next/prev source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("enum_next_prev"),
            &PreprocessOptions::default(),
        )
        .expect("lower enum next/prev");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 9);

    let mut simulator = Simulator::new(&design).expect("create enum next/prev simulator");
    for (selector, expected_next, expected_prev, expected_by_two) in [
        (0b00, 0b01, 0b10, 0b10),
        (0b01, 0b10, 0b00, 0b00),
        (0b10, 0b00, 0b01, 0b01),
    ] {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(
            simulator.read("next_state").unwrap().to_u64(),
            Some(expected_next)
        );
        assert_eq!(
            simulator.read("prev_state").unwrap().to_u64(),
            Some(expected_prev)
        );
        assert_eq!(
            simulator.read("next_by_two").unwrap().to_u64(),
            Some(expected_by_two)
        );
    }

    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .unwrap();
    assert_eq!(
        simulator.read("constant_next").unwrap().to_u64(),
        Some(0b00)
    );
    assert_eq!(
        simulator.read("constant_prev").unwrap().to_u64(),
        Some(0b10)
    );
    assert_eq!(simulator.read("first_value").unwrap().to_u64(), Some(0b00));
    assert_eq!(simulator.read("last_value").unwrap().to_u64(), Some(0b10));
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(3));

    simulator
        .set_input("selector", &LogicWord::from_u64(2, 0b11))
        .unwrap();
    assert_eq!(simulator.read("invalid_next").unwrap().to_hex_str(), "x");
}

#[test]
fn slang_ast_direct_lowering_supports_string_literals_and_enum_names() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-strings");
    let source_path = root.join("strings.sv");
    std::fs::write(
        &source_path,
        r#"
module strings(
  input logic [1:0] selector,
  input string left,
  input string right,
  input string mixed,
  input string same,
  input string case_variant,
  input string decimal_text,
  input string hex_text,
  input string binary_text,
  input int byte_index,
  input int substr_left,
  input int substr_right,
  output string literal_value,
  output string joined,
  output string literal_joined,
  output string state_name,
  output string constant_name,
  output string alias_constant_name,
  output string invalid_name,
  output int left_length,
  output int right_length,
  output byte constant_byte,
  output byte dynamic_byte,
  output byte method_byte,
  output byte out_of_range,
  output string upper_value,
  output string lower_value,
  output string put_value,
  output string substr_value,
  output string empty_substr,
  output string out_of_range_substr,
  output string decimal_string,
  output string hex_string,
  output string binary_string,
  output int decimal_value,
  output int hex_value,
  output int binary_value,
  output logic strings_equal,
  output logic strings_less,
  output int compare_result,
  output int case_compare_result,
  output int case_insensitive_compare_result
);
  typedef enum logic [1:0] {
    A = 2'b00,
    B = 2'b01,
    C = 2'b10
  } state_t;

  typedef bit [1:0] alias_base_t;
  typedef enum alias_base_t {
    ALIAS_A = 2'b00,
    ALIAS_B = 2'b01
  } alias_state_t;

  string literal_source = "aBcD";
  int conversion_source = -42;
  state_t state;
  always_comb begin
    state = state_t'(selector);
    constant_byte = right[1];
    dynamic_byte = right[byte_index];
    method_byte = right.getc(byte_index);
    out_of_range = right.getc(100);
  end
  initial begin
    put_value = literal_source;
    put_value.putc(1, 8'h5A);
    put_value.putc(64, 8'h21);
    put_value.putc(0, 8'h00);
    decimal_string.itoa(conversion_source);
    hex_string.hextoa(conversion_source);
    binary_string.bintoa(conversion_source);
  end
  assign literal_value = "READY";
  assign joined = {left, right};
  assign literal_joined = {"AB", "CD"};
  assign state_name = state.name();
  assign constant_name = B.name();
  assign alias_constant_name = ALIAS_B.name();
  assign invalid_name = state_t'(2'b11).name();
  assign left_length = left.len();
  assign right_length = right.len();
  assign upper_value = mixed.toupper();
  assign lower_value = mixed.tolower();
  assign strings_equal = mixed == same;
  assign strings_less = mixed < right;
  assign compare_result = mixed.compare(case_variant);
  assign case_compare_result = mixed.compare(case_variant);
  assign case_insensitive_compare_result = mixed.icompare(case_variant);
  assign substr_value = mixed.substr(substr_left, substr_right);
  assign empty_substr = mixed.substr(2, 1);
  assign out_of_range_substr = mixed.substr(0, 64);
  assign decimal_value = decimal_text.atoi();
  assign hex_value = hex_text.atohex();
  assign binary_value = binary_text.atobin();
endmodule
"#,
    )
    .expect("write string and enum-name source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("strings"),
            &PreprocessOptions::default(),
        )
        .expect("lower strings and enum names");
    assert_eq!(design.processes.len(), 1);
    assert_eq!(design.assigns.len(), 24);
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "literal_value")
            .expect("string output declaration")
            .width,
        512
    );
    let string_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("strings"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize string AST");
    let string_summary = slang
        .summarize_ast_json(&string_json)
        .expect("summarize string AST");
    string_summary
        .validate_against_design(&design)
        .expect("exact direct string method fingerprints should match");

    let mut enum_receiver_summary = string_summary.clone();
    if let Some(target) = enum_receiver_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "state_name")
    {
        target.rhs.operands[0].symbol = Some("wrong_state".into());
    }
    let enum_receiver_error = enum_receiver_summary
        .validate_against_design(&design)
        .expect_err("wrong enum-name receiver must fail");
    assert!(
        enum_receiver_error.contains("Ref:2:u[wrong_state]")
            && enum_receiver_error.contains("Ref:2:u[state]"),
        "unexpected enum-name receiver error: {enum_receiver_error}"
    );

    let mut enum_value_summary = string_summary.clone();
    if let Some(definition) = enum_value_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "state_name")
        .and_then(|target| target.rhs.enum_definition.as_mut())
    {
        definition.members[0].literal = "2:11".into();
    }
    let enum_value_error = enum_value_summary
        .validate_against_design(&design)
        .expect_err("wrong enum-name value must fail");
    assert!(
        enum_value_error.contains("Const:2:u[2:00]")
            && enum_value_error.contains("Const:2:u[2:11]"),
        "unexpected enum-name value error: {enum_value_error}"
    );

    let mut enum_member_name_summary = string_summary.clone();
    if let Some(definition) = enum_member_name_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "state_name")
        .and_then(|target| target.rhs.enum_definition.as_mut())
    {
        definition.members[1].name = "WRONG".into();
    }
    assert!(
        enum_member_name_summary
            .validate_against_design(&design)
            .is_err(),
        "wrong enum member name must fail"
    );

    let mut enum_order_summary = string_summary.clone();
    if let Some(definition) = enum_order_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "state_name")
        .and_then(|target| target.rhs.enum_definition.as_mut())
    {
        definition.members.swap(0, 1);
    }
    let enum_order_error = enum_order_summary
        .validate_against_design(&design)
        .expect_err("wrong enum member order must fail");
    assert!(
        enum_order_error.contains("state_name") && enum_order_error.contains("Cond:512:u["),
        "unexpected enum member order error: {enum_order_error}"
    );

    let mut case_summary = string_summary.clone();
    if let Some(target) = case_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "upper_value")
    {
        target.rhs.operator = "tolower".into();
    }
    let case_error = case_summary
        .validate_against_design(&design)
        .expect_err("wrong string case direction must fail");
    assert!(
        case_error.contains("StringCase:512:u[Upper:Ref:512:u[mixed]]")
            && case_error.contains("StringCase:512:u[Lower:Ref:512:u[mixed]]"),
        "unexpected string case error: {case_error}"
    );

    let mut comparison_summary = string_summary.clone();
    if let Some(target) = comparison_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "compare_result")
    {
        target.rhs.operator = "icompare".into();
    }
    let comparison_error = comparison_summary
        .validate_against_design(&design)
        .expect_err("wrong string comparison mode must fail");
    assert!(
        comparison_error.contains("StringCompare:32:s[CaseSensitive:")
            && comparison_error.contains("StringCompare:32:s[IgnoreCase:"),
        "unexpected string comparison error: {comparison_error}"
    );

    let mut concatenation_summary = string_summary.clone();
    if let Some(target) = concatenation_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "joined")
    {
        target.rhs.operands[1].symbol = Some("wrong_right".into());
    }
    let concatenation_error = concatenation_summary
        .validate_against_design(&design)
        .expect_err("wrong string concatenation operand must fail");
    assert!(
        concatenation_error.contains("StringConcat:512:u[")
            && concatenation_error.contains("Ref:512:u[wrong_right]")
            && concatenation_error.contains("Ref:512:u[right]"),
        "unexpected string concatenation error: {concatenation_error}"
    );

    let mut literal_conversion_summary = string_summary.clone();
    if let Some(target) = literal_conversion_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "literal_value")
    {
        target.rhs.operands[0].literal = Some(format!("40:{}", "1".repeat(40)));
    }
    let literal_conversion_error = literal_conversion_summary
        .validate_against_design(&design)
        .expect_err("wrong string literal conversion must fail");
    let ready_bits = LogicWord::from_bytes_le(40, b"READY").to_bin_str();
    assert!(
        literal_conversion_error.contains(&format!("Const:40:u[40:{ready_bits}]"))
            && literal_conversion_error.contains(&format!("Const:40:u[40:{}]", "1".repeat(40))),
        "unexpected string literal conversion error: {literal_conversion_error}"
    );

    let mut byte_summary = string_summary.clone();
    if let Some(assignment) = byte_summary.procedural_blocks.iter_mut().find_map(|block| {
        block.assignments.iter_mut().find(|assignment| {
            assignment
                .target_names
                .first()
                .is_some_and(|name| name == "dynamic_byte")
        })
    }) {
        assignment.rhs.operands[1].symbol = Some("wrong_byte_index".into());
    }
    let byte_error = byte_summary
        .validate_against_design(&design)
        .expect_err("wrong string byte index must fail");
    assert!(
        byte_error.contains("Ref:32:s[wrong_byte_index]")
            && byte_error.contains("Ref:32:s[byte_index]"),
        "unexpected string byte error: {byte_error}"
    );

    let mut radix_summary = string_summary;
    if let Some(target) = radix_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "decimal_value")
    {
        target.rhs.operands[0].operator = "atohex".into();
    }
    let radix_error = radix_summary
        .validate_against_design(&design)
        .expect_err("wrong string parse radix must fail");
    assert!(
        radix_error.contains("StringToRadix:32:s[Radix10:")
            && radix_error.contains("StringToRadix:32:s[Radix16:"),
        "unexpected string radix error: {radix_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create string simulator");
    for (selector, name) in [(0, "A"), (1, "B"), (2, "C")] {
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        simulator
            .set_input(
                "left",
                &LogicWord::from_bytes_le(512, selector.to_string().as_bytes()),
            )
            .unwrap();
        simulator
            .set_input("right", &LogicWord::from_bytes_le(512, b"-STATE"))
            .unwrap();
        simulator
            .set_input("mixed", &LogicWord::from_bytes_le(512, b"aBcD"))
            .unwrap();
        simulator
            .set_input("same", &LogicWord::from_bytes_le(512, b"aBcD"))
            .unwrap();
        simulator
            .set_input("case_variant", &LogicWord::from_bytes_le(512, b"AbCd"))
            .unwrap();
        simulator
            .set_input(
                "decimal_text",
                &LogicWord::from_bytes_le(512, b"  -042junk"),
            )
            .unwrap();
        simulator
            .set_input("hex_text", &LogicWord::from_bytes_le(512, b" -2fUVM"))
            .unwrap();
        simulator
            .set_input("binary_text", &LogicWord::from_bytes_le(512, b" +0101uvm"))
            .unwrap();
        simulator
            .set_input("byte_index", &LogicWord::from_u64(32, selector))
            .unwrap();
        simulator
            .set_input("substr_left", &LogicWord::from_u64(32, 1))
            .unwrap();
        simulator
            .set_input("substr_right", &LogicWord::from_u64(32, 2))
            .unwrap();
        assert_eq!(
            simulator
                .read("literal_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "READY"
        );
        assert_eq!(
            simulator
                .read("state_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            name
        );
        assert_eq!(
            simulator
                .read("constant_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "B"
        );
        assert_eq!(
            simulator
                .read("alias_constant_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ALIAS_B"
        );
        assert_eq!(
            simulator
                .read("invalid_name")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        let joined = format!("{selector}-STATE");
        assert_eq!(
            simulator
                .read("joined")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            joined
        );
        assert_eq!(
            simulator
                .read("literal_joined")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ABCD"
        );
        assert_eq!(
            simulator.read("left_length").unwrap().to_u64(),
            Some(selector.to_string().len() as u64)
        );
        assert_eq!(simulator.read("right_length").unwrap().to_u64(), Some(6));
        assert_eq!(
            simulator.read("constant_byte").unwrap().to_u64(),
            Some(b'S' as u64)
        );
        let expected_byte = b"-STATE"[selector as usize] as u64;
        assert_eq!(
            simulator.read("dynamic_byte").unwrap().to_u64(),
            Some(expected_byte)
        );
        assert_eq!(
            simulator.read("method_byte").unwrap().to_u64(),
            Some(expected_byte)
        );
        assert_eq!(simulator.read("out_of_range").unwrap().to_u64(), Some(0));
        assert_eq!(
            simulator
                .read("upper_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ABCD"
        );
        assert_eq!(
            simulator
                .read("lower_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "abcd"
        );
        assert_eq!(simulator.read("strings_equal").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("strings_less").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("compare_result").unwrap().to_u64(), Some(1));
        assert_eq!(
            simulator.read("case_compare_result").unwrap().to_u64(),
            Some(1)
        );
        assert_eq!(
            simulator
                .read("case_insensitive_compare_result")
                .unwrap()
                .to_u64(),
            Some(0)
        );
        assert_eq!(
            simulator
                .read("put_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "aZcD"
        );
        assert_eq!(
            simulator
                .read("substr_value")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "Bc"
        );
        assert_eq!(
            simulator
                .read("empty_substr")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        assert_eq!(
            simulator
                .read("out_of_range_substr")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            ""
        );
        assert_eq!(
            simulator
                .read("decimal_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "-42"
        );
        assert_eq!(
            simulator
                .read("hex_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "ffffffd6"
        );
        assert_eq!(
            simulator
                .read("binary_string")
                .unwrap()
                .to_ascii_string_nul_terminated(),
            "11111111111111111111111111010110"
        );
        assert_eq!(
            simulator.read("decimal_value").unwrap().to_u64(),
            Some((-42_i32) as u32 as u64)
        );
        assert_eq!(
            simulator.read("hex_value").unwrap().to_u64(),
            Some((-47_i32) as u32 as u64)
        );
        assert_eq!(simulator.read("binary_value").unwrap().to_u64(), Some(5));
    }

    let bounded_source_path = root.join("strings-bounded.sv");
    std::fs::write(
        &bounded_source_path,
        r#"
module strings_bounded(
  output string put_value,
  output string substr_value
);
  string literal_source = "aBcD";
  initial begin
    substr_value = literal_source.substr(1, 2);
    literal_source.putc(1, 8'h5A);
    literal_source.putc(64, 8'h21);
    literal_source.putc(0, 8'h00);
    put_value = literal_source;
  end
endmodule
"#,
    )
    .expect("write bounded string source");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&bounded_source_path),
            Some("strings_bounded"),
            &PreprocessOptions::default(),
        )
        .expect("lower strings through the bounded compatibility path");
    let bounded_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&bounded_source_path),
            Some("strings_bounded"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize bounded string AST");
    let mut bounded_summary = slang
        .summarize_ast_json(&bounded_json)
        .expect("summarize bounded string AST");
    bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact bounded string literal fingerprints should match");
    if let Some(target) = bounded_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "literal_source")
    {
        target.rhs.operands[0].literal = Some(format!("32:{}", "1".repeat(32)));
    }
    let bounded_literal_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong bounded string literal must fail");
    assert!(
        bounded_literal_error.contains("Const:512:u[512:")
            && bounded_literal_error.contains(&"1".repeat(32)),
        "unexpected bounded string literal error: {bounded_literal_error}"
    );
    let mut bounded_simulator = Simulator::new(&bounded).expect("create bounded string simulator");
    bounded_simulator.run_until(0).expect("run bounded strings");
    assert_eq!(
        bounded_simulator
            .read("put_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "aZcD"
    );
    assert_eq!(
        bounded_simulator
            .read("substr_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "Bc"
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_event_sensitivity_lists() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-events");
    let source_path = root.join("events.sv");
    std::fs::write(
        &source_path,
        r#"
module events(
  input logic a,
  input logic b,
  input logic c,
  input logic clk,
  input logic rst,
  input logic [3:0] vector,
  output logic level_y,
  output logic edge_q,
  output logic bit_q
);
  always @(a or b) level_y = a & c;
  always @(posedge clk or posedge rst) begin
    if (rst) begin
      edge_q <= 1'b0;
    end else begin
      edge_q <= ~edge_q;
    end
  end
  always @(posedge vector[0]) bit_q <= vector[1];
endmodule
"#,
    )
    .expect("write event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event sensitivity lists");
    assert_eq!(design.processes.len(), 3);
    assert!(matches!(
        &design.processes[0].sens,
        eda_core::ir::Sens::List(list)
            if matches!(list.as_slice(), [eda_core::ir::Sens::Any(_), eda_core::ir::Sens::Any(_)])
    ));
    assert!(matches!(
        &design.processes[1].sens,
        eda_core::ir::Sens::List(list)
            if matches!(list.as_slice(), [eda_core::ir::Sens::Posedge(_), eda_core::ir::Sens::Posedge(_)])
    ));

    let reference = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("lower event sensitivity lists through the bounded path");
    assert_eq!(reference.processes.len(), design.processes.len());

    let run_events = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create event-list simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.set_input("c", &LogicWord::zero(1)).unwrap();
        simulator.set_input("b", &LogicWord::zero(1)).unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(0));

        simulator
            .set_input("c", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(0));

        simulator
            .set_input("b", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("level_y").unwrap().to_u64(), Some(1));

        simulator
            .set_input("rst", &LogicWord::from_u64(1, 1))
            .unwrap();
        assert_eq!(simulator.read("edge_q").unwrap().to_u64(), Some(0));
        simulator.set_input("rst", &LogicWord::zero(1)).unwrap();
        for expected in [1, 0, 1] {
            simulator
                .set_input("clk", &LogicWord::from_u64(1, 1))
                .unwrap();
            assert_eq!(simulator.read("edge_q").unwrap().to_u64(), Some(expected));
            simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        }

        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0010))
            .unwrap();
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0011))
            .unwrap();
        assert_eq!(simulator.read("bit_q").unwrap().to_u64(), Some(1));
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0010))
            .unwrap();
        simulator
            .set_input("vector", &LogicWord::from_u64(4, 0b0011))
            .unwrap();
        assert_eq!(simulator.read("bit_q").unwrap().to_u64(), Some(1));
    };
    run_events(&design);
    run_events(&reference);
}

#[test]
fn slang_ast_direct_lowering_supports_named_event_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-events");
    let source_path = root.join("events.sv");
    std::fs::write(
        &source_path,
        r#"
module events(input logic go, output logic [3:0] count, output logic done);
  event e;
  initial begin
    count = 4'd0;
    @(e);
    count = count + 1;
    @(e);
    count = count + 1;
    done = 1'b1;
  end

  initial begin
    wait (go) ->e;
    #1 ->e;
  end
endmodule
"#,
    )
    .expect("write named-event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named events");
    let mut simulator = Simulator::new(&design).expect("create named-event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("done").unwrap().to_u64(), None);
    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_event_list_controls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-list");
    let source_path = root.join("event_list.sv");
    std::fs::write(
        &source_path,
        r#"
module event_list(input logic go, output logic y);
  event first;
  event second;
  initial begin
    y = 1'b0;
    @(first or second);
    y = 1'b1;
  end

  initial begin
    wait (go);
    ->second;
  end
endmodule
"#,
    )
    .expect("write event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_list"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-list control");
    let mut simulator = Simulator::new(&design).expect("create event-list simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_edge_qualified_event_lists() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-edge-event-list");
    let source_path = root.join("edge_event_list.sv");
    std::fs::write(
        &source_path,
        r#"
module edge_event_list(
  input logic clk,
  input logic go,
  input logic reset,
  output logic rose
);
  initial begin
    rose = 1'b0;
    wait (go);
    @(posedge clk or negedge reset);
    rose = clk;
    @(posedge clk);
    rose = 1'b1;
  end
endmodule
"#,
    )
    .expect("write edge-qualified event-list source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("edge_event_list"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower edge-qualified event list");
    let mut simulator = Simulator::new(&design).expect("create edge-event simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("reset", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(0));
    simulator.set_input("reset", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("rose").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_event_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array");
    let source_path = root.join("event_array.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array(
  input logic go,
  input int index,
  input int wrong,
  output logic first,
  output logic second,
  output logic triggered_now,
  output logic triggered_later
);
  event events[2];
  initial begin
    first = 1'b0;
    second = 1'b0;
    wait (go);
    fork
      begin
        @(events[index]);
        #2 first = 1'b1;
        @(events[index]);
        second = 1'b1;
      end
      begin
        #1 ->events[wrong];
        #1 ->events[index];
        #3 ->events[index];
      end
    join
  end
  initial begin
    triggered_now = 1'b0;
    triggered_later = 1'b0;
    wait (go);
    ->events[index];
    triggered_now = events[index].triggered;
    #1 triggered_later = events[index].triggered;
  end
endmodule
"#,
    )
    .expect("write event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event array");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "events")
            .expect("event-array declaration")
            .width,
        2
    );
    let mut simulator = Simulator::new(&design).expect("create event-array simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(32, 0))
        .unwrap();
    simulator
        .set_input("wrong", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(10).unwrap();
    assert_eq!(simulator.read("triggered_now").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("triggered_later").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_rejects_runtime_out_of_range_event_waits() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-range");
    let source_path = root.join("event_array_range.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_range(input logic go, input int index);
  event events[2];
  initial begin
    wait (go);
    @(events[index]);
  end
endmodule
"#,
    )
    .expect("write out-of-range event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_range"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-array range source");
    let mut simulator = Simulator::new(&design).expect("create event-array range simulator");
    simulator
        .set_input("index", &LogicWord::from_u64(32, 2))
        .unwrap();
    let error = simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .expect_err("runtime out-of-range event waits must fail closed");
    assert!(
        error.contains("event array index is out of range"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_constant_nonblocking_event_array_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-constant");
    let source_path = root.join("event_array_constant.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_constant(input logic go, output logic done);
  event events[2];
  initial begin
    done = 1'b0;
    wait (go);
    fork
      @(events[1]) done = 1'b1;
      #1 ->>events[1];
    join
  end
endmodule
"#,
    )
    .expect("write constant event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_constant"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower constant event-array trigger");
    let mut simulator = Simulator::new(&design).expect("create constant event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_multidimensional_event_arrays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-array-2d");
    let source_path = root.join("event_array_2d.sv");
    std::fs::write(
        &source_path,
        r#"
module event_array_2d(
  input logic go,
  input int row,
  input int column,
  output logic done
);
  event events[1:0][1:0];
  initial begin
    done = 1'b0;
    wait (go);
    fork
      @(events[row][column]) done = 1'b1;
      begin
        #1 ->events[1 - row][column];
        #1 ->events[row][column];
      end
    join
  end
endmodule
"#,
    )
    .expect("write multidimensional event-array source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_array_2d"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower multidimensional event array");
    assert_eq!(
        design
            .net_decls
            .iter()
            .find(|declaration| declaration.name == "events")
            .expect("multidimensional event-array declaration")
            .width,
        4
    );
    let mut simulator = Simulator::new(&design).expect("create multidimensional event simulator");
    simulator
        .set_input("row", &LogicWord::from_u64(32, 1))
        .unwrap();
    simulator
        .set_input("column", &LogicWord::from_u64(32, 0))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.run_until(3).unwrap();
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_block_events() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-event");
    let source_path = root.join("clocking_event.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_event(
  input logic clk,
  input logic go,
  input logic sampled_input,
  output logic first_sample,
  output logic second_sample
);
  clocking cb @(posedge clk);
    input sampled_input;
  endclocking

  initial begin
    first_sample = 1'b0;
    second_sample = 1'b0;
    wait (go);
    @(cb);
    first_sample = cb.sampled_input;
    @(cb);
    second_sample = cb.sampled_input;
  end
endmodule
"#,
    )
    .expect("write clocking-event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_event"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking-block event");
    let mut simulator = Simulator::new(&design).expect("create clocking simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::zero(1))
        .unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first_sample").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first_sample").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("second_sample").unwrap().to_u64(), Some(0));
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second_sample").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_rejects_unsupported_clocking_semantics() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-unsupported");

    let skew_path = root.join("clocking_skew.sv");
    std::fs::write(
        &skew_path,
        r#"
module clocking_skew(input logic clk, input logic a);
  clocking cb @(posedge clk);
    input #1step a;
  endclocking
endmodule
"#,
    )
    .expect("write clocking-skew source");
    let skew_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&skew_path),
            Some("clocking_skew"),
            &PreprocessOptions::default(),
        )
        .expect_err("clocking skew must fail closed");
    assert!(
        skew_error.contains("skew is not supported"),
        "unexpected skew error: {skew_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_output_drives() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-drive");
    let source_path = root.join("clocking_drive.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_drive(
  input logic clk,
  input logic a,
  output logic b
);
  clocking cb @(posedge clk);
    input a;
    output b;
  endclocking

  initial begin
    b = 1'b0;
    @(cb);
    cb.b <= cb.a;
  end
endmodule
"#,
    )
    .expect("write clocking-drive source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_drive"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking output drive");
    let mut simulator = Simulator::new(&design).expect("create clocking-drive simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("b").unwrap().to_u64(), Some(0));
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("b").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_clocking_cycle_delays() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-cycle-delay");
    let source_path = root.join("cycle_delay.sv");
    std::fs::write(
        &source_path,
        r#"
module cycle_delay(
  input logic clk,
  input logic go,
  output logic zero_cycle,
  output logic first,
  output logic second
);
  default clocking cb @(posedge clk);
  endclocking

  initial begin
    zero_cycle = 1'b0;
    first = 1'b0;
    second = 1'b0;
    wait (go);
    ##0 zero_cycle = 1'b1;
    ##1 first = 1'b1;
    ##2 second = 1'b1;
  end
endmodule
"#,
    )
    .expect("write clocking cycle-delay source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("cycle_delay"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower clocking cycle delays");
    let mut simulator = Simulator::new(&design).expect("create cycle-delay simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("zero_cycle").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(0));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_preserves_clocking_blocks_across_hierarchy() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-clocking-hierarchy");
    let source_path = root.join("clocking_hierarchy.sv");
    std::fs::write(
        &source_path,
        r#"
module clocking_child(
  input logic clk,
  input logic sampled_input,
  output logic driven_output
);
  default clocking cb @(posedge clk);
    input sampled_input;
    output driven_output;
  endclocking

  initial begin
    driven_output = 1'b0;
    @(cb);
    cb.driven_output <= cb.sampled_input;
    ##1 cb.driven_output <= 1'b0;
  end
endmodule

module clocking_hierarchy(
  input logic clk,
  input logic sampled_input,
  output logic driven_output
);
  clocking_child child(
    .clk(clk),
    .sampled_input(sampled_input),
    .driven_output(driven_output)
  );
endmodule
"#,
    )
    .expect("write hierarchical clocking source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("clocking_hierarchy"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower hierarchical clocking block");
    assert_eq!(design.clocking_blocks.len(), 1);
    let mut simulator = Simulator::new(&design).expect("create hierarchical clocking simulator");
    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("sampled_input", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(0));

    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(1));

    simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
    simulator
        .set_input("clk", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("driven_output").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_supports_nonblocking_event_triggers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-nonblocking-events");
    let source_path = root.join("nonblocking_events.sv");
    std::fs::write(
        &source_path,
        r#"
module nonblocking_events(
  input logic go,
  output logic ready,
  output logic observed_ready,
  output logic waiter_done
);
  event e;
  initial begin
    ready = 1'b0;
    observed_ready = 1'b0;
    waiter_done = 1'b0;
    @(e);
    observed_ready = ready;
    waiter_done = 1'b1;
  end

  initial begin
    wait (go);
    ready = 1'b1;
    ->>e;
  end
endmodule
"#,
    )
    .expect("write nonblocking event source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("nonblocking_events"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower nonblocking event trigger");
    let mut simulator = Simulator::new(&design).expect("create nonblocking event simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("ready").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("observed_ready").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("waiter_done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_force_and_release() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-force");
    let source_path = root.join("force.sv");
    std::fs::write(
        &source_path,
        r#"
module force_release(
  input logic a,
  input logic en,
  output logic y,
  output logic released
);
  assign y = a;
  initial begin
    released = 1'b0;
    wait (en);
    force y = 1'b0;
    #2;
    release y;
    released = 1'b1;
  end
endmodule
"#,
    )
    .expect("write force/release source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("force_release"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower force/release");
    let mut simulator = Simulator::new(&design).expect("create force/release simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));

    simulator
        .set_input("en", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));

    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
    simulator.run_until(2).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("released").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_wait_order() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wait-order");
    let ordered_path = root.join("ordered.sv");
    std::fs::write(
        &ordered_path,
        r#"
module ordered(input logic go, output logic done);
  event first;
  event second;
  initial begin
    done = 1'b0;
    wait_order(first, second);
    done = 1'b1;
  end

  initial begin
    wait (go);
    ->first;
    #1;
    ->second;
  end
endmodule
"#,
    )
    .expect("write ordered wait_order source");
    let ordered_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&ordered_path),
            Some("ordered"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower ordered wait_order");
    let mut ordered_simulator =
        Simulator::new(&ordered_design).expect("create ordered wait_order simulator");
    ordered_simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(ordered_simulator.read("done").unwrap().to_u64(), Some(0));
    ordered_simulator.run_until(1).unwrap();
    assert_eq!(ordered_simulator.read("done").unwrap().to_u64(), Some(1));

    let out_of_order_path = root.join("out-of-order.sv");
    std::fs::write(
        &out_of_order_path,
        r#"
module out_of_order(input logic go, output int status);
  event first;
  event second;
  initial begin
    status = 0;
    wait_order(first, second) status = 1; else status = 2;
  end

  initial begin
    wait (go);
    ->second;
  end
endmodule
"#,
    )
    .expect("write out-of-order wait_order source");
    let out_of_order_design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&out_of_order_path),
            Some("out_of_order"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower out-of-order wait_order");
    let mut out_of_order_simulator =
        Simulator::new(&out_of_order_design).expect("create out-of-order simulator");
    out_of_order_simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(
        out_of_order_simulator.read("status").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_event_triggered() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-event-triggered");
    let source_path = root.join("event_triggered.sv");
    std::fs::write(
        &source_path,
        r#"
module event_triggered(
  input logic go,
  output logic was_triggered,
  output logic later_not_triggered
);
  event e;
  initial begin
    was_triggered = 1'b0;
    later_not_triggered = 1'b0;
    @(e);
    was_triggered = e.triggered;
    #1;
    later_not_triggered = !e.triggered;
  end

  initial begin
    wait (go);
    ->e;
  end
endmodule
"#,
    )
    .expect("write event-triggered source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("event_triggered"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower event-triggered check");
    let mut simulator = Simulator::new(&design).expect("create event-triggered simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("was_triggered").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("later_not_triggered").unwrap().to_u64(),
        Some(0)
    );
    simulator.run_until(1).unwrap();
    assert_eq!(
        simulator.read("later_not_triggered").unwrap().to_u64(),
        Some(1)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_final_blocks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-final-blocks");
    let source_path = root.join("final_blocks.sv");
    std::fs::write(
        &source_path,
        r#"
module final_blocks(
  input logic go,
  output logic y,
  output logic finalized
);
  initial begin
    y = 1'b0;
    finalized = 1'b0;
    wait (go);
    #1 y = 1'b1;
    $finish;
  end

  final begin
    finalized = 1'b1;
  end
endmodule
"#,
    )
    .expect("write final-block source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("final_blocks"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower final blocks");
    assert_eq!(design.final_blocks.len(), 1);
    let mut simulator = Simulator::new(&design).expect("create final-block simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("finalized").unwrap().to_u64(), Some(0));
    simulator.run_until(1).unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert!(simulator.finished());
    assert_eq!(simulator.read("finalized").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_constant_delay_controls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-delays");
    let source_path = root.join("delays.sv");
    std::fs::write(
        &source_path,
        r#"
module delays(
  input logic a,
  input logic clk,
  output logic y,
  output logic q,
  output logic clock,
  output logic delayed_q
);
  initial begin
    q = 1'b0;
    clock = 1'b0;
    delayed_q = 1'b0;
  end
  initial begin
    #5 y = a;
    #3 y = ~a;
  end
  always #5 q = ~q;
  initial forever #5 clock = ~clock;
  always @(posedge clk) #2 delayed_q = ~delayed_q;
endmodule
"#,
    )
    .expect("write delay-control source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("delays"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower constant delay controls");
    let reference = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("delays"),
            &PreprocessOptions::default(),
        )
        .expect("lower constant delay controls through the bounded path");
    assert_eq!(design.initials.len(), 3);
    assert_eq!(design.processes.len(), 2);
    assert_eq!(reference.initials.len(), design.initials.len());
    assert_eq!(reference.processes.len(), design.processes.len());

    let run_delays = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create delay simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), None);
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(0));
        simulator.run_until(4).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), None);
        simulator.run_until(5).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator.run_until(8).unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator.run_until(10).unwrap();
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(0));
        simulator.run_until(15).unwrap();
        assert_eq!(simulator.read("q").unwrap().to_u64(), Some(1));
        assert_eq!(simulator.read("clock").unwrap().to_u64(), Some(1));
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.run_until(17).unwrap();
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(1));
        simulator.set_input("clk", &LogicWord::zero(1)).unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator.run_until(19).unwrap();
        assert_eq!(simulator.read("delayed_q").unwrap().to_u64(), Some(0));
    };
    run_delays(&design);
    run_delays(&reference);

    let min_typ_max_path = root.join("min-typ-max.sv");
    std::fs::write(
        &min_typ_max_path,
        r#"
module min_typ_max(input logic a, output logic y);
  initial #(1:2:3) y = a;
endmodule
"#,
    )
    .expect("write min:typ:max delay source");
    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&min_typ_max_path),
            Some("min_typ_max"),
            &PreprocessOptions::default(),
        )
        .expect_err("min:typ:max delays must fail closed");
    assert!(
        error.contains("min:typ:max delay controls are not supported"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_wait_statements() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wait");
    let source_path = root.join("wait.sv");
    std::fs::write(
        &source_path,
        r#"
module waits(
  input logic a,
  input logic b,
  output logic y,
  output logic z,
  output logic invert
);
  initial begin
    wait (a && b) y = 1'b1;
    wait (a) z = b;
  end
  initial wait (!a) invert = 1'b1;
endmodule
"#,
    )
    .expect("write wait source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("waits"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wait statements");
    assert_eq!(design.initials.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create wait simulator");
    simulator
        .set_input("b", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), None);
    assert_eq!(simulator.read("z").unwrap().to_u64(), None);
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);

    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);

    simulator.run_until(10).unwrap();
    assert_eq!(simulator.read("invert").unwrap().to_u64(), None);
    simulator.set_input("a", &LogicWord::zero(1)).unwrap();
    assert_eq!(simulator.read("invert").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_repeat_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-repeat");
    let source_path = root.join("repeat.sv");
    std::fs::write(
        &source_path,
        r#"
module repeats(
  input logic a,
  input int dynamic_count,
  output logic [3:0] y,
  output logic [3:0] immediate,
  output logic zero,
  output logic [3:0] dynamic
);
  initial begin
    y = 4'd0;
    repeat (4) begin
      #1 y = y + 1;
    end
  end

  initial begin
    immediate = 4'd0;
    repeat (2) immediate = immediate + 1;

    zero = 1'b0;
    repeat (0) zero = 1'b1;
  end

  initial begin
    #1 dynamic = 4'd0;
    repeat (dynamic_count) dynamic = dynamic + 1;
  end
endmodule
"#,
    )
    .expect("write repeat-loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("repeats"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower repeat loops");

    let run_repeat = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create repeat simulator");
        simulator
            .set_input("a", &LogicWord::from_u64(1, 1))
            .unwrap();
        simulator
            .set_input("dynamic_count", &LogicWord::from_u64(32, 3))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(0));
        assert_eq!(simulator.read("immediate").unwrap().to_u64(), Some(2));
        assert_eq!(simulator.read("zero").unwrap().to_u64(), Some(0));
        for expected in 1..=4 {
            simulator.run_until(expected).unwrap();
            assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
        }
        assert_eq!(simulator.read("dynamic").unwrap().to_u64(), Some(3));
    };
    // The bounded compatibility frontend still rejects module-level repeat
    // loops; this regression intentionally covers the authoritative direct AST
    // path until the legacy elaborator catches up.
    run_repeat(&design);

    let excessive_path = root.join("excessive.sv");
    std::fs::write(
        &excessive_path,
        r#"
module excessive(output logic y);
  initial repeat (70000) y = 1'b1;
endmodule
"#,
    )
    .expect("write excessive repeat source");
    let excessive = slang
        .compile_files_via_ast(
            std::slice::from_ref(&excessive_path),
            Some("excessive"),
            &PreprocessOptions::default(),
        )
        .expect("lower excessive repeat loop");
    let mut excessive_simulator = Simulator::new(&excessive).expect("create excessive simulator");
    let error = excessive_simulator
        .run_until(0)
        .expect_err("excessive repeat loops must fail closed");
    assert!(
        error.contains("exceeds the 65536-iteration execution limit"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_while_and_for_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-runtime-loops");
    let source_path = root.join("loops.sv");
    std::fs::write(
        &source_path,
        r#"
module loops(
  input logic a,
  output logic [3:0] for_value,
  output logic [3:0] increment_value,
  output logic [3:0] while_value
);
  int i;
  int j;
  int k;
  initial begin
    for_value = 4'd0;
    for (i = 0; i < 4; i = i + 1) begin
      for_value[i] = 1'b1;
    end

    increment_value = 4'd0;
    for (j = 0; j < 3; j++) begin
      increment_value = increment_value + 1;
    end

    while_value = 4'd0;
    k = 0;
    while (k < 3) begin
      #1 while_value = while_value + 1;
      k++;
    end
  end
endmodule
"#,
    )
    .expect("write runtime loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("loops"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower runtime while and for loops");
    let mut simulator = Simulator::new(&design).expect("create runtime loop simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("for_value").unwrap().to_u64(), Some(0b1111));
    assert_eq!(simulator.read("increment_value").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("while_value").unwrap().to_u64(), Some(0));
    for expected in 1..=3 {
        simulator.run_until(expected).unwrap();
        assert_eq!(
            simulator.read("while_value").unwrap().to_u64(),
            Some(expected)
        );
    }

    let inline_local_path = root.join("inline-local.sv");
    std::fs::write(
        &inline_local_path,
        r#"
module inline_local(output logic [3:0] y, output logic [3:0] z);
  initial begin
    y = 4'd0;
    z = 4'd0;
    for (int i = 0; i < 4; i++) y[i] = 1'b1;
    for (int i = 0; i < 2; i++) z[i] = 1'b1;
  end
endmodule
"#,
    )
    .expect("write inline local loop source");
    let inline_local = slang
        .compile_files_via_ast(
            std::slice::from_ref(&inline_local_path),
            Some("inline_local"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower procedure-local loop variables");
    let mut inline_simulator =
        Simulator::new(&inline_local).expect("create procedure-local loop simulator");
    inline_simulator.run_until(0).unwrap();
    assert_eq!(inline_simulator.read("y").unwrap().to_u64(), Some(0b1111));
    assert_eq!(inline_simulator.read("z").unwrap().to_u64(), Some(0b0011));
    let inline_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&inline_local_path),
            Some("inline_local"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize procedure-local loop AST");
    let mut inline_summary = slang
        .summarize_ast_json(&inline_json)
        .expect("summarize procedure-local loop AST");
    inline_summary
        .validate_against_design(&inline_local)
        .expect("procedure-local dynamic packed lvalue fingerprint should match");
    if let Some(assignment) = inline_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| {
            block.assignments.iter_mut().find(|assignment| {
                assignment
                    .target_names
                    .first()
                    .is_some_and(|name| name == "y")
                    && assignment.dynamic_packed_lvalue_selector.is_some()
            })
        })
    {
        assignment
            .dynamic_packed_lvalue_selector
            .as_mut()
            .expect("procedure-local dynamic selector")
            .symbol = Some("wrong_local_selector".into());
    }
    let error = inline_summary
        .validate_against_design(&inline_local)
        .expect_err("wrong procedure-local dynamic selector must fail");
    assert!(
        error.contains("Ref:32:s[wrong_local_selector]"),
        "unexpected structural error: {error}"
    );

    let excessive_path = root.join("excessive.sv");
    std::fs::write(
        &excessive_path,
        r#"
module excessive(output logic y);
  int i;
  initial begin
    i = 0;
    while (i < 70000) i = i + 1;
    y = 1'b1;
  end
endmodule
"#,
    )
    .expect("write excessive while source");
    let excessive = slang
        .compile_files_via_ast(
            std::slice::from_ref(&excessive_path),
            Some("excessive"),
            &PreprocessOptions::default(),
        )
        .expect("lower excessive while loop");
    let mut excessive_simulator = Simulator::new(&excessive).expect("create excessive simulator");
    let error = excessive_simulator
        .run_until(0)
        .expect_err("excessive while loop must fail closed");
    assert!(
        error.contains("exceeds the 65536-iteration execution limit"),
        "unexpected error: {error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_lexical_named_block_disable() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable");
    let source_path = root.join("named_disable.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable(
  input logic go,
  output logic reached_disable,
  output logic after_inner,
  output logic after_outer
);
  initial begin : outer
    reached_disable = 1'b0;
    after_inner = 1'b0;
    after_outer = 1'b0;
    wait (go);
    begin : inner
      reached_disable = 1'b1;
      disable outer;
      after_inner = 1'b1;
    end
    after_outer = 1'b1;
  end
endmodule
"#,
    )
    .expect("write named-disable source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named-block disable");
    let mut simulator = Simulator::new(&design).expect("create named-disable simulator");
    simulator
        .set_input("go", &LogicWord::from_u64(1, 1))
        .unwrap();
    assert_eq!(simulator.read("reached_disable").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("after_inner").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("after_outer").unwrap().to_u64(), Some(0));

    let inner_path = root.join("disable_inner.sv");
    std::fs::write(
        &inner_path,
        r#"
module disable_inner(output logic skipped, output logic continued);
  initial begin : outer
    skipped = 1'b0;
    continued = 1'b0;
    begin : inner
      disable inner;
      skipped = 1'b1;
    end
    continued = 1'b1;
  end
endmodule
"#,
    )
    .expect("write inner-disable source");
    let inner = slang
        .compile_files_via_ast(
            std::slice::from_ref(&inner_path),
            Some("disable_inner"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower inner named-block disable");
    let mut inner_simulator = Simulator::new(&inner).expect("create inner-disable simulator");
    inner_simulator.run_until(0).unwrap();
    assert_eq!(inner_simulator.read("skipped").unwrap().to_u64(), Some(0));
    assert_eq!(inner_simulator.read("continued").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_named_block_disable_from_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable-loop");
    let source_path = root.join("named_disable_loop.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable_loop(
  output logic [3:0] count,
  output logic unreachable
);
  int index;
  initial begin : outer
    count = 4'd0;
    unreachable = 1'b0;
    index = 0;
    while (index < 5) begin
      count = count + 1;
      index = index + 1;
      if (index == 2) begin
        disable outer;
      end
    end
    unreachable = 1'b1;
  end
endmodule
"#,
    )
    .expect("write named-disable loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable_loop"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower named-block loop disable");
    let mut simulator = Simulator::new(&design).expect("create named-loop simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("unreachable").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_rejects_forks_inside_named_blocks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-named-disable-fork");
    let source_path = root.join("named_disable_fork.sv");
    std::fs::write(
        &source_path,
        r#"
module named_disable_fork(output logic x, output logic y);
  initial begin : outer
    fork
      x = 1'b1;
      begin : cancellable
        disable outer;
        y = 1'b1;
      end
    join
  end
endmodule
"#,
    )
    .expect("write named-block fork source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("named_disable_fork"),
            &PreprocessOptions::default(),
        )
        .expect_err("named-block process cancellation must fail closed");
    assert!(
        error.contains("named blocks containing fork are not supported"),
        "unexpected error: {error}"
    );

    let safe_path = root.join("named_fork_no_disable.sv");
    std::fs::write(
        &safe_path,
        r#"
module named_fork_no_disable(output logic x, output logic y);
  initial begin : outer
    fork
      x = 1'b1;
      y = 1'b1;
    join
  end
endmodule
"#,
    )
    .expect("write named-block fork without disable source");
    let safe = slang
        .compile_files_via_ast(
            std::slice::from_ref(&safe_path),
            Some("named_fork_no_disable"),
            &PreprocessOptions::default(),
        )
        .expect("named blocks without disable still support fork");
    let mut safe_simulator = Simulator::new(&safe).expect("create named-block fork simulator");
    safe_simulator.run_until(0).unwrap();
    assert_eq!(safe_simulator.read("x").unwrap().to_u64(), Some(1));
    assert_eq!(safe_simulator.read("y").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_do_while_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-do-while");
    let source_path = root.join("do_while.sv");
    std::fs::write(
        &source_path,
        r#"
module do_while(
  output logic [3:0] count,
  output logic execute_once
);
  int i;
  initial begin
    i = 0;
    count = 4'd0;
    do begin
      #1 count = count + 1;
      i = i + 1;
    end while (i < 3);

    i = 100;
    do begin
      execute_once = 1'b1;
    end while (i < 0);
  end
endmodule
"#,
    )
    .expect("write do-while source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("do_while"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower do-while loops");
    let mut simulator = Simulator::new(&design).expect("create do-while simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("count").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("execute_once").unwrap().to_u64(), None);
    for expected in 1..=3 {
        simulator.run_until(expected).unwrap();
        assert_eq!(simulator.read("count").unwrap().to_u64(), Some(expected));
    }
    assert_eq!(simulator.read("execute_once").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_runtime_break_and_continue() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-loop-jumps");
    let source_path = root.join("jumps.sv");
    std::fs::write(
        &source_path,
        r#"
module jumps(
  output logic [3:0] break_index,
  output logic [3:0] continue_count,
  output logic [3:0] repeat_break_count,
  output logic [3:0] repeat_continue_count,
  output logic [3:0] repeat_late_count,
  output logic [3:0] after_loops
);
  int i;
  initial begin
    break_index = 4'd0;
    for (i = 0; i < 10; i++) begin
      if (i == 3) begin
        break;
      end
      break_index = i[3:0] + 1;
    end

    continue_count = 4'd0;
    for (i = 0; i < 8; i++) begin
      if (i < 4) begin
        continue;
      end
      continue_count = continue_count + 1;
    end

    repeat_break_count = 4'd0;
    repeat (10) begin
      repeat_break_count = repeat_break_count + 1;
      if (repeat_break_count == 3) begin
        break;
      end
    end

    repeat_continue_count = 4'd0;
    repeat_late_count = 4'd0;
    repeat (6) begin
      repeat_continue_count = repeat_continue_count + 1;
      if (repeat_continue_count < 3) begin
        continue;
      end
      repeat_late_count = repeat_late_count + 1;
    end

    after_loops = 4'd1;
  end
endmodule
"#,
    )
    .expect("write loop-jump source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("jumps"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower break and continue");
    let mut simulator = Simulator::new(&design).expect("create loop-jump simulator");
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("break_index").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("continue_count").unwrap().to_u64(), Some(4));
    assert_eq!(
        simulator.read("repeat_break_count").unwrap().to_u64(),
        Some(3)
    );
    assert_eq!(
        simulator.read("repeat_continue_count").unwrap().to_u64(),
        Some(6)
    );
    assert_eq!(
        simulator.read("repeat_late_count").unwrap().to_u64(),
        Some(4)
    );
    assert_eq!(simulator.read("after_loops").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_replication() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-replication");
    let source_path = root.join("replication.sv");
    std::fs::write(
        &source_path,
        r#"
module replication(input logic [3:0] a, output logic [15:0] y);
  assign y = {4{a}};
endmodule
"#,
    )
    .expect("write replication source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("replication"),
            &PreprocessOptions::default(),
        )
        .expect("lower replication");
    assert_eq!(design.assigns.len(), 1);
    assert_eq!(design.assigns[0].e.width(), 16);

    let mut simulator = Simulator::new(&design).expect("create replication simulator");
    for value in [0x3_u64, 0x5, 0xa, 0xf] {
        simulator
            .set_input("a", &LogicWord::from_u64(4, value))
            .unwrap();
        let expected = 0x1111_u64 * value;
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_packed_select() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-packed");
    let source_path = root.join("dynamic_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_packed(
  input logic [7:0] a,
  input logic [2:0] selector,
  output logic y
);
  assign y = a[selector];
endmodule
"#,
    )
    .expect("write dynamic packed-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic packed select");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create dynamic packed simulator");
    for (selector, expected) in [(0_u64, 1_u64), (1, 0), (6, 0), (7, 1)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, 0x81))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(3, selector))
            .unwrap();
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_supports_dynamic_packed_lvalues() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-dynamic-packed-lvalue");
    let source_path = root.join("dynamic_lvalue.sv");
    std::fs::write(
        &source_path,
        r#"
module dynamic_lvalue(
  input logic clk,
  input logic [2:0] selector,
  input logic value,
  output logic [7:0] q
);
  always_ff @(posedge clk) q[selector] <= value;
endmodule
"#,
    )
    .expect("write dynamic packed-lvalue source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("dynamic_lvalue"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic packed lvalue");
    assert_eq!(design.initials.len(), 0);
    assert_eq!(design.processes.len(), 1);
    let json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("dynamic_lvalue"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize authoritative dynamic-lvalue AST");
    let mut summary = slang
        .summarize_ast_json(&json)
        .expect("summarize authoritative dynamic-lvalue AST");
    summary
        .validate_against_design(&design)
        .expect("exact dynamic packed-lvalue fingerprint should match");
    {
        let assignment = summary
            .procedural_blocks
            .iter_mut()
            .find_map(|block| block.assignments.first_mut())
            .expect("dynamic packed-lvalue assignment");
        assignment
            .dynamic_packed_lvalue_selector
            .as_mut()
            .expect("dynamic packed-lvalue selector")
            .symbol = Some("clk".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong dynamic packed-lvalue selector must fail");
    assert!(
        error.contains("Eq:1:u[Ref:3:u[clk],"),
        "unexpected structural error: {error}"
    );
    {
        let assignment = summary
            .procedural_blocks
            .iter_mut()
            .find_map(|block| block.assignments.first_mut())
            .expect("dynamic packed-lvalue assignment");
        assignment
            .dynamic_packed_lvalue_selector
            .as_mut()
            .expect("dynamic packed-lvalue selector")
            .symbol = Some("selector".into());
        assignment.rhs.symbol = Some("wrong_value".into());
    }
    let error = summary
        .validate_against_design(&design)
        .expect_err("wrong dynamic packed-lvalue RHS must fail");
    assert!(
        error.contains("PackedSelect:1:u[Ref:1:u[wrong_value],"),
        "unexpected structural error: {error}"
    );
    {
        let assignment = summary
            .procedural_blocks
            .iter_mut()
            .find_map(|block| block.assignments.first_mut())
            .expect("dynamic packed-lvalue assignment");
        assignment.rhs.symbol = Some("value".into());
    }

    let mut simulator = Simulator::new(&design).expect("create dynamic lvalue simulator");
    let mut expected = 0_u64;
    for selector in 0..8_u64 {
        let value = selector % 2;
        expected |= value << selector;
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        simulator
            .set_input("selector", &LogicWord::from_u64(3, selector))
            .unwrap();
        simulator
            .set_input("value", &LogicWord::from_u64(1, value))
            .unwrap();
        simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(expected));

    let packed_path = root.join("dynamic_packed_lvalue.sv");
    std::fs::write(
        &packed_path,
        r#"
module dynamic_packed_lvalue(
  input logic clk,
  input logic [1:0] selector,
  input logic [7:0] value,
  output logic [3:0][7:0] q
);
  always_ff @(posedge clk) q[selector] <= value;
endmodule
"#,
    )
    .expect("write dynamic multi-packed lvalue source");
    let packed = slang
        .compile_files_via_ast(
            std::slice::from_ref(&packed_path),
            Some("dynamic_packed_lvalue"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic multi-packed lvalue");
    let mut packed_simulator = Simulator::new(&packed).expect("create packed lvalue simulator");
    let mut bytes = [0_u64; 4];
    for selector in 0..4_u64 {
        let value = 0x10 + selector;
        bytes[selector as usize] = value;
        packed_simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        packed_simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        packed_simulator
            .set_input("value", &LogicWord::from_u64(8, value))
            .unwrap();
        packed_simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    let expected = bytes
        .iter()
        .rev()
        .fold(0_u64, |acc, byte| (acc << 8) | byte);
    assert_eq!(packed_simulator.read("q").unwrap().to_u64(), Some(expected));

    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("dynamic_lvalue"),
            &PreprocessOptions::default(),
        )
        .expect("lower dynamic packed lvalue through the bounded differential path");
    let mut bounded_summary = slang
        .summarize_ast_json(&json)
        .expect("resummarize authoritative dynamic-lvalue AST");
    bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect("exact bounded dynamic packed-lvalue fingerprint should match");
    if let Some(assignment) = bounded_summary
        .procedural_blocks
        .iter_mut()
        .find_map(|block| block.assignments.first_mut())
    {
        assignment
            .dynamic_packed_lvalue_selector
            .as_mut()
            .expect("bounded dynamic packed-lvalue selector")
            .symbol = Some("wrong_selector".into());
    }
    let bounded_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded)
        .expect_err("wrong bounded dynamic packed-lvalue selector must fail");
    assert!(
        bounded_error.contains("Eq:1:u[Ref:3:u[wrong_selector],"),
        "unexpected bounded dynamic-lvalue error: {bounded_error}"
    );

    let mut bounded_simulator =
        Simulator::new(&bounded).expect("create bounded dynamic-lvalue simulator");
    let mut bounded_expected = 0_u64;
    for selector in 0..8_u64 {
        let value = selector % 2;
        bounded_expected |= value << selector;
        bounded_simulator
            .set_input("clk", &LogicWord::from_u64(1, 0))
            .unwrap();
        bounded_simulator
            .set_input("selector", &LogicWord::from_u64(3, selector))
            .unwrap();
        bounded_simulator
            .set_input("value", &LogicWord::from_u64(1, value))
            .unwrap();
        bounded_simulator
            .set_input("clk", &LogicWord::from_u64(1, 1))
            .unwrap();
    }
    assert_eq!(
        bounded_simulator.read("q").unwrap().to_u64(),
        Some(bounded_expected)
    );
}

#[test]
fn slang_ast_direct_lowering_supports_arithmetic_shifts() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-arithmetic-shift");
    let source_path = root.join("arithmetic_shift.sv");
    std::fs::write(
        &source_path,
        r#"
module arithmetic_shift(
  input logic signed [7:0] a,
  input logic [2:0] n,
  output logic signed [7:0] left,
  output logic signed [7:0] right
);
  assign left = a <<< n;
  assign right = a >>> n;
endmodule
"#,
    )
    .expect("write arithmetic-shift source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("arithmetic_shift"),
            &PreprocessOptions::default(),
        )
        .expect("lower arithmetic shifts");
    assert_eq!(design.assigns.len(), 2);
    assert!(design
        .net_decls
        .iter()
        .any(|declaration| declaration.name == "a" && declaration.is_signed));

    let mut simulator = Simulator::new(&design).expect("create shift simulator");
    for (shift, expected_left, expected_right) in [
        (0_u64, 0xf8_u64, 0xf8_u64),
        (1, 0xf0, 0xfc),
        (2, 0xe0, 0xfe),
        (7, 0x00, 0xff),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, 0xf8))
            .unwrap();
        simulator
            .set_input("n", &LogicWord::from_u64(3, shift))
            .unwrap();
        assert_eq!(
            simulator.read("left").unwrap().to_u64(),
            Some(expected_left),
            "shift left {shift}"
        );
        assert_eq!(
            simulator.read("right").unwrap().to_u64(),
            Some(expected_right),
            "shift right {shift}"
        );
    }
}

#[test]
fn slang_direct_and_bounded_paths_match_64_bit_and_unsigned_i64_boundaries() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("width-boundaries");
    let source_path = root.join("width_boundaries.sv");
    std::fs::write(
        &source_path,
        r#"
module width_boundaries(
  input logic [63:0] unsigned_value,
  input logic signed [63:0] signed_min,
  input logic [64:0] wide_value,
  input logic signed [64:0] signed_wide_min,
  output logic [63:0] wrapped_increment,
  output logic [63:0] logical_right,
  output logic above_i64_max,
  output logic signed_negative,
  output logic wide_above_u64,
  output logic signed_wide_negative,
  output logic [64:0] wide_wrapped_increment
);
  assign wrapped_increment = unsigned_value + 64'd1;
  assign logical_right = unsigned_value >> 1;
  assign above_i64_max = unsigned_value > 64'h7fffffffffffffff;
  assign signed_negative = signed_min < 64'sd0;
  assign wide_above_u64 = wide_value > 65'h0000ffffffffffffffff;
  assign signed_wide_negative = signed_wide_min < 65'sd0;
  assign wide_wrapped_increment = wide_value + 1'b1;
endmodule
"#,
    )
    .expect("write width-boundary source");

    let direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("width_boundaries"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower 64-bit and wider boundary expressions");
    let bounded = slang
        .compile_files_bounded(
            std::slice::from_ref(&source_path),
            Some("width_boundaries"),
            &PreprocessOptions::default(),
        )
        .expect("lower boundary expressions through the bounded differential path");

    let run_boundaries = |design: &eda_core::ir::Design| {
        let mut simulator = Simulator::new(design).expect("create width-boundary simulator");
        simulator
            .set_input("unsigned_value", &LogicWord::ones(64))
            .unwrap();
        simulator
            .set_input(
                "signed_min",
                &LogicWord::from_u64(64, 0x8000_0000_0000_0000),
            )
            .unwrap();
        simulator
            .set_input("wide_value", &LogicWord::ones(65))
            .unwrap();
        let signed_wide_minimum = {
            let mut value = LogicWord::zero(65);
            value.bits[64] = LogicValue::One;
            value
        };
        simulator
            .set_input("signed_wide_min", &signed_wide_minimum)
            .unwrap();

        assert_eq!(
            simulator.read("wrapped_increment").unwrap().to_bin_str(),
            LogicWord::zero(64).to_bin_str(),
            "64-bit unsigned addition must wrap at U64::MAX"
        );
        assert_eq!(
            simulator.read("logical_right").unwrap().to_bin_str(),
            LogicWord::from_u64(64, 0x7fff_ffff_ffff_ffff).to_bin_str(),
            "logical right shift must not sign-extend"
        );
        assert_eq!(
            simulator.read("above_i64_max").unwrap().to_u64(),
            Some(1),
            "unsigned comparison must accept values above I64::MAX in {} path",
            if std::ptr::eq(design, &direct) {
                "direct"
            } else {
                "bounded"
            }
        );
        assert_eq!(
            simulator.read("signed_negative").unwrap().to_u64(),
            Some(1),
            "signed comparison must preserve sign in {} path",
            if std::ptr::eq(design, &direct) {
                "direct"
            } else {
                "bounded"
            }
        );
        assert_eq!(
            simulator.read("wide_above_u64").unwrap().to_u64(),
            Some(1),
            "unsigned ordering must work beyond U64::MAX in {} path",
            if std::ptr::eq(design, &direct) {
                "direct"
            } else {
                "bounded"
            }
        );
        assert_eq!(
            simulator.read("signed_wide_negative").unwrap().to_u64(),
            Some(1),
            "signed ordering must work beyond 64 bits in {} path",
            if std::ptr::eq(design, &direct) {
                "direct"
            } else {
                "bounded"
            }
        );
        assert_eq!(
            simulator
                .read("wide_wrapped_increment")
                .unwrap()
                .to_bin_str(),
            LogicWord::zero(65).to_bin_str(),
            "65-bit addition must wrap beyond U64::MAX without i64 truncation"
        );
    };
    run_boundaries(&direct);
    run_boundaries(&bounded);
}

#[test]
fn slang_ast_direct_lowering_supports_power_operator() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-power");
    let source_path = root.join("power.sv");
    std::fs::write(
        &source_path,
        r#"
module power(input logic [7:0] base, input logic [7:0] exponent, output logic [7:0] y);
  assign y = base ** exponent;
endmodule
"#,
    )
    .expect("write power source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("power"),
            &PreprocessOptions::default(),
        )
        .expect("lower power operator");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create power simulator");
    for (base, exponent, expected) in [
        (2_u64, 3_u64, 8_u64),
        (3, 4, 81),
        (17, 2, 33),
        (5, 0, 1),
        (0, 0, 1),
    ] {
        simulator
            .set_input("base", &LogicWord::from_u64(8, base))
            .unwrap();
        simulator
            .set_input("exponent", &LogicWord::from_u64(8, exponent))
            .unwrap();
        assert_eq!(
            simulator.read("y").unwrap().to_u64(),
            Some(expected),
            "{base} ** {exponent}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_streaming_concatenation() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-streaming");
    let source_path = root.join("streaming.sv");
    std::fs::write(
        &source_path,
        r#"
module streaming(
  input logic [7:0] a,
  input logic [3:0] b,
  output logic [7:0] reverse_bits,
  output logic [7:0] forward_bits,
  output logic [7:0] reverse_nibbles,
  output logic [7:0] reverse_pairs,
  output logic [11:0] multi_bits,
  output logic [11:0] multi_nibbles
);
  assign reverse_bits = {<<{a}};
  assign forward_bits = {>>{a}};
  assign reverse_nibbles = {<<4{a}};
  assign reverse_pairs = {<<2{a}};
  assign multi_bits = {<<{a, b}};
  assign multi_nibbles = {<<4{a, b}};
endmodule
"#,
    )
    .expect("write streaming-concatenation source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("streaming"),
            &PreprocessOptions::default(),
        )
        .expect("lower streaming concatenation");
    assert_eq!(design.assigns.len(), 6);

    let mut simulator = Simulator::new(&design).expect("create streaming simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xb2))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(4, 0x2))
        .unwrap();
    assert_eq!(simulator.read("reverse_bits").unwrap().to_u64(), Some(0x4d));
    assert_eq!(simulator.read("forward_bits").unwrap().to_u64(), Some(0xb2));
    assert_eq!(
        simulator.read("reverse_nibbles").unwrap().to_u64(),
        Some(0x2b)
    );
    assert_eq!(
        simulator.read("reverse_pairs").unwrap().to_u64(),
        Some(0x8e)
    );
    // The stream operands form one bitstream in source order before the
    // streaming operator reorders it. Thus `{a, b}` is 12'b1011_0010_0010.
    assert_eq!(simulator.read("multi_bits").unwrap().to_u64(), Some(0x44d));
    assert_eq!(
        simulator.read("multi_nibbles").unwrap().to_u64(),
        Some(0x22b)
    );
}

#[test]
fn slang_ast_direct_lowering_preserves_ascending_indexed_packed_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-ascending-indexed");
    let source_path = root.join("ascending_indexed.sv");
    std::fs::write(
        &source_path,
        r#"
module ascending_indexed(
  input logic [0:7] a,
  input logic [2:0] start,
  output logic [3:0] up,
  output logic [3:0] down
);
  assign up = a[start +: 4];
  assign down = a[start -: 4];
endmodule
"#,
    )
    .expect("write ascending indexed-select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("ascending_indexed"),
            &PreprocessOptions::default(),
        )
        .expect("lower ascending indexed selects");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create ascending simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xa5))
        .unwrap();
    simulator
        .set_input("start", &LogicWord::from_u64(3, 0))
        .unwrap();
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0xa));
    assert_eq!(simulator.read("down").unwrap().to_hex_str(), "x");

    simulator
        .set_input("start", &LogicWord::from_u64(3, 4))
        .unwrap();
    assert_eq!(simulator.read("up").unwrap().to_u64(), Some(0x5));
    assert_eq!(simulator.read("down").unwrap().to_u64(), Some(0x4));
}

#[test]
fn slang_ast_direct_lowering_supports_integral_conversions() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    fn set_conversion_leaf_symbol(
        expression: &mut eda_slang::SlangExpressionSummary,
        symbol: &str,
    ) {
        if let Some(operand) = expression.operands.first_mut() {
            set_conversion_leaf_symbol(operand, symbol);
        } else {
            expression.symbol = Some(symbol.to_string());
        }
    }
    let root = temporary_directory("direct-ast-conversions");
    let source_path = root.join("conversions.sv");
    std::fs::write(
        &source_path,
        r#"
module conversions(
  input byte signed source,
  input logic [15:0] wide,
  output logic [15:0] widen,
  output logic [3:0] truncate,
  output logic [15:0] unsigned_same,
  output logic signed [15:0] signed_widen,
  output logic signed_lt_zero,
  output logic unsigned_lt_zero
);
  assign widen = 16'(source);
  assign truncate = 4'(wide);
  assign unsigned_same = unsigned'(source);
  assign signed_widen = signed'(16'(source));
  assign signed_lt_zero = signed_widen < 16'sd0;
  assign unsigned_lt_zero = widen < 16'd0;
endmodule
"#,
    )
    .expect("write integral-conversion source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("conversions"),
            &PreprocessOptions::default(),
        )
        .expect("lower integral conversions");
    let conversion_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("conversions"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize integral conversion AST");
    let mut conversion_summary = slang
        .summarize_ast_json(&conversion_json)
        .expect("summarize integral conversion AST");
    conversion_summary
        .validate_against_design(&design)
        .expect("exact direct integral conversion fingerprint should match");
    if let Some(target) = conversion_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "widen")
    {
        set_conversion_leaf_symbol(&mut target.rhs, "wrong_source");
    }
    let direct_conversion_error = conversion_summary
        .validate_against_design(&design)
        .expect_err("wrong direct conversion operand must fail");
    assert!(
        direct_conversion_error.contains("Ref:8:s[wrong_source]")
            && direct_conversion_error.contains("Ref:8:s[source]"),
        "unexpected direct conversion error: {direct_conversion_error}"
    );
    let signed_output = design
        .net_decls
        .iter()
        .find(|declaration| declaration.name == "signed_widen")
        .expect("signed conversion output");
    assert!(signed_output.is_signed);

    let mut simulator = Simulator::new(&design).expect("create conversion simulator");
    simulator
        .set_input("source", &LogicWord::from_u64(8, 0xfb))
        .unwrap();
    simulator
        .set_input("wide", &LogicWord::from_u64(16, 0xabcd))
        .unwrap();

    assert_eq!(simulator.read("widen").unwrap().to_u64(), Some(0xfffb));
    assert_eq!(simulator.read("truncate").unwrap().to_u64(), Some(0xd));
    assert_eq!(
        simulator.read("unsigned_same").unwrap().to_u64(),
        Some(0x00fb)
    );
    assert_eq!(simulator.read("signed_widen").unwrap().to_hex_str(), "fffb");
    assert_eq!(simulator.read("signed_lt_zero").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("unsigned_lt_zero").unwrap().to_u64(),
        Some(0)
    );

    let bounded_conversion_path = root.join("bounded_conversion.sv");
    std::fs::write(
        &bounded_conversion_path,
        r#"
module bounded_conversion(
  input byte signed source,
  output logic [15:0] widened
);
  assign widened = source;
endmodule
"#,
    )
    .expect("write bounded-compatible conversion source");
    let bounded_conversion_direct = slang
        .compile_files_via_ast(
            std::slice::from_ref(&bounded_conversion_path),
            Some("bounded_conversion"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower implicit conversion");
    let bounded_conversion = slang
        .compile_files_bounded(
            std::slice::from_ref(&bounded_conversion_path),
            Some("bounded_conversion"),
            &PreprocessOptions::default(),
        )
        .expect("lower implicit conversion through the bounded differential path");
    let bounded_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&bounded_conversion_path),
            Some("bounded_conversion"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize implicit conversion AST");
    let mut bounded_summary = slang
        .summarize_ast_json(&bounded_json)
        .expect("summarize implicit conversion AST");
    bounded_summary
        .validate_against_design(&bounded_conversion_direct)
        .expect("exact direct implicit conversion fingerprint should match");
    bounded_summary
        .validate_against_design_legacy_compatibility(&bounded_conversion)
        .expect("exact bounded implicit conversion fingerprint should match");
    if let Some(target) = bounded_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "widened")
    {
        set_conversion_leaf_symbol(&mut target.rhs, "wrong_source");
    }
    let bounded_error = bounded_summary
        .validate_against_design_legacy_compatibility(&bounded_conversion)
        .expect_err("wrong bounded conversion operand must fail");
    assert!(
        bounded_error.contains("Ref:8:s[wrong_source]")
            && bounded_error.contains("Ref:8:s[source]"),
        "unexpected bounded conversion error: {bounded_error}"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_straight_line_function_calls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-functions");
    let source_path = root.join("functions.sv");
    std::fs::write(
        &source_path,
        r#"
module functions(
  input byte signed a,
  input int b,
  output int sum,
  output int constant,
  output int nested,
  output int initial_sum,
  output int selected,
  output int assigned,
  output int loop_sum,
  output int while_sum,
  output int repeat_sum
);
  function automatic int add(int x, int y);
    int result;
    begin
      result = x + y;
      return result;
    end
  endfunction

  function automatic int twice(int x);
    return add(x, x);
  endfunction

  function automatic int pick(int x, int y);
    if (x < y) return x;
    else return y;
  endfunction

  function automatic int cond_assign(int x, int y);
    int result = x;
    if (x < y) result = y;
    return result;
  endfunction

  function automatic int sum_four();
    int result = 0;
    for (int i = 0; i < 4; i = i + 1) result = result + i;
    return result;
  endfunction

  function automatic int while_four();
    int result = 0;
    int i = 0;
    while (i < 4) begin
      result = result + i;
      i = i + 1;
    end
    return result;
  endfunction

  function automatic int repeat_four();
    int result = 0;
    repeat (4) result = result + 1;
    return result;
  endfunction

  assign sum = add(a, b);
  assign constant = add(2, 3);
  assign nested = twice(b);
  initial initial_sum = add(7, 8);
  assign selected = pick(a, b);
  assign assigned = cond_assign(a, b);
  assign loop_sum = sum_four();
  assign while_sum = while_four();
  assign repeat_sum = repeat_four();
endmodule
"#,
    )
    .expect("write function-call source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("functions"),
            &PreprocessOptions::default(),
        )
        .expect("lower function calls");
    assert_eq!(design.assigns.len(), 8);
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create function simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xff))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::from_u64(32, 4))
        .unwrap();
    assert_eq!(simulator.read("sum").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("constant").unwrap().to_u64(), Some(5));
    assert_eq!(simulator.read("nested").unwrap().to_u64(), Some(8));
    assert_eq!(simulator.read("initial_sum").unwrap().to_u64(), Some(15));
    assert_eq!(
        simulator.read("selected").unwrap().to_u64(),
        Some(0xffff_ffff)
    );
    assert_eq!(simulator.read("assigned").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("loop_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("while_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("repeat_sum").unwrap().to_u64(), Some(4));

    simulator
        .set_input("a", &LogicWord::from_u64(8, 5))
        .unwrap();
    assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("assigned").unwrap().to_u64(), Some(5));
}

#[test]
fn slang_ast_direct_lowering_rejects_recursive_function_calls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-recursive-function");
    let source_path = root.join("recursive_function.sv");
    std::fs::write(
        &source_path,
        r#"
module recursive_function(output int y);
  function automatic int loop(int x);
    return loop(x);
  endfunction

  assign y = loop(0);
endmodule
"#,
    )
    .expect("write recursive-function source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("recursive_function"),
            &PreprocessOptions::default(),
        )
        .expect_err("unbounded recursive function inlining must be rejected");
    assert!(error.contains("recursive function"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_supports_tasks_with_output_arguments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-output");
    let source_path = root.join("task_output.sv");
    std::fs::write(
        &source_path,
        r#"
module task_output(
  input int a,
  output int y,
  output int z
);
  task automatic add_one(input int x, output int result);
    result = x + 1;
  endtask

  always_comb begin
    add_one(a, y);
    add_one(41, z);
  end
endmodule
"#,
    )
    .expect("write task-output source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_output"),
            &PreprocessOptions::default(),
        )
        .expect("lower task with output argument");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 7))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(8));
    assert_eq!(simulator.read("z").unwrap().to_u64(), Some(42));
}

#[test]
fn slang_ast_direct_lowering_supports_task_inout_and_ref_arguments() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-inout-ref");
    let source_path = root.join("task_inout_ref.sv");
    std::fs::write(
        &source_path,
        r#"
module task_inout_ref(
  output int inout_result,
  output int ref_result
);
  task automatic add_inout(inout int value, input int delta);
    value = value + delta;
  endtask

  task automatic add_ref(ref int value, input int delta);
    value = value + delta;
  endtask

  int inout_state = 10;
  int ref_state = 20;

  initial begin
    add_inout(inout_state, 5);
    add_ref(ref_state, 7);
  end

  assign inout_result = inout_state;
  assign ref_result = ref_state;
endmodule
"#,
    )
    .expect("write task inout/ref source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_inout_ref"),
            &PreprocessOptions::default(),
        )
        .expect("lower task inout/ref arguments");
    assert_eq!(design.initials.len(), 1);
    assert_eq!(design.assigns.len(), 4);

    let mut simulator = Simulator::new(&design).expect("create task inout/ref simulator");
    simulator.run_until(0).expect("settle task inout/ref calls");
    assert_eq!(simulator.read("inout_result").unwrap().to_u64(), Some(15));
    assert_eq!(simulator.read("ref_result").unwrap().to_u64(), Some(27));
}

#[test]
fn slang_ast_direct_lowering_supports_task_returns() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-return");
    let source_path = root.join("task_return.sv");
    std::fs::write(
        &source_path,
        r#"
module task_return(
  output int true_state,
  output int true_skipped,
  output int false_state,
  output int false_skipped,
  output int task_after
);
  task automatic stop_early(
    input logic stop,
    output int state,
    output int skipped
  );
    state = 1;
    if (stop) return;
    skipped = 1;
  endtask

  initial begin
    stop_early(1'b1, true_state, true_skipped);
    stop_early(1'b0, false_state, false_skipped);
    task_after = 2;
  end
endmodule
"#,
    )
    .expect("write task-return source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_return"),
            &PreprocessOptions::default(),
        )
        .expect("lower task returns");
    assert_eq!(design.initials.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create return simulator");
    simulator.run_until(0).expect("run return initializers");
    assert_eq!(simulator.read("true_state").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("true_skipped").unwrap().to_u64(),
        None,
        "task return must skip only the remainder of the task body"
    );
    assert_eq!(simulator.read("false_state").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("false_skipped").unwrap().to_u64(),
        Some(1),
        "a non-taken return must preserve normal task execution"
    );
    assert_eq!(
        simulator.read("task_after").unwrap().to_u64(),
        Some(2),
        "task return must resume the caller"
    );
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_variables() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-locals");
    let source_path = root.join("task_locals.sv");
    std::fs::write(
        &source_path,
        r#"
module task_locals(
  input int a,
  output int y
);
  task automatic compute(input int x, output int result);
    int initialized = 10;
    int accumulated;
    accumulated = initialized + x;
    result = accumulated * 2;
  endtask

  always_comb compute(a, y);
endmodule
"#,
    )
    .expect("write task-local source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_locals"),
            &PreprocessOptions::default(),
        )
        .expect("lower task with local variables");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(32, 5))
        .unwrap();
    assert_eq!(simulator.read("y").unwrap().to_u64(), Some(30));
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_conditionals() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-local-conditionals");
    let source_path = root.join("task_local_conditionals.sv");
    std::fs::write(
        &source_path,
        r#"
module task_local_conditionals(
  input logic select,
  input int a,
  input int b,
  output int selected,
  output int maybe_incremented
);
  task automatic pick(
    input logic use_a,
    input int a_value,
    input int b_value,
    output int result
  );
    int value = 0;
    if (use_a) value = a_value;
    else value = b_value;
    result = value + 1;
  endtask

  task automatic increment_if(
    input logic enable,
    input int value,
    output int result
  );
    int accumulated = 7;
    if (enable) accumulated = value;
    result = accumulated;
  endtask

  always_comb begin
    pick(select, a, b, selected);
    increment_if(select, a, maybe_incremented);
  end
endmodule
"#,
    )
    .expect("write task-local conditional source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_local_conditionals"),
            &PreprocessOptions::default(),
        )
        .expect("lower task-local conditionals");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local conditional simulator");
    for (select, a, b, selected, maybe_incremented) in
        [(false, 11, 22, 23, 7), (true, 11, 22, 12, 11)]
    {
        simulator
            .set_input("select", &LogicWord::from_u64(1, select as u64))
            .unwrap();
        simulator
            .set_input("a", &LogicWord::from_u64(32, a))
            .unwrap();
        simulator
            .set_input("b", &LogicWord::from_u64(32, b))
            .unwrap();
        assert_eq!(simulator.read("selected").unwrap().to_u64(), Some(selected));
        assert_eq!(
            simulator.read("maybe_incremented").unwrap().to_u64(),
            Some(maybe_incremented)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_task_local_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-task-local-loops");
    let source_path = root.join("task_local_loops.sv");
    std::fs::write(
        &source_path,
        r#"
module task_local_loops(
  output int for_sum,
  output int while_product,
  output int repeat_sum
);
  task automatic sum_for(output int result);
    int total = 0;
    for (int i = 0; i < 4; i = i + 1) total = total + i;
    result = total;
  endtask

  task automatic product_while(output int result);
    int total = 1;
    int i = 0;
    while (i < 4) begin
      total = total * 2;
      i = i + 1;
    end
    result = total;
  endtask

  task automatic sum_repeat(output int result);
    int total = 2;
    repeat (4) total = total + 1;
    result = total;
  endtask

  always_comb begin
    sum_for(for_sum);
    product_while(while_product);
    sum_repeat(repeat_sum);
  end
endmodule
"#,
    )
    .expect("write task-local loop source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("task_local_loops"),
            &PreprocessOptions::default(),
        )
        .expect("lower task-local loops");
    assert_eq!(design.processes.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create task-local loop simulator");
    simulator.run_until(0).expect("settle task-local loops");
    assert_eq!(simulator.read("for_sum").unwrap().to_u64(), Some(6));
    assert_eq!(simulator.read("while_product").unwrap().to_u64(), Some(16));
    assert_eq!(simulator.read("repeat_sum").unwrap().to_u64(), Some(6));
}

#[test]
fn slang_ast_direct_lowering_rejects_recursive_tasks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-recursive-task");
    let source_path = root.join("recursive_task.sv");
    std::fs::write(
        &source_path,
        r#"
module recursive_task(output int y);
  task automatic loop(input int x, output int result);
    if (x == 0) result = 1;
    else loop(x - 1, result);
  endtask

  always_comb loop(2, y);
endmodule
"#,
    )
    .expect("write recursive-task source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("recursive_task"),
            &PreprocessOptions::default(),
        )
        .expect_err("unbounded recursive task inlining must be rejected");
    assert!(error.contains("recursive task"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_rejects_excessive_function_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-excessive-function-loop");
    let source_path = root.join("excessive_function_loop.sv");
    std::fs::write(
        &source_path,
        r#"
module excessive_function_loop(output int y);
  function automatic int sum();
    int result = 0;
    for (int i = 0; i < 257; i = i + 1) result = result + 1;
    return result;
  endfunction

  assign y = sum();
endmodule
"#,
    )
    .expect("write excessive-function-loop source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("excessive_function_loop"),
            &PreprocessOptions::default(),
        )
        .expect_err("excessive function-loop inlining must be bounded");
    assert!(error.contains("256-iteration"), "{error}");
}

#[test]
fn slang_ast_direct_lowering_rejects_excessive_while_and_repeat_loops() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let cases = [
        (
            "while",
            r#"
module m(output int y);
  function automatic int sum();
    int result = 0;
    int i = 0;
    while (i < 257) begin
      result = result + 1;
      i = i + 1;
    end
    return result;
  endfunction
  assign y = sum();
endmodule
"#,
        ),
        (
            "repeat",
            r#"
module m(output int y);
  function automatic int sum();
    int result = 0;
    repeat (257) result = result + 1;
    return result;
  endfunction
  assign y = sum();
endmodule
"#,
        ),
    ];
    for (name, source) in cases {
        let root = temporary_directory("direct-ast-excessive-function-loop");
        let source_path = root.join(format!("{name}.sv"));
        std::fs::write(&source_path, source).expect("write excessive-loop source");
        let error = slang
            .compile_files_via_ast(
                std::slice::from_ref(&source_path),
                Some("m"),
                &PreprocessOptions::default(),
            )
            .expect_err("function-loop inlining must stay bounded");
        assert!(error.contains("256-iteration"), "{name}: {error}");
    }
}

#[test]
fn slang_ast_direct_lowering_supports_case_equality() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-case-equality");
    let source_path = root.join("case_equality.sv");
    std::fs::write(
        &source_path,
        r#"
module case_equality(
  input logic [3:0] a,
  input logic [3:0] b,
  output logic equal,
  output logic unequal
);
  assign equal = a === b;
  assign unequal = a !== b;
endmodule
"#,
    )
    .expect("write case-equality source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("case_equality"),
            &PreprocessOptions::default(),
        )
        .expect("lower case equality");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create case-equality simulator");
    simulator
        .set_input("a", &LogicWord::parse(4, "'b1x0z").expect("a with X/Z"))
        .unwrap();
    simulator
        .set_input("b", &LogicWord::parse(4, "'b1x0z").expect("matching X/Z"))
        .unwrap();
    assert_eq!(simulator.read("equal").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("unequal").unwrap().to_u64(), Some(0));

    simulator
        .set_input(
            "b",
            &LogicWord::parse(4, "'b1x00").expect("different Z bit"),
        )
        .unwrap();
    assert_eq!(simulator.read("equal").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("unequal").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_unary_operators() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-unary-operators");
    let source_path = root.join("unary_operators.sv");
    std::fs::write(
        &source_path,
        r#"
module unary_operators(
  input int signed value,
  input logic [3:0] bits,
  output int signed plus,
  output int signed minus,
  output logic reduction_and,
  output logic reduction_nand,
  output logic reduction_or,
  output logic reduction_nor,
  output logic reduction_xor,
  output logic reduction_xnor
);
  assign plus = +value;
  assign minus = -value;
  assign reduction_and = &bits;
  assign reduction_nand = ~&bits;
  assign reduction_or = |bits;
  assign reduction_nor = ~|bits;
  assign reduction_xor = ^bits;
  assign reduction_xnor = ~^bits;
endmodule
"#,
    )
    .expect("write unary-operator source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("unary_operators"),
            &PreprocessOptions::default(),
        )
        .expect("lower unary operators");
    assert_eq!(design.assigns.len(), 8);
    let unary_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("unary_operators"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize unary-operator AST");
    let mut unary_summary = slang
        .summarize_ast_json(&unary_json)
        .expect("summarize unary-operator AST");
    unary_summary
        .validate_against_design(&design)
        .expect("exact unary-operator fingerprint should match");
    if let Some(target) = unary_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "reduction_xor")
    {
        target.rhs.operator = "BitwiseAnd".into();
    }
    let unary_error = unary_summary
        .validate_against_design(&design)
        .expect_err("wrong unary reduction operator must fail");
    assert!(
        unary_error.contains("XorR:1:u[Ref:4:u[bits]]")
            && unary_error.contains("AndR:1:u[Ref:4:u[bits]]"),
        "unexpected unary structural error: {unary_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create unary simulator");
    simulator
        .set_input("value", &LogicWord::from_u64(32, 5))
        .unwrap();
    simulator
        .set_input("bits", &LogicWord::from_u64(4, 0b0011))
        .unwrap();
    assert_eq!(simulator.read("plus").unwrap().to_u64(), Some(5));
    assert_eq!(simulator.read("minus").unwrap().to_hex_str(), "fffffffb");
    assert_eq!(simulator.read("reduction_and").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_nand").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("reduction_or").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("reduction_nor").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_xor").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("reduction_xnor").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_ast_direct_lowering_supports_logical_implication_and_equivalence() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-logical-composition");
    let source_path = root.join("logical_composition.sv");
    std::fs::write(
        &source_path,
        r#"
module logical_composition(
  input logic a,
  input logic b,
  output logic imply_00,
  output logic imply_01,
  output logic imply_10,
  output logic imply_11,
  output logic equiv_00,
  output logic equiv_01,
  output logic equiv_10,
  output logic equiv_11,
  output logic binary_xor,
  output logic binary_xnor,
  output logic binary_xnor_operator
);
  assign imply_00 = a -> b;
  assign imply_01 = a -> b;
  assign imply_10 = a -> b;
  assign imply_11 = a -> b;
  assign equiv_00 = a <-> b;
  assign equiv_01 = a <-> b;
  assign equiv_10 = a <-> b;
  assign equiv_11 = a <-> b;
  assign binary_xor = a ^ b;
  assign binary_xnor = ~(a ^ b);
  assign binary_xnor_operator = a ~^ b;
endmodule
"#,
    )
    .expect("write logical-composition source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("logical_composition"),
            &PreprocessOptions::default(),
        )
        .expect("lower logical composition");
    assert_eq!(design.assigns.len(), 11);
    let logical_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("logical_composition"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize logical-composition AST");
    let mut logical_summary = slang
        .summarize_ast_json(&logical_json)
        .expect("summarize logical-composition AST");
    let mut binary_summary = logical_summary.clone();
    logical_summary
        .validate_against_design(&design)
        .expect("exact logical-composition fingerprint should match");
    if let Some(target) = logical_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "imply_00")
    {
        target.rhs.operator = "LogicalEquivalence".into();
    }
    let logical_error = logical_summary
        .validate_against_design(&design)
        .expect_err("wrong logical composition operator must fail");
    assert!(
        logical_error.contains("LogOr:1:u[Not:1:u[Ref:1:u[a]],Ref:1:u[b]]")
            && logical_error.contains("LogAnd:1:u["),
        "unexpected logical structural error: {logical_error}"
    );
    if let Some(target) = binary_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "binary_xor")
    {
        target.rhs.operator = "BinaryOr".into();
    }
    let binary_error = binary_summary
        .validate_against_design(&design)
        .expect_err("wrong binary xor operator must fail");
    assert!(
        binary_error.contains("Xor:1:u[Ref:1:u[a],Ref:1:u[b]]")
            && binary_error.contains("Or:1:u[Ref:1:u[a],Ref:1:u[b]]"),
        "unexpected binary structural error: {binary_error}"
    );
    if let Some(target) = binary_summary
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "binary_xnor_operator")
    {
        target.rhs.operator = "BinaryXor".into();
    }
    let xnor_error = binary_summary
        .validate_against_design(&design)
        .expect_err("wrong binary xnor operator must fail");
    assert!(
        xnor_error.contains("Not:1:u[Xor:1:u[Ref:1:u[a],Ref:1:u[b]]]"),
        "unexpected xnor structural error: {xnor_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create logical simulator");
    for (a, b, label) in [
        (false, false, "00"),
        (false, true, "01"),
        (true, false, "10"),
        (true, true, "11"),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(1, a as u64))
            .unwrap();
        simulator
            .set_input("b", &LogicWord::from_u64(1, b as u64))
            .unwrap();
        assert_eq!(
            simulator.read(&format!("imply_{label}")).unwrap().to_u64(),
            Some((!a || b) as u64)
        );
        assert_eq!(
            simulator.read(&format!("equiv_{label}")).unwrap().to_u64(),
            Some((a == b) as u64)
        );
        assert_eq!(
            simulator.read("binary_xor").unwrap().to_u64(),
            Some((a ^ b) as u64)
        );
        assert_eq!(
            simulator.read("binary_xnor").unwrap().to_u64(),
            Some((!(a ^ b)) as u64)
        );
        assert_eq!(
            simulator.read("binary_xnor_operator").unwrap().to_u64(),
            Some((!(a ^ b)) as u64)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_and_inside_cases() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wildcard-cases");
    let source_path = root.join("wildcard_cases.sv");
    std::fs::write(
        &source_path,
        r#"
module wildcard_cases(
  input logic [3:0] selector,
  input logic [1:0] pair,
  output logic [1:0] casez_y,
  output logic [1:0] casex_y,
  output logic [1:0] inside_y
);
  always_comb begin
    casez_y = 2'd0;
    casez (selector)
      4'b1???: casez_y = 2'd1;
      4'b01??: casez_y = 2'd2;
      default casez_y = 2'd3;
    endcase
  end

  always_comb begin
    casex_y = 2'd0;
    casex (selector)
      4'b1xxx: casex_y = 2'd1;
      4'b01xx: casex_y = 2'd2;
      default casex_y = 2'd3;
    endcase
  end

  always_comb begin
    inside_y = 2'd0;
    case (pair) inside
      [0:1]: inside_y = 2'd1;
      2: inside_y = 2'd2;
      default inside_y = 2'd3;
    endcase
  end
endmodule
"#,
    )
    .expect("write wildcard-case source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("wildcard_cases"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower wildcard and inside cases");
    let mut simulator = Simulator::new(&design).expect("create wildcard-case simulator");
    for (selector, expected) in [
        (0b1000_u64, 1_u64),
        (0b1101, 1),
        (0b0100, 2),
        (0b0110, 2),
        (0b0010, 3),
    ] {
        simulator
            .set_input("selector", &LogicWord::from_u64(4, selector))
            .unwrap();
        assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(expected));
        assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(expected));
    }
    for (pair, expected) in [(0_u64, 1_u64), (1, 1), (2, 2), (3, 3)] {
        simulator
            .set_input("pair", &LogicWord::from_u64(2, pair))
            .unwrap();
        assert_eq!(simulator.read("inside_y").unwrap().to_u64(), Some(expected));
    }

    simulator
        .set_input(
            "selector",
            &LogicWord {
                bits: vec![
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::Z,
                ],
            },
        )
        .unwrap();
    assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(1));

    simulator
        .set_input(
            "selector",
            &LogicWord {
                bits: vec![
                    LogicValue::Zero,
                    LogicValue::Zero,
                    LogicValue::X,
                    LogicValue::Zero,
                ],
            },
        )
        .unwrap();
    assert_eq!(simulator.read("casez_y").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("casex_y").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_ast_direct_lowering_supports_qualified_if_and_case_checks() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-qualified-checks");
    let source_path = root.join("qualified.sv");
    std::fs::write(
        &source_path,
        r#"
module qualified(
  input logic [1:0] selector,
  output logic [1:0] priority_if_y,
  output logic [1:0] unique_if_y,
  output logic [1:0] priority_case_y,
  output logic [1:0] unique_case_y
);
  always_comb begin
    priority_if_y = 2'd0;
    priority if (selector == 2'd0) priority_if_y = 2'd1;
    else if (selector == 2'd1) priority_if_y = 2'd2;
    else priority_if_y = 2'd3;
  end

  always_comb begin
    unique_if_y = 2'd0;
    unique if (selector == 2'd0) unique_if_y = 2'd1;
    else if (selector == 2'd1) unique_if_y = 2'd2;
    else unique_if_y = 2'd3;
  end

  always_comb begin
    priority_case_y = 2'd0;
    priority case (selector)
      2'd0: priority_case_y = 2'd1;
      2'd1: priority_case_y = 2'd2;
      default priority_case_y = 2'd3;
    endcase
  end

  always_comb begin
    unique_case_y = 2'd0;
    unique case (selector)
      2'd0: unique_case_y = 2'd1;
      2'd1: unique_case_y = 2'd2;
      default unique_case_y = 2'd3;
    endcase
  end
endmodule
"#,
    )
    .expect("write qualified-check source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("qualified"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower qualified if/case checks");
    let mut simulator = Simulator::new(&design).expect("create qualified-check simulator");
    for selector in 0..4 {
        let expected = if selector < 2 { selector + 1 } else { 3 };
        simulator
            .set_input("selector", &LogicWord::from_u64(2, selector))
            .unwrap();
        assert_eq!(
            simulator.read("priority_if_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("unique_if_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("priority_case_y").unwrap().to_u64(),
            Some(expected)
        );
        assert_eq!(
            simulator.read("unique_case_y").unwrap().to_u64(),
            Some(expected)
        );
    }

    let qualified_wildcard_path = root.join("qualified_wildcard.sv");
    std::fs::write(
        &qualified_wildcard_path,
        r#"
module qualified_wildcard(input logic [1:0] selector, output logic y);
  always_comb unique casez (selector)
    2'b?0: y = 1'b1;
    default y = 1'b0;
  endcase
endmodule
"#,
    )
    .expect("write qualified-wildcard source");
    let qualified_wildcard_error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&qualified_wildcard_path),
            Some("qualified_wildcard"),
            &PreprocessOptions::default(),
        )
        .expect_err("qualified wildcard checks must fail closed");
    assert!(
        qualified_wildcard_error.contains("Unique wildcard case semantics"),
        "unexpected qualified wildcard error: {qualified_wildcard_error}"
    );

    let overlap_path = root.join("unique_overlap.sv");
    std::fs::write(
        &overlap_path,
        r#"
module unique_overlap(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique if (selector == 2'd0) y = 2'd1;
    else if (selector < 2'd2) y = 2'd2;
    else y = 2'd3;
  end
endmodule
"#,
    )
    .expect("write unique-if overlap source");
    let overlap = slang
        .compile_files_via_ast(
            std::slice::from_ref(&overlap_path),
            Some("unique_overlap"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique-if overlap check");
    let mut overlap_simulator = Simulator::new(&overlap).expect("create overlap simulator");
    let error = overlap_simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .expect_err("multiple unique-if matches must fail at runtime");
    assert!(
        error.contains("unique if has multiple matching conditions"),
        "unexpected overlap error: {error}"
    );

    let no_match_path = root.join("priority_no_match.sv");
    std::fs::write(
        &no_match_path,
        r#"
module priority_no_match(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    priority case (selector)
      2'd0: y = 2'd1;
      2'd1: y = 2'd2;
    endcase
  end
endmodule
"#,
    )
    .expect("write priority no-match source");
    let no_match = slang
        .compile_files_via_ast(
            std::slice::from_ref(&no_match_path),
            Some("priority_no_match"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower priority no-match check");
    let mut no_match_simulator = Simulator::new(&no_match).expect("create no-match simulator");
    let error = no_match_simulator
        .set_input("selector", &LogicWord::from_u64(2, 2))
        .expect_err("priority case no-match must fail at runtime");
    assert!(
        error.contains("priority case has no matching item"),
        "unexpected no-match error: {error}"
    );

    let duplicate_path = root.join("unique_duplicate.sv");
    std::fs::write(
        &duplicate_path,
        r#"
module unique_duplicate(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique case (selector)
      2'd0: y = 2'd1;
      2'd0: y = 2'd2;
    endcase
  end
endmodule
"#,
    )
    .expect("write unique-case duplicate source");
    let duplicate = slang
        .compile_files_via_ast(
            std::slice::from_ref(&duplicate_path),
            Some("unique_duplicate"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique-case overlap check");
    let mut duplicate_simulator = Simulator::new(&duplicate).expect("create duplicate simulator");
    let error = duplicate_simulator
        .set_input("selector", &LogicWord::from_u64(2, 0))
        .expect_err("multiple unique-case items must fail at runtime");
    assert!(
        error.contains("unique case has multiple matching items"),
        "unexpected duplicate error: {error}"
    );

    let unique_zero_path = root.join("unique_zero.sv");
    std::fs::write(
        &unique_zero_path,
        r#"
module unique_zero(input logic [1:0] selector, output logic [1:0] y);
  always_comb begin
    y = 2'd0;
    unique0 case (selector)
      2'd0: y = 2'd1;
    endcase
  end
endmodule
"#,
    )
    .expect("write unique0 source");
    let unique_zero = slang
        .compile_files_via_ast(
            std::slice::from_ref(&unique_zero_path),
            Some("unique_zero"),
            &PreprocessOptions::default(),
        )
        .expect("directly lower unique0 case check");
    let mut unique_zero_simulator = Simulator::new(&unique_zero).expect("create unique0 simulator");
    unique_zero_simulator
        .set_input("selector", &LogicWord::from_u64(2, 1))
        .unwrap();
    assert_eq!(unique_zero_simulator.read("y").unwrap().to_u64(), Some(0));
}

#[test]
fn slang_ast_direct_lowering_supports_indexed_packed_part_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-indexed-packed");
    let source_path = root.join("indexed_packed.sv");
    std::fs::write(
        &source_path,
        r#"
module indexed_packed(
  input logic [15:0] a,
  input logic [3:0] start,
  output logic [7:0] up,
  output logic [7:0] down
);
  assign up = a[start +: 8];
  assign down = a[start -: 8];
endmodule
"#,
    )
    .expect("write indexed packed select source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("indexed_packed"),
            &PreprocessOptions::default(),
        )
        .expect("lower indexed packed selects");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create indexed select simulator");
    for (start, expected_up, expected_down) in [
        (0_u64, 0x34_u64, 0),
        (4, 0x23, 0),
        (7, 0x24, 0x34),
        (8, 0x12, 0x1a),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(16, 0x1234))
            .unwrap();
        simulator
            .set_input("start", &LogicWord::from_u64(4, start))
            .unwrap();
        assert_eq!(simulator.read("up").unwrap().to_u64(), Some(expected_up));
        if start >= 7 {
            assert_eq!(
                simulator.read("down").unwrap().to_u64(),
                Some(expected_down)
            );
        } else {
            assert!(simulator.read("down").unwrap().to_u64().is_none());
        }
    }
}

#[test]
fn slang_ast_direct_lowering_resolves_elaborated_parameters() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-parameters");
    let source_path = root.join("parameters.sv");
    std::fs::write(
        &source_path,
        r#"
module parameters(output logic [7:0] byte_value, output logic [15:0] word_value);
  parameter logic [7:0] BYTE = 8'hA5;
  localparam logic [15:0] WORD = 16'h1234;
  assign byte_value = BYTE;
  assign word_value = WORD;
endmodule
"#,
    )
    .expect("write parameter source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("parameters"),
            &PreprocessOptions::default(),
        )
        .expect("resolve elaborated parameters");
    assert_eq!(design.assigns.len(), 2);

    let mut simulator = Simulator::new(&design).expect("create parameter simulator");
    simulator
        .run_until(1)
        .expect("settle parameter assignments");
    assert_eq!(simulator.read("byte_value").unwrap().to_u64(), Some(0xa5));
    assert_eq!(simulator.read("word_value").unwrap().to_u64(), Some(0x1234));
}

#[test]
fn slang_ast_direct_lowering_reduces_logical_not_to_one_bit() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-logical-not");
    let source_path = root.join("logical_not.sv");
    std::fs::write(
        &source_path,
        r#"
module logical_not(input logic [7:0] a, output logic y);
  assign y = !a;
endmodule
"#,
    )
    .expect("write logical-not source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("logical_not"),
            &PreprocessOptions::default(),
        )
        .expect("lower logical reduction");

    let mut simulator = Simulator::new(&design).expect("create logical-not simulator");
    for (input, expected) in [(0_u64, 1_u64), (1, 0), (0x80, 0), (0xff, 0)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, input))
            .expect("set logical-not input");
        assert_eq!(simulator.read("y").unwrap().to_u64(), Some(expected));
    }
}

#[test]
fn slang_ast_direct_lowering_preserves_ascending_packed_selects() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-ascending");
    let source_path = root.join("ascending.sv");
    std::fs::write(
        &source_path,
        r#"
module ascending(
  input logic [0:7] a,
  output logic bit0,
  output logic [3:0] high
);
  assign bit0 = a[0];
  assign high = a[0:3];
endmodule
"#,
    )
    .expect("write ascending packed source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("ascending"),
            &PreprocessOptions::default(),
        )
        .expect("lower ascending packed declaration");
    assert_eq!(design.n_nets, 13);

    let mut simulator = Simulator::new(&design).expect("create ascending simulator");
    simulator
        .set_input("a", &LogicWord::from_u64(8, 0xf0))
        .expect("set ascending input");
    assert_eq!(simulator.read("bit0").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("high").unwrap().to_u64(), Some(0xf));
}

#[test]
fn slang_ast_direct_lowering_supports_wildcard_equality() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-wildcard-equality");
    let source_path = root.join("wildcard_equality.sv");
    std::fs::write(
        &source_path,
        r#"
module wildcard_equality(
  input logic [7:0] a,
  output logic equal,
  output logic unequal
);
  assign equal = (a ==? 8'b1010??01);
  assign unequal = (a !=? 8'b1010??01);
endmodule
"#,
    )
    .expect("write wildcard-equality source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("wildcard_equality"),
            &PreprocessOptions::default(),
        )
        .expect("lower wildcard equality");
    assert_eq!(design.assigns.len(), 2);
    let wildcard_json = slang
        .ast_json_with_options(
            std::slice::from_ref(&source_path),
            Some("wildcard_equality"),
            &[],
            &PreprocessOptions::default(),
        )
        .expect("serialize wildcard-equality AST");
    let wildcard_summary = slang
        .summarize_ast_json(&wildcard_json)
        .expect("summarize wildcard-equality AST");
    wildcard_summary
        .validate_against_design(&design)
        .expect("exact direct wildcard-equality fingerprints should match");

    let mut changed_pattern = wildcard_summary.clone();
    if let Some(target) = changed_pattern
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "equal")
    {
        target.rhs.operands[1].literal = Some("8:1010zz11".into());
    }
    let pattern_error = changed_pattern
        .validate_against_design(&design)
        .expect_err("wrong wildcard-equality pattern must fail");
    assert!(
        pattern_error.contains("Const:8:u[8:11110011]")
            && pattern_error.contains("Const:8:u[8:10100011]"),
        "unexpected wildcard-equality pattern error: {pattern_error}"
    );

    let mut changed_direction = wildcard_summary;
    if let Some(target) = changed_direction
        .continuous_targets
        .iter_mut()
        .find(|target| target.name == "equal")
    {
        target.rhs.operator = "WildcardInequality".into();
    }
    let direction_error = changed_direction
        .validate_against_design(&design)
        .expect_err("wrong wildcard-equality direction must fail");
    assert!(
        direction_error.contains("Not:1:u[Eq:1:u[") && direction_error.contains("Eq:1:u[And:8:u["),
        "unexpected wildcard-equality direction error: {direction_error}"
    );

    let mut simulator = Simulator::new(&design).expect("create wildcard simulator");
    for (value, expected) in [(0xa5_u64, true), (0xa1, true), (0x85, false), (0xa4, false)] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(
            simulator.read("equal").unwrap().to_u64(),
            Some(expected as u64)
        );
        assert_eq!(
            simulator.read("unequal").unwrap().to_u64(),
            Some((!expected) as u64)
        );
    }
}

#[test]
fn slang_ast_direct_lowering_supports_inside() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-inside");
    let source_path = root.join("inside.sv");
    std::fs::write(
        &source_path,
        r#"
module inside_expr(input logic [7:0] a, output logic y);
  assign y = a inside {8'h00, [8'h0a:8'h0f], 8'hf0};
endmodule
"#,
    )
    .expect("write inside source");

    let design = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("inside_expr"),
            &PreprocessOptions::default(),
        )
        .expect("lower inside expression");
    assert_eq!(design.assigns.len(), 1);

    let mut simulator = Simulator::new(&design).expect("create inside simulator");
    for (value, expected) in [
        (0x00_u64, true),
        (0x01, false),
        (0x09, false),
        (0x0a, true),
        (0x0d, true),
        (0x0f, true),
        (0x10, false),
        (0xef, false),
        (0xf0, true),
        (0xf1, false),
    ] {
        simulator
            .set_input("a", &LogicWord::from_u64(8, value))
            .unwrap();
        assert_eq!(
            simulator.read("y").unwrap().to_u64(),
            Some(expected as u64),
            "value {value:#04x}"
        );
    }
}

#[test]
fn slang_ast_direct_lowering_reports_expression_source_spans() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-ast-source-span");
    let source_path = root.join("spanned.sv");
    std::fs::write(
        &source_path,
        r#"
module spanned(
  input logic [3:0] a,
  output logic [3:0] y
);
  assign y = $countones(a);
endmodule
"#,
    )
    .expect("write source-span source");

    let error = slang
        .compile_files_via_ast(
            std::slice::from_ref(&source_path),
            Some("spanned"),
            &PreprocessOptions::default(),
        )
        .expect_err("unsupported direct expression must include its source span");
    assert!(
        error.contains("unknown direct-lowering function"),
        "{error}"
    );
    assert!(error.contains("[slang at "), "{error}");
    assert!(error.contains("spanned.sv:6:14"), "{error}");
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
fn slang_directly_lowers_and_runs_a_minimal_class_derived_from_official_uvm() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-direct-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_uvm_test extends uvm_test;
  function new(string name = "direct_uvm_test", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    #10;
  endtask
endclass

module official_uvm_direct_probe(output int done);
  direct_uvm_test test;
  initial begin
    test = new("direct_uvm_test", null);
    test.run_phase(null);
    done = 1;
  end
endmodule
"#,
    )
    .expect("write official-UVM direct-lowering wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_direct_probe"),
            &options,
        )
        .expect("directly lower a class derived from official UVM");
    assert_eq!(design.name, "official_uvm_direct_probe");
    let mut simulator =
        Simulator::new(&design).expect("create official-UVM direct-lowering simulator");
    simulator
        .run_until(10)
        .expect("run the official-UVM-derived class");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_run_test_bootstraps_a_minimal_official_uvm_class() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-run-test-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_run_test_probe extends uvm_test;
  function new(string name = "direct_run_test_probe", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    #10;
  endtask
endclass

module official_uvm_run_test_top(output int done);
  initial begin
    run_test("direct_run_test_probe");
    done = 1;
  end
endmodule
"#,
    )
    .expect("write official-UVM run_test wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_run_test_top"),
            &options,
        )
        .expect("directly lower a run_test class derived from official UVM");
    assert_eq!(design.name, "official_uvm_run_test_top");
    let mut simulator = Simulator::new(&design).expect("create official-UVM run_test simulator");
    simulator
        .run_until(10)
        .expect("run the official-UVM-derived run_phase");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_run_test_rejects_a_class_outside_official_uvm() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let root = temporary_directory("direct-run-test-non-uvm");
    let source_path = root.join("run_test_non_uvm.sv");
    std::fs::write(
        &source_path,
        r#"
class Plain;
  function new();
  endfunction

  virtual task run_phase();
    #1;
  endtask
endclass

task run_test(string class_name);
endtask

module run_test_non_uvm_top;
  initial begin
    run_test("Plain");
  end
endmodule
"#,
    )
    .expect("write direct run_test non-UVM source");

    let error = slang
        .compile_files(
            std::slice::from_ref(&source_path),
            Some("run_test_non_uvm_top"),
            &PreprocessOptions::default(),
        )
        .expect_err("run_test must fail closed outside official UVM");
    assert!(
        error.contains("run_test class 'Plain' must derive from official uvm_test"),
        "{error}"
    );
}

#[test]
fn slang_direct_run_test_executes_a_minimal_phase_schedule() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-phases-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class PhaseTrace;
  static int stage = 0;

  static function void mark(int value);
    stage = stage * 10 + value;
  endfunction
endclass

class direct_phase_probe extends uvm_test;
  function new(string name = "direct_phase_probe", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    PhaseTrace::mark(1);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    PhaseTrace::mark(2);
  endfunction

  virtual function void end_of_elaboration_phase(uvm_phase phase);
    PhaseTrace::mark(3);
  endfunction

  virtual function void start_of_simulation_phase(uvm_phase phase);
    PhaseTrace::mark(4);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    #3;
    PhaseTrace::mark(5);
    phase.drop_objection(this);
  endtask

  virtual function void extract_phase(uvm_phase phase);
    PhaseTrace::mark(6);
  endfunction

  virtual function void check_phase(uvm_phase phase);
    PhaseTrace::mark(7);
  endfunction

  virtual function void report_phase(uvm_phase phase);
    PhaseTrace::mark(8);
  endfunction

  virtual function void final_phase(uvm_phase phase);
    PhaseTrace::mark(9);
  endfunction
endclass

module official_uvm_phase_probe(output int stage);
  initial begin
    run_test("direct_phase_probe");
    stage = PhaseTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM phase wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_phase_probe"),
            &options,
        )
        .expect("directly lower a phased run_test class");
    let mut simulator = Simulator::new(&design).expect("create phased run_test simulator");
    simulator
        .run_until(3)
        .expect("run the minimal UVM phase schedule");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(123_456_789));
}

#[test]
fn slang_direct_official_uvm_reports_literal_messages() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_report_probe extends uvm_test;
  function new(string name = "direct_report_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    this.set_report_verbosity_level(UVM_HIGH);
    this.set_report_id_action("SILENCE", UVM_NO_ACTION);
    this.set_report_severity_id_action(UVM_INFO, "MUTE", UVM_DISPLAY);
    this.set_report_severity_id_action(UVM_WARNING, "NO_WARN", UVM_NO_ACTION);
  endfunction

  virtual task run_phase(uvm_phase phase);
    `uvm_info("HIGH", "high message", UVM_HIGH)
    `uvm_info("SILENCE", "silent message", UVM_LOW)
    `uvm_info("MUTE", "pair action message", UVM_LOW)
    `uvm_warning("NO_WARN", "silent warning")
    `uvm_info("DIRECT", "macro message", UVM_LOW)
    `uvm_warning("DIRECT", "macro warning")
    `uvm_error("DIRECT", "macro error")
    uvm_report_info("DIRECT", "informational message");
    uvm_report_warning("DIRECT", "warning message");
    uvm_report_error("DIRECT", "error message");
    `uvm_fatal("DIRECT", "macro fatal")
  endtask
endclass

module official_uvm_report_probe(output int done);
  initial begin
    done = 1;
    run_test("direct_report_probe");
  end
endmodule
"#,
    )
    .expect("write official-UVM report wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_probe"),
            &options,
        )
        .expect("directly lower official UVM report calls");
    let serialized = format!("{design:?}");
    assert!(serialized.contains("[UVM_INFO][HIGH][UVM_HIGH] high message"));
    assert!(!serialized.contains("silent message"));
    assert!(serialized.contains("[UVM_INFO][MUTE][UVM_LOW] pair action message"));
    assert!(!serialized.contains("silent warning"));
    assert!(serialized.contains("[UVM_INFO][DIRECT][UVM_LOW] macro message"));
    assert!(serialized.contains("[UVM_WARNING][DIRECT][UVM_NONE] macro warning"));
    assert!(serialized.contains("[UVM_ERROR][DIRECT][UVM_NONE] macro error"));
    assert!(serialized.contains("[UVM_FATAL][DIRECT][UVM_NONE] macro fatal"));
    assert!(serialized.contains("[UVM_INFO][DIRECT][UVM_MEDIUM] informational message"));
    assert!(serialized.contains("[UVM_WARNING][DIRECT][UVM_NONE] warning message"));
    assert!(serialized.contains("[UVM_ERROR][DIRECT][UVM_NONE] error message"));
    let mut simulator = Simulator::new(&design).expect("create UVM report simulator");
    simulator.run_until(0).expect("run UVM report messages");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_official_uvm_queries_report_controls() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-query-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ReportQueryTrace;
  static int maximum = -1;
  static int verbosity = -1;
  static int action = -1;
  static int max_quit = -1;
  static int quit = -1;
  static int id_count = -1;
  static int severity_count = -1;
endclass

class direct_report_query_probe extends uvm_test;
  function new(string name = "direct_report_query_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    this.set_report_verbosity_level(UVM_HIGH);
    this.set_report_id_action("SILENCE", UVM_NO_ACTION);
    uvm_report_server::get_server().set_max_quit_count(2);
    this.set_report_id_action("COUNTED", UVM_COUNT);
  endfunction

  virtual task run_phase(uvm_phase phase);
    ReportQueryTrace::maximum = this.get_report_max_verbosity_level();
    ReportQueryTrace::verbosity = this.get_report_verbosity_level(
      UVM_INFO,
      "HIGH"
    );
    ReportQueryTrace::action = this.get_report_action(
      UVM_INFO,
      "SILENCE"
    );
    uvm_report_info("COUNTED", "count only message", UVM_LOW);
    uvm_report_info("OTHER", "display message", UVM_LOW);
    ReportQueryTrace::max_quit =
      uvm_report_server::get_server().get_max_quit_count();
    ReportQueryTrace::quit =
      uvm_report_server::get_server().get_quit_count();
    ReportQueryTrace::id_count =
      uvm_report_server::get_server().get_id_count("COUNTED");
    ReportQueryTrace::severity_count =
      uvm_report_server::get_server().get_severity_count(UVM_INFO);
  endtask
endclass

module official_uvm_report_query_probe(
  output int maximum,
  output int verbosity,
  output int action,
  output int max_quit,
  output int quit,
  output int id_count,
  output int severity_count
);
  initial begin
    run_test("direct_report_query_probe");
    maximum = ReportQueryTrace::maximum;
    verbosity = ReportQueryTrace::verbosity;
    action = ReportQueryTrace::action;
    max_quit = ReportQueryTrace::max_quit;
    quit = ReportQueryTrace::quit;
    id_count = ReportQueryTrace::id_count;
    severity_count = ReportQueryTrace::severity_count;
  end
endmodule
"#,
    )
    .expect("write official-UVM report query wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_query_probe"),
            &options,
        )
        .expect("directly lower official UVM report queries");
    let mut simulator = Simulator::new(&design).expect("create UVM report query simulator");
    simulator.run_until(0).expect("run UVM report queries");
    assert_eq!(simulator.read("maximum").unwrap().to_u64(), Some(300));
    assert_eq!(simulator.read("verbosity").unwrap().to_u64(), Some(300));
    assert_eq!(simulator.read("action").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("max_quit").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("quit").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("id_count").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("severity_count").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_official_uvm_report_hooks_can_suppress_messages() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-hooks-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ReportHookTrace;
  static int stage = 0;
endclass

class direct_report_hook_probe extends uvm_test;
  function new(string name = "direct_report_hook_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function bit report_hook(
    string id,
    string message,
    int verbosity,
    string filename,
    int line
  );
    ReportHookTrace::stage = ReportHookTrace::stage * 10 + 1;
    return 1;
  endfunction

  virtual function bit report_info_hook(
    string id,
    string message,
    int verbosity,
    string filename,
    int line
  );
    ReportHookTrace::stage = ReportHookTrace::stage * 10 + 2;
    return 0;
  endfunction

  virtual function void build_phase(uvm_phase phase);
    this.set_report_verbosity_level(UVM_NONE);
    this.set_report_id_verbosity("HOOK", UVM_FULL);
    this.set_report_id_action("HOOK", UVM_CALL_HOOK);
    this.set_report_id_action("HOOK_SUPPRESSED", UVM_CALL_HOOK);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info("NO_HOOK", "hook action disabled message", UVM_LOW);
    ReportHookTrace::stage = ReportHookTrace::stage * 10 + 4;
    uvm_report_info("HOOK", "hook suppressed message", UVM_LOW);
    ReportHookTrace::stage = ReportHookTrace::stage * 10 + 3;
    uvm_report_info("HOOK_SUPPRESSED", "verbosity suppressed message", UVM_LOW);
    ReportHookTrace::stage = ReportHookTrace::stage * 10 + 5;
  endtask
endclass

module official_uvm_report_hook_probe(output int stage);
  initial begin
    run_test("direct_report_hook_probe");
    stage = ReportHookTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM report hook wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_hook_probe"),
            &options,
        )
        .expect("directly lower official UVM report hooks");
    let serialized = format!("{design:?}");
    assert!(!serialized.contains("hook suppressed message"));
    let mut simulator = Simulator::new(&design).expect("create UVM report hook simulator");
    simulator.run_until(0).expect("run UVM report hooks");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(41235));
}

#[test]
fn slang_direct_official_uvm_log_action_uses_default_stdout_suppression() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-log-action-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ReportLogTrace;
  static int counts = 0;
endclass

class direct_report_log_probe extends uvm_test;
  function new(string name = "direct_report_log_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    this.set_report_id_action("LOG_ONLY", UVM_LOG);
    this.set_report_id_action("LOG_DISPLAY", UVM_LOG | UVM_DISPLAY);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info("LOG_ONLY", "log-only message", UVM_LOW);
    uvm_report_info("LOG_DISPLAY", "logged display message", UVM_LOW);
    ReportLogTrace::counts =
      uvm_report_server::get_server().get_id_count("LOG_ONLY") * 10 +
      uvm_report_server::get_server().get_id_count("LOG_DISPLAY");
  endtask
endclass

module official_uvm_report_log_probe(output int counts);
  initial begin
    run_test("direct_report_log_probe");
    counts = ReportLogTrace::counts;
  end
endmodule
"#,
    )
    .expect("write official-UVM log-action wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_log_probe"),
            &options,
        )
        .expect("directly lower official UVM log actions");
    let serialized = format!("{design:?}");
    assert!(!serialized.contains("log-only message"));
    assert!(serialized.contains("logged display message"));
    let mut simulator = Simulator::new(&design).expect("create UVM log-action simulator");
    simulator.run_until(0).expect("run UVM log actions");
    assert_eq!(simulator.read("counts").unwrap().to_u64(), Some(11));
}

#[test]
fn slang_direct_official_uvm_report_catcher_can_catch_messages() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-catcher-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class CatcherTrace;
  static int stage = 0;
endclass

class direct_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_report_catcher");
  endfunction

  virtual function action_e catch();
    CatcherTrace::stage = CatcherTrace::stage * 10 + 1;
    return CAUGHT;
  endfunction
endclass

class direct_second_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_second_report_catcher");
  endfunction

  virtual function action_e catch();
    CatcherTrace::stage = 2;
    return THROW;
  endfunction
endclass

class direct_report_catcher_probe extends uvm_test;
  function new(string name = "direct_report_catcher_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info("CAUGHT", "catcher suppressed message", UVM_LOW);
    CatcherTrace::stage = CatcherTrace::stage * 10 + 2;
  endtask
endclass

module official_uvm_report_catcher_probe(output int stage);
  direct_report_catcher catcher;
  direct_second_report_catcher second;
  initial begin
    catcher = new("direct_report_catcher");
    second = new("direct_second_report_catcher");
    uvm_report_cb::add(null, catcher);
    uvm_report_cb::add(null, second, UVM_PREPEND);
    run_test("direct_report_catcher_probe");
    stage = CatcherTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM report catcher wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_catcher_probe"),
            &options,
        )
        .expect("directly lower an official UVM report catcher");
    let serialized = format!("{design:?}");
    assert!(!serialized.contains("catcher suppressed message"));
    let mut simulator = Simulator::new(&design).expect("create UVM report catcher simulator");
    simulator.run_until(0).expect("run UVM report catcher");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(212));
}

#[test]
fn slang_direct_official_uvm_report_catcher_modifies_messages() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-mod-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ReportModificationTrace;
  static int counts = 0;
endclass

class direct_modifying_catcher extends uvm_report_catcher;
  function new(string name = "direct_modifying_catcher");
  endfunction

  virtual function action_e catch();
    set_severity(UVM_WARNING);
    set_message("modified catcher message");
    set_action(UVM_DISPLAY | UVM_COUNT);
    return THROW;
  endfunction
endclass

class direct_report_modification_probe extends uvm_test;
  function new(string name = "direct_report_modification_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    this.set_report_id_action("MOD", UVM_NO_ACTION);
    uvm_report_server::get_server().set_max_quit_count(2);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_error("MOD", "original error message", UVM_NONE);
    ReportModificationTrace::counts =
      uvm_report_server::get_server().get_severity_count(UVM_WARNING) * 100 +
      uvm_report_server::get_server().get_severity_count(UVM_ERROR) * 10 +
      uvm_report_server::get_server().get_quit_count();
  endtask
endclass

module official_uvm_report_modification_probe(
  output int counts
);
  direct_modifying_catcher catcher;
  initial begin
    catcher = new("direct_modifying_catcher");
    uvm_report_cb::add(null, catcher);
    run_test("direct_report_modification_probe");
    counts = ReportModificationTrace::counts;
  end
endmodule
"#,
    )
    .expect("write official-UVM report-modification wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_modification_probe"),
            &options,
        )
        .expect("directly lower official UVM report modifications");
    let serialized = format!("{design:?}");
    assert!(!serialized.contains("original error message"));
    assert!(serialized.contains("modified catcher message"));
    assert!(serialized.contains("[UVM_WARNING]"));
    let mut simulator = Simulator::new(&design).expect("create UVM modification simulator");
    simulator
        .run_until(0)
        .expect("run UVM report modifications");
    assert_eq!(simulator.read("counts").unwrap().to_u64(), Some(101));
}

#[test]
fn slang_direct_official_uvm_report_catcher_scopes_to_context_object() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-scoped-report-catcher-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ScopedCatcherTrace;
  static int stage = 0;
endclass

class direct_missed_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_missed_report_catcher");
  endfunction

  virtual function action_e catch();
    ScopedCatcherTrace::stage =
      ScopedCatcherTrace::stage * 10 + 1;
    return CAUGHT;
  endfunction
endclass

class direct_applied_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_applied_report_catcher");
  endfunction

  virtual function action_e catch();
    ScopedCatcherTrace::stage =
      ScopedCatcherTrace::stage * 10 + 2;
    return CAUGHT;
  endfunction
endclass

class direct_report_other_object extends uvm_test;
  function new(string name = "direct_report_other_object",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction
endclass

class direct_scoped_report_catcher_probe extends uvm_test;
  direct_report_other_object other;
  direct_missed_report_catcher missed;
  direct_applied_report_catcher applied;

  function new(string name = "direct_scoped_report_catcher_probe",
               uvm_component parent = null);
    super.new(name, parent);
    other = new("direct_report_other_object", null);
    missed = new("direct_missed_report_catcher");
    applied = new("direct_applied_report_catcher");
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_report_cb::add(other, missed);
    uvm_report_cb::add(this, applied);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info(
      "SCOPED",
      "scoped catcher suppressed message",
      UVM_LOW
    );
    ScopedCatcherTrace::stage =
      ScopedCatcherTrace::stage * 10 + 3;
  endtask
endclass

module official_uvm_scoped_report_catcher_probe(output int stage);
  initial begin
    run_test("direct_scoped_report_catcher_probe");
    stage = ScopedCatcherTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM scoped report catcher wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_scoped_report_catcher_probe"),
            &options,
        )
        .expect("directly lower object-scoped UVM report catchers");
    let serialized = format!("{design:?}");
    assert!(!serialized.contains("scoped catcher suppressed message"));
    let mut simulator =
        Simulator::new(&design).expect("create scoped UVM report catcher simulator");
    simulator
        .run_until(0)
        .expect("run object-scoped UVM report catchers");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(23));
}

#[test]
fn slang_direct_official_uvm_report_catcher_delete_unregisters() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-catcher-delete-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class CatcherDeleteTrace;
  static int stage = 0;
endclass

class direct_deleted_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_deleted_report_catcher");
  endfunction

  virtual function action_e catch();
    CatcherDeleteTrace::stage = 1;
    return CAUGHT;
  endfunction
endclass

class direct_report_catcher_delete_probe extends uvm_test;
  function new(string name = "direct_report_catcher_delete_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info(
      "DELETED",
      "deleted catcher message",
      UVM_LOW
    );
    CatcherDeleteTrace::stage = CatcherDeleteTrace::stage * 10 + 2;
  endtask
endclass

module official_uvm_report_catcher_delete_probe(output int stage);
  direct_deleted_report_catcher catcher;
  initial begin
    catcher = new("direct_deleted_report_catcher");
    uvm_report_cb::add(null, catcher);
    uvm_report_cb::delete(null, catcher);
    run_test("direct_report_catcher_delete_probe");
    stage = CatcherDeleteTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM report catcher deletion wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_catcher_delete_probe"),
            &options,
        )
        .expect("directly lower UVM report catcher deletion");
    let serialized = format!("{design:?}");
    assert!(serialized.contains("deleted catcher message"));
    let mut simulator =
        Simulator::new(&design).expect("create UVM report catcher deletion simulator");
    simulator
        .run_until(0)
        .expect("run after deleting the UVM report catcher");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_official_uvm_object_callbacks_execute_derived_methods() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-object-callback-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class CallbackTrace;
  static int stage = 0;
  static int mode_before = -1;
  static int mode_after = -1;
endclass

class direct_callback extends uvm_callback;
  function new(string name = "direct_callback");
  endfunction

  virtual function void observe(int value);
    CallbackTrace::stage = CallbackTrace::stage + value;
  endfunction
endclass

class direct_derived_callback extends direct_callback;
  function new(string name = "direct_derived_callback");
  endfunction

  virtual function void observe(int value);
    CallbackTrace::stage = CallbackTrace::stage * 10 + value + 1;
  endfunction
endclass

class direct_object_callback_probe extends uvm_test;
  direct_callback first;
  direct_derived_callback second;

  function new(string name = "direct_object_callback_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    first = new("first_callback");
    second = new("second_callback");
    uvm_callbacks#(direct_object_callback_probe, direct_callback)::add(this, first);
    uvm_callbacks#(direct_object_callback_probe, direct_callback)::add(
      this,
      second,
      UVM_PREPEND
    );
    second.callback_mode(0);
  endfunction

  virtual task run_phase(uvm_phase phase);
    `uvm_do_callbacks(direct_object_callback_probe, direct_callback, observe(1))
    CallbackTrace::mode_before = second.callback_mode();
    second.callback_mode(1);
    CallbackTrace::mode_after = second.callback_mode();
    `uvm_do_callbacks(direct_object_callback_probe, direct_callback, observe(1))
    uvm_callbacks#(direct_object_callback_probe, direct_callback)::delete(this, second);
    uvm_callbacks#(direct_object_callback_probe, direct_callback)::add(this, second);
    uvm_callbacks#(direct_object_callback_probe, direct_callback)::delete_by_name(
      "uvm_test_top",
      second,
      null
    );
    `uvm_do_callbacks(direct_object_callback_probe, direct_callback, observe(1))
    phase.raise_objection(this);
    #1;
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_object_callback_probe(
  output int stage,
  output int mode_before,
  output int mode_after
);
  initial begin
    run_test("direct_object_callback_probe");
    stage = CallbackTrace::stage;
    mode_before = CallbackTrace::mode_before;
    mode_after = CallbackTrace::mode_after;
  end
endmodule
"#,
    )
    .expect("write official-UVM object-callback wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_object_callback_probe"),
            &options,
        )
        .expect("directly lower official UVM object callbacks");
    let mut simulator = Simulator::new(&design).expect("create UVM object-callback simulator");
    simulator.run_until(1).expect("run UVM object callbacks");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(14));
    assert_eq!(
        simulator.read("mode_before").unwrap().to_u64(),
        Some(0),
        "disabled callback mode should query as zero"
    );
    assert_eq!(
        simulator.read("mode_after").unwrap().to_u64(),
        Some(1),
        "re-enabled callback mode should query as one"
    );
}

#[test]
fn slang_direct_official_uvm_typewide_callbacks_execute_for_matching_targets() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-typewide-callback-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class CallbackTrace;
  static int stage = 0;
endclass

class direct_typewide_callback extends uvm_callback;
  function new(string name = "direct_typewide_callback");
  endfunction

  virtual function void observe(int value);
    CallbackTrace::stage = CallbackTrace::stage + value;
  endfunction
endclass

class direct_typewide_callback_probe extends uvm_test;

  function new(string name = "direct_typewide_callback_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
  endfunction

  virtual task run_phase(uvm_phase phase);
    `uvm_do_callbacks(direct_typewide_callback_probe, direct_typewide_callback, observe(7))
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

class direct_derived_typewide_callback_probe extends direct_typewide_callback_probe;
endclass

module official_uvm_typewide_callback_probe(output int stage);
  direct_typewide_callback callback;
  initial begin
    callback = new("typewide_callback");
    uvm_callbacks#(direct_typewide_callback_probe, direct_typewide_callback)::add(
      null,
      callback
    );
    run_test("direct_derived_typewide_callback_probe");
    stage = CallbackTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM typewide-callback wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_typewide_callback_probe"),
            &options,
        )
        .expect("directly lower official UVM typewide callbacks");
    let mut simulator = Simulator::new(&design).expect("create UVM typewide-callback simulator");
    simulator.run_until(0).expect("run UVM typewide callbacks");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(7));
}

#[test]
fn slang_direct_official_uvm_analysis_port_broadcasts_to_subscribers() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-analysis-port-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class AnalysisTrace;
  static int observed = 0;
  static int writes = 0;
endclass

class direct_analysis_subscriber extends uvm_subscriber#(int);
  function new(string name = "direct_analysis_subscriber",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void write(int t);
    AnalysisTrace::observed = AnalysisTrace::observed + t;
    AnalysisTrace::writes = AnalysisTrace::writes + 1;
  endfunction
endclass

class direct_analysis_probe extends uvm_test;
  direct_analysis_subscriber subscriber;
  direct_analysis_subscriber second_subscriber;
  uvm_analysis_port#(int) port;
  uvm_analysis_export#(int) analysis_export;

  function new(string name = "direct_analysis_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    subscriber = new("subscriber", this);
    second_subscriber = new("second_subscriber", this);
    port = new("port", this);
    analysis_export = new("analysis_export", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    analysis_export.connect(subscriber.analysis_export);
    analysis_export.connect(second_subscriber.analysis_export);
    port.connect(analysis_export);
  endfunction

  virtual task run_phase(uvm_phase phase);
    port.write(41);
    phase.raise_objection(this);
    #1;
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_analysis_port_probe(
  output int observed,
  output int writes
);
  initial begin
    run_test("direct_analysis_probe");
    observed = AnalysisTrace::observed;
    writes = AnalysisTrace::writes;
  end
endmodule
"#,
    )
    .expect("write official-UVM analysis-port wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_analysis_port_probe"),
            &options,
        )
        .expect("directly lower an official UVM analysis port");
    let mut simulator = Simulator::new(&design).expect("create UVM analysis simulator");
    simulator.run_until(1).expect("run UVM analysis port");
    assert_eq!(simulator.read("observed").unwrap().to_u64(), Some(82));
    assert_eq!(simulator.read("writes").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_official_uvm_analysis_export_cycles_fail_closed() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-analysis-cycle-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_analysis_cycle_probe extends uvm_test;
  uvm_analysis_port#(int) port;
  uvm_analysis_export#(int) first_export;
  uvm_analysis_export#(int) second_export;

  function new(string name = "direct_analysis_cycle_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    port = new("port", this);
    first_export = new("first_export", this);
    second_export = new("second_export", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    port.connect(first_export);
    first_export.connect(second_export);
    second_export.connect(first_export);
  endfunction

  virtual task run_phase(uvm_phase phase);
    port.write(1);
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_analysis_cycle_probe;
  initial run_test("direct_analysis_cycle_probe");
endmodule
"#,
    )
    .expect("write official-UVM analysis-cycle wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_analysis_cycle_probe"),
            &options,
        )
        .expect_err("UVM analysis export cycles must fail closed");
    assert!(error.contains("UVM analysis connection cycle"), "{error}");
}

#[test]
fn slang_direct_official_uvm_tlm_fifo_try_operations_execute_in_order() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-fifo-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class TlmTrace;
  static int capacity = -1;
  static int full_before_flush = -1;
  static int peeked = -1;
  static int first_get = -1;
  static int second_get = -1;
  static int used_after_flush = -1;
endclass

class direct_tlm_fifo_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;

  function new(string name = "direct_tlm_fifo_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 2);
  endfunction

  virtual task run_phase(uvm_phase phase);
    fifo.try_put(10);
    fifo.try_put(20);
    TlmTrace::capacity = fifo.size();
    TlmTrace::full_before_flush = fifo.is_full();
    fifo.try_put(99);
    fifo.try_peek(TlmTrace::peeked);
    fifo.try_get(TlmTrace::first_get);
    fifo.try_put(30);
    fifo.try_get(TlmTrace::second_get);
    fifo.flush();
    TlmTrace::used_after_flush = fifo.used();
    phase.raise_objection(this);
    #1;
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_tlm_fifo_probe(
  output int capacity,
  output int full_before_flush,
  output int peeked,
  output int first_get,
  output int second_get,
  output int used_after_flush
);
  initial begin
    run_test("direct_tlm_fifo_probe");
    capacity = TlmTrace::capacity;
    full_before_flush = TlmTrace::full_before_flush;
    peeked = TlmTrace::peeked;
    first_get = TlmTrace::first_get;
    second_get = TlmTrace::second_get;
    used_after_flush = TlmTrace::used_after_flush;
  end
endmodule
"#,
    )
    .expect("write official-UVM TLM-FIFO wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_fifo_probe"),
            &options,
        )
        .expect("directly lower an official UVM TLM FIFO");
    let mut simulator = Simulator::new(&design).expect("create UVM TLM FIFO simulator");
    simulator.run_until(1).expect("run UVM TLM FIFO");
    assert_eq!(
        simulator.read("capacity").unwrap().to_u64(),
        Some(2),
        "bounded FIFO capacity"
    );
    assert_eq!(
        simulator.read("full_before_flush").unwrap().to_u64(),
        Some(1),
        "FIFO should be full after two puts"
    );
    assert_eq!(simulator.read("peeked").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("first_get").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("second_get").unwrap().to_u64(), Some(20));
    assert_eq!(
        simulator.read("used_after_flush").unwrap().to_u64(),
        Some(0),
        "flush should empty the bounded FIFO"
    );
}

#[test]
fn slang_direct_official_uvm_tlm_fifo_expression_side_effects_fail_closed() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-fifo-expression-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_tlm_expression_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;

  function new(string name = "direct_tlm_expression_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 1);
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (fifo.try_put(1)) begin
      phase.raise_objection(this);
      phase.drop_objection(this);
    end
  endtask
endclass

module official_uvm_tlm_expression_probe;
  initial run_test("direct_tlm_expression_probe");
endmodule
"#,
    )
    .expect("write official-UVM TLM expression wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_expression_probe"),
            &options,
        )
        .expect_err("side-effecting TLM expressions must fail closed");
    assert!(
        error.contains("side-effecting UVM TLM FIFO try_put"),
        "{error}"
    );
}

#[test]
fn slang_direct_official_uvm_tlm_ports_route_to_bounded_fifos() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-ports-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class TlmPortTrace;
  static int can_put_before = -1;
  static int peeked = -1;
  static int first_get = -1;
  static int can_get_after_flush = -1;
endclass

class direct_tlm_port_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;
  uvm_put_port#(int) producer;
  uvm_get_peek_port#(int) consumer;

  function new(string name = "direct_tlm_port_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 2);
    producer = new("producer", this);
    consumer = new("consumer", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    producer.connect(fifo.put_export);
    consumer.connect(fifo.get_peek_export);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TlmPortTrace::can_put_before = producer.can_put();
    producer.try_put(10);
    producer.try_put(20);
    consumer.try_peek(TlmPortTrace::peeked);
    consumer.try_get(TlmPortTrace::first_get);
    fifo.flush();
    TlmPortTrace::can_get_after_flush = consumer.can_get();
    phase.raise_objection(this);
    #1;
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_tlm_port_probe(
  output int can_put_before,
  output int peeked,
  output int first_get,
  output int can_get_after_flush
);
  initial begin
    run_test("direct_tlm_port_probe");
    can_put_before = TlmPortTrace::can_put_before;
    peeked = TlmPortTrace::peeked;
    first_get = TlmPortTrace::first_get;
    can_get_after_flush = TlmPortTrace::can_get_after_flush;
  end
endmodule
"#,
    )
    .expect("write official-UVM TLM-port wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_port_probe"),
            &options,
        )
        .expect("directly lower official UVM TLM ports");
    let mut simulator = Simulator::new(&design).expect("create UVM TLM port simulator");
    simulator.run_until(1).expect("run UVM TLM ports");
    assert_eq!(simulator.read("can_put_before").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("peeked").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("first_get").unwrap().to_u64(), Some(10));
    assert_eq!(
        simulator.read("can_get_after_flush").unwrap().to_u64(),
        Some(0)
    );
}

#[test]
fn slang_direct_official_uvm_tlm_port_export_mismatches_fail_closed() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-port-mismatch-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_tlm_mismatch_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;
  uvm_put_port#(int) producer;

  function new(string name = "direct_tlm_mismatch_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 1);
    producer = new("producer", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    producer.connect(fifo.get_peek_export);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_tlm_mismatch_probe;
  initial run_test("direct_tlm_mismatch_probe");
endmodule
"#,
    )
    .expect("write official-UVM TLM mismatch wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_mismatch_probe"),
            &options,
        )
        .expect_err("UVM TLM port/export mismatches must fail closed");
    assert!(
        error.contains("UVM TLM endpoint requires 'put_export', found 'get_peek_export'"),
        "{error}"
    );
}

#[test]
fn slang_direct_official_uvm_tlm_export_chains_route_to_fifos() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-export-chain-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class TlmExportTrace;
  static int peeked = -1;
  static int first_get = -1;
  static int can_get_after_flush = -1;
endclass

class direct_tlm_export_chain_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;
  uvm_put_port#(int) producer;
  uvm_put_export#(int) put_link;
  uvm_get_peek_port#(int) consumer;
  uvm_get_peek_export#(int) get_link;

  function new(string name = "direct_tlm_export_chain_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 2);
    producer = new("producer", this);
    put_link = new("put_link", this);
    consumer = new("consumer", this);
    get_link = new("get_link", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    put_link.connect(fifo.put_export);
    producer.connect(put_link);
    get_link.connect(fifo.get_peek_export);
    consumer.connect(get_link);
  endfunction

  virtual task run_phase(uvm_phase phase);
    producer.try_put(10);
    producer.try_put(20);
    consumer.try_peek(TlmExportTrace::peeked);
    consumer.try_get(TlmExportTrace::first_get);
    fifo.flush();
    TlmExportTrace::can_get_after_flush = consumer.can_get();
    phase.raise_objection(this);
    #1;
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_tlm_export_chain_probe(
  output int peeked,
  output int first_get,
  output int can_get_after_flush
);
  initial begin
    run_test("direct_tlm_export_chain_probe");
    peeked = TlmExportTrace::peeked;
    first_get = TlmExportTrace::first_get;
    can_get_after_flush = TlmExportTrace::can_get_after_flush;
  end
endmodule
"#,
    )
    .expect("write official-UVM TLM export-chain wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_export_chain_probe"),
            &options,
        )
        .expect("directly lower official UVM TLM export chains");
    let mut simulator = Simulator::new(&design).expect("create UVM TLM export-chain simulator");
    simulator.run_until(1).expect("run UVM TLM export chains");
    assert_eq!(simulator.read("peeked").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("first_get").unwrap().to_u64(), Some(10));
    assert_eq!(
        simulator.read("can_get_after_flush").unwrap().to_u64(),
        Some(0)
    );
}

#[test]
fn slang_direct_official_uvm_tlm_export_cycles_fail_closed() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-export-cycle-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_tlm_export_cycle_probe extends uvm_test;
  uvm_put_port#(int) producer;
  uvm_put_export#(int) first_link;
  uvm_put_export#(int) second_link;

  function new(string name = "direct_tlm_export_cycle_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    producer = new("producer", this);
    first_link = new("first_link", this);
    second_link = new("second_link", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    first_link.connect(second_link);
    second_link.connect(first_link);
    producer.connect(first_link);
  endfunction

  virtual task run_phase(uvm_phase phase);
    producer.try_put(1);
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_tlm_export_cycle_probe;
  initial run_test("direct_tlm_export_cycle_probe");
endmodule
"#,
    )
    .expect("write official-UVM TLM export-cycle wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_tlm_export_cycle_probe"),
            &options,
        )
        .expect_err("UVM TLM export cycles must fail closed");
    assert!(
        error.contains("UVM TLM endpoint connection cycle"),
        "{error}"
    );
}

#[test]
fn slang_direct_official_uvm_tlm_blocking_ports_handshake_through_fifo() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-tlm-blocking-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class BlockingTlmTrace;
  static int first = -1;
  static int second = -1;
  static int producer_done = 0;
  static int consumer_done = 0;
endclass

class direct_blocking_tlm_probe extends uvm_test;
  uvm_tlm_fifo#(int) fifo;
  uvm_put_port#(int) producer;
  uvm_get_peek_port#(int) consumer;

  function new(string name = "direct_blocking_tlm_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    fifo = new("fifo", this, 1);
    producer = new("producer", this);
    consumer = new("consumer", this);
  endfunction

  virtual function void connect_phase(uvm_phase phase);
    producer.connect(fifo.put_export);
    consumer.connect(fifo.get_peek_export);
  endfunction

  virtual task run_phase(uvm_phase phase);
    fork
      begin
        producer.put(10);
        producer.put(20);
        BlockingTlmTrace::producer_done = 1;
      end
      begin
        #1;
        consumer.get(BlockingTlmTrace::first);
        #1;
        consumer.get(BlockingTlmTrace::second);
        BlockingTlmTrace::consumer_done = 1;
      end
    join
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_blocking_tlm_probe(
  output int first,
  output int second,
  output int producer_done,
  output int consumer_done
);
  initial begin
    run_test("direct_blocking_tlm_probe");
    first = BlockingTlmTrace::first;
    second = BlockingTlmTrace::second;
    producer_done = BlockingTlmTrace::producer_done;
    consumer_done = BlockingTlmTrace::consumer_done;
  end
endmodule
"#,
    )
    .expect("write official-UVM blocking-TLM wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_blocking_tlm_probe"),
            &options,
        )
        .expect("directly lower blocking UVM TLM ports");
    let mut simulator = Simulator::new(&design).expect("create blocking-UVM-TLM simulator");
    simulator
        .run_until(2)
        .expect("run blocking UVM TLM handshake");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(20));
    assert_eq!(
        simulator.read("producer_done").unwrap().to_u64(),
        Some(1),
        "blocked producer should resume after consumer get"
    );
    assert_eq!(
        simulator.read("consumer_done").unwrap().to_u64(),
        Some(1),
        "blocked consumer should resume after producer put"
    );
}

#[test]
fn slang_direct_official_uvm_sequence_sequencer_driver_handshake() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-sequence-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class SequenceTrace;
  static int first = -1;
  static int second = -1;
  static int first_response = -1;
  static int second_response = -1;
  static int driver_done = 0;
endclass

class direct_request extends uvm_sequence_item;
  int value;

  function new(string name = "direct_request");
    super.new(name);
  endfunction
endclass

class direct_test_sequence extends uvm_sequence#(direct_request);
  direct_request item;
  direct_request response;

  function new(string name = "direct_test_sequence");
    super.new(name);
  endfunction

  virtual task body();
    item = new("first_item");
    start_item(item);
    item.value = 42;
    finish_item(item);
    response = new("first_sequence_response");
    get_response(response);
    SequenceTrace::first_response = response.value;

    item = new("second_item");
    start_item(item);
    item.value = 17;
    finish_item(item);
    response = new("second_sequence_response");
    get_response(response);
    SequenceTrace::second_response = response.value;
  endtask
endclass

class direct_sequence_probe extends uvm_test;
  uvm_sequencer#(direct_request) sequencer;
  direct_test_sequence test_sequence;
  direct_request request;
  direct_request driver_response;

  function new(string name = "direct_sequence_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    sequencer = new("sequencer", this);
    test_sequence = new("sequence");
    request = new("driver_request");
    driver_response = new("driver_response");
  endfunction

  virtual task run_phase(uvm_phase phase);
    fork
      test_sequence.start(sequencer);
      begin
        sequencer.get_next_item(request);
        SequenceTrace::first = request.value;
        driver_response.value = 99;
        sequencer.item_done(driver_response);

        sequencer.get_next_item(request);
        SequenceTrace::second = request.value;
        driver_response.value = 84;
        sequencer.item_done(driver_response);
        SequenceTrace::driver_done = 1;
      end
    join
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_sequence_probe(
  output int first,
  output int second,
  output int first_response,
  output int second_response,
  output int driver_done
);
  initial begin
    run_test("direct_sequence_probe");
    first = SequenceTrace::first;
    second = SequenceTrace::second;
    first_response = SequenceTrace::first_response;
    second_response = SequenceTrace::second_response;
    driver_done = SequenceTrace::driver_done;
  end
endmodule
"#,
    )
    .expect("write official-UVM sequence wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_sequence_probe"),
            &options,
        )
        .expect("directly lower an official UVM sequence handshake");
    let mut simulator = Simulator::new(&design).expect("create UVM sequence simulator");
    simulator.run_until(0).expect("run UVM sequence handshake");
    assert_eq!(simulator.read("first").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("second").unwrap().to_u64(), Some(17));
    assert_eq!(simulator.read("first_response").unwrap().to_u64(), Some(99));
    assert_eq!(
        simulator.read("second_response").unwrap().to_u64(),
        Some(84)
    );
    assert_eq!(
        simulator.read("driver_done").unwrap().to_u64(),
        Some(1),
        "sequence and driver handshake should complete"
    );
}

#[test]
fn slang_direct_official_uvm_sequence_response_queue_reaches_depth_eight() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-response-queue-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ResponseQueueTrace;
  static int response_sum = 0;
  static int completed_responses = 0;
endclass

class queued_request extends uvm_sequence_item;
  int value;

  function new(string name = "queued_request");
    super.new(name);
  endfunction
endclass

class queued_response_sequence extends uvm_sequence#(queued_request);
  queued_request item;
  queued_request response;

  function new(string name = "queued_response_sequence");
    super.new(name);
  endfunction

  virtual task body();
    item = new("queued_item");
    start_item(item); item.value = 1;  finish_item(item);
    start_item(item); item.value = 2;  finish_item(item);
    start_item(item); item.value = 3;  finish_item(item);
    start_item(item); item.value = 4;  finish_item(item);
    start_item(item); item.value = 5;  finish_item(item);
    start_item(item); item.value = 6;  finish_item(item);
    start_item(item); item.value = 7;  finish_item(item);
    start_item(item); item.value = 8;  finish_item(item);

    response = new("queued_response");
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    get_response(response); ResponseQueueTrace::response_sum = ResponseQueueTrace::response_sum + response.value;
    ResponseQueueTrace::completed_responses = 8;
  endtask
endclass

class direct_response_queue_probe extends uvm_test;
  uvm_sequencer#(queued_request) sequencer;
  queued_response_sequence test_sequence;
  queued_request request;
  queued_request driver_response;

  function new(string name = "direct_response_queue_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    sequencer = new("sequencer", this);
    test_sequence = new("sequence");
    request = new("driver_request");
    driver_response = new("driver_response");
  endfunction

  virtual task run_phase(uvm_phase phase);
    fork
      test_sequence.start(sequencer);
      begin
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
        sequencer.get_next_item(request);
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
      end    join
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_response_queue_probe(
  output int response_sum,
  output int completed_responses
);
  initial begin
    run_test("direct_response_queue_probe");
    response_sum = ResponseQueueTrace::response_sum;
    completed_responses = ResponseQueueTrace::completed_responses;
  end
endmodule
"#,
    )
    .expect("write official-UVM response-queue wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_response_queue_probe"),
            &options,
        )
        .expect("directly lower official UVM response queue");
    let mut simulator = Simulator::new(&design).expect("create UVM response-queue simulator");
    simulator.run_until(0).expect("run UVM response queue");
    assert_eq!(
        simulator.read("response_sum").unwrap().to_u64(),
        Some(836),
        "eight queued responses should emerge in FIFO order"
    );
    assert_eq!(
        simulator.read("completed_responses").unwrap().to_u64(),
        Some(8)
    );
}

#[test]
fn slang_direct_official_uvm_sequence_selects_response_by_transaction_id() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-selective-response-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class SelectiveResponseTrace;
  static int first_value = -1;
  static int first_transaction_id = -1;
  static int second_value = -1;
  static int second_transaction_id = -1;
  static int completed_responses = 0;
endclass

class selective_request extends uvm_sequence_item;
  int value;

  function new(string name = "selective_request");
    super.new(name);
  endfunction
endclass

class selective_response_sequence extends uvm_sequence#(selective_request);
  selective_request item;
  selective_request response;

  function new(string name = "selective_response_sequence");
    super.new(name);
  endfunction

  virtual task body();
    item = new("first_item");
    item.set_transaction_id(10);
    start_item(item); item.value = 1; finish_item(item);

    item = new("second_item");
    item.set_transaction_id(11);
    start_item(item); item.value = 2; finish_item(item);

    response = new("selective_response");
    get_response(response, 11);
    SelectiveResponseTrace::first_value = response.value;
    SelectiveResponseTrace::first_transaction_id = response.get_transaction_id();

    get_response(response, 10);
    SelectiveResponseTrace::second_value = response.value;
    SelectiveResponseTrace::second_transaction_id = response.get_transaction_id();
    SelectiveResponseTrace::completed_responses = 2;
  endtask
endclass

class direct_selective_response_probe extends uvm_test;
  uvm_sequencer#(selective_request) sequencer;
  selective_response_sequence test_sequence;
  selective_request request;
  selective_request driver_response;

  function new(string name = "direct_selective_response_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  function void build_phase(uvm_phase phase);
    sequencer = new("sequencer", this);
    test_sequence = new("sequence");
    request = new("driver_request");
    driver_response = new("driver_response");
  endfunction

  virtual task run_phase(uvm_phase phase);
    fork
      test_sequence.start(sequencer);
      begin
        sequencer.get_next_item(request);
        driver_response.set_transaction_id(request.get_transaction_id());
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);

        sequencer.get_next_item(request);
        driver_response.set_transaction_id(request.get_transaction_id());
        driver_response.value = request.value + 100;
        sequencer.item_done(driver_response);
      end
    join
    phase.raise_objection(this);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_selective_response_probe(
  output int first_value,
  output int first_transaction_id,
  output int second_value,
  output int second_transaction_id,
  output int completed_responses
);
  initial begin
    run_test("direct_selective_response_probe");
    first_value = SelectiveResponseTrace::first_value;
    first_transaction_id = SelectiveResponseTrace::first_transaction_id;
    second_value = SelectiveResponseTrace::second_value;
    second_transaction_id = SelectiveResponseTrace::second_transaction_id;
    completed_responses = SelectiveResponseTrace::completed_responses;
  end
endmodule
"#,
    )
    .expect("write official-UVM selective-response wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_selective_response_probe"),
            &options,
        )
        .expect("directly lower selective UVM sequence responses");
    let mut simulator = Simulator::new(&design).expect("create selective-response simulator");
    simulator
        .run_until(0)
        .expect("run selective UVM response retrieval");
    assert_eq!(simulator.read("first_value").unwrap().to_u64(), Some(102));
    assert_eq!(
        simulator.read("first_transaction_id").unwrap().to_u64(),
        Some(11),
        "transaction 11 should be selected before queued transaction 10"
    );
    assert_eq!(simulator.read("second_value").unwrap().to_u64(), Some(101));
    assert_eq!(
        simulator.read("second_transaction_id").unwrap().to_u64(),
        Some(10),
        "the remaining FIFO response should stay available after out-of-order removal"
    );
    assert_eq!(
        simulator.read("completed_responses").unwrap().to_u64(),
        Some(2)
    );
}

#[test]
fn slang_direct_official_uvm_report_catcher_callback_mode_can_disable() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-catcher-mode-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class CatcherModeTrace;
  static int stage = 0;
endclass

class direct_mode_report_catcher extends uvm_report_catcher;
  function new(string name = "direct_mode_report_catcher");
  endfunction

  virtual function action_e catch();
    CatcherModeTrace::stage = 1;
    return CAUGHT;
  endfunction
endclass

class direct_report_catcher_mode_probe extends uvm_test;
  function new(string name = "direct_report_catcher_mode_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info(
      "MODE",
      "callback mode enabled message",
      UVM_LOW
    );
    CatcherModeTrace::stage = CatcherModeTrace::stage * 10 + 2;
  endtask
endclass

module official_uvm_report_catcher_mode_probe(output int stage);
  direct_mode_report_catcher catcher;
  initial begin
    catcher = new("direct_mode_report_catcher");
    uvm_report_cb::add(null, catcher);
    catcher.callback_mode(0);
    run_test("direct_report_catcher_mode_probe");
    stage = CatcherModeTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM report catcher mode wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_catcher_mode_probe"),
            &options,
        )
        .expect("directly lower UVM report catcher callback_mode");
    let serialized = format!("{design:?}");
    assert!(serialized.contains("callback mode enabled message"));
    let mut simulator = Simulator::new(&design).expect("create UVM report catcher mode simulator");
    simulator
        .run_until(0)
        .expect("run with the disabled UVM report catcher");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_official_uvm_report_catcher_delete_by_name_unregisters() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-catcher-delete-by-name-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class DeleteByNameTrace;
  static int stage = 0;
endclass

class direct_delete_by_name_catcher extends uvm_report_catcher;
  function new(string name = "direct_delete_by_name_catcher");
  endfunction

  virtual function action_e catch();
    DeleteByNameTrace::stage = 1;
    return CAUGHT;
  endfunction
endclass

class direct_delete_by_name_object extends uvm_test;
  function new(string name = "direct_delete_by_name_object",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction
endclass

class direct_report_delete_by_name_probe extends uvm_test;
  direct_delete_by_name_object other;
  direct_delete_by_name_catcher catcher;

  function new(string name = "direct_report_delete_by_name_probe",
               uvm_component parent = null);
    super.new(name, parent);
    other = new("other", this);
    catcher = new("direct_delete_by_name_catcher");
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_report_cb::add(other, catcher);
    uvm_report_cb::delete_by_name(
      "uvm_test_top.other",
      catcher,
      null
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info(
      "BY_NAME",
      "delete-by-name catcher message",
      UVM_LOW
    );
    DeleteByNameTrace::stage = DeleteByNameTrace::stage * 10 + 2;
  endtask
endclass

module official_uvm_report_delete_by_name_probe(output int stage);
  initial begin
    run_test("direct_report_delete_by_name_probe");
    stage = DeleteByNameTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM report delete-by-name wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_delete_by_name_probe"),
            &options,
        )
        .expect("directly lower UVM report catcher delete_by_name");
    let serialized = format!("{design:?}");
    assert!(serialized.contains("delete-by-name catcher message"));
    let mut simulator = Simulator::new(&design).expect("create UVM delete-by-name simulator");
    simulator.run_until(0).expect("run after delete_by_name");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_official_uvm_report_count_exits_at_max() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-report-count-exit-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_report_count_exit_probe extends uvm_test;
  function new(string name = "direct_report_count_exit_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_report_server::get_server().set_max_quit_count(1);
    this.set_report_id_action("QUIT", UVM_COUNT);
  endfunction

  virtual task run_phase(uvm_phase phase);
    uvm_report_info("QUIT", "count exit message", UVM_LOW);
  endtask
endclass

module official_uvm_report_count_exit_probe(output int done);
  initial begin
    done = 1;
    run_test("direct_report_count_exit_probe");
    done = 2;
  end
endmodule
"#,
    )
    .expect("write official-UVM report count-exit wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_report_count_exit_probe"),
            &options,
        )
        .expect("directly lower a UVM count-exit report");
    let mut simulator = Simulator::new(&design).expect("create UVM count-exit simulator");
    simulator.run_until(0).expect("run until the count exit");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_run_test_fails_closed_on_undrained_objections() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-objection-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_unbalanced_probe extends uvm_test;
  function new(string name = "direct_unbalanced_probe", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    #1;
  endtask
endclass

module official_uvm_objection_probe;
  initial begin
    run_test("direct_unbalanced_probe");
  end
endmodule
"#,
    )
    .expect("write official-UVM objection wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_objection_probe"),
            &options,
        )
        .expect("directly lower an unbalanced objection test");
    let mut simulator = Simulator::new(&design).expect("create unbalanced objection simulator");
    let error = simulator
        .run_until(1_001)
        .expect_err("undrained UVM objections must fail the simulation");
    assert!(
        error.contains("objections did not drain before the UVM bootstrap timeout"),
        "{error}"
    );
}

#[test]
fn slang_direct_run_test_waits_for_asynchronous_objection_drain() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-drain-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_drain_probe extends uvm_test;
  function new(string name = "direct_drain_probe", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    fork
      #2 phase.drop_objection(this);
    join_none
  endtask
endclass

module official_uvm_drain_probe(output int done);
  initial begin
    run_test("direct_drain_probe");
    done = 1;
  end
endmodule
"#,
    )
    .expect("write official-UVM objection drain wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_drain_probe"),
            &options,
        )
        .expect("directly lower an asynchronous objection drain test");
    let mut simulator = Simulator::new(&design).expect("create objection drain simulator");
    simulator
        .run_until(0)
        .expect("run until the pending objection is observed");
    assert!(
        simulator.read("done").unwrap().to_u64().is_none(),
        "run_test must not finish before the forked objection drains"
    );
    simulator
        .run_until(2)
        .expect("wait for the forked objection to drain");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_run_test_honors_configured_objection_timeout() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-custom-timeout-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_custom_timeout_probe extends uvm_test;
  function new(string name = "direct_custom_timeout_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(this);
    fork
      #1500 phase.drop_objection(this);
    join_none
  endtask
endclass

module official_uvm_custom_timeout_probe(output int done);
  initial begin
    uvm_root::get().set_timeout(2000ns);
    run_test("direct_custom_timeout_probe");
    done = 1;
  end
endmodule
"#,
    )
    .expect("write official-UVM custom timeout wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_custom_timeout_probe"),
            &options,
        )
        .expect("directly lower a configured UVM timeout");
    let mut simulator = Simulator::new(&design).expect("create custom timeout simulator");
    simulator
        .run_until(1500)
        .expect("configured UVM timeout must permit the delayed objection drop");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
}

#[test]
fn slang_direct_config_db_sets_and_gets_literals() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ConfigDbTrace;
  static int integer_found = 0;
  static int integer_value = -1;
  static int string_found = 0;
  static string string_value = "";
endclass

class direct_config_db_probe extends uvm_test;
  int integer_value;
  string string_value;

  function new(string name = "direct_config_db_probe",
               uvm_component parent = null);
    super.new(name, parent);
    integer_value = -1;
    string_value = "";
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_config_db#(int)::set(
      null,
      "uvm_test_top",
      "integer_value",
      42
    );
    uvm_config_db#(string)::set(
      null,
      "uvm_test_top",
      "string_value",
      "configured"
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (!uvm_config_db#(int)::get(
      this,
      "",
      "integer_value",
      integer_value
    )) begin
      ConfigDbTrace::integer_found = 0;
    end else begin
      ConfigDbTrace::integer_found = 1;
      ConfigDbTrace::integer_value = integer_value;
    end

    if (!uvm_config_db#(string)::get(
      this,
      "",
      "string_value",
      string_value
    )) begin
      ConfigDbTrace::string_found = 0;
    end else begin
      ConfigDbTrace::string_found = 1;
      ConfigDbTrace::string_value = string_value;
    end
  endtask
endclass

module official_uvm_config_db_probe(
  output int integer_found,
  output int integer_value,
  output int string_found,
  output string string_value
);
  initial begin
    run_test("direct_config_db_probe");
    integer_found = ConfigDbTrace::integer_found;
    integer_value = ConfigDbTrace::integer_value;
    string_found = ConfigDbTrace::string_found;
    string_value = ConfigDbTrace::string_value;
  end
endmodule
"#,
    )
    .expect("write official-UVM config DB wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_config_db_probe"),
            &options,
        )
        .expect("directly lower literal UVM config DB entries");
    let mut simulator = Simulator::new(&design).expect("create UVM config DB simulator");
    simulator
        .run_until(0)
        .expect("run the UVM config DB phases");
    assert_eq!(simulator.read("integer_found").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("integer_value").unwrap().to_u64(), Some(42));
    assert_eq!(simulator.read("string_found").unwrap().to_u64(), Some(1));
    let observed_string = simulator.read("string_value").expect("read config string");
    assert_eq!(
        observed_string.to_ascii_string_nul_terminated(),
        "configured"
    );
}

#[test]
fn slang_direct_config_db_matches_wildcards_and_precedence() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-priority-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ConfigPriorityTrace;
  static int global_value = -1;
  static int wildcard_value = -1;
  static int question_value = -1;
  static int precedence_value = -1;
  static int latest_value = -1;
  static int runtime_value = -1;
  static int regex_value = -1;
  static int regex_child_value = -1;
endclass

class direct_config_priority_probe extends uvm_test;
  int global_value;
  int wildcard_value;
  int question_value;
  int precedence_value;
  int latest_value;
  int runtime_value;
  int regex_value;
  int regex_child_value;

  function new(string name = "direct_config_priority_probe",
               uvm_component parent = null);
    super.new(name, parent);
    global_value = -1;
    wildcard_value = -1;
    question_value = -1;
    precedence_value = -1;
    latest_value = -1;
    runtime_value = -1;
    regex_value = -1;
    regex_child_value = -1;
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_config_db#(int)::set(null, "", "global_value", 30);
    uvm_config_db#(int)::set(null, "*", "wildcard_value", 10);
    uvm_config_db#(int)::set(null, "uvm_test_to?", "question_value", 20);
    uvm_config_db#(int)::set(
      null,
      "uvm_test_top*",
      "precedence_value",
      100
    );
    uvm_config_db#(int)::set(this, "", "precedence_value", 200);
    uvm_config_db#(int)::set(null, "uvm_test_top", "latest_value", 1);
    uvm_config_db#(int)::set(null, "uvm_test_top", "latest_value", 2);
    uvm_config_db#(int)::set(null, "uvm_test_top", "runtime_value", 400);
    uvm_config_db#(int)::set(null, "/uvm_test_to./", "regex_value", 70);
    uvm_config_db#(int)::set(this, "/.*/", "regex_child_value", 71);
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (!uvm_config_db#(int)::get(
      this,
      "",
      "global_value",
      global_value
    )) begin
      ConfigPriorityTrace::global_value = -1;
    end else begin
      ConfigPriorityTrace::global_value = global_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "wildcard_value",
      wildcard_value
    )) begin
      ConfigPriorityTrace::wildcard_value = -1;
    end else begin
      ConfigPriorityTrace::wildcard_value = wildcard_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "question_value",
      question_value
    )) begin
      ConfigPriorityTrace::question_value = -1;
    end else begin
      ConfigPriorityTrace::question_value = question_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "precedence_value",
      precedence_value
    )) begin
      ConfigPriorityTrace::precedence_value = -1;
    end else begin
      ConfigPriorityTrace::precedence_value = precedence_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "latest_value",
      latest_value
    )) begin
      ConfigPriorityTrace::latest_value = -1;
    end else begin
      ConfigPriorityTrace::latest_value = latest_value;
    end

    uvm_config_db#(int)::set(this, "", "runtime_value", 300);

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "runtime_value",
      runtime_value
    )) begin
      ConfigPriorityTrace::runtime_value = -1;
    end else begin
      ConfigPriorityTrace::runtime_value = runtime_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "",
      "regex_value",
      regex_value
    )) begin
      ConfigPriorityTrace::regex_value = -1;
    end else begin
      ConfigPriorityTrace::regex_value = regex_value;
    end

    if (!uvm_config_db#(int)::get(
      this,
      "leaf",
      "regex_child_value",
      regex_child_value
    )) begin
      ConfigPriorityTrace::regex_child_value = -1;
    end else begin
      ConfigPriorityTrace::regex_child_value = regex_child_value;
    end
  endtask
endclass

module official_uvm_config_priority_probe(
  output int global_value,
  output int wildcard_value,
  output int question_value,
  output int precedence_value,
  output int latest_value,
  output int runtime_value,
  output int regex_value,
  output int regex_child_value
);
  initial begin
    run_test("direct_config_priority_probe");
    global_value = ConfigPriorityTrace::global_value;
    wildcard_value = ConfigPriorityTrace::wildcard_value;
    question_value = ConfigPriorityTrace::question_value;
    precedence_value = ConfigPriorityTrace::precedence_value;
    latest_value = ConfigPriorityTrace::latest_value;
    runtime_value = ConfigPriorityTrace::runtime_value;
    regex_value = ConfigPriorityTrace::regex_value;
    regex_child_value = ConfigPriorityTrace::regex_child_value;
  end
endmodule
"#,
    )
    .expect("write official-UVM config priority wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_config_priority_probe"),
            &options,
        )
        .expect("directly lower scoped UVM config resources");
    let mut simulator = Simulator::new(&design).expect("create config priority simulator");
    simulator.run_until(0).expect("run config priority phases");
    assert_eq!(simulator.read("global_value").unwrap().to_u64(), Some(30));
    assert_eq!(simulator.read("wildcard_value").unwrap().to_u64(), Some(10));
    assert_eq!(simulator.read("question_value").unwrap().to_u64(), Some(20));
    assert_eq!(
        simulator.read("precedence_value").unwrap().to_u64(),
        Some(100)
    );
    assert_eq!(simulator.read("latest_value").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("runtime_value").unwrap().to_u64(), Some(300));
    assert_eq!(simulator.read("regex_value").unwrap().to_u64(), Some(70));
    assert_eq!(
        simulator.read("regex_child_value").unwrap().to_u64(),
        Some(71)
    );
}

#[test]
fn slang_direct_resource_db_sets_and_reads_literals() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-resource-db-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ResourceDbTrace;
  static int integer_found = 0;
  static int integer_value = -1;
  static int string_found = 0;
  static string string_value = "";
  static int shared_found = 0;
  static int shared_value = -1;
  static int missing_executed = 0;
  static int write_found = 0;
  static int missing_write_executed = 0;
  static int typed_found = 0;
  static int typed_value = -1;
  static int type_override_by_name = -1;
  static int type_override_by_type = -1;
  static int name_override_by_name = -1;
  static int name_override_by_type = -1;
  static int both_override_by_name = -1;
  static int both_override_by_type = -1;
  static int default_int_found = 0;
  static int default_int_value = -1;
  static int default_string_found = 0;
  static string default_string_value = "";
  static int anonymous_int_found = 0;
  static int anonymous_int_value = -1;
  static int anonymous_string_found = 0;
  static string anonymous_string_value = "";
endclass

class direct_resource_db_probe extends uvm_test;
  int integer_value;
  string string_value;
  int shared_value;
  int missing_value;
  int typed_value;
  int type_override_by_name;
  int type_override_by_type;
  int name_override_by_name;
  int name_override_by_type;
  int both_override_by_name;
  int both_override_by_type;
  int default_int_value;
  string default_string_value;
  int anonymous_int_value;
  string anonymous_string_value;

  function new(string name = "direct_resource_db_probe",
               uvm_component parent = null);
    super.new(name, parent);
    integer_value = -1;
    string_value = "";
    shared_value = -1;
    missing_value = -1;
    typed_value = -1;
    type_override_by_name = -1;
    type_override_by_type = -1;
    name_override_by_name = -1;
    name_override_by_type = -1;
    both_override_by_name = -1;
    both_override_by_type = -1;
    default_int_value = -1;
    default_string_value = "";
    anonymous_int_value = -1;
    anonymous_string_value = "";
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_resource_db#(int)::set(
      "uvm_test_top",
      "integer_value",
      42
    );
    uvm_resource_db#(string)::set(
      "uvm_test_top",
      "string_value",
      "resource"
    );
    uvm_config_db#(int)::set(
      null,
      "uvm_test_top",
      "shared_value",
      77
    );
    uvm_resource_db#(int)::set(
      "uvm_test_top.typed",
      "resource_name_is_ignored",
      88
    );
    uvm_resource_db#(int)::set_override_type(
      "uvm_test_top.priority",
      "value",
      21
    );
    uvm_resource_db#(int)::set(
      "uvm_test_top.priority",
      "value",
      20
    );
    uvm_resource_db#(int)::set_override_name(
      "uvm_test_top.name_priority",
      "value",
      31
    );
    uvm_resource_db#(int)::set(
      "uvm_test_top.name_priority",
      "value",
      30
    );
    uvm_resource_db#(int)::set_override(
      "uvm_test_top.both_priority",
      "value",
      41
    );
    uvm_resource_db#(int)::set(
      "uvm_test_top.both_priority",
      "value",
      40
    );
    uvm_resource_db#(int)::set_default(
      "uvm_test_top.default_int",
      "value"
    );
    uvm_resource_db#(string)::set_default(
      "uvm_test_top.default_string",
      "text"
    );
    uvm_resource_db#(int)::set_anonymous(
      "uvm_test_top.anonymous_int",
      57
    );
    uvm_resource_db#(string)::set_anonymous(
      "uvm_test_top.anonymous_string",
      "anonymous"
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (!uvm_resource_db#(int)::write_by_name(
      "uvm_test_top",
      "integer_value",
      43
    )) begin
      ResourceDbTrace::write_found = 0;
    end else begin
      ResourceDbTrace::write_found = 1;
    end

    if (!uvm_resource_db#(int)::write_by_type(
      "uvm_test_top.typed",
      89
    )) begin
      ResourceDbTrace::typed_found = 0;
    end else begin
      ResourceDbTrace::typed_found = 1;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.typed",
      typed_value
    )) begin
      ResourceDbTrace::typed_found = 0;
    end else begin
      ResourceDbTrace::typed_value = typed_value;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top.priority",
      "value",
      type_override_by_name
    )) begin
      ResourceDbTrace::type_override_by_name = -1;
    end else begin
      ResourceDbTrace::type_override_by_name = type_override_by_name;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.priority",
      type_override_by_type
    )) begin
      ResourceDbTrace::type_override_by_type = -1;
    end else begin
      ResourceDbTrace::type_override_by_type = type_override_by_type;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top.name_priority",
      "value",
      name_override_by_name
    )) begin
      ResourceDbTrace::name_override_by_name = -1;
    end else begin
      ResourceDbTrace::name_override_by_name = name_override_by_name;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.name_priority",
      name_override_by_type
    )) begin
      ResourceDbTrace::name_override_by_type = -1;
    end else begin
      ResourceDbTrace::name_override_by_type = name_override_by_type;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top.both_priority",
      "value",
      both_override_by_name
    )) begin
      ResourceDbTrace::both_override_by_name = -1;
    end else begin
      ResourceDbTrace::both_override_by_name = both_override_by_name;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.both_priority",
      both_override_by_type
    )) begin
      ResourceDbTrace::both_override_by_type = -1;
    end else begin
      ResourceDbTrace::both_override_by_type = both_override_by_type;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.default_int",
      default_int_value
    )) begin
      ResourceDbTrace::default_int_found = 0;
    end else begin
      ResourceDbTrace::default_int_found = 1;
      ResourceDbTrace::default_int_value = default_int_value;
    end

    if (!uvm_resource_db#(string)::read_by_type(
      "uvm_test_top.default_string",
      default_string_value
    )) begin
      ResourceDbTrace::default_string_found = 0;
    end else begin
      ResourceDbTrace::default_string_found = 1;
      ResourceDbTrace::default_string_value = default_string_value;
    end

    if (!uvm_resource_db#(int)::read_by_type(
      "uvm_test_top.anonymous_int",
      anonymous_int_value
    )) begin
      ResourceDbTrace::anonymous_int_found = 0;
    end else begin
      ResourceDbTrace::anonymous_int_found = 1;
      ResourceDbTrace::anonymous_int_value = anonymous_int_value;
    end

    if (!uvm_resource_db#(string)::read_by_type(
      "uvm_test_top.anonymous_string",
      anonymous_string_value
    )) begin
      ResourceDbTrace::anonymous_string_found = 0;
    end else begin
      ResourceDbTrace::anonymous_string_found = 1;
      ResourceDbTrace::anonymous_string_value = anonymous_string_value;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top",
      "integer_value",
      integer_value
    )) begin
      ResourceDbTrace::integer_found = 0;
    end else begin
      ResourceDbTrace::integer_found = 1;
      ResourceDbTrace::integer_value = integer_value;
    end

    if (!uvm_resource_db#(string)::read_by_name(
      "uvm_test_top",
      "string_value",
      string_value
    )) begin
      ResourceDbTrace::string_found = 0;
    end else begin
      ResourceDbTrace::string_found = 1;
      ResourceDbTrace::string_value = string_value;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top",
      "shared_value",
      shared_value
    )) begin
      ResourceDbTrace::shared_found = 0;
    end else begin
      ResourceDbTrace::shared_found = 1;
      ResourceDbTrace::shared_value = shared_value;
    end

    if (!uvm_resource_db#(int)::read_by_name(
      "uvm_test_top",
      "missing_value",
      missing_value
    )) begin
      ResourceDbTrace::missing_executed = 1;
    end else begin
      ResourceDbTrace::missing_executed = 0;
    end

    if (!uvm_resource_db#(int)::write_by_name(
      "uvm_test_top",
      "missing_value",
      99
    )) begin
      ResourceDbTrace::missing_write_executed = 1;
    end else begin
      ResourceDbTrace::missing_write_executed = 0;
    end
  endtask
endclass

module official_uvm_resource_db_probe(
  output int integer_found,
  output int integer_value,
  output int string_found,
  output string string_value,
  output int shared_found,
  output int shared_value,
  output int missing_executed,
  output int write_found,
  output int missing_write_executed,
  output int typed_found,
  output int typed_value,
  output int type_override_by_name,
  output int type_override_by_type,
  output int name_override_by_name,
  output int name_override_by_type,
  output int both_override_by_name,
  output int both_override_by_type,
  output int default_int_found,
  output int default_int_value,
  output int default_string_found,
  output string default_string_value,
  output int anonymous_int_found,
  output int anonymous_int_value,
  output int anonymous_string_found,
  output string anonymous_string_value
);
  initial begin
    run_test("direct_resource_db_probe");
    integer_found = ResourceDbTrace::integer_found;
    integer_value = ResourceDbTrace::integer_value;
    string_found = ResourceDbTrace::string_found;
    string_value = ResourceDbTrace::string_value;
    shared_found = ResourceDbTrace::shared_found;
    shared_value = ResourceDbTrace::shared_value;
    missing_executed = ResourceDbTrace::missing_executed;
    write_found = ResourceDbTrace::write_found;
    missing_write_executed = ResourceDbTrace::missing_write_executed;
    typed_found = ResourceDbTrace::typed_found;
    typed_value = ResourceDbTrace::typed_value;
    type_override_by_name = ResourceDbTrace::type_override_by_name;
    type_override_by_type = ResourceDbTrace::type_override_by_type;
    name_override_by_name = ResourceDbTrace::name_override_by_name;
    name_override_by_type = ResourceDbTrace::name_override_by_type;
    both_override_by_name = ResourceDbTrace::both_override_by_name;
    both_override_by_type = ResourceDbTrace::both_override_by_type;
    default_int_found = ResourceDbTrace::default_int_found;
    default_int_value = ResourceDbTrace::default_int_value;
    default_string_found = ResourceDbTrace::default_string_found;
    default_string_value = ResourceDbTrace::default_string_value;
    anonymous_int_found = ResourceDbTrace::anonymous_int_found;
    anonymous_int_value = ResourceDbTrace::anonymous_int_value;
    anonymous_string_found = ResourceDbTrace::anonymous_string_found;
    anonymous_string_value = ResourceDbTrace::anonymous_string_value;
  end
endmodule
"#,
    )
    .expect("write official-UVM resource DB wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_resource_db_probe"),
            &options,
        )
        .expect("directly lower literal UVM resource DB entries");
    let mut simulator = Simulator::new(&design).expect("create UVM resource DB simulator");
    simulator.run_until(0).expect("run UVM resource DB phases");
    assert_eq!(simulator.read("integer_found").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("integer_value").unwrap().to_u64(), Some(43));
    assert_eq!(simulator.read("write_found").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("typed_found").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("typed_value").unwrap().to_u64(), Some(89));
    assert_eq!(
        simulator.read("type_override_by_name").unwrap().to_u64(),
        Some(20)
    );
    assert_eq!(
        simulator.read("type_override_by_type").unwrap().to_u64(),
        Some(21)
    );
    assert_eq!(
        simulator.read("name_override_by_name").unwrap().to_u64(),
        Some(31)
    );
    assert_eq!(
        simulator.read("name_override_by_type").unwrap().to_u64(),
        Some(30)
    );
    assert_eq!(
        simulator.read("both_override_by_name").unwrap().to_u64(),
        Some(41)
    );
    assert_eq!(
        simulator.read("both_override_by_type").unwrap().to_u64(),
        Some(41)
    );
    assert_eq!(
        simulator.read("default_int_found").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator.read("default_int_value").unwrap().to_u64(),
        Some(0)
    );
    assert_eq!(
        simulator.read("default_string_found").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator
            .read("default_string_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        ""
    );
    assert_eq!(
        simulator.read("anonymous_int_found").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator.read("anonymous_int_value").unwrap().to_u64(),
        Some(57)
    );
    assert_eq!(
        simulator.read("anonymous_string_found").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator
            .read("anonymous_string_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "anonymous"
    );
    assert_eq!(simulator.read("string_found").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator
            .read("string_value")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "resource"
    );
    assert_eq!(simulator.read("shared_found").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("shared_value").unwrap().to_u64(), Some(77));
    assert_eq!(
        simulator.read("missing_executed").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator.read("missing_write_executed").unwrap().to_u64(),
        Some(1)
    );
}

#[test]
fn slang_direct_config_db_exists_checks_scoped_resources() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-exists-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ConfigExistsTrace;
  static int present = 0;
  static int missing = 0;
  static int spell_checked = 0;
  static int cross_type_spell_checked = 0;
endclass

class direct_config_exists_probe extends uvm_test;
  function new(string name = "direct_config_exists_probe",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_config_db#(int)::set(null, "uvm_test_top*", "value", 42);
    uvm_config_db#(string)::set(
      null,
      "uvm_test_top*",
      "description",
      "prepared"
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (uvm_config_db#(int)::exists(this, "", "value")) begin
      ConfigExistsTrace::present = 1;
    end else begin
      ConfigExistsTrace::present = 0;
    end

    if (!uvm_config_db#(int)::exists(this, "", "missing")) begin
      ConfigExistsTrace::missing = 1;
    end else begin
      ConfigExistsTrace::missing = 0;
    end

    if (!uvm_config_db#(int)::exists(this, "", "valeu", 1)) begin
      ConfigExistsTrace::spell_checked = 1;
    end else begin
      ConfigExistsTrace::spell_checked = 0;
    end

    if (!uvm_config_db#(int)::exists(this, "", "descriptoin", 1)) begin
      ConfigExistsTrace::cross_type_spell_checked = 1;
    end else begin
      ConfigExistsTrace::cross_type_spell_checked = 0;
    end
  endtask
endclass

module official_uvm_config_exists_probe(
  output int present,
  output int missing,
  output int spell_checked,
  output int cross_type_spell_checked
);
  initial begin
    run_test("direct_config_exists_probe");
    present = ConfigExistsTrace::present;
    missing = ConfigExistsTrace::missing;
    spell_checked = ConfigExistsTrace::spell_checked;
    cross_type_spell_checked = ConfigExistsTrace::cross_type_spell_checked;
  end
endmodule
"#,
    )
    .expect("write official-UVM config exists wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_config_exists_probe"),
            &options,
        )
        .expect("directly lower scoped UVM config exists checks");
    assert!(
        format!("{design:?}").contains("valeu not located, did you mean value"),
        "expected the config spell checker to suggest the prepared field"
    );
    assert!(
        format!("{design:?}").contains("descriptoin not located, did you mean description"),
        "expected the config spell checker to use the global resource dictionary"
    );
    let mut simulator = Simulator::new(&design).expect("create config exists simulator");
    simulator.run_until(0).expect("run config exists phases");
    assert_eq!(simulator.read("present").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("missing").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("spell_checked").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("cross_type_spell_checked").unwrap().to_u64(),
        Some(1)
    );
}

#[test]
fn slang_direct_config_db_wait_modified_wakes_on_matching_set() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-wait-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class ConfigWaitTrace;
  static int woken = 0;
  static int value = -1;
endclass

class direct_config_wait_probe extends uvm_test;
  int value;

  function new(string name = "direct_config_wait_probe",
               uvm_component parent = null);
    super.new(name, parent);
    value = -1;
  endfunction

  virtual task run_phase(uvm_phase phase);
    fork
      begin
        #10;
        uvm_config_db#(int)::set(this, "", "value", 42);
      end

      begin
        uvm_config_db#(int)::wait_modified(this, "", "value");
        ConfigWaitTrace::woken = 1;
        if (!uvm_config_db#(int)::get(
          this,
          "",
          "value",
          value
        )) begin
          ConfigWaitTrace::value = -1;
        end else begin
          ConfigWaitTrace::value = value;
        end
      end
    join
  endtask
endclass

module official_uvm_config_wait_probe(
  output int woken,
  output int value
);
  initial begin
    run_test("direct_config_wait_probe");
    woken = ConfigWaitTrace::woken;
    value = ConfigWaitTrace::value;
  end
endmodule
"#,
    )
    .expect("write official-UVM config wait wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_config_wait_probe"),
            &options,
        )
        .expect("directly lower a scoped UVM config wait");
    let mut simulator = Simulator::new(&design).expect("create config wait simulator");
    simulator
        .run_until(10)
        .expect("run until the matching UVM config set");
    assert_eq!(simulator.read("woken").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("value").unwrap().to_u64(), Some(42));
}

#[test]
fn slang_direct_config_db_missing_entry_runs_the_failure_branch() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-missing-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class MissingConfigTrace;
  static int failure_executed = 0;
  static int value = -1;
endclass

class direct_missing_config_probe extends uvm_test;
  int value;

  function new(string name = "direct_missing_config_probe",
               uvm_component parent = null);
    super.new(name, parent);
    value = -1;
  endfunction

  virtual task run_phase(uvm_phase phase);
    if (!uvm_config_db#(int)::get(
      this,
      "",
      "missing_value",
      value
    )) begin
      MissingConfigTrace::failure_executed = 1;
      MissingConfigTrace::value = value;
    end else begin
      MissingConfigTrace::failure_executed = 0;
      MissingConfigTrace::value = value;
    end
  endtask
endclass

module official_uvm_missing_config_probe(
  output int failure_executed,
  output int value
);
  initial begin
    run_test("direct_missing_config_probe");
    failure_executed = MissingConfigTrace::failure_executed;
    value = MissingConfigTrace::value;
  end
endmodule
"#,
    )
    .expect("write official-UVM missing config wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_missing_config_probe"),
            &options,
        )
        .expect("directly lower a missing UVM config lookup");
    let mut simulator = Simulator::new(&design).expect("create missing config simulator");
    simulator.run_until(0).expect("run the failure branch");
    assert_eq!(
        simulator.read("failure_executed").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator.read("value").unwrap().to_u64(),
        Some(u32::MAX as u64)
    );
}

#[test]
fn slang_direct_config_db_fails_closed_on_unsupported_forms() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let nonliteral = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-nonliteral-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &nonliteral,
        r#"
import uvm_pkg::*;

class direct_nonliteral_config extends uvm_test;
  int value;

  function new(string name = "direct_nonliteral_config",
               uvm_component parent = null);
    super.new(name, parent);
    value = 42;
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_config_db#(int)::set(
      null,
      "uvm_test_top",
      "value",
      value
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_nonliteral_config_probe;
  initial begin
    run_test("direct_nonliteral_config");
  end
endmodule
"#,
    )
    .expect("write nonliteral UVM config wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), nonliteral],
            Some("official_uvm_nonliteral_config_probe"),
            &options,
        )
        .expect_err("nonliteral config values must fail closed");
    assert!(
        error.contains("UVM config DB bootstrap values must be integer or string literals"),
        "unexpected nonliteral config error: {error}"
    );

    let direct_get = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-direct-get-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &direct_get,
        r#"
import uvm_pkg::*;

class direct_config_get_call extends uvm_test;
  int value;
  bit found;

  function new(string name = "direct_config_get_call",
               uvm_component parent = null);
    super.new(name, parent);
    value = -1;
    found = 0;
  endfunction

  virtual task run_phase(uvm_phase phase);
    found = uvm_config_db#(int)::get(
      this,
      "",
      "value",
      value
    );
  endtask
endclass

module official_uvm_direct_config_get_probe;
  initial begin
    run_test("direct_config_get_call");
  end
endmodule
"#,
    )
    .expect("write direct UVM config get wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), direct_get],
            Some("official_uvm_direct_config_get_probe"),
            &options,
        )
        .expect_err("a direct config get must fail closed instead of losing its output");
    assert!(
        error.contains("direct UVM config DB get is supported only as a conditional expression"),
        "unexpected direct config get error: {error}"
    );

    let direct_exists = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-direct-exists-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &direct_exists,
        r#"
import uvm_pkg::*;

class direct_config_exists_call extends uvm_test;
  bit present;

  function new(string name = "direct_config_exists_call",
               uvm_component parent = null);
    super.new(name, parent);
    present = 0;
  endfunction

  virtual task run_phase(uvm_phase phase);
    present = uvm_config_db#(int)::exists(this, "", "value");
  endtask
endclass

module official_uvm_direct_config_exists_probe;
  initial begin
    run_test("direct_config_exists_call");
  end
endmodule
"#,
    )
    .expect("write direct UVM config exists wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), direct_exists],
            Some("official_uvm_direct_config_exists_probe"),
            &options,
        )
        .expect_err("a direct config exists value must fail closed");
    assert!(
        error.contains("direct UVM config DB exists is supported only as a conditional expression"),
        "unexpected direct config exists error: {error}"
    );

    let invalid_regex = std::env::temp_dir().join(format!(
        "eda-official-uvm-config-db-invalid-regex-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &invalid_regex,
        r#"
import uvm_pkg::*;

class direct_invalid_regex_config extends uvm_test;
  function new(string name = "direct_invalid_regex_config",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    uvm_config_db#(int)::set(null, "/[/", "value", 42);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_invalid_regex_config_probe;
  initial begin
    run_test("direct_invalid_regex_config");
  end
endmodule
"#,
    )
    .expect("write invalid UVM config regex wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), invalid_regex],
            Some("official_uvm_invalid_regex_config_probe"),
            &options,
        )
        .expect_err("invalid config regexes must fail closed");
    assert!(
        error.contains("invalid UVM config DB regex scope"),
        "unexpected invalid config regex error: {error}"
    );

    let direct_resource_read = std::env::temp_dir().join(format!(
        "eda-official-uvm-direct-resource-read-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &direct_resource_read,
        r#"
import uvm_pkg::*;

class direct_resource_read_call extends uvm_test;
  int value;
  bit found;

  function new(string name = "direct_resource_read_call",
               uvm_component parent = null);
    super.new(name, parent);
    value = -1;
    found = 0;
  endfunction

  virtual task run_phase(uvm_phase phase);
    found = uvm_resource_db#(int)::read_by_name(
      "uvm_test_top",
      "value",
      value
    );
  endtask
endclass

module official_uvm_direct_resource_read_probe;
  initial begin
    run_test("direct_resource_read_call");
  end
endmodule
"#,
    )
    .expect("write direct UVM resource read wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), direct_resource_read],
            Some("official_uvm_direct_resource_read_probe"),
            &options,
        )
        .expect_err("a direct resource read must fail closed");
    assert!(
        error.contains(
            "direct UVM resource DB read_by_name is supported only as a conditional expression"
        ),
        "unexpected direct resource read error: {error}"
    );

    let direct_resource_write = std::env::temp_dir().join(format!(
        "eda-official-uvm-direct-resource-write-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &direct_resource_write,
        r#"
import uvm_pkg::*;

class direct_resource_write_call extends uvm_test;
  bit found;

  function new(string name = "direct_resource_write_call",
               uvm_component parent = null);
    super.new(name, parent);
    found = 0;
  endfunction

  virtual task run_phase(uvm_phase phase);
    found = uvm_resource_db#(int)::write_by_name(
      "uvm_test_top",
      "value",
      42
    );
  endtask
endclass

module official_uvm_direct_resource_write_probe;
  initial begin
    run_test("direct_resource_write_call");
  end
endmodule
"#,
    )
    .expect("write direct UVM resource write wrapper");
    let error = slang
        .compile_files(
            &[source.clone(), direct_resource_write],
            Some("official_uvm_direct_resource_write_probe"),
            &options,
        )
        .expect_err("a direct resource write must fail closed");
    assert!(
        error.contains(
            "direct UVM resource DB write_by_name is supported only as a conditional expression"
        ),
        "unexpected direct resource write error: {error}"
    );

    for (method_name, call) in [
        (
            "read_by_type",
            r#"uvm_resource_db#(int)::read_by_type("uvm_test_top", value)"#,
        ),
        (
            "write_by_type",
            r#"uvm_resource_db#(int)::write_by_type("uvm_test_top", 42)"#,
        ),
    ] {
        let path = std::env::temp_dir().join(format!(
            "eda-official-uvm-direct-{method_name}-{}-{}.sv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            format!(
                r#"
import uvm_pkg::*;

class DirectResourceTypeCall extends uvm_test;
  int value;
  bit found;

  function new(string name = "DirectResourceTypeCall",
               uvm_component parent = null);
    super.new(name, parent);
    value = -1;
    found = 0;
  endfunction

  virtual task run_phase(uvm_phase phase);
    found = {call};
  endtask
endclass

module official_uvm_direct_{method_name}_probe;
  initial begin
    run_test("DirectResourceTypeCall");
  end
endmodule
"#
            ),
        )
        .expect("write direct UVM resource type-call wrapper");
        let top = format!("official_uvm_direct_{method_name}_probe");
        let error = slang
            .compile_files(&[source.clone(), path], Some(&top), &options)
            .expect_err("a direct resource type call must fail closed");
        assert!(
            error.contains(&format!(
                "direct UVM resource DB {method_name} is supported only as a conditional expression"
            )),
            "unexpected direct {method_name} error: {error}"
        );
    }
}

#[test]
fn slang_direct_run_test_applies_a_name_based_factory_override() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-factory-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class FactoryTrace;
  static int stage = 0;
endclass

class direct_factory_base extends uvm_test;
  function new(string name = "direct_factory_base", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    FactoryTrace::stage = 1;
  endtask
endclass

class direct_factory_derived extends direct_factory_base;
  function new(string name = "direct_factory_derived", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    FactoryTrace::stage = 2;
  endtask
endclass

module official_uvm_factory_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_override_by_name(
      "direct_factory_base",
      "direct_factory_derived"
    );
    run_test("direct_factory_base");
    stage = FactoryTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM factory wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_factory_probe"),
            &options,
        )
        .expect("directly lower a name-based UVM factory override");
    let mut simulator = Simulator::new(&design).expect("create UVM factory simulator");
    simulator
        .run_until(0)
        .expect("run the factory-selected UVM test");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_run_test_applies_a_type_based_factory_override() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-type-factory-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class TypeFactoryTrace;
  static int stage = 0;
endclass

class direct_type_factory_base extends uvm_test;
  `uvm_component_utils(direct_type_factory_base)

  function new(string name = "direct_type_factory_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeFactoryTrace::stage = 1;
  endtask
endclass

class direct_type_factory_derived extends direct_type_factory_base;
  `uvm_component_utils(direct_type_factory_derived)

  function new(string name = "direct_type_factory_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeFactoryTrace::stage = 2;
  endtask
endclass

class direct_type_factory_second extends direct_type_factory_base;
  `uvm_component_utils(direct_type_factory_second)

  function new(string name = "direct_type_factory_second",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeFactoryTrace::stage = 3;
  endtask
endclass

class direct_type_factory_third extends direct_type_factory_base;
  `uvm_component_utils(direct_type_factory_third)

  function new(string name = "direct_type_factory_third",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeFactoryTrace::stage = 4;
  endtask
endclass

module official_uvm_type_factory_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_override_by_type(
      direct_type_factory_base::get_type(),
      direct_type_factory_derived::get_type()
    );
    uvm_factory::get().set_type_override_by_type(
      direct_type_factory_base::get_type(),
      direct_type_factory_second::get_type(),
      1'b0
    );
    uvm_factory::get().set_type_override_by_type(
      direct_type_factory_base::get_type(),
      direct_type_factory_third::get_type()
    );
    run_test("direct_type_factory_base");
    stage = TypeFactoryTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM type factory wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_type_factory_probe"),
            &options,
        )
        .expect("directly lower a type-based UVM factory override");
    let mut simulator = Simulator::new(&design).expect("create UVM type factory simulator");
    simulator
        .run_until(0)
        .expect("run the type-factory-selected UVM test");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(4));
}

#[test]
fn slang_direct_run_test_resolves_factory_type_aliases() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-type-alias-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class TypeAliasTrace;
  static int stage = 0;
endclass

class direct_type_alias_base extends uvm_test;
  `uvm_component_utils(direct_type_alias_base)

  function new(string name = "direct_type_alias_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeAliasTrace::stage = 1;
  endtask
endclass

class direct_type_alias_derived extends direct_type_alias_base;
  `uvm_component_utils(direct_type_alias_derived)

  function new(string name = "direct_type_alias_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    TypeAliasTrace::stage = 2;
  endtask
endclass

module official_uvm_type_alias_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_alias(
      "direct_type_alias",
      direct_type_alias_base::get_type()
    );
    uvm_factory::get().set_type_override_by_name(
      "direct_type_alias",
      "direct_type_alias_derived"
    );
    run_test("direct_type_alias_base");
    stage = TypeAliasTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM type-alias wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_type_alias_probe"),
            &options,
        )
        .expect("directly lower a UVM factory type alias");
    let mut simulator = Simulator::new(&design).expect("create type-alias simulator");
    simulator
        .run_until(0)
        .expect("run the alias-resolved factory override");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_factory_creates_components_by_name() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-component-by-name-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ComponentNameCreateTrace;
  static int kind = 0;
endclass

class direct_name_create_base extends uvm_test;
  `uvm_component_utils(direct_name_create_base)

  function new(string name = "direct_name_create_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 1;
  endfunction
endclass

class direct_name_create_derived extends direct_name_create_base;
  `uvm_component_utils(direct_name_create_derived)

  function new(string name = "direct_name_create_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

class direct_name_create_env extends uvm_test;
  `uvm_component_utils(direct_name_create_env)

  direct_name_create_base component;

  function new(string name = "direct_name_create_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    if (!$cast(
      component,
      uvm_factory::get().create_component_by_name(
        "direct_name_alias",
        "",
        "leaf",
        this
      )
    )) begin
      ComponentNameCreateTrace::kind = 0;
    end else begin
      ComponentNameCreateTrace::kind = component.kind();
    end
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_component_by_name_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_alias(
      "direct_name_alias",
      direct_name_create_base::get_type()
    );
    uvm_factory::get().set_type_override_by_name(
      "direct_name_alias",
      "direct_name_create_derived"
    );
    run_test("direct_name_create_env");
    stage = ComponentNameCreateTrace::kind;
  end
endmodule
"#,
    )
    .expect("write official-UVM create-component-by-name wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_component_by_name_probe"),
            &options,
        )
        .expect("directly lower a factory component-by-name creation");
    let mut simulator = Simulator::new(&design).expect("create component-by-name simulator");
    simulator
        .run_until(0)
        .expect("run the name-created component");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_factory_creates_objects_by_name() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-object-by-name-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ObjectNameCreateTrace;
  static int kind = 0;
endclass

class direct_object_name_base extends uvm_test;
  `uvm_object_utils(direct_object_name_base)

  function new(string name = "direct_object_name_base");
    super.new(name, null);
  endfunction

  virtual function int kind();
    return 1;
  endfunction
endclass

class direct_object_name_derived extends direct_object_name_base;
  `uvm_object_utils(direct_object_name_derived)

  function new(string name = "direct_object_name_derived");
    super.new(name);
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

class direct_object_name_env extends uvm_test;
  `uvm_component_utils(direct_object_name_env)

  direct_object_name_base item;

  function new(string name = "direct_object_name_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    if (!$cast(
      item,
      uvm_factory::get().create_object_by_name(
        "direct_object_name_alias",
        "",
        "item"
      )
    )) begin
      ObjectNameCreateTrace::kind = 0;
    end else begin
      ObjectNameCreateTrace::kind = item.kind();
    end
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_object_by_name_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_alias(
      "direct_object_name_alias",
      direct_object_name_base::get_type()
    );
    uvm_factory::get().set_type_override_by_name(
      "direct_object_name_alias",
      "direct_object_name_derived"
    );
    run_test("direct_object_name_env");
    stage = ObjectNameCreateTrace::kind;
  end
endmodule
"#,
    )
    .expect("write official-UVM create-object-by-name wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_object_by_name_probe"),
            &options,
        )
        .expect("directly lower a factory object-by-name creation");
    let mut simulator = Simulator::new(&design).expect("create object-by-name simulator");
    simulator.run_until(0).expect("run the name-created object");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_factory_creates_components_and_objects_by_type() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-create-by-type-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class DirectByTypeTrace;
  static int component_kind = 0;
  static int object_kind = 0;
endclass

class direct_by_type_component_base extends uvm_test;
  `uvm_component_utils(direct_by_type_component_base)

  function new(string name = "direct_by_type_component_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 1;
  endfunction
endclass

class direct_by_type_component_derived extends direct_by_type_component_base;
  `uvm_component_utils(direct_by_type_component_derived)

  function new(string name = "direct_by_type_component_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

class direct_by_type_object_base extends uvm_test;
  `uvm_object_utils(direct_by_type_object_base)

  function new(string name = "direct_by_type_object_base");
    super.new(name, null);
  endfunction

  virtual function int kind();
    return 3;
  endfunction
endclass

class direct_by_type_object_derived extends direct_by_type_object_base;
  `uvm_object_utils(direct_by_type_object_derived)

  function new(string name = "direct_by_type_object_derived");
    super.new(name);
  endfunction

  virtual function int kind();
    return 4;
  endfunction
endclass

class direct_by_type_env extends uvm_test;
  `uvm_component_utils(direct_by_type_env)

  direct_by_type_component_base component;
  direct_by_type_object_base item;

  function new(string name = "direct_by_type_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    if (!$cast(
      component,
      uvm_factory::get().create_component_by_type(
        direct_by_type_component_base::get_type(),
        "",
        "leaf",
        this
      )
    )) begin
      DirectByTypeTrace::component_kind = 0;
    end else begin
      DirectByTypeTrace::component_kind = component.kind();
    end

    if (!$cast(
      item,
      uvm_factory::get().create_object_by_type(
        direct_by_type_object_base::get_type(),
        "",
        "item"
      )
    )) begin
      DirectByTypeTrace::object_kind = 0;
    end else begin
      DirectByTypeTrace::object_kind = item.kind();
    end
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_create_by_type_probe(
  output int component_stage,
  output int object_stage
);
  initial begin
    uvm_factory::get().set_type_override_by_type(
      direct_by_type_component_base::get_type(),
      direct_by_type_component_derived::get_type()
    );
    uvm_factory::get().set_type_override_by_type(
      direct_by_type_object_base::get_type(),
      direct_by_type_object_derived::get_type()
    );
    run_test("direct_by_type_env");
    component_stage = DirectByTypeTrace::component_kind;
    object_stage = DirectByTypeTrace::object_kind;
  end
endmodule
"#,
    )
    .expect("write official-UVM create-by-type wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_create_by_type_probe"),
            &options,
        )
        .expect("directly lower factory create-by-type calls");
    let mut simulator = Simulator::new(&design).expect("create by-type simulator");
    simulator
        .run_until(0)
        .expect("run the by-type-created component and object");
    assert_eq!(simulator.read("component_stage").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("object_stage").unwrap().to_u64(), Some(4));
}

#[test]
fn slang_direct_component_registry_applies_instance_overrides() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-instance-factory-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class InstanceOverrideTrace;
  static int kind = 0;
  static int unaffected_kind = 0;
  static int wildcard_kind = 0;
  static int alias_kind = 0;
endclass

class direct_instance_base extends uvm_test;
  `uvm_component_utils(direct_instance_base)

  function new(string name = "direct_instance_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask

  virtual function int kind();
    return 1;
  endfunction
endclass

class direct_instance_derived extends direct_instance_base;
  `uvm_component_utils(direct_instance_derived)

  function new(string name = "direct_instance_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

class direct_instance_second extends direct_instance_base;
  `uvm_component_utils(direct_instance_second)

  function new(string name = "direct_instance_second",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int kind();
    return 3;
  endfunction
endclass

class direct_instance_env extends uvm_test;
  `uvm_component_utils(direct_instance_env)

  direct_instance_base component;
  direct_instance_base unaffected;
  direct_instance_base wildcarded;
  direct_instance_base aliased;

  function new(string name = "direct_instance_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    component = direct_instance_base::type_id::create("leaf", this);
    unaffected = direct_instance_base::type_id::create("other", this);
    wildcarded = direct_instance_base::type_id::create("wildcarded", this);
    aliased = direct_instance_base::type_id::create("aliased", this);
  endfunction

  virtual task run_phase(uvm_phase phase);
    InstanceOverrideTrace::kind = component.kind();
    InstanceOverrideTrace::unaffected_kind = unaffected.kind();
    InstanceOverrideTrace::wildcard_kind = wildcarded.kind();
    InstanceOverrideTrace::alias_kind = aliased.kind();
  endtask
endclass

module official_uvm_instance_factory_probe(
  output int stage,
  output int unaffected_stage,
  output int wildcard_stage,
  output int alias_stage
);
  initial begin
    uvm_factory::get().set_inst_alias(
      "direct_instance_base_alias",
      direct_instance_base::get_type(),
      "uvm_test_top.aliased"
    );
    uvm_factory::get().set_inst_override_by_name(
      "direct_instance_base_alias",
      "direct_instance_derived",
      "uvm_test_top.aliased"
    );
    uvm_factory::get().set_inst_override_by_type(
      direct_instance_base::get_type(),
      direct_instance_derived::get_type(),
      "uvm_test_top.leaf"
    );
    uvm_factory::get().set_inst_override_by_type(
      direct_instance_base::get_type(),
      direct_instance_derived::get_type(),
      "uvm_test_top.wild*"
    );
    uvm_factory::get().set_inst_override_by_type(
      direct_instance_base::get_type(),
      direct_instance_second::get_type(),
      "uvm_test_top.wild*"
    );
    run_test("direct_instance_env");
    stage = InstanceOverrideTrace::kind;
    unaffected_stage = InstanceOverrideTrace::unaffected_kind;
    wildcard_stage = InstanceOverrideTrace::wildcard_kind;
    alias_stage = InstanceOverrideTrace::alias_kind;
  end
endmodule
"#,
    )
    .expect("write official-UVM instance-override wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_instance_factory_probe"),
            &options,
        )
        .expect("directly lower an instance-specific UVM factory override");
    let mut simulator = Simulator::new(&design).expect("create instance-override simulator");
    simulator
        .run_until(0)
        .expect("run the instance-overridden component");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
    assert_eq!(
        simulator.read("unaffected_stage").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(simulator.read("wildcard_stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_component_registry_creates_a_component() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-create-component-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class CreateTrace;
  static int stage = 0;
endclass

class direct_created_component extends uvm_test;
  `uvm_component_utils(direct_created_component)

  function new(string name = "direct_created_component",
               uvm_component parent = null);
    super.new(name, parent);
    CreateTrace::stage = 7;
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

class direct_component_creator extends uvm_test;
  `uvm_component_utils(direct_component_creator)

  direct_created_component component;

  function new(string name = "direct_component_creator",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    component = direct_created_component::type_id::create(
      "component",
      this
    );
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_create_component_probe(output int stage);
  initial begin
    run_test("direct_component_creator");
    stage = CreateTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM create-component wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_create_component_probe"),
            &options,
        )
        .expect("directly lower a factory component creation");
    let mut simulator = Simulator::new(&design).expect("create factory component simulator");
    simulator
        .run_until(0)
        .expect("run build_phase and the factory-created component constructor");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(7));
}

#[test]
fn slang_direct_component_registry_applies_factory_overrides() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-component-override-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ComponentOverrideTrace;
  static int stage = 0;
endclass

class direct_component_base extends uvm_test;
  `uvm_component_utils(direct_component_base)

  function new(string name = "direct_component_base",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    ComponentOverrideTrace::stage = 1;
  endtask
endclass

class direct_component_derived extends direct_component_base;
  `uvm_component_utils(direct_component_derived)

  function new(string name = "direct_component_derived",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
    ComponentOverrideTrace::stage = 2;
  endtask
endclass

class direct_component_env extends uvm_test;
  `uvm_component_utils(direct_component_env)

  direct_component_base component;

  function new(string name = "direct_component_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    component = direct_component_base::type_id::create("component", this);
  endfunction

  virtual task run_phase(uvm_phase phase);
    component.run_phase(phase);
  endtask
endclass

module official_uvm_component_override_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_override_by_type(
      direct_component_base::get_type(),
      direct_component_derived::get_type()
    );
    run_test("direct_component_env");
    stage = ComponentOverrideTrace::stage;
  end
endmodule
"#,
    )
    .expect("write official-UVM component override wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_component_override_probe"),
            &options,
        )
        .expect("directly lower a factory-overridden component creation");
    let mut simulator =
        Simulator::new(&design).expect("create factory component override simulator");
    simulator
        .run_until(0)
        .expect("run the factory-overridden component phase");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_component_registry_preserves_component_names() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-component-name-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ComponentNameTrace;
  static string name = "";
  static string full_name = "";
  static string root_name = "";
  static int parent_match = 0;
  static int parent_identity = 0;
  static int child_match = 0;
  static int child_identity = 0;
  static int child_count = 0;
  static int lookup_match = 0;
  static int lookup_identity = 0;
  static int nested_match = 0;
  static int nested_identity = 0;
  static int absolute_match = 0;
  static int absolute_identity = 0;
  static int root_child_match = 0;
  static int root_child_count = 0;
  static int root_depth = -1;
  static int root_has_child = 0;
  static int root_lookup_match = 0;
  static int has_leaf = 0;
  static int has_nested = 0;
  static int missing_child = 1;
  static int test_depth = 0;
  static int component_depth = 0;
  static int nested_depth = 0;
endclass

class direct_named_leaf extends uvm_test;
  `uvm_component_utils(direct_named_leaf)

  function new(string name = "direct_named_leaf",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int identity();
    return 4;
  endfunction
endclass

class direct_named_component extends uvm_test;
  `uvm_component_utils(direct_named_component)

  direct_named_leaf nested;

  function new(string name = "direct_named_component",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask

  virtual function int identity();
    return 3;
  endfunction
endclass

class direct_named_base extends uvm_test;
  function new(string name = "direct_named_base", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function int identity();
    return 1;
  endfunction
endclass

class direct_named_component_env extends direct_named_base;
  `uvm_component_utils(direct_named_component_env)

  direct_named_component component;
  direct_named_component recovered_child;
  direct_named_leaf recovered_nested;
  direct_named_base parent_handle;

  function new(string name = "direct_named_component_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    component = direct_named_component::type_id::create("leaf", this);
    component.nested =
      direct_named_leaf::type_id::create("nested", component);
  endfunction

  virtual function int identity();
    return 2;
  endfunction

  virtual task run_phase(uvm_phase phase);
    ComponentNameTrace::name = component.get_name();
    ComponentNameTrace::full_name = component.get_full_name();
    ComponentNameTrace::root_name = this.get_name();
    if (component.get_parent() == this) begin
      ComponentNameTrace::parent_match = 1;
    end
    if (!$cast(parent_handle, component.get_parent())) begin
      ComponentNameTrace::parent_identity = 0;
    end else begin
      ComponentNameTrace::parent_identity = parent_handle.identity();
    end
    ComponentNameTrace::child_count = this.get_num_children();
    if (this.get_child("leaf") == component) begin
      ComponentNameTrace::child_match = 1;
    end
    if (!$cast(recovered_child, this.get_child("leaf"))) begin
      ComponentNameTrace::child_identity = 0;
    end else begin
      ComponentNameTrace::child_identity = recovered_child.identity();
    end
    if (this.lookup("leaf") == component) begin
      ComponentNameTrace::lookup_match = 1;
    end
    if (!$cast(recovered_child, this.lookup("leaf"))) begin
      ComponentNameTrace::lookup_identity = 0;
    end else begin
      ComponentNameTrace::lookup_identity = recovered_child.identity();
    end
    if (this.lookup("leaf.nested") == component.nested) begin
      ComponentNameTrace::nested_match = 1;
    end
    if (!$cast(recovered_nested, this.lookup("leaf.nested"))) begin
      ComponentNameTrace::nested_identity = 0;
    end else begin
      ComponentNameTrace::nested_identity = recovered_nested.identity();
    end
    if (component.nested.lookup(".uvm_test_top.leaf") == component) begin
      ComponentNameTrace::absolute_match = 1;
    end
    if (uvm_root::get().get_child("uvm_test_top") == this) begin
      ComponentNameTrace::root_child_match = 1;
    end
    ComponentNameTrace::root_child_count = uvm_root::get().get_num_children();
    ComponentNameTrace::root_depth = uvm_root::get().get_depth();
    ComponentNameTrace::root_has_child = uvm_root::get().has_child("uvm_test_top");
    if (uvm_root::get().lookup(".uvm_test_top.leaf") == component) begin
      ComponentNameTrace::root_lookup_match = 1;
    end
    if (!$cast(
      recovered_child,
      component.nested.lookup(".uvm_test_top.leaf")
    )) begin
      ComponentNameTrace::absolute_identity = 0;
    end else begin
      ComponentNameTrace::absolute_identity = recovered_child.identity();
    end
    ComponentNameTrace::has_leaf = this.has_child("leaf");
    ComponentNameTrace::has_nested = component.has_child("nested");
    ComponentNameTrace::missing_child = this.has_child("missing");
    ComponentNameTrace::test_depth = this.get_depth();
    ComponentNameTrace::component_depth = component.get_depth();
    ComponentNameTrace::nested_depth = component.nested.get_depth();
  endtask
endclass

module official_uvm_component_name_probe(
  output string name,
  output string full_name,
  output string root_name,
  output int parent_match,
  output int parent_identity,
  output int child_match,
  output int child_identity,
  output int child_count,
  output int lookup_match,
  output int lookup_identity,
  output int nested_match,
  output int nested_identity,
  output int absolute_match,
  output int absolute_identity,
  output int root_child_match,
  output int root_child_count,
  output int root_depth,
  output int root_has_child,
  output int root_lookup_match,
  output int has_leaf,
  output int has_nested,
  output int missing_child,
  output int test_depth,
  output int component_depth,
  output int nested_depth
);
  initial begin
    run_test("direct_named_component_env");
    name = ComponentNameTrace::name;
    full_name = ComponentNameTrace::full_name;
    root_name = ComponentNameTrace::root_name;
    parent_match = ComponentNameTrace::parent_match;
    parent_identity = ComponentNameTrace::parent_identity;
    child_match = ComponentNameTrace::child_match;
    child_identity = ComponentNameTrace::child_identity;
    child_count = ComponentNameTrace::child_count;
    lookup_match = ComponentNameTrace::lookup_match;
    lookup_identity = ComponentNameTrace::lookup_identity;
    nested_match = ComponentNameTrace::nested_match;
    nested_identity = ComponentNameTrace::nested_identity;
    absolute_match = ComponentNameTrace::absolute_match;
    absolute_identity = ComponentNameTrace::absolute_identity;
    root_child_match = ComponentNameTrace::root_child_match;
    root_child_count = ComponentNameTrace::root_child_count;
    root_depth = ComponentNameTrace::root_depth;
    root_has_child = ComponentNameTrace::root_has_child;
    root_lookup_match = ComponentNameTrace::root_lookup_match;
    has_leaf = ComponentNameTrace::has_leaf;
    has_nested = ComponentNameTrace::has_nested;
    missing_child = ComponentNameTrace::missing_child;
    test_depth = ComponentNameTrace::test_depth;
    component_depth = ComponentNameTrace::component_depth;
    nested_depth = ComponentNameTrace::nested_depth;
  end
endmodule
"#,
    )
    .expect("write official-UVM component-name wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_component_name_probe"),
            &options,
        )
        .expect("directly lower a named component creation");
    let mut simulator = Simulator::new(&design).expect("create named component simulator");
    simulator
        .run_until(0)
        .expect("run the named component phase");
    assert_eq!(
        simulator
            .read("name")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "leaf"
    );
    assert_eq!(
        simulator
            .read("full_name")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "uvm_test_top.leaf"
    );
    assert_eq!(
        simulator
            .read("root_name")
            .unwrap()
            .to_ascii_string_nul_terminated(),
        "uvm_test_top"
    );
    assert_eq!(simulator.read("parent_match").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("parent_identity").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("child_match").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("child_identity").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("child_count").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("lookup_match").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("lookup_identity").unwrap().to_u64(), Some(3));
    assert_eq!(simulator.read("nested_match").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("nested_identity").unwrap().to_u64(), Some(4));
    assert_eq!(simulator.read("absolute_match").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("absolute_identity").unwrap().to_u64(),
        Some(3)
    );
    assert_eq!(
        simulator.read("root_child_match").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(
        simulator.read("root_child_count").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(simulator.read("root_depth").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("root_has_child").unwrap().to_u64(), Some(1));
    assert_eq!(
        simulator.read("root_lookup_match").unwrap().to_u64(),
        Some(1)
    );
    assert_eq!(simulator.read("has_leaf").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("has_nested").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("missing_child").unwrap().to_u64(), Some(0));
    assert_eq!(simulator.read("test_depth").unwrap().to_u64(), Some(1));
    assert_eq!(simulator.read("component_depth").unwrap().to_u64(), Some(2));
    assert_eq!(simulator.read("nested_depth").unwrap().to_u64(), Some(3));
}

#[test]
fn slang_direct_component_registry_rejects_duplicate_child_names() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-duplicate-child-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class direct_duplicate_component extends uvm_test;
  `uvm_component_utils(direct_duplicate_component)

  function new(string name = "direct_duplicate_component",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

class direct_duplicate_env extends uvm_test;
  `uvm_component_utils(direct_duplicate_env)

  direct_duplicate_component first;
  direct_duplicate_component second;

  function new(string name = "direct_duplicate_env",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    first = direct_duplicate_component::type_id::create("same", this);
    second = direct_duplicate_component::type_id::create("same", this);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_duplicate_child_probe;
  initial begin
    run_test("direct_duplicate_env");
  end
endmodule
"#,
    )
    .expect("write official-UVM duplicate-child wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_duplicate_child_probe"),
            &options,
        )
        .expect_err("duplicate UVM child names must fail closed");
    assert!(error.contains("duplicate child name 'same'"), "{error}");
}

#[test]
fn slang_direct_object_registry_applies_factory_overrides() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-object-override-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
`include "uvm_macros.svh"

import uvm_pkg::*;

class ObjectOverrideTrace;
  static int kind = 0;
endclass

class direct_object_base extends uvm_test;
  `uvm_object_utils(direct_object_base)

  function new(string name = "direct_object_base");
    super.new(name, null);
  endfunction

  virtual function int kind();
    return 1;
  endfunction
endclass

class direct_object_derived extends direct_object_base;
  `uvm_object_utils(direct_object_derived)

  function new(string name = "direct_object_derived");
    super.new(name);
  endfunction

  virtual function int kind();
    return 2;
  endfunction
endclass

class direct_object_env extends uvm_test;
  `uvm_component_utils(direct_object_env)

  direct_object_base item;

  function new(string name = "direct_object_env", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual function void build_phase(uvm_phase phase);
    item = direct_object_base::type_id::create("item");
  endfunction

  virtual task run_phase(uvm_phase phase);
    ObjectOverrideTrace::kind = item.kind();
  endtask
endclass

module official_uvm_object_override_probe(output int stage);
  initial begin
    uvm_factory::get().set_type_override_by_type(
      direct_object_base::get_type(),
      direct_object_derived::get_type()
    );
    run_test("direct_object_env");
    stage = ObjectOverrideTrace::kind;
  end
endmodule
"#,
    )
    .expect("write official-UVM object override wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_object_override_probe"),
            &options,
        )
        .expect("directly lower a factory-overridden object creation");
    let mut simulator = Simulator::new(&design).expect("create factory object override simulator");
    simulator
        .run_until(0)
        .expect("run the factory-overridden object method");
    assert_eq!(simulator.read("stage").unwrap().to_u64(), Some(2));
}

#[test]
fn slang_direct_run_test_rejects_incompatible_factory_override() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-bad-factory-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_factory_base extends uvm_test;
  function new(string name = "direct_factory_base", uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

class direct_incompatible_factory extends uvm_test;
  function new(string name = "direct_incompatible_factory",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction

  virtual task run_phase(uvm_phase phase);
  endtask
endclass

module official_uvm_bad_factory_probe;
  initial begin
    uvm_factory::get().set_type_override_by_name(
      "direct_factory_base",
      "direct_incompatible_factory"
    );
    run_test("direct_factory_base");
  end
endmodule
"#,
    )
    .expect("write incompatible official-UVM factory wrapper");
    let error = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_bad_factory_probe"),
            &options,
        )
        .expect_err("incompatible UVM factory overrides must fail closed");
    assert!(
        error.contains("is not derived from 'direct_factory_base'"),
        "{error}"
    );
}

#[test]
fn slang_direct_run_test_tracks_objections_by_owner() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-owner-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_other_object extends uvm_test;
  function new(string name = "direct_other_object", uvm_component parent = null);
    super.new(name, parent);
  endfunction
endclass

class direct_owner_probe extends uvm_test;
  direct_other_object other;

  function new(string name = "direct_owner_probe", uvm_component parent = null);
    super.new(name, parent);
    other = new("direct_other_object", null);
  endfunction

  virtual task run_phase(uvm_phase phase);
    phase.raise_objection(other);
    phase.drop_objection(this);
  endtask
endclass

module official_uvm_owner_probe;
  initial begin
    run_test("direct_owner_probe");
  end
endmodule
"#,
    )
    .expect("write official-UVM owner wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_owner_probe"),
            &options,
        )
        .expect("directly lower a per-owner objection test");
    let mut simulator = Simulator::new(&design).expect("create per-owner objection simulator");
    let error = simulator
        .run_until(0)
        .expect_err("dropping another owner's objection must fail closed");
    assert!(
        error.contains("dropped an objection it did not own"),
        "{error}"
    );
}

#[test]
fn slang_direct_run_test_selects_objection_owners_by_runtime_identity() {
    let Some(slang) = SlangFrontend::discover_optional() else {
        return;
    };
    let Some((root, source)) = official_uvm_sources() else {
        return;
    };
    let options = PreprocessOptions {
        include_directories: vec![root.join("src")],
        defines: vec!["UVM_NO_DPI".to_string()],
        undefines: Vec::new(),
    };
    let wrapper = std::env::temp_dir().join(format!(
        "eda-official-uvm-runtime-owner-{}-{}.sv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(
        &wrapper,
        r#"
import uvm_pkg::*;

class direct_runtime_owner_object extends uvm_test;
  function new(string name = "direct_runtime_owner_object",
               uvm_component parent = null);
    super.new(name, parent);
  endfunction
endclass

class direct_runtime_owner_probe extends uvm_test;
  direct_runtime_owner_object first;
  direct_runtime_owner_object second;
  direct_runtime_owner_object selected;

  function new(string name = "direct_runtime_owner_probe",
               uvm_component parent = null);
    super.new(name, parent);
    first = new("first", null);
    second = new("second", null);
  endfunction

  virtual task run_phase(uvm_phase phase);
    selected = second;
    phase.raise_objection(selected);
    phase.drop_objection(second);
  endtask
endclass

module official_uvm_runtime_owner_probe(output int done);
  initial begin
    run_test("direct_runtime_owner_probe");
    done = 1;
  end
endmodule
"#,
    )
    .expect("write official-UVM runtime-owner wrapper");
    let design = slang
        .compile_files(
            &[source, wrapper],
            Some("official_uvm_runtime_owner_probe"),
            &options,
        )
        .expect("directly lower a runtime-identity objection test");
    let mut simulator =
        Simulator::new(&design).expect("create runtime-identity objection simulator");
    simulator
        .run_until(0)
        .expect("balanced runtime-selected UVM objections must drain");
    assert_eq!(simulator.read("done").unwrap().to_u64(), Some(1));
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
