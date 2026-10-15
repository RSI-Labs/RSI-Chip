//! Shared class/UVM simulation assertion helper.

use eda_sim::Simulator;

pub(in crate::hdl_cases::classes) fn expect_net(
    sim: &Simulator,
    name: &str,
    want: u64,
    what: &str,
) -> Result<(), String> {
    match sim.read(name).and_then(|w| w.to_u64()) {
        Some(got) if got == want => Ok(()),
        other => Err(format!("{what}: {name} = {other:?}, want {want}")),
    }
}
