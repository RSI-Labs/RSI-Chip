use super::*;
use eda_core::ir::{
    Assign, Expr, LValue, MemoryDecl, MemoryDimension, NetDecl, PortDecl, Range, Stmt,
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
