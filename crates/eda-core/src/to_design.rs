//! Lower a gate-level [`Netlist`] back into a RTL [`Design`] so the same
//! event-driven simulator can run gate-level netlists.

use crate::ir;
use crate::library::LibFunc;
use crate::logic::LogicWord;
use crate::netlist::{self, CellKind, Netlist};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

type BlockRomKey = (usize, Vec<netlist::NetId>, usize);

struct ReconstructedBlockRom {
    address_base: usize,
    address_width: usize,
    element_width: usize,
    memory: usize,
}

struct ReconstructedBlockRam {
    memory: usize,
    element_width: usize,
}

struct ReconstructedBlockRamReadPort {
    address_base: usize,
    address_width: usize,
}

pub fn netlist_to_design(nl: &Netlist) -> ir::Design {
    let mut d = ir::Design {
        name: nl.name.clone(),
        ..Default::default()
    };
    // One 1-bit declaration per net, name preserved for probing.
    let mut net_base: Vec<usize> = Vec::with_capacity(nl.net_count());
    for n in nl.nets() {
        let base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: n.name.clone(),
            width: 1,
            is_reg: false,
            is_signed: false,
            base,
        });
        net_base.push(base);
        d.n_nets += 1;
    }
    let bit = |i: usize| ir::Expr::Ref {
        net: net_base[i],
        decl_width: 1,
        range: ir::Range::Bit(0),
        signed: false,
    };

    // Ports: fresh contiguous vector declarations, connected by assigns.
    for p in &nl.ports {
        let base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: p.name.clone(),
            width: p.bits.len(),
            is_reg: false,
            is_signed: false,
            base,
        });
        d.n_nets += p.bits.len();
        d.ports.push(ir::PortDecl {
            name: p.name.clone(),
            dir: match p.dir {
                netlist::Dir::In => ir::Dir::In,
                netlist::Dir::Out => ir::Dir::Out,
            },
            base,
            width: p.bits.len(),
        });
        // Port bits are stored MSB-first; port-declaration bit k (LSB-first)
        // corresponds to p.bits[len-1-k].
        let w = p.bits.len();
        for k in 0..w {
            let n = p.bits[w - 1 - k];
            let port_bit = ir::Expr::Ref {
                net: base + k,
                decl_width: w,
                range: ir::Range::Bit(0),
                signed: false,
            };
            match p.dir {
                netlist::Dir::In => d.assigns.push(ir::Assign {
                    lv: ir::LValue {
                        net: net_base[n],
                        decl_width: 1,
                        range: ir::Range::Bit(0),
                    },
                    e: port_bit,
                    delay: 0,
                }),
                netlist::Dir::Out => d.assigns.push(ir::Assign {
                    lv: ir::LValue {
                        net: base + k,
                        decl_width: w,
                        range: ir::Range::Bit(0),
                    },
                    e: bit(n),
                    delay: 0,
                }),
            }
        }
    }

    // Reconstruct one sparse ROM per shared macro payload/address port rather
    // than duplicating its contents once per output-bit cell.
    let mut block_rom_group_ids = HashMap::<BlockRomKey, usize>::new();
    let mut block_rom_groups = Vec::<ReconstructedBlockRom>::new();
    for c in nl.cells() {
        if let CellKind::BlockRom {
            address_width,
            contents,
            ..
        } = &c.kind
        {
            let key = (
                Arc::as_ptr(contents) as *const _ as usize,
                c.inputs.clone(),
                *address_width,
            );
            if block_rom_group_ids.contains_key(&key) {
                continue;
            }
            let Some(first) = contents.first() else {
                continue;
            };
            let element_width = first.width();
            if element_width == 0
                || c.inputs.len() != *address_width
                || !contents.iter().all(|word| word.width() == element_width)
            {
                continue;
            }

            let group_index = block_rom_groups.len();
            block_rom_group_ids.insert(key, group_index);
            let address_base = d.n_nets;
            d.net_decls.push(ir::NetDecl {
                name: format!("block_rom_{group_index}_address"),
                width: *address_width,
                is_reg: false,
                is_signed: false,
                base: address_base,
            });
            d.n_nets += address_width;
            for (address_bit, source) in c.inputs.iter().enumerate() {
                d.assigns.push(ir::Assign {
                    lv: ir::LValue {
                        net: address_base + address_bit,
                        decl_width: 1,
                        range: ir::Range::Bit(0),
                    },
                    e: bit(*source),
                    delay: 0,
                });
            }

            let memory = d.memories.len();
            d.memories.push(ir::MemoryDecl {
                name: format!("block_rom_{group_index}_rom"),
                element_width,
                element_msb: element_width as i64 - 1,
                element_lsb: 0,
                dimensions: vec![ir::MemoryDimension {
                    base: 0,
                    step: 1,
                    length: contents.len(),
                }],
                flat_length: contents.len() as u64,
                dynamic: false,
                queue: false,
                queue_bound: None,
                associative: false,
                index_width: 0,
                index_signed: false,
                storage: ir::MemoryStorage::Sparse,
                kind: ir::MemoryKind::Rom,
                template: ir::MemoryTemplate::AsyncRom,
                write_mask: None,
                byte_enable: None,
                banking: ir::MemoryBanking {
                    count: 1,
                    depth: contents.len() as u64,
                },
            });
            let initialization = contents
                .iter()
                .enumerate()
                .map(|(address, word)| ir::Stmt::MemoryBl {
                    memory,
                    indexes: vec![ir::Expr::Const(LogicWord::from_u64(
                        32,
                        u64::try_from(address).unwrap_or(u64::MAX),
                    ))],
                    e: ir::Expr::Const(word.clone()),
                })
                .collect();
            d.initials.push(ir::Stmt::Block(initialization));
            block_rom_groups.push(ReconstructedBlockRom {
                address_base,
                address_width: *address_width,
                element_width,
                memory,
            });
        }
    }

    // Group the per-bit write cells emitted by synthesis into one sparse RAM
    // object and one clocked write process.
    let mut block_ram_write_groups = BTreeMap::<usize, Vec<&netlist::Cell>>::new();
    for c in nl.cells() {
        if let CellKind::BlockRamWrite { macro_id, .. } = &c.kind {
            block_ram_write_groups.entry(*macro_id).or_default().push(c);
        }
    }
    let mut block_ram_groups = HashMap::<usize, ReconstructedBlockRam>::new();
    for (macro_id, cells) in block_ram_write_groups {
        let Some(first_cell) = cells.first() else {
            continue;
        };
        let CellKind::BlockRamWrite {
            address_width,
            initial,
            posedge,
            ..
        } = &first_cell.kind
        else {
            continue;
        };
        let Some(first_word) = initial.first() else {
            continue;
        };
        let element_width = first_word.width();
        if element_width == 0
            || cells.len() != element_width
            || !initial.iter().all(|word| word.width() == element_width)
        {
            continue;
        }

        let mut bits = vec![None; element_width];
        let mut valid = true;
        for cell in &cells {
            let CellKind::BlockRamWrite {
                address_width: cell_address_width,
                bit_index,
                initial: cell_initial,
                posedge: cell_posedge,
                ..
            } = &cell.kind
            else {
                valid = false;
                break;
            };
            let expected_prefix = 2 + address_width;
            if cell_address_width != address_width
                || cell_initial.as_ptr() != initial.as_ptr()
                || cell_posedge != posedge
                || cell.inputs.len() != expected_prefix + 1
                || cell.inputs[..expected_prefix] != first_cell.inputs[..expected_prefix]
                || *bit_index >= element_width
                || bits[*bit_index].is_some()
            {
                valid = false;
                break;
            }
            bits[*bit_index] = Some((cell.inputs[expected_prefix], cell.inputs[0], cell.inputs[1]));
        }
        if !valid {
            continue;
        }
        // All cells were validated to have the same control/address prefix.
        let clock_source = bits[0].expect("validated write bit").1;
        let enable_source = bits[0].expect("validated write bit").2;

        let clock_base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: format!("block_ram_{macro_id}_clock"),
            width: 1,
            is_reg: false,
            is_signed: false,
            base: clock_base,
        });
        d.n_nets += 1;
        d.assigns.push(ir::Assign {
            lv: ir::LValue {
                net: clock_base,
                decl_width: 1,
                range: ir::Range::Bit(0),
            },
            e: bit(clock_source),
            delay: 0,
        });

        let enable_base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: format!("block_ram_{macro_id}_write_enable"),
            width: 1,
            is_reg: false,
            is_signed: false,
            base: enable_base,
        });
        d.n_nets += 1;
        d.assigns.push(ir::Assign {
            lv: ir::LValue {
                net: enable_base,
                decl_width: 1,
                range: ir::Range::Bit(0),
            },
            e: bit(enable_source),
            delay: 0,
        });

        let write_address_base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: format!("block_ram_{macro_id}_write_address"),
            width: *address_width,
            is_reg: false,
            is_signed: false,
            base: write_address_base,
        });
        d.n_nets += address_width;
        for (address_bit, source) in first_cell.inputs[2..2 + address_width].iter().enumerate() {
            d.assigns.push(ir::Assign {
                lv: ir::LValue {
                    net: write_address_base + address_bit,
                    decl_width: 1,
                    range: ir::Range::Bit(0),
                },
                e: bit(*source),
                delay: 0,
            });
        }

        let write_data_base = d.n_nets;
        d.net_decls.push(ir::NetDecl {
            name: format!("block_ram_{macro_id}_write_data"),
            width: element_width,
            is_reg: false,
            is_signed: false,
            base: write_data_base,
        });
        d.n_nets += element_width;
        for (bit_index, source) in bits
            .iter()
            .map(|bit| bit.expect("validated write bit").0)
            .enumerate()
        {
            d.assigns.push(ir::Assign {
                lv: ir::LValue {
                    net: write_data_base + bit_index,
                    decl_width: 1,
                    range: ir::Range::Bit(0),
                },
                e: bit(source),
                delay: 0,
            });
        }

        let memory = d.memories.len();
        d.memories.push(ir::MemoryDecl {
            name: format!("block_ram_{macro_id}_ram"),
            element_width,
            element_msb: element_width as i64 - 1,
            element_lsb: 0,
            dimensions: vec![ir::MemoryDimension {
                base: 0,
                step: 1,
                length: initial.len(),
            }],
            flat_length: initial.len() as u64,
            dynamic: false,
            queue: false,
            queue_bound: None,
            associative: false,
            index_width: 0,
            index_signed: false,
            storage: ir::MemoryStorage::Sparse,
            kind: ir::MemoryKind::Ram,
            template: ir::MemoryTemplate::SyncRam,
            write_mask: Some(ir::MemoryWriteMask::full(element_width)),
            byte_enable: None,
            banking: ir::MemoryBanking {
                count: 1,
                depth: initial.len() as u64,
            },
        });
        let initialization = initial
            .iter()
            .enumerate()
            .map(|(address, word)| ir::Stmt::MemoryBl {
                memory,
                indexes: vec![ir::Expr::Const(LogicWord::from_u64(
                    32,
                    u64::try_from(address).unwrap_or(u64::MAX),
                ))],
                e: ir::Expr::Const(word.clone()),
            })
            .collect();
        d.initials.push(ir::Stmt::Block(initialization));

        let write = ir::Stmt::MemoryNBl {
            memory,
            indexes: vec![ir::Expr::Ref {
                net: write_address_base,
                decl_width: *address_width,
                range: if *address_width == 1 {
                    ir::Range::Bit(0)
                } else {
                    ir::Range::Span {
                        msb: *address_width - 1,
                        lsb: 0,
                    }
                },
                signed: false,
            }],
            e: ir::Expr::Ref {
                net: write_data_base,
                decl_width: element_width,
                range: if element_width == 1 {
                    ir::Range::Bit(0)
                } else {
                    ir::Range::Span {
                        msb: element_width - 1,
                        lsb: 0,
                    }
                },
                signed: false,
            },
        };
        d.processes.push(ir::Process {
            sens: edge_sens(*posedge, clock_base),
            body: ir::Stmt::If {
                c: ir::Expr::Ref {
                    net: enable_base,
                    decl_width: 1,
                    range: ir::Range::Bit(0),
                    signed: false,
                },
                t: Box::new(write),
                f: None,
            },
        });
        block_ram_groups.insert(
            macro_id,
            ReconstructedBlockRam {
                memory,
                element_width,
            },
        );
    }

    // Reconstruct one address vector per distinct read port, shared by every
    // output-bit cell on that port.
    let mut block_ram_read_ports =
        HashMap::<(usize, Vec<netlist::NetId>), ReconstructedBlockRamReadPort>::new();
    for c in nl.cells() {
        if let CellKind::BlockRamRead {
            macro_id,
            address_width,
            ..
        } = &c.kind
        {
            if !block_ram_groups.contains_key(macro_id) || c.inputs.len() != *address_width {
                continue;
            }
            let key = (*macro_id, c.inputs.clone());
            if block_ram_read_ports.contains_key(&key) {
                continue;
            }
            let address_base = d.n_nets;
            d.net_decls.push(ir::NetDecl {
                name: format!(
                    "block_ram_{macro_id}_read_address_{}",
                    block_ram_read_ports.len()
                ),
                width: *address_width,
                is_reg: false,
                is_signed: false,
                base: address_base,
            });
            d.n_nets += address_width;
            for (address_bit, source) in c.inputs.iter().enumerate() {
                d.assigns.push(ir::Assign {
                    lv: ir::LValue {
                        net: address_base + address_bit,
                        decl_width: 1,
                        range: ir::Range::Bit(0),
                    },
                    e: bit(*source),
                    delay: 0,
                });
            }
            block_ram_read_ports.insert(
                key,
                ReconstructedBlockRamReadPort {
                    address_base,
                    address_width: *address_width,
                },
            );
        }
    }

    // Cells: combinational become continuous assigns, flops become clocked
    // processes.
    for c in nl.cells() {
        let out = c.output;
        let lv_out = ir::LValue {
            net: net_base[out],
            decl_width: 1,
            range: ir::Range::Bit(0),
        };
        let in_ref = |k: usize| bit(c.inputs[k]);
        match &c.kind {
            CellKind::Const(v) => d.assigns.push(ir::Assign {
                lv: lv_out,
                e: ir::Expr::Const(LogicWord::new_const(1, *v)),
                delay: 0,
            }),
            CellKind::Not => d.assigns.push(ir::Assign {
                lv: lv_out,
                e: ir::Expr::Un {
                    op: ir::UnOp::Not,
                    a: Box::new(in_ref(0)),
                },
                delay: 0,
            }),
            CellKind::Buf => d.assigns.push(ir::Assign {
                lv: lv_out,
                e: in_ref(0),
                delay: 0,
            }),
            CellKind::And | CellKind::Or | CellKind::Xor => {
                let op = match c.kind {
                    CellKind::And => ir::BinOp::And,
                    CellKind::Or => ir::BinOp::Or,
                    _ => ir::BinOp::Xor,
                };
                let e = fold_exprs((0..c.inputs.len()).map(in_ref), op, LogicWord::ones(1));
                d.assigns.push(ir::Assign {
                    lv: lv_out,
                    e,
                    delay: 0,
                });
            }
            CellKind::Nand | CellKind::Nor | CellKind::Xnor => {
                let op = match c.kind {
                    CellKind::Nand => ir::BinOp::And,
                    CellKind::Nor => ir::BinOp::Or,
                    _ => ir::BinOp::Xor,
                };
                let inner = fold_exprs((0..c.inputs.len()).map(in_ref), op, LogicWord::ones(1));
                d.assigns.push(ir::Assign {
                    lv: lv_out,
                    e: ir::Expr::Un {
                        op: ir::UnOp::Not,
                        a: Box::new(inner),
                    },
                    delay: 0,
                });
            }
            CellKind::Mux => d.assigns.push(ir::Assign {
                lv: lv_out,
                e: ir::Expr::Cond {
                    c: Box::new(in_ref(0)),
                    t: Box::new(in_ref(2)),
                    f: Box::new(in_ref(1)),
                },
                delay: 0,
            }),
            CellKind::Dff { posedge } => {
                d.processes.push(ir::Process {
                    sens: edge_sens(*posedge, net_base[c.inputs[0]]),
                    body: ir::Stmt::NBl {
                        lv: lv_out,
                        e: in_ref(1),
                    },
                });
            }
            CellKind::DffR {
                posedge,
                reset_high,
            } => {
                d.processes.push(ir::Process {
                    sens: ir::Sens::List(vec![
                        edge_sens(*posedge, net_base[c.inputs[0]]),
                        edge_sens(*reset_high, net_base[c.inputs[2]]),
                    ]),
                    body: ir::Stmt::If {
                        c: if *reset_high {
                            in_ref(2)
                        } else {
                            ir::Expr::Un {
                                op: ir::UnOp::Not,
                                a: Box::new(in_ref(2)),
                            }
                        },
                        t: Box::new(ir::Stmt::NBl {
                            lv: lv_out.clone(),
                            e: ir::Expr::Const(LogicWord::zero(1)),
                        }),
                        f: Some(Box::new(ir::Stmt::NBl {
                            lv: lv_out.clone(),
                            e: in_ref(1),
                        })),
                    },
                });
            }
            CellKind::BlockRom {
                address_width,
                bit_index,
                contents,
            } => {
                let key = (
                    Arc::as_ptr(contents) as *const _ as usize,
                    c.inputs.clone(),
                    *address_width,
                );
                let group = block_rom_group_ids
                    .get(&key)
                    .and_then(|index| block_rom_groups.get(*index));
                if let Some(group) = group {
                    let read = ir::Expr::MemoryRead {
                        memory: group.memory,
                        indexes: vec![ir::Expr::Ref {
                            net: group.address_base,
                            decl_width: group.address_width,
                            range: if group.address_width == 1 {
                                ir::Range::Bit(0)
                            } else {
                                ir::Range::Span {
                                    msb: group.address_width - 1,
                                    lsb: 0,
                                }
                            },
                            signed: false,
                        }],
                        width: group.element_width,
                        signed: false,
                    };
                    d.assigns.push(ir::Assign {
                        lv: lv_out,
                        e: ir::Expr::Index {
                            e: Box::new(read),
                            msb: *bit_index,
                            lsb: *bit_index,
                        },
                        delay: 0,
                    });
                } else {
                    d.assigns.push(ir::Assign {
                        lv: lv_out,
                        e: ir::Expr::Const(LogicWord::xes(1)),
                        delay: 0,
                    });
                }
            }
            CellKind::BlockRamWrite { .. } => {
                // Reconstructed above as part of one sparse-memory write port.
            }
            CellKind::BlockRamRead {
                macro_id,
                address_width,
                bit_index,
                ..
            } => {
                let key = (*macro_id, c.inputs.clone());
                let group = block_ram_groups.get(macro_id);
                let port = block_ram_read_ports.get(&key);
                if let (Some(group), Some(port)) = (group, port) {
                    if port.address_width != *address_width || *bit_index >= group.element_width {
                        d.assigns.push(ir::Assign {
                            lv: lv_out,
                            e: ir::Expr::Const(LogicWord::xes(1)),
                            delay: 0,
                        });
                        continue;
                    }
                    let read = ir::Expr::MemoryRead {
                        memory: group.memory,
                        indexes: vec![ir::Expr::Ref {
                            net: port.address_base,
                            decl_width: port.address_width,
                            range: if port.address_width == 1 {
                                ir::Range::Bit(0)
                            } else {
                                ir::Range::Span {
                                    msb: port.address_width - 1,
                                    lsb: 0,
                                }
                            },
                            signed: false,
                        }],
                        width: group.element_width,
                        signed: false,
                    };
                    d.assigns.push(ir::Assign {
                        lv: lv_out,
                        e: ir::Expr::Index {
                            e: Box::new(read),
                            msb: *bit_index,
                            lsb: *bit_index,
                        },
                        delay: 0,
                    });
                } else {
                    d.assigns.push(ir::Assign {
                        lv: lv_out,
                        e: ir::Expr::Const(LogicWord::xes(1)),
                        delay: 0,
                    });
                }
            }
            CellKind::Lib(lid) => {
                if let Some(lib) = &nl.lib_cache {
                    let def = lib.get(*lid);
                    if let Some(seq) = &def.seq {
                        let rst_pin = seq.reset_pin;
                        let body = if let Some(rp) = rst_pin {
                            ir::Stmt::If {
                                c: in_ref(rp),
                                t: Box::new(ir::Stmt::NBl {
                                    lv: lv_out.clone(),
                                    e: ir::Expr::Const(LogicWord::zero(1)),
                                }),
                                f: Some(Box::new(ir::Stmt::NBl {
                                    lv: lv_out.clone(),
                                    e: in_ref(1),
                                })),
                            }
                        } else {
                            ir::Stmt::NBl {
                                lv: lv_out.clone(),
                                e: in_ref(1),
                            }
                        };
                        let mut sens = vec![edge_sens(true, net_base[c.inputs[seq.clk_pin]])];
                        if let Some(rp) = rst_pin {
                            sens.push(edge_sens(true, net_base[c.inputs[rp]]));
                        }
                        d.processes.push(ir::Process {
                            sens: ir::Sens::List(sens),
                            body,
                        });
                    } else {
                        let e = libfunc_expr(&def.func, &in_ref);
                        d.assigns.push(ir::Assign {
                            lv: lv_out,
                            e,
                            delay: 0,
                        });
                    }
                }
            }
        }
    }
    d
}

fn edge_sens(posedge: bool, net: usize) -> ir::Sens {
    if posedge {
        ir::Sens::Posedge(net)
    } else {
        ir::Sens::Negedge(net)
    }
}

fn fold_exprs(items: impl Iterator<Item = ir::Expr>, op: ir::BinOp, unit: LogicWord) -> ir::Expr {
    items
        .reduce(|a, b| ir::Expr::Bin {
            op,
            a: Box::new(a),
            b: Box::new(b),
        })
        .unwrap_or(ir::Expr::Const(unit))
}

fn libfunc_expr(f: &LibFunc, pin: &dyn Fn(usize) -> ir::Expr) -> ir::Expr {
    match f {
        LibFunc::Const(v) => ir::Expr::Const(LogicWord::new_const(1, *v)),
        LibFunc::Id(i) => pin(*i),
        LibFunc::Not(e) => ir::Expr::Un {
            op: ir::UnOp::Not,
            a: Box::new(libfunc_expr(e, pin)),
        },
        LibFunc::And(es) => fold_exprs(
            es.iter().map(|e| libfunc_expr(e, pin)),
            ir::BinOp::And,
            LogicWord::ones(1),
        ),
        LibFunc::Or(es) => fold_exprs(
            es.iter().map(|e| libfunc_expr(e, pin)),
            ir::BinOp::Or,
            LogicWord::zero(1),
        ),
        LibFunc::Xor(es) => fold_exprs(
            es.iter().map(|e| libfunc_expr(e, pin)),
            ir::BinOp::Xor,
            LogicWord::zero(1),
        ),
        LibFunc::Nand(es) => ir::Expr::Un {
            op: ir::UnOp::Not,
            a: Box::new(fold_exprs(
                es.iter().map(|e| libfunc_expr(e, pin)),
                ir::BinOp::And,
                LogicWord::ones(1),
            )),
        },
        LibFunc::Nor(es) => ir::Expr::Un {
            op: ir::UnOp::Not,
            a: Box::new(fold_exprs(
                es.iter().map(|e| libfunc_expr(e, pin)),
                ir::BinOp::Or,
                LogicWord::zero(1),
            )),
        },
        LibFunc::Xnor(es) => ir::Expr::Un {
            op: ir::UnOp::Not,
            a: Box::new(fold_exprs(
                es.iter().map(|e| libfunc_expr(e, pin)),
                ir::BinOp::Xor,
                LogicWord::zero(1),
            )),
        },
        LibFunc::Mux { sel, a, b } => ir::Expr::Cond {
            c: Box::new(libfunc_expr(sel, pin)),
            t: Box::new(libfunc_expr(b, pin)),
            f: Box::new(libfunc_expr(a, pin)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::CellLibrary;
    use crate::netlist::{Cell, Netlist};
    use std::sync::Arc;

    #[test]
    fn round_trip_simple_netlist() {
        let mut nl = Netlist::new("and_inv");
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::And,
            inputs: vec![a, b],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Not,
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", netlist::Dir::In, vec![a]);
        nl.add_port("b", netlist::Dir::In, vec![b]);
        nl.add_port("y", netlist::Dir::Out, vec![y]);
        let d = netlist_to_design(&nl);
        assert_eq!(d.name, "and_inv");
        assert_eq!(d.ports.len(), 3);
        // 4 internal nets + 3 port decls
        assert_eq!(d.net_decls.len(), 7);
        // 2 logic assigns + 2 input feeds + 1 output feed
        assert_eq!(d.assigns.len(), 5);
        assert_eq!(d.processes.len(), 0);
    }

    #[test]
    fn flop_netlist_becomes_process() {
        let mut nl = Netlist::new("flop");
        let clk = nl.add_net("clk");
        let din = nl.add_net("d");
        let q = nl.add_net("q");
        nl.add_cell(Cell {
            name: "ff".into(),
            kind: CellKind::Dff { posedge: true },
            inputs: vec![clk, din],
            output: q,
        });
        nl.add_port("clk", netlist::Dir::In, vec![clk]);
        nl.add_port("d", netlist::Dir::In, vec![din]);
        nl.add_port("q", netlist::Dir::Out, vec![q]);
        let d = netlist_to_design(&nl);
        assert_eq!(d.processes.len(), 1);
        assert!(matches!(d.processes[0].sens, ir::Sens::Posedge(_)));
    }

    #[test]
    fn lib_cell_becomes_assign() {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("nand");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u".into(),
            kind: CellKind::Lib(lib.find("NAND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", netlist::Dir::In, vec![a]);
        nl.add_port("b", netlist::Dir::In, vec![b]);
        nl.add_port("y", netlist::Dir::Out, vec![y]);
        let d = netlist_to_design(&nl);
        assert_eq!(d.processes.len(), 0);
        assert!(d.assigns.iter().any(|a| matches!(
            a.e,
            ir::Expr::Un {
                op: ir::UnOp::Not,
                ..
            }
        )));
    }

    #[test]
    fn block_rom_output_bits_share_one_sparse_memory() {
        let contents = Arc::new(vec![
            LogicWord::from_u64(2, 0b01),
            LogicWord::from_u64(2, 0b10),
            LogicWord::from_u64(2, 0b11),
        ]);
        let mut nl = Netlist::new("rom");
        let address0 = nl.add_net("address0");
        let address1 = nl.add_net("address1");
        let q0 = nl.add_net("q0");
        let q1 = nl.add_net("q1");
        for (bit_index, output) in [(0, q0), (1, q1)] {
            nl.add_cell(Cell {
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
        // Netlist ports store bits MSB-first.
        nl.add_port("address", netlist::Dir::In, vec![address1, address0]);
        nl.add_port("q0", netlist::Dir::Out, vec![q0]);
        nl.add_port("q1", netlist::Dir::Out, vec![q1]);

        let design = netlist_to_design(&nl);
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].flat_length, 3);
        assert_eq!(design.memories[0].kind, ir::MemoryKind::Rom);
        assert_eq!(design.initials.len(), 1);
        assert!(matches!(
            &design.initials[0],
            ir::Stmt::Block(statements) if statements.len() == 3
        ));
    }

    #[test]
    fn block_ram_write_and_read_cells_share_one_sparse_process() {
        let initial = Arc::new(vec![
            LogicWord::from_u64(2, 0b01),
            LogicWord::from_u64(2, 0b10),
            LogicWord::from_u64(2, 0b11),
            LogicWord::from_u64(2, 0b00),
        ]);
        let mut nl = Netlist::new("ram");
        let clock = nl.add_net("clock");
        let write_enable = nl.add_net("write_enable");
        let address0 = nl.add_net("address0");
        let address1 = nl.add_net("address1");
        let data0 = nl.add_net("data0");
        let data1 = nl.add_net("data1");
        for (bit_index, data) in [(0, data0), (1, data1)] {
            let state = nl.add_net(format!("state{bit_index}"));
            nl.add_cell(Cell {
                name: format!("ram_write_bit{bit_index}"),
                kind: CellKind::BlockRamWrite {
                    macro_id: 7,
                    address_width: 2,
                    bit_index,
                    initial: initial.clone(),
                    posedge: true,
                },
                inputs: vec![clock, write_enable, address0, address1, data],
                output: state,
            });
        }
        for bit_index in 0..2 {
            let output = nl.add_net(format!("q{bit_index}"));
            nl.add_cell(Cell {
                name: format!("ram_read_bit{bit_index}"),
                kind: CellKind::BlockRamRead {
                    macro_id: 7,
                    address_width: 2,
                    bit_index,
                    initial: initial.clone(),
                },
                inputs: vec![address0, address1],
                output,
            });
        }
        nl.add_port("clock", netlist::Dir::In, vec![clock]);
        nl.add_port("write_enable", netlist::Dir::In, vec![write_enable]);
        nl.add_port("address", netlist::Dir::In, vec![address1, address0]);
        nl.add_port("din", netlist::Dir::In, vec![data1, data0]);
        nl.add_port("q0", netlist::Dir::Out, vec![nl.net_by_name("q0").unwrap()]);
        nl.add_port("q1", netlist::Dir::Out, vec![nl.net_by_name("q1").unwrap()]);

        let design = netlist_to_design(&nl);
        assert_eq!(design.memories.len(), 1);
        assert_eq!(design.memories[0].flat_length, 4);
        assert_eq!(design.memories[0].kind, ir::MemoryKind::Ram);
        assert_eq!(design.memories[0].template, ir::MemoryTemplate::SyncRam);
        assert_eq!(design.processes.len(), 1);
        assert!(matches!(&design.processes[0].body, ir::Stmt::If { .. }));
        assert!(matches!(&design.initials[0], ir::Stmt::Block(_)));
        let memory_reads = design
            .assigns
            .iter()
            .filter(|assignment| matches!(assignment.e, ir::Expr::Index { .. }))
            .count();
        assert_eq!(memory_reads, 2);
    }
}
