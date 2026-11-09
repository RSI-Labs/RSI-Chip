//! End-to-end tests for the `sim` command (RV32I platform and i8086 COM).

use super::common::run_edu;

const MINIOS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../platform/os/rv32i/minios.bin"
);
const HELLO_COM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../platform/programs/hello.com"
);

#[test]
fn sim_runs_minios_and_the_user_asm_program() {
    let (ok, output) = run_edu(&["sim", MINIOS]);
    assert!(ok, "{output}");
    assert!(output.contains("MiniOS-RV32I: boot"), "{output}");
    assert!(
        output.contains("user-asm: hello from the RV32I user program"),
        "{output}"
    );
    assert!(output.contains("user-asm: sum(1..10) = 55"), "{output}");
    assert!(output.contains("sim rv32i: exit=Some(0)"), "{output}");
}

#[test]
fn sim_writes_a_vcd_waveform() {
    let path = std::env::temp_dir().join(format!("eda-sim-{}.vcd", std::process::id()));
    let path_string = path.to_str().expect("temp path").to_string();
    let (ok, output) = run_edu(&["sim", MINIOS, "--cycles", "20000", "--wave", &path_string]);
    assert!(ok, "{output}");
    let vcd = std::fs::read_to_string(&path).expect("VCD file");
    let _ = std::fs::remove_file(&path);
    assert!(vcd.contains("$enddefinitions"), "malformed VCD");
    assert!(vcd.contains("$var"), "VCD has no variables");
}

#[test]
fn sim_runs_i8086_com_programs() {
    let (ok, output) = run_edu(&["sim", HELLO_COM, "--cpu", "i8086"]);
    assert!(ok, "{output}");
    assert!(output.contains("Hello"), "{output}");
    assert!(output.contains("sim i8086: halted=true"), "{output}");
}

#[test]
fn sim_runs_a_generic_systemverilog_testbench() {
    let tb = super::common::TempFile::new(
        "sim-tb.sv",
        r#"module tb;
            initial begin
                $display("generic-sim-ok");
                #10 $display("time-ten");
                $finish;
            end
        endmodule"#,
    );
    let (ok, output) = run_edu(&["sim", tb.path(), "--top", "tb", "--cycles", "100"]);
    assert!(ok, "{output}");
    assert!(output.contains("generic-sim-ok"), "{output}");
    assert!(output.contains("time-ten"), "{output}");
    assert!(output.contains("sim rtl: top=tb finished=true"), "{output}");
}

#[test]
fn sim_runs_the_rv64i_conformance_image() {
    let image = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../platform/programs/rv64g/rv64i.bin"
    );
    let (ok, output) = run_edu(&["sim", image, "--cpu", "rv64i"]);
    assert!(ok, "{output}");
    assert!(output.contains("sim rv64i: exit=Some(0)"), "{output}");
    assert!(output.contains("trap=false"), "{output}");
}
