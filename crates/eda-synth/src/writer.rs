//! Structural Verilog netlist writer. Library cells become instances;
//! generic primitives become Verilog gate primitives / assigns.

use eda_core::library::CellLibrary;
use eda_core::logic::LogicWord;
use eda_core::netlist::{CellKind, Dir, NetId, Netlist};
use std::collections::HashMap;

struct BlockRomGroup<'a> {
    name: String,
    address_width: usize,
    contents: &'a [LogicWord],
}

type BlockRomKey = (usize, Vec<NetId>, usize);

fn block_rom_key(address_width: usize, contents: &[LogicWord], inputs: &[NetId]) -> BlockRomKey {
    (contents.as_ptr() as usize, inputs.to_vec(), address_width)
}

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
    let mut block_rom_group_ids = HashMap::<BlockRomKey, usize>::new();
    let mut block_rom_groups = Vec::<BlockRomGroup>::new();
    for c in nl.cells() {
        if let CellKind::BlockRom {
            address_width,
            contents,
            ..
        } = &c.kind
        {
            let key = block_rom_key(*address_width, contents, &c.inputs);
            if let std::collections::hash_map::Entry::Vacant(entry) = block_rom_group_ids.entry(key)
            {
                let index = block_rom_groups.len();
                entry.insert(index);
                block_rom_groups.push(BlockRomGroup {
                    name: format!("block_rom_{index}"),
                    address_width: *address_width,
                    contents,
                });
            }
        }
    }
    let mut block_rom_models = String::new();
    for group in &block_rom_groups {
        let Some(first) = group.contents.first() else {
            continue;
        };
        let element_width = first.width();
        block_rom_models.push_str(&format!(
            "  reg [{}:0] {} [0:{}];\n",
            element_width.saturating_sub(1),
            group.name,
            group.contents.len() - 1
        ));
        block_rom_models.push_str("  initial begin\n");
        for (address, word) in group.contents.iter().enumerate() {
            block_rom_models.push_str(&format!(
                "    {}[{}] = {}'b{};\n",
                group.name,
                address,
                element_width,
                word.to_bin_str()
            ));
        }
        block_rom_models.push_str("  end\n");
    }
    s.push_str(&format!("module {}(", nl.name));
    let port_names: Vec<String> = nl.ports.iter().map(|p| p.name.clone()).collect();
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
    s.push_str(&block_rom_models);
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
    let port_net_names = nl
        .ports
        .iter()
        .flat_map(|port| {
            let width = port.bits.len();
            port.bits.iter().enumerate().map(move |(msb, net)| {
                let logical_bit = width - 1 - msb;
                (
                    *net,
                    if width == 1 {
                        port.name.clone()
                    } else {
                        format!("{}[{}]", port.name, logical_bit)
                    },
                )
            })
        })
        .collect::<HashMap<_, _>>();
    for (ci, c) in nl.cells().iter().enumerate() {
        let netname = |n: usize| {
            port_net_names
                .get(&n)
                .cloned()
                .unwrap_or_else(|| nl.nets()[n].name.clone())
        };
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
            CellKind::BlockRom {
                address_width,
                bit_index,
                contents,
            } => {
                let key = block_rom_key(*address_width, contents, &c.inputs);
                let valid = block_rom_group_ids
                    .get(&key)
                    .and_then(|index| block_rom_groups.get(*index))
                    .is_some_and(|group| {
                        c.inputs.len() == group.address_width
                            && group
                                .contents
                                .first()
                                .is_some_and(|word| *bit_index < word.width())
                    });
                if valid {
                    let address_bits = (0..*address_width)
                        .rev()
                        .map(|bit| netname(c.inputs[bit]))
                        .collect::<Vec<_>>();
                    let address = if address_bits.len() == 1 {
                        address_bits[0].clone()
                    } else {
                        format!("{{{}}}", address_bits.join(", "))
                    };
                    s.push_str(&format!(
                        "  assign {} = block_rom_{}[{}][{}:{}];\n",
                        netname(c.output),
                        block_rom_group_ids[&key],
                        address,
                        bit_index,
                        bit_index
                    ));
                } else {
                    s.push_str(&format!(
                        "  assign {} = 1'bx; // invalid BlockRom cell\n",
                        netname(c.output)
                    ));
                }
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

    #[test]
    fn writes_one_behavioral_model_per_block_rom_macro() {
        let lib = CellLibrary::std();
        let contents = vec![
            LogicWord::from_u64(2, 0b01),
            LogicWord::from_u64(2, 0b10),
            LogicWord::from_u64(2, 0b11),
        ];
        let contents = std::sync::Arc::new(contents);
        let mut nl = Netlist::new("rom");
        let address0 = nl.add_net("address0");
        let address1 = nl.add_net("address1");
        for bit_index in 0..2 {
            let output = nl.add_net(format!("q{bit_index}"));
            nl.add_cell(eda_core::netlist::Cell {
                name: format!("rom_bit{bit_index}"),
                kind: CellKind::BlockRom {
                    address_width: 2,
                    bit_index,
                    contents: contents.clone(),
                },
                inputs: vec![address0, address1],
                output,
            });
        }
        nl.add_port("address", Dir::In, vec![address1, address0]);
        nl.add_port("q0", Dir::Out, vec![nl.net_by_name("q0").unwrap()]);
        nl.add_port("q1", Dir::Out, vec![nl.net_by_name("q1").unwrap()]);

        let text = write_verilog(&nl, &lib);
        assert_eq!(
            text.matches("  reg [1:0] block_rom_0 [0:2];\n").count(),
            1,
            "{text}"
        );
        assert!(text.contains("assign q0 = block_rom_0[{address[1], address[0]}][0:0];"));
        assert!(text.contains("assign q1 = block_rom_0[{address[1], address[0]}][1:1];"));
        assert!(!text.contains("block_rom_1"));

        let reparsed = eda_hdl::compile(&text, Some("rom")).expect("reparse BlockRom model");
        let mut simulator = eda_sim::Simulator::new(&reparsed).unwrap();
        for (address, expected) in [(0_u64, 0b01), (1, 0b10), (2, 0b11)] {
            simulator
                .set_input("address", &LogicWord::from_u64(2, address))
                .unwrap();
            assert_eq!(simulator.read("q0").unwrap().to_u64(), Some(expected & 1));
            assert_eq!(
                simulator.read("q1").unwrap().to_u64(),
                Some((expected >> 1) & 1)
            );
        }
    }
}
