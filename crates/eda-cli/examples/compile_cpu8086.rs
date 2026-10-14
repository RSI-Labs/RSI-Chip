fn main() {
    let mut src = String::new();
    for f in [
        "primitives",
        "ripple_adder16",
        "alu16",
        "regfile16",
        "cpu8086",
        "ram64x16",
        "sys8086",
    ] {
        src.push_str(&std::fs::read_to_string(format!("cpu8086/rtl/{f}.v")).unwrap());
        src.push('\n');
    }
    match eda_api::compile(&src, Some("sys8086")) {
        Ok(d) => println!(
            "OK: nets={} ports={} assigns={} processes={}",
            d.n_nets,
            d.ports.len(),
            d.assigns.len(),
            d.processes.len()
        ),
        Err(e) => {
            println!("ERR: {e}");
            std::process::exit(1);
        }
    }
}
