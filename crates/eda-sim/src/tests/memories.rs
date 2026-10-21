use super::*;
use eda_core::ir::{
    Assign, Expr, LValue, MemoryDecl, MemoryDimension, MemoryStorage, NetDecl, PortDecl, Range,
    Stmt,
};

fn sparse_memory_design(nonblocking: bool) -> Design {
    let memory_write = if nonblocking {
        Stmt::MemoryNBl {
            memory: 0,
            indexes: vec![Expr::Const(LogicWord::from_u64(8, 5))],
            e: Expr::Const(LogicWord::from_u64(8, 0x5a)),
        }
    } else {
        Stmt::MemoryBl {
            memory: 0,
            indexes: vec![Expr::Const(LogicWord::from_u64(8, 5))],
            e: Expr::Const(LogicWord::from_u64(8, 0x3c)),
        }
    };

    Design {
        name: "sparse_memory".into(),
        time_scale: Default::default(),
        memory_ports: Vec::new(),
        array_ports: Vec::new(),
        net_decls: vec![NetDecl {
            name: "q".into(),
            width: 8,
            is_reg: false,
            is_signed: false,
            base: 0,
        }],
        memories: vec![MemoryDecl {
            name: "mem".into(),
            element_width: 8,
            element_msb: 7,
            element_lsb: 0,
            dimensions: vec![MemoryDimension {
                base: 4,
                step: 1,
                length: 4,
            }],
            flat_length: 4,
            storage: eda_core::ir::MemoryStorage::Sparse,
            kind: eda_core::ir::MemoryKind::Ram,
            template: eda_core::ir::MemoryTemplate::Unmapped,
            write_mask: None,
            byte_enable: None,
            banking: eda_core::ir::MemoryBanking::default(),
        }],
        memory_connections: Vec::new(),
        n_nets: 8,
        ports: vec![PortDecl {
            name: "q".into(),
            dir: eda_core::ir::Dir::Out,
            base: 0,
            width: 8,
        }],
        assigns: vec![Assign {
            lv: LValue {
                net: 0,
                decl_width: 8,
                range: Range::Span { msb: 7, lsb: 0 },
            },
            e: Expr::MemoryRead {
                memory: 0,
                indexes: vec![Expr::Const(LogicWord::from_u64(8, 5))],
                width: 8,
                signed: false,
            },
            delay: 0,
        }],
        processes: Vec::new(),
        initials: vec![Stmt::Block(vec![memory_write])],
        final_blocks: Vec::new(),
        clocking_blocks: Vec::new(),
        hierarchy: Vec::new(),
    }
}

fn ordinary_array_design() -> Design {
    let memory_write = |index: u8, value: u8| Stmt::MemoryBl {
        memory: 0,
        indexes: vec![Expr::Const(LogicWord::from_u64(8, index.into()))],
        e: Expr::Const(LogicWord::from_u64(8, value.into())),
    };
    Design {
        name: "ordinary_array".into(),
        time_scale: Default::default(),
        memory_ports: Vec::new(),
        array_ports: Vec::new(),
        net_decls: vec![NetDecl {
            name: "q".into(),
            width: 8,
            is_reg: false,
            is_signed: false,
            base: 0,
        }],
        memories: vec![MemoryDecl {
            name: "values".into(),
            element_width: 8,
            element_msb: 7,
            element_lsb: 0,
            dimensions: vec![MemoryDimension {
                base: 0,
                step: 1,
                length: 4,
            }],
            flat_length: 4,
            storage: MemoryStorage::Ordinary,
            kind: Default::default(),
            template: Default::default(),
            write_mask: None,
            byte_enable: None,
            banking: Default::default(),
        }],
        memory_connections: Vec::new(),
        n_nets: 8,
        ports: vec![PortDecl {
            name: "q".into(),
            dir: eda_core::ir::Dir::Out,
            base: 0,
            width: 8,
        }],
        assigns: vec![Assign {
            lv: LValue {
                net: 0,
                decl_width: 8,
                range: Range::Span { msb: 7, lsb: 0 },
            },
            e: Expr::MemoryRead {
                memory: 0,
                indexes: vec![Expr::Const(LogicWord::from_u64(8, 1))],
                width: 8,
                signed: false,
            },
            delay: 0,
        }],
        processes: Vec::new(),
        initials: vec![Stmt::Block(vec![
            memory_write(1, 0x3c),
            memory_write(3, 0xa5),
        ])],
        final_blocks: Vec::new(),
        clocking_blocks: Vec::new(),
        hierarchy: Vec::new(),
    }
}

#[test]
fn sparse_memory_blocking_write_updates_continuous_reader() {
    let design = sparse_memory_design(false);
    let mut simulator = Simulator::new(&design).unwrap();
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0x3c));
    assert_eq!(design.n_nets, 8);
}

#[test]
fn sparse_memory_part_write_following_whole_write_updates_reader() {
    let mut design = sparse_memory_design(false);
    design.initials = vec![Stmt::Block(vec![
        Stmt::MemoryBl {
            memory: 0,
            indexes: vec![Expr::Const(LogicWord::from_u64(8, 5))],
            e: Expr::Const(LogicWord::from_u64(8, 0x9c)),
        },
        Stmt::MemoryBlPart {
            memory: 0,
            indexes: vec![Expr::Const(LogicWord::from_u64(8, 5))],
            start: Box::new(Expr::Const(LogicWord::from_u64(32, 1))),
            width: 2,
            ascending: true,
            e: Expr::Const(LogicWord::from_u64(2, 0b01)),
        },
    ])];
    let mut simulator = Simulator::new(&design).unwrap();
    simulator.run_until(0).unwrap();
    let value = simulator.read("q").unwrap();
    assert_eq!(value.to_u64(), Some(154));
    assert!(value.to_bin_str().ends_with("1010"));
}

#[test]
fn sparse_memory_nonblocking_write_commits_after_active_region() {
    let design = sparse_memory_design(true);
    let mut simulator = Simulator::new(&design).unwrap();
    simulator.run_until(0).unwrap();
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0x5a));
    assert_eq!(design.n_nets, 8);
}

#[test]
fn sparse_memory_runtime_index_and_out_of_range_read() {
    let mut design = sparse_memory_design(false);
    design.net_decls.insert(
        0,
        NetDecl {
            name: "address".into(),
            width: 8,
            is_reg: false,
            is_signed: false,
            base: 8,
        },
    );
    design.n_nets = 16;
    design.ports.insert(
        0,
        PortDecl {
            name: "address".into(),
            dir: eda_core::ir::Dir::In,
            base: 8,
            width: 8,
        },
    );
    let assign = &mut design.assigns[0];
    let Expr::MemoryRead { indexes, .. } = &mut assign.e else {
        unreachable!("test assigns a memory read");
    };
    indexes[0] = Expr::Ref {
        net: 8,
        decl_width: 8,
        range: Range::Span { msb: 7, lsb: 0 },
        signed: false,
    };

    let mut simulator = Simulator::new(&design).unwrap();
    simulator.run_until(0).unwrap();
    simulator
        .set_input("address", &LogicWord::from_u64(8, 5))
        .unwrap();
    assert_eq!(simulator.read("q").unwrap().to_u64(), Some(0x3c));

    simulator
        .set_input("address", &LogicWord::from_u64(8, 3))
        .unwrap();
    assert_eq!(simulator.read("q").unwrap().to_u64(), None);
}

#[test]
fn sparse_memory_vcd_dumps_touched_elements() {
    let mut sim = sim_for(
        r#"
module memory_vcd(
  input logic clk,
  input logic [19:0] address,
  input logic [7:0] din,
  output logic [7:0] initialized,
  output logic [7:0] runtime
);
  logic [7:0] memory [0:1048575];
  initial memory[5] = 8'h3c;
  always @(posedge clk) memory[address] <= din;
  assign initialized = memory[5];
  assign runtime = memory[12345];
endmodule
"#,
    );
    sim.enable_vcd();
    sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
    let initial_vcd = sim.vcd().unwrap();
    assert!(initial_vcd.contains("$var reg 8 "));
    assert!(initial_vcd.contains(" memory[5] "));
    assert!(initial_vcd.contains("b00111100"));
    assert!(!initial_vcd.contains(" memory[999999] $end"));

    sim.set_input("address", &LogicWord::from_u64(20, 12345))
        .unwrap();
    sim.set_input("din", &LogicWord::from_u64(8, 0xa5)).unwrap();
    sim.set_input("clk", &LogicWord::from_u64(1, 1)).unwrap();
    let runtime_vcd = sim.vcd().unwrap();
    assert!(runtime_vcd.contains(" memory[12345] "));
    assert!(runtime_vcd.contains("b10100101"));
    assert!(runtime_vcd.contains("b00111100"));
    assert!(!runtime_vcd.contains(" memory[999999] $end"));
}

#[test]
fn ordinary_array_vcd_dumps_touched_dense_elements() {
    let design = ordinary_array_design();
    assert_eq!(design.memories[0].storage, MemoryStorage::Ordinary);
    let design: &'static Design = Box::leak(Box::new(design));
    let mut sim = Simulator::new(design).unwrap();
    sim.enable_vcd();
    sim.run_until(1).unwrap();
    assert_eq!(sim.read("q").unwrap().to_u64(), Some(0x3c));

    let vcd = sim.vcd().unwrap();
    assert!(vcd.contains(" values[1] "));
    assert!(vcd.contains(" values[3] "));
    assert!(!vcd.contains(" values[0] "));
    assert!(!vcd.contains(" values[2] "));
}
