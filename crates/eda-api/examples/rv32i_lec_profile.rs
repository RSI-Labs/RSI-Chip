use eda_core::library::CellLibrary;
use eda_synth::SynthOptions;

fn main() -> Result<(), String> {
    let core = std::fs::read_to_string("rv32i/rtl/rv32i_core.v").map_err(|e| e.to_string())?;
    let rtl = format!(
        "{core}\nmodule rv32i_top(input clk, input rst, input prog_ld, \
         input [511:0] prog, output halted, output [1:0] trap_cause, output [31:0] dbg_pc, \
         output [31:0] dbg_x10, output [31:0] dbg_x11, output [31:0] dbg_mepc, \
         output [31:0] dbg_mcause);\n\
         rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog), \
         .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10), \
         .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));\nendmodule\n"
    );
    let design = eda_api::compile(&rtl, Some("rv32i_top"))?;
    let gold = eda_synth::synthesize(
        &design,
        &SynthOptions {
            optimize: false,
            map_lib: false,
        },
    )?
    .netlist;
    let library = CellLibrary::std();
    let mapped =
        eda_synth::synthesize_with_library(&design, &SynthOptions::default(), &library)?.netlist;
    let result = eda_formal::check_equivalent(&gold, &mapped)?;
    println!("equivalent={}", result.equivalent);
    println!("counterexample={:?}", result.counterexample.is_some());
    for note in result.notes {
        println!("note={note}");
    }
    Ok(())
}
