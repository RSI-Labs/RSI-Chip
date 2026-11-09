use eda_sim::Simulator;

fn main() {
    let source = include_str!("../../../platform/rtl/rv64i_full_core.sv");
    let design = eda_hdl::compile(source, Some("rv64i_full_core")).expect("compile");
    let sim = Simulator::new(&design).expect("sim");
    println!("{:?}", sim.zero_delay_feedback_nets());
}
