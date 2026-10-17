//! Optimization, deterministic output, and hierarchy-preservation tests.

use super::super::*;
use eda_hdl::compile;

fn synth(src: &str) -> SynthResult {
    let d = compile(src, None).unwrap();
    synthesize(&d, &SynthOptions::default()).unwrap_or_else(|e| panic!("synth failed: {}", e))
}

#[test]
fn exhaustive_case_without_default_does_not_create_a_false_loop() {
    let r = synth(
        r#"
                module m(input [2:0] sel, input [1:0] d, output reg [1:0] y);
                    always @(*) begin
                        case (sel)
                            3'd0: y = d;
                            3'd1: y = ~d;
                            3'd2: y = d & 2'b10;
                            3'd3: y = d | 2'b01;
                            3'd4: y = d ^ 2'b11;
                            3'd5: y = {d[0], d[1]};
                            3'd6: y = d + 2'b01;
                            3'd7: y = 2'b00;
                        endcase
                    end
                endmodule
            "#,
    );
    assert!(
        r.netlist.topo_cells().is_some(),
        "an exhaustive exact case must not feed its own output back to itself"
    );
    assert!(r.memory_plans.is_empty());
}

#[test]
fn incomplete_case_without_default_still_preserves_the_old_value() {
    let r = synth(
        r#"
                module m(input [1:0] sel, input d, output reg y);
                    always @(*) begin
                        case (sel)
                            2'd0: y = d;
                            2'd1: y = ~d;
                        endcase
                    end
                endmodule
            "#,
    );
    assert!(
        r.netlist.topo_cells().is_none(),
        "an incomplete combinational case intentionally models value retention"
    );
}

#[test]
fn optimization_preserves_named_dead_state_for_sequential_audits() {
    let r = synth(
        r#"
                module m(input clk, d, output reg active);
                    reg hidden;
                    always @(posedge clk) begin
                        hidden <= ~d;
                        active <= d;
                    end
                endmodule
            "#,
    );
    assert!(r.netlist.net_by_name("hidden").is_some());
    assert_eq!(r.netlist.stats().seq, 2);
}

#[test]
fn synthesis_is_deterministic_across_thread_hash_seeds() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::thread;

    const SRC: &str = r#"
            module m(input clk, rst, a, b, sel, output reg [3:0] x, y,
                output reg [1:0] z, output n);
                always @(*) begin
                    x = 4'b0;
                    y = 4'b0;
                    z = 2'b0;
                    case (sel)
                        1'b0: begin
                            x = {a, b, a, b};
                            z = {b, a};
                        end
                        default: begin
                            y = {b, a, b, a};
                            z = {a, b};
                        end
                    endcase
                    if (a) x = x ^ 4'b0101;
                    if (b) y = y | 4'b1010;
                end
                assign n = ~((a & b) | (a & b));
            endmodule
        "#;

    fn signature(nl: &Netlist) -> u64 {
        let mut hasher = DefaultHasher::new();
        for net_id in 0..nl.net_count() {
            nl.net(net_id).name.hash(&mut hasher);
        }
        for cell in nl.cells() {
            cell.name.hash(&mut hasher);
            format!("{:?}", cell.kind).hash(&mut hasher);
            for input in &cell.inputs {
                nl.net(*input).name.hash(&mut hasher);
            }
            nl.net(cell.output).name.hash(&mut hasher);
        }
        for port in &nl.ports {
            port.name.hash(&mut hasher);
            format!("{:?}", port.dir).hash(&mut hasher);
            for bit in &port.bits {
                nl.net(*bit).name.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    let workers = (0..4)
        .map(|_| {
            thread::spawn(|| {
                let design = compile(SRC, None).unwrap();
                let result = synthesize(&design, &SynthOptions::default()).unwrap();
                signature(&result.netlist)
            })
        })
        .collect::<Vec<_>>();
    let signatures = workers
        .into_iter()
        .map(thread::JoinHandle::join)
        .map(|result| result.expect("synthesis worker panicked"))
        .collect::<Vec<_>>();
    assert!(
        signatures.windows(2).all(|pair| pair[0] == pair[1]),
        "synthesis changed with hasher/thread seed: {signatures:?}"
    );
}

#[test]
fn duplicate_merging_preserves_a_stable_rtl_alias_name() {
    let mut nl = Netlist::new("aliases".to_string());
    let a = nl.add_net("a");
    let b = nl.add_net("b");
    let temporary = nl.add_net("g_n1");
    let stable = nl.add_net("keep");
    let y = nl.add_net("y");
    nl.add_cell(Cell {
        name: "and0".into(),
        kind: CellKind::And,
        inputs: vec![a, b],
        output: temporary,
    });
    nl.add_cell(Cell {
        name: "and1".into(),
        kind: CellKind::And,
        inputs: vec![a, b],
        output: stable,
    });
    nl.add_cell(Cell {
        name: "or".into(),
        kind: CellKind::Or,
        inputs: vec![temporary, stable],
        output: y,
    });
    nl.add_port("a", Dir::In, vec![a]);
    nl.add_port("b", Dir::In, vec![b]);
    nl.add_port("y", Dir::Out, vec![y]);

    let mut warnings = Vec::new();
    opt::optimize(&mut nl, &mut warnings);
    assert!(nl.net_by_name("keep").is_some());
    assert!(nl.net_by_name("g_n1").is_none());
}

#[test]
fn synthesis_preserves_hierarchy_metadata() {
    let src = "module leaf(input a, output y); assign y = ~a; endmodule
            module sub(input a, output y); leaf u_leaf(.a(a), .y(y)); endmodule
            module top(input a, output y); sub u1(.a(a), .y(y)); endmodule";
    let r = synth(src);
    assert_eq!(r.netlist.hierarchy.len(), 3);
    assert_eq!(
        r.netlist
            .hierarchy_instance("top.u1.u_leaf")
            .unwrap()
            .module,
        "leaf"
    );
    let leaf = r.netlist.hierarchy_instance("top.u1.u_leaf").unwrap();
    assert_eq!(leaf.parent.as_deref(), Some("top.u1"));
    assert_eq!(leaf.ports.len(), 2);
    let input = leaf
        .ports
        .iter()
        .find(|port| port.name == "a")
        .expect("leaf input retained");
    assert_eq!(input.direction, Dir::In);
    let net = input
        .nets
        .first()
        .copied()
        .expect("leaf input net retained");
    assert_eq!(
        r.netlist
            .owning_hierarchy_instance(net)
            .map(|instance| instance.path.as_str()),
        Some("top.u1.u_leaf")
    );
    assert!(r.netlist.hierarchy_report().contains("top.u1.u_leaf"));
}
