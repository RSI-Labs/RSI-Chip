//! SDC scenario, MMMC, violation, block timing, waiver, and command fixtures.

use super::*;

mod block_timing;
mod checks;
mod constraints;
mod multicorner;
mod violations;

pub(super) fn register(c: &mut CaseList) {
    constraints::register_constraints(c);
    multicorner::register_multicorner(c);
    violations::register_violations(c);
    block_timing::register_block_timing(c);
    checks::register_checks(c);
}

pub(super) fn sdc_netlist() -> Netlist {
    let src: &'static str = "module m(input a, b, output y); assign y = a & b; endmodule";
    let d = eda_hdl::compile(src, None).unwrap();
    eda_synth::synthesize(
        &d,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .unwrap()
    .netlist
}

pub(super) fn through_chain_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("through_chain");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let n0 = nl.add_net("n0");
    let n1 = nl.add_net("n1");
    let y = nl.add_net("y");
    for (name, input, output) in [("u0", a, n0), ("u1", n0, n1), ("u2", n1, y)] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl
}

pub(super) fn hierarchical_through_chain_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("hier_through_chain");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let n0 = nl.add_net("n0");
    let n1 = nl.add_net("n1");
    let y = nl.add_net("y");
    for (name, input, output) in [("top/u0", a, n0), ("top/u1", n0, n1), ("top/u2", n1, y)] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl
}

pub(super) fn incremental_branch_netlist() -> Netlist {
    let lib = Arc::new(CellLibrary::std());
    let mut nl = Netlist::new("incremental_branches");
    nl.set_library(lib.clone());
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let a_n0 = nl.add_net("a_n0");
    let a_n1 = nl.add_net("a_n1");
    let b_n0 = nl.add_net("b_n0");
    let b_n1 = nl.add_net("b_n1");
    let y = nl.add_net("y");
    let z = nl.add_net("z");
    for (name, input, output) in [
        ("a_u0", a, a_n0),
        ("a_u1", a_n0, a_n1),
        ("a_u2", a_n1, y),
        ("b_u0", b, b_n0),
        ("b_u1", b_n0, b_n1),
        ("b_u2", b_n1, z),
    ] {
        nl.add_cell(Cell {
            name: name.into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![input],
            output,
        });
    }
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("b", Dir::In, vec![b]);
    nl.add_port("y", Dir::Out, vec![y]);
    nl.add_port("z", Dir::Out, vec![z]);
    nl
}
