//! Structural Verilog netlist writer. Library cells become instances;
//! generic primitives become Verilog gate primitives / assigns.

use eda_core::library::CellLibrary;
use eda_core::netlist::{CellKind, Dir, Netlist};

/// Emit one behavioral model module per library cell used by the netlist so
/// the output can be re-parsed by our own frontend.
fn lib_models(nl: &Netlist, lib: &CellLibrary) -> String {
    let mut used: Vec<usize> = Vec::new();
    for c in nl.cells() {
        if let CellKind::Lib(id) = &c.kind {
            if !used.contains(id) {
                used.push(*id);
            }
        }
    }
    let mut s = String::new();
    for id in used {
        let def = lib.get(id);
        let pins: Vec<String> = def
            .inputs
            .iter()
            .map(|p| format!("input {}", p))
            .chain(std::iter::once(format!("output {}", def.output)))
            .collect();
        s.push_str(&format!("module {}({});\n", def.name, pins_joined(def)));
        s.push_str(&format!("  {};\n", pins.join("; ")));
        if let Some(seq) = &def.seq {
            let clk = def.inputs[seq.clk_pin].clone();
            let d = def.inputs[1].clone();
            let q = def.output.clone();
            match seq.reset_pin {
                Some(rp) => {
                    let rst = def.inputs[rp].clone();
                    s.push_str(&format!(
                        "  always @(posedge {} or posedge {}) if ({}) {} <= 1'b0; else {} <= {};\n",
                        clk, rst, rst, q, q, d
                    ));
                }
                None => {
                    s.push_str(&format!("  always @(posedge {}) {} <= {};\n", clk, q, d));
                }
            }
        } else {
            let body = func_to_expr(&def.func, def);
            s.push_str(&format!("  assign {} = {};\n", def.output, body));
        }
        s.push_str("endmodule\n");
    }
    s
}

fn pins_joined(def: &eda_core::library::LibCellDef) -> String {
    let mut names: Vec<String> = def.inputs.clone();
    names.push(def.output.clone());
    names.join(", ")
}

fn func_to_expr(f: &eda_core::library::LibFunc, def: &eda_core::library::LibCellDef) -> String {
    match f {
        eda_core::library::LibFunc::Const(v) => format!("1'b{}", v.to_char()),
        eda_core::library::LibFunc::Id(i) => def.inputs[*i].clone(),
        eda_core::library::LibFunc::Not(e) => format!("~({})", func_to_expr(e, def)),
        eda_core::library::LibFunc::And(es) => paren_join(es, "&", def),
        eda_core::library::LibFunc::Nand(es) => format!("~({})", paren_join(es, "&", def)),
        eda_core::library::LibFunc::Or(es) => paren_join(es, "|", def),
        eda_core::library::LibFunc::Nor(es) => format!("~({})", paren_join(es, "|", def)),
        eda_core::library::LibFunc::Xor(es) => paren_join(es, "^", def),
        eda_core::library::LibFunc::Xnor(es) => format!("~({})", paren_join(es, "^", def)),
        eda_core::library::LibFunc::Mux { sel, a, b } => format!(
            "{} ? ({}) : ({})",
            func_to_expr(sel, def),
            func_to_expr(b, def),
            func_to_expr(a, def)
        ),
    }
}

fn paren_join(
    es: &[eda_core::library::LibFunc],
    op: &str,
    def: &eda_core::library::LibCellDef,
) -> String {
    es.iter()
        .map(|e| format!("({})", func_to_expr(e, def)))
        .collect::<Vec<_>>()
        .join(&format!(" {} ", op))
}

pub fn write_verilog(nl: &Netlist, lib: &CellLibrary) -> String {
    let mut s = String::new();
    s.push_str(&lib_models(nl, lib));
    s.push_str(&format!("module {}(", nl.name));
    let port_names: Vec<String> = nl
        .ports
        .iter()
        .map(|p| {
            if p.bits.len() > 1 {
                format!("{}[{}:0]", p.name, p.bits.len() - 1)
            } else {
                p.name.clone()
            }
        })
        .collect();
    s.push_str(&port_names.join(", "));
    s.push_str(");\n");
    for p in &nl.ports {
        let dir = if p.dir == Dir::In { "input" } else { "output" };
        if p.bits.len() > 1 {
            s.push_str(&format!("  {} [{}:0] {};\n", dir, p.bits.len() - 1, p.name));
        } else {
            s.push_str(&format!("  {} {};\n", dir, p.name));
        }
    }
    // internal wires
    let port_nets: Vec<usize> = nl
        .ports
        .iter()
        .flat_map(|p| p.bits.iter().copied())
        .collect();
    let mut wires: Vec<&str> = Vec::new();
    for (i, n) in nl.nets().iter().enumerate() {
        if !port_nets.contains(&i) && !n.drivers.is_empty() {
            wires.push(&n.name);
        }
    }
    if !wires.is_empty() {
        s.push_str(&format!("  wire {};\n", wires.join(", ")));
    }
    for (ci, c) in nl.cells().iter().enumerate() {
        let netname = |n: usize| nl.nets()[n].name.clone();
        match &c.kind {
            CellKind::Lib(lid) => {
                let def = lib.get(*lid);
                let conns: Vec<String> = def
                    .inputs
                    .iter()
                    .zip(c.inputs.iter())
                    .map(|(pin, n)| format!(".{}({})", pin, netname(*n)))
                    .chain(std::iter::once(format!(
                        ".{}({})",
                        def.output,
                        netname(c.output)
                    )))
                    .collect();
                s.push_str(&format!(
                    "  {} {} ({});\n",
                    def.name,
                    c.name,
                    conns.join(", ")
                ));
            }
            CellKind::Const(v) => {
                let bit = v.to_char();
                s.push_str(&format!("  assign {} = 1'b{};\n", netname(c.output), bit));
            }
            CellKind::Not => {
                s.push_str(&format!(
                    "  not {} ({}, {});\n",
                    c.name,
                    netname(c.output),
                    netname(c.inputs[0])
                ));
            }
            CellKind::Buf => {
                s.push_str(&format!(
                    "  buf {} ({}, {});\n",
                    c.name,
                    netname(c.output),
                    netname(c.inputs[0])
                ));
            }
            CellKind::And
            | CellKind::Nand
            | CellKind::Or
            | CellKind::Nor
            | CellKind::Xor
            | CellKind::Xnor => {
                let prim = match c.kind {
                    CellKind::And => "and",
                    CellKind::Nand => "nand",
                    CellKind::Or => "or",
                    CellKind::Nor => "nor",
                    CellKind::Xor => "xor",
                    _ => "xnor",
                };
                let ins: Vec<String> = c.inputs.iter().map(|n| netname(*n)).collect();
                s.push_str(&format!(
                    "  {} {} ({}, {});\n",
                    prim,
                    c.name,
                    netname(c.output),
                    ins.join(", ")
                ));
            }
            CellKind::Mux => {
                // no primitive: behavioral
                s.push_str(&format!(
                    "  assign {} = {} ? {} : {};\n",
                    netname(c.output),
                    netname(c.inputs[0]),
                    netname(c.inputs[2]),
                    netname(c.inputs[1])
                ));
            }
            CellKind::Dff { posedge } => {
                let edge = if *posedge { "posedge" } else { "negedge" };
                s.push_str(&format!(
                    "  always @({} {}) {} <= {};\n",
                    edge,
                    netname(c.inputs[0]),
                    netname(c.output),
                    netname(c.inputs[1])
                ));
            }
            CellKind::DffR {
                posedge,
                reset_high,
            } => {
                let edge = if *posedge { "posedge" } else { "negedge" };
                let cond = if *reset_high {
                    netname(c.inputs[2])
                } else {
                    format!("!{}", netname(c.inputs[2]))
                };
                s.push_str(&format!(
                    "  always @({} {} or {} {}) if ({}) {} <= 1'b0; else {} <= {};\n",
                    edge,
                    netname(c.inputs[0]),
                    if *reset_high { "posedge" } else { "negedge" },
                    netname(c.inputs[2]),
                    cond,
                    netname(c.output),
                    netname(c.output),
                    netname(c.inputs[1])
                ));
            }
        }
        let _ = ci;
    }
    s.push_str("endmodule\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_mapped_netlist() {
        let lib = CellLibrary::std();
        let mut nl = Netlist::new("m");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(eda_core::netlist::Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("NAND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let text = write_verilog(&nl, &lib);
        assert!(text.contains("module m(a, b, y);"));
        assert!(text.contains("NAND2 u1 (.a(a), .b(b), .y(y));"));
    }
}
