//! Equivalence, BMC, safety, assertion replay, and formal trace facade APIs.

use super::*;

pub fn flow_equivalent(f: &Flow) -> Result<bool, String> {
    Ok(eda_formal::check_equivalent(&f.gold, &f.netlist)?.equivalent)
}

/// Symbolically compare two netlists through the unified API.
pub fn check_netlists_equivalent(gold: &Netlist, mapped: &Netlist) -> Result<EquivResult, String> {
    check_netlists_equivalent_cancellable(gold, mapped, &FlowCancellation::new())
}

/// Cooperatively cancel netlist equivalence checking.
pub fn check_netlists_equivalent_cancellable(
    gold: &Netlist,
    mapped: &Netlist,
    cancellation: &FlowCancellation,
) -> Result<EquivResult, String> {
    eda_formal::check_equivalent_cancellable(gold, mapped, cancellation)
}

/// Search for a initialized bad-state trace through the unified API.
pub fn bounded_model_check(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
) -> Result<BmcResult, String> {
    bounded_model_check_cancellable(netlist, bad_net, depth, &FlowCancellation::new())
}

/// Cooperatively cancel bad-state bounded model checking.
pub fn bounded_model_check_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<BmcResult, String> {
    eda_formal::bmc_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Search for an initialized false-property trace through the unified API.
pub fn bounded_model_check_expression(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
) -> Result<BmcResult, String> {
    bounded_model_check_expression_cancellable(netlist, property, depth, &FlowCancellation::new())
}

/// Cooperatively cancel expression bounded model checking.
pub fn bounded_model_check_expression_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<BmcResult, String> {
    eda_formal::bmc_expression_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a safety property using bounded reachability plus k-induction.
pub fn prove_safety(netlist: &Netlist, bad_net: &str, depth: usize) -> Result<ProofResult, String> {
    prove_safety_cancellable(netlist, bad_net, depth, &FlowCancellation::new())
}

/// Cooperatively cancel a safety proof.
pub fn prove_safety_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a Boolean safety expression using BMC plus k-induction.
pub fn prove_safety_expression(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
) -> Result<ProofResult, String> {
    prove_safety_expression_cancellable(netlist, property, depth, &FlowCancellation::new())
}

/// Cooperatively cancel an expression safety proof.
pub fn prove_safety_expression_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified net invariants.
pub fn prove_safety_expression_with_value_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariant_nets: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_value_invariants_cancellable(
        netlist,
        property,
        depth,
        invariant_nets,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with net invariants.
pub fn prove_safety_expression_with_value_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariant_nets: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_value_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariant_nets,
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified expression invariants.
pub fn prove_safety_expression_with_expression_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[(&str, &BoolExpr)],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_expression_invariants_cancellable(
        netlist,
        property,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with expression invariants.
pub fn prove_safety_expression_with_expression_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[(&str, &BoolExpr)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_expression_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Prove a Boolean expression using independently verified net or expression invariants.
pub fn prove_safety_expression_with_proof_invariants(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[ProofInvariant],
) -> Result<ProofResult, String> {
    prove_safety_expression_with_proof_invariants_cancellable(
        netlist,
        property,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel an expression proof with typed invariants.
pub fn prove_safety_expression_with_proof_invariants_cancellable(
    netlist: &Netlist,
    property: &BoolExpr,
    depth: usize,
    invariants: &[ProofInvariant],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_expression_with_proof_invariants_cancellable(
        netlist,
        property,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Bounded SAT checking of immediate and temporal assertions.
pub fn check_assertions_bounded(
    netlist: &Netlist,
    assertions: &[Assertion],
    depth: usize,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions(
        netlist,
        assertions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
    )
}

/// Cooperatively cancel bounded SAT checking of assertions.
pub fn check_assertions_bounded_cancellable(
    netlist: &Netlist,
    assertions: &[Assertion],
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_cancellable(
        netlist,
        assertions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Bounded SAT checking of assert/cover targets under SVA assumptions.
pub fn check_assertions_bounded_with_assumptions(
    netlist: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    depth: usize,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_with_assumptions(
        netlist,
        assertions,
        assumptions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
    )
}

/// Cooperatively cancel bounded SAT checking under SVA assumptions.
pub fn check_assertions_bounded_with_assumptions_cancellable(
    netlist: &Netlist,
    assertions: &[Assertion],
    assumptions: &[Assertion],
    depth: usize,
    cancellation: &FlowCancellation,
) -> Result<Vec<PropertyBmcResult>, String> {
    eda_formal::bmc_assertions_with_assumptions_cancellable(
        netlist,
        assertions,
        assumptions,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        cancellation,
    )
}

/// Export a SAT input-assignment trace as VCD through the unified API.
pub fn export_formal_trace_vcd(
    netlist: &Netlist,
    trace: &[std::collections::HashMap<String, u64>],
) -> Result<String, String> {
    eda_formal::trace_input_vcd(netlist, trace)
}

/// Export every SAT-reconstructed net value in a formal trace as VCD.
pub fn export_formal_net_trace_vcd(
    netlist: &Netlist,
    trace: &[std::collections::HashMap<String, u64>],
) -> Result<String, String> {
    eda_formal::trace_net_vcd(netlist, trace)
}

/// Export a temporal-property counterexample as VCD.
pub fn export_property_counterexample_vcd(
    netlist: &Netlist,
    result: &PropertyBmcResult,
) -> Result<String, String> {
    eda_formal::trace_property_vcd(netlist, result)
}

/// Independently replay a formal property counterexample at netlist level.
pub fn verify_property_counterexample(
    netlist: &Netlist,
    result: &PropertyBmcResult,
) -> Result<PropertyReplayResult, String> {
    eda_formal::verify_property_trace(netlist, result)
}

/// Parse and independently replay an exported formal property VCD.
pub fn replay_property_counterexample_vcd(
    netlist: &Netlist,
    result: &PropertyBmcResult,
    vcd: &str,
) -> Result<PropertyReplayResult, String> {
    eda_formal::replay_property_vcd(netlist, result, vcd)
}

/// Export a refuted safety proof's complete SAT net trace as VCD.
pub fn export_proof_counterexample_vcd(
    netlist: &Netlist,
    result: &ProofResult,
) -> Result<String, String> {
    let trace = result
        .bmc
        .net_trace
        .as_ref()
        .ok_or_else(|| "proof result has no counterexample net trace".to_string())?;
    eda_formal::trace_net_vcd(netlist, trace)
}

/// Prove a safety property using independently verified zero-valued invariants.
pub fn prove_safety_with_invariants(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariant_nets: &[&str],
) -> Result<ProofResult, String> {
    prove_safety_with_invariants_cancellable(
        netlist,
        bad_net,
        depth,
        invariant_nets,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel a proof with zero-valued invariants.
pub fn prove_safety_with_invariants_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariant_nets: &[&str],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_with_invariants_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariant_nets,
        cancellation,
    )
}

/// Prove a safety property using independently verified Boolean invariants.
pub fn prove_safety_with_value_invariants(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariants: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_safety_with_value_invariants_cancellable(
        netlist,
        bad_net,
        depth,
        invariants,
        &FlowCancellation::new(),
    )
}

/// Cooperatively cancel a proof with Boolean invariants.
pub fn prove_safety_with_value_invariants_cancellable(
    netlist: &Netlist,
    bad_net: &str,
    depth: usize,
    invariants: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    eda_formal::prove_with_value_invariants_cancellable(
        netlist,
        bad_net,
        &eda_formal::BmcOptions {
            depth,
            init_state_zero: true,
        },
        invariants,
        cancellation,
    )
}

/// Evaluate the gate-level netlist of a flow on one combinational vector.
pub fn gate_eval_comb(f: &Flow, inputs: &[(&str, u64)], out: &str) -> Result<u64, String> {
    let d = netlist_to_design(&f.netlist);
    let mut sim = Simulator::new(&d)?;
    for (p, v) in inputs {
        let w = f
            .netlist
            .port_by_name(p)
            .map(|pt| pt.bits.len())
            .ok_or_else(|| format!("no port '{}'", p))?;
        sim.set_input(p, &LogicWord::from_u64(w, *v))
            .map_err(|e| e.to_string())?;
    }
    let w = sim
        .read_port(out)
        .ok_or_else(|| format!("no port '{}'", out))?;
    w.to_u64().ok_or_else(|| format!("'{}' is X", out))
}
