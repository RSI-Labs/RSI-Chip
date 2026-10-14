//! k-induction proof engine for safety properties.
//!
//! `prove` combines a bounded reachability base case with an inductive-step
//! SAT query. The inductive step assumes the property in `k` consecutive
//! states and asks whether one more transition can violate it. Unlike BMC, an
//! UNSAT inductive step proves safety for all reachable states.

use crate::bmc::{bmc_target, target_lit, BmcOptions, BmcResult, BmcTarget};
use crate::sat::{solve_cancellable, SatResult};
use eda_assert::BoolExpr;
use eda_core::cancellation::FlowCancellation;
use eda_core::netlist::{NetId, Netlist};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofStatus {
    /// No bounded counterexample exists and the property is inductive.
    Proved,
    /// BMC found a real initialized counterexample.
    Refuted,
    /// No counterexample was found, but k-induction did not converge.
    Unknown,
}

/// A caller-provided invariant that must be independently proved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProofInvariant {
    Net { net: String, value: bool },
    Expression { name: String, expr: BoolExpr },
}

impl ProofInvariant {
    pub fn new(net: impl Into<String>, value: bool) -> Self {
        Self::Net {
            net: net.into(),
            value,
        }
    }

    pub fn expression(name: impl Into<String>, expr: BoolExpr) -> Self {
        Self::Expression {
            name: name.into(),
            expr,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Net { net, .. } => net,
            Self::Expression { name, .. } => name,
        }
    }

    fn target(&self, nl: &Netlist) -> Result<BmcTarget, String> {
        match self {
            Self::Net { net, value } => {
                let net = nl
                    .net_by_name(net)
                    .ok_or_else(|| format!("no invariant net named '{}'", net))?;
                Ok(BmcTarget::Net(net, *value))
            }
            Self::Expression { expr, .. } => Ok(BmcTarget::Expr(expr.clone())),
        }
    }

    fn display(&self) -> String {
        match self {
            Self::Net { net, value } => format!("{}={}", net, if *value { 1 } else { 0 }),
            Self::Expression { name, .. } => name.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProofResult {
    pub status: ProofStatus,
    pub depth_checked: usize,
    /// True when the inductive-step query was UNSAT.
    pub inductive: bool,
    /// Real trace when refuted.
    pub counterexample: Option<Vec<HashMap<String, u64>>>,
    /// State/input assignment that prevents induction from converging. This
    /// is not necessarily reachable from the reset state.
    pub inductive_witness: Option<Vec<HashMap<String, u64>>>,
    /// Invariants that were independently proved before being used to
    /// strengthen the target-property induction query.
    pub assumed_invariants: Vec<ProofInvariant>,
    /// Set when a caller-supplied invariant could not be proved.
    pub failed_invariant: Option<String>,
    /// Full diagnostic result for `failed_invariant`.
    pub invariant_result: Option<Box<ProofResult>>,
    pub bmc: BmcResult,
}

pub fn prove(nl: &Netlist, bad_net_name: &str, opts: &BmcOptions) -> Result<ProofResult, String> {
    prove_cancellable(nl, bad_net_name, opts, &FlowCancellation::new())
}

pub fn prove_cancellable(
    nl: &Netlist,
    bad_net_name: &str,
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    let net = nl
        .net_by_name(bad_net_name)
        .ok_or_else(|| format!("no net named '{}'", bad_net_name))?;
    prove_property_with_invariants(nl, BmcTarget::Net(net, false), opts, &[], cancellation)
}

/// Prove that a Boolean expression over scalar net names always holds.
pub fn prove_expression(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
) -> Result<ProofResult, String> {
    prove_expression_cancellable(nl, property, opts, &FlowCancellation::new())
}

pub fn prove_expression_cancellable(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    prove_property_with_invariants(
        nl,
        BmcTarget::Expr(property.clone()),
        opts,
        &[],
        cancellation,
    )
}

/// Prove a Boolean expression using independently proved net invariants.
pub fn prove_expression_with_value_invariants(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariant_nets: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_expression_with_value_invariants_cancellable(
        nl,
        property,
        opts,
        invariant_nets,
        &FlowCancellation::new(),
    )
}

pub fn prove_expression_with_value_invariants_cancellable(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariant_nets: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    let invariants = invariant_nets
        .iter()
        .map(|(net, value)| ProofInvariant::new((*net).to_string(), *value))
        .collect::<Vec<_>>();
    prove_property_with_invariants(
        nl,
        BmcTarget::Expr(property.clone()),
        opts,
        &invariants,
        cancellation,
    )
}

/// Prove a Boolean expression using independently proved expression invariants.
pub fn prove_expression_with_expression_invariants(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariants: &[(&str, &BoolExpr)],
) -> Result<ProofResult, String> {
    prove_expression_with_expression_invariants_cancellable(
        nl,
        property,
        opts,
        invariants,
        &FlowCancellation::new(),
    )
}

pub fn prove_expression_with_expression_invariants_cancellable(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariants: &[(&str, &BoolExpr)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    let invariants = invariants
        .iter()
        .map(|(name, expr)| ProofInvariant::expression((*name).to_string(), (*expr).clone()))
        .collect::<Vec<_>>();
    prove_property_with_invariants(
        nl,
        BmcTarget::Expr(property.clone()),
        opts,
        &invariants,
        cancellation,
    )
}

/// Prove a Boolean expression using any independently proved invariant kind.
pub fn prove_expression_with_proof_invariants(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariants: &[ProofInvariant],
) -> Result<ProofResult, String> {
    prove_expression_with_proof_invariants_cancellable(
        nl,
        property,
        opts,
        invariants,
        &FlowCancellation::new(),
    )
}

pub fn prove_expression_with_proof_invariants_cancellable(
    nl: &Netlist,
    property: &BoolExpr,
    opts: &BmcOptions,
    invariants: &[ProofInvariant],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    prove_property_with_invariants(
        nl,
        BmcTarget::Expr(property.clone()),
        opts,
        invariants,
        cancellation,
    )
}

/// Prove `bad_net_name == 0` using independently proved zero-valued
/// invariants to strengthen k-induction.
///
/// Every invariant is first checked by BMC and k-induction. Only after an
/// invariant is proved is it assumed in the target property query. This keeps
/// caller-provided hints sound: they cannot introduce an unreachable state
/// that was not already excluded by a proof.
pub fn prove_with_invariants(
    nl: &Netlist,
    bad_net_name: &str,
    opts: &BmcOptions,
    invariant_net_names: &[&str],
) -> Result<ProofResult, String> {
    prove_with_invariants_cancellable(
        nl,
        bad_net_name,
        opts,
        invariant_net_names,
        &FlowCancellation::new(),
    )
}

pub fn prove_with_invariants_cancellable(
    nl: &Netlist,
    bad_net_name: &str,
    opts: &BmcOptions,
    invariant_net_names: &[&str],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    let invariants = invariant_net_names
        .iter()
        .map(|net| ProofInvariant::new((*net).to_string(), false))
        .collect::<Vec<_>>();
    let net = nl
        .net_by_name(bad_net_name)
        .ok_or_else(|| format!("no net named '{}'", bad_net_name))?;
    prove_property_with_invariants(
        nl,
        BmcTarget::Net(net, false),
        opts,
        &invariants,
        cancellation,
    )
}

/// Prove `bad_net_name == 0` using independently proved Boolean invariants.
pub fn prove_with_value_invariants(
    nl: &Netlist,
    bad_net_name: &str,
    opts: &BmcOptions,
    invariants: &[(&str, bool)],
) -> Result<ProofResult, String> {
    prove_with_value_invariants_cancellable(
        nl,
        bad_net_name,
        opts,
        invariants,
        &FlowCancellation::new(),
    )
}

pub fn prove_with_value_invariants_cancellable(
    nl: &Netlist,
    bad_net_name: &str,
    opts: &BmcOptions,
    invariants: &[(&str, bool)],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    let invariants = invariants
        .iter()
        .map(|(net, value)| ProofInvariant::new((*net).to_string(), *value))
        .collect::<Vec<_>>();
    let net = nl
        .net_by_name(bad_net_name)
        .ok_or_else(|| format!("no net named '{}'", bad_net_name))?;
    prove_property_with_invariants(
        nl,
        BmcTarget::Net(net, false),
        opts,
        &invariants,
        cancellation,
    )
}

fn prove_property_with_invariants(
    nl: &Netlist,
    target: BmcTarget,
    opts: &BmcOptions,
    invariant_specs: &[ProofInvariant],
    cancellation: &FlowCancellation,
) -> Result<ProofResult, String> {
    if cancellation.is_cancelled() {
        return Err("k-induction proof cancelled".into());
    }
    let depth = opts.depth.max(1);

    let mut invariant_targets = Vec::new();
    for invariant in invariant_specs {
        let target = invariant.target(nl)?;
        if invariant_targets
            .iter()
            .any(|existing| same_target(existing, &target))
        {
            return Err(format!("duplicate invariant '{}'", invariant.name()));
        }
        invariant_targets.push(target);
    }

    let mut proved_targets = Vec::new();
    let mut proved_specs = Vec::new();
    for (target, invariant) in invariant_targets.iter().zip(invariant_specs.iter()) {
        if cancellation.is_cancelled() {
            return Err("k-induction proof cancelled".into());
        }
        let invariant_result = Box::new(prove_property_with_invariants(
            nl,
            target.clone(),
            opts,
            &proved_specs,
            cancellation,
        )?);
        if invariant_result.status == ProofStatus::Proved {
            proved_targets.push(target.clone());
            proved_specs.push(invariant.clone());
        } else {
            let (witness, bmc) = match invariant_result.status {
                ProofStatus::Unknown => {
                    let witness = invariant_result.inductive_witness.clone();
                    let bmc = invariant_result.bmc.clone();
                    (witness, bmc)
                }
                _ => (None, invariant_result.bmc.clone()),
            };
            return Ok(ProofResult {
                status: ProofStatus::Unknown,
                depth_checked: depth,
                inductive: false,
                counterexample: None,
                inductive_witness: witness,
                assumed_invariants: proved_specs,
                failed_invariant: Some(invariant.name().to_string()),
                invariant_result: Some(invariant_result),
                bmc,
            });
        }
    }

    let bounded = bmc_target(nl, target.clone(), opts, cancellation)?;
    if bounded.violated {
        return Ok(ProofResult {
            status: ProofStatus::Refuted,
            depth_checked: depth,
            inductive: false,
            counterexample: bounded.trace.clone(),
            inductive_witness: None,
            assumed_invariants: proved_specs,
            failed_invariant: None,
            invariant_result: None,
            bmc: bounded,
        });
    }

    let (cnf, frames) = inductive_step(nl, target, depth, &proved_targets, cancellation)?;
    if cancellation.is_cancelled() {
        return Err("k-induction proof cancelled".into());
    }
    match solve_cancellable(&cnf, cancellation)? {
        SatResult::Unsat => Ok(ProofResult {
            status: ProofStatus::Proved,
            depth_checked: depth,
            inductive: true,
            counterexample: None,
            inductive_witness: None,
            assumed_invariants: proved_specs,
            failed_invariant: None,
            invariant_result: None,
            bmc: bounded,
        }),
        SatResult::Sat(model) => Ok(ProofResult {
            status: ProofStatus::Unknown,
            depth_checked: depth,
            inductive: false,
            counterexample: None,
            inductive_witness: Some(trace_from_model(nl, &frames, &model)),
            assumed_invariants: proved_specs,
            failed_invariant: None,
            invariant_result: None,
            bmc: bounded,
        }),
    }
}

/// Build the k-frame transition relation, assume the Boolean property in
/// every earlier frame, and force its violation in the final frame.
fn inductive_step(
    nl: &Netlist,
    target: BmcTarget,
    depth: usize,
    invariant_targets: &[BmcTarget],
    cancellation: &FlowCancellation,
) -> Result<(crate::sat::Cnf, Vec<HashMap<NetId, i32>>), String> {
    use crate::bmc::{encode_frame, flop_pins, shift_lit};

    let depth = depth.max(1);
    let mut cnf = crate::sat::Cnf::new();
    let mut frames: Vec<HashMap<NetId, i32>> = Vec::new();
    let flops = flop_pins(nl)?;
    for t in 0..=depth {
        if cancellation.is_cancelled() {
            return Err("k-induction proof cancelled".into());
        }
        let (frame_cnf, frame_lit) = encode_frame(nl)?;
        let off = cnf.n_vars as i32;
        while cnf.n_vars < off as usize + frame_cnf.n_vars {
            cnf.add_var();
        }
        for clause in &frame_cnf.clauses {
            cnf.add_clause(clause.iter().map(|lit| shift_lit(*lit, off)).collect());
        }
        let lit = frame_lit
            .iter()
            .map(|(net, value)| (*net, shift_lit(*value, off)))
            .collect::<HashMap<_, _>>();
        if t > 0 {
            let previous = &frames[t - 1];
            for flop in &flops {
                if cancellation.is_cancelled() {
                    return Err("k-induction proof cancelled".into());
                }
                let q_now = lit[&flop.q];
                let d_previous = previous[&flop.d];
                match flop.reset {
                    Some((reset_previous, reset_high)) => {
                        let reset_active = if reset_high {
                            previous[&reset_previous]
                        } else {
                            -previous[&reset_previous]
                        };
                        cnf.add_clause(vec![-q_now, -reset_active]);
                        cnf.add_clause(vec![-q_now, reset_active, d_previous]);
                        cnf.add_clause(vec![q_now, reset_active, -d_previous]);
                    }
                    None => {
                        cnf.add_clause(vec![-q_now, d_previous]);
                        cnf.add_clause(vec![q_now, -d_previous]);
                    }
                }
                if let Some((reset_now, reset_high)) = flop.reset {
                    let active_now = if reset_high {
                        lit[&reset_now]
                    } else {
                        -lit[&reset_now]
                    };
                    cnf.add_clause(vec![-q_now, -active_now]);
                }
            }
        }
        if t < depth {
            let property = target_lit(nl, &mut cnf, &lit, &target)?;
            cnf.add_clause(vec![property]);
        }
        // Invariants have already been proved separately. Restricting every
        // induction candidate to them preserves soundness while removing the
        // unreachable states that commonly block plain k-induction.
        for invariant_target in invariant_targets {
            let invariant = target_lit(nl, &mut cnf, &lit, invariant_target)?;
            cnf.add_clause(vec![invariant]);
        }
        frames.push(lit);
    }
    let final_property = target_lit(nl, &mut cnf, &frames[depth], &target)?;
    let final_violation = -final_property;
    cnf.add_clause(vec![final_violation]);
    Ok((cnf, frames))
}

fn same_target(a: &BmcTarget, b: &BmcTarget) -> bool {
    match (a, b) {
        (BmcTarget::Net(anet, avalue), BmcTarget::Net(bnet, bvalue)) => {
            anet == bnet && avalue == bvalue
        }
        (BmcTarget::Expr(aexpr), BmcTarget::Expr(bexpr)) => aexpr == bexpr,
        _ => false,
    }
}

fn trace_from_model(
    nl: &Netlist,
    frames: &[HashMap<NetId, i32>],
    model: &[bool],
) -> Vec<HashMap<String, u64>> {
    frames
        .iter()
        .map(|frame| {
            let mut values = HashMap::new();
            // Unlike a BMC counterexample, an inductive witness should expose
            // the state observations that make the candidate transition fail;
            // including every port also reports sequential outputs.
            for port in &nl.ports {
                let mut value = 0u64;
                for (bit, net) in port.bits.iter().enumerate() {
                    let lit = frame[net];
                    if model[(lit - 1) as usize] {
                        value |= 1 << bit;
                    }
                }
                values.insert(port.name.clone(), value);
            }
            values
        })
        .collect()
}

pub fn report(result: &ProofResult) -> String {
    match result.status {
        ProofStatus::Proved => {
            let invariants = if result.assumed_invariants.is_empty() {
                String::new()
            } else {
                format!(
                    " using proved invariants [{}]",
                    result
                        .assumed_invariants
                        .iter()
                        .map(ProofInvariant::display)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!(
                "Proof report: PROVED by {}-induction after safe BMC{}\n",
                result.depth_checked, invariants
            )
        }
        ProofStatus::Refuted => format!(
            "Proof report: REFUTED at bounded depth {}\n",
            result.depth_checked
        ),
        ProofStatus::Unknown => {
            if let Some(name) = &result.failed_invariant {
                let nested = result
                    .invariant_result
                    .as_ref()
                    .map(|r| format!(" ({:?})", r.status))
                    .unwrap_or_default();
                format!(
                    "Proof report: UNKNOWN; invariant '{}' could not be proved{}\n",
                    name, nested
                )
            } else {
                format!(
                    "Proof report: UNKNOWN; safe through depth {} but not inductive at k={}\n",
                    result.depth_checked, result.depth_checked
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eda_hdl::compile;
    use eda_synth::{synthesize, SynthOptions};

    fn netlist_of(src: &str) -> Netlist {
        let design = compile(src, None).unwrap();
        // Keep state bits symbolic rather than optimizing a constant next
        // assignment away; induction itself is exercised by the data-driven
        // suite against mapped library cells as well.
        synthesize(
            &design,
            &SynthOptions {
                optimize: false,
                map_lib: false,
            },
        )
        .unwrap()
        .netlist
    }

    #[test]
    fn k_induction_honors_cancellation() {
        let nl = netlist_of(
            "module m(input clk, d, output reg q, output bad); always @(posedge clk) q <= d; assign bad = q & d; endmodule",
        );
        let cancellation = FlowCancellation::new();
        cancellation.cancel();
        let options = BmcOptions {
            depth: 16,
            init_state_zero: true,
        };
        assert_eq!(
            prove_cancellable(&nl, "bad", &options, &cancellation).unwrap_err(),
            "k-induction proof cancelled"
        );
        assert_eq!(
            prove_expression_cancellable(
                &nl,
                &BoolExpr::Signal("bad".into()),
                &options,
                &cancellation
            )
            .unwrap_err(),
            "k-induction proof cancelled"
        );
    }

    #[test]
    fn async_reset_property_is_inductive() {
        let src = "module m(input clk, d, rst, output reg q, output bad);
            always @(posedge clk or posedge rst) if (rst) q <= 1'b0; else q <= d;
            assign bad = q & rst;
         endmodule";
        let nl = netlist_of(src);
        let result = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 8,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Proved, "{:?}", result.bmc);
    }

    #[test]
    fn reachable_property_is_refuted() {
        let src = "module m(input clk, d, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & d;
         endmodule";
        let nl = netlist_of(src);
        let result = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 3,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Refuted);
    }

    #[test]
    fn unreachable_candidate_state_is_unknown() {
        let src = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let nl = netlist_of(src);
        let result = prove(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
        )
        .unwrap();
        assert!(!result.bmc.violated);
        assert_eq!(result.status, ProofStatus::Unknown);
        assert!(!result.inductive);
        assert!(result.counterexample.is_none());
        let witness = result.inductive_witness.expect("non-inductive witness");
        assert_eq!(witness.len(), 5);
        assert_eq!(witness[4].get("held"), Some(&1));
        assert_eq!(witness[4].get("bad"), Some(&1));
    }

    #[test]
    fn proved_invariant_strengthens_induction() {
        let src = "module m(input clk, enable, output reg held, output bad);
            always @(posedge clk) held <= held;
            assign bad = held & enable;
         endmodule";
        let nl = netlist_of(src);
        let result = prove_with_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &["held"],
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Proved, "{:?}", result.bmc);
        assert_eq!(
            result.assumed_invariants,
            vec![ProofInvariant::new("held", false)]
        );
        assert_eq!(result.failed_invariant, None);
        assert!(result.invariant_result.is_none());
    }

    #[test]
    fn unproved_invariant_is_rejected_soundly() {
        let src = "module m(input clk, d, enable, output reg q, output bad);
            always @(posedge clk) q <= d;
            assign bad = q & enable;
         endmodule";
        let nl = netlist_of(src);
        let result = prove_with_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &["q"],
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Unknown);
        assert!(!result.inductive);
        assert_eq!(result.failed_invariant.as_deref(), Some("q"));
        assert!(result.invariant_result.is_some());
        assert!(result.counterexample.is_none());
    }

    #[test]
    fn true_valued_invariant_strengthens_induction() {
        let src = "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
        let nl = netlist_of(src);
        let result = prove_with_value_invariants(
            &nl,
            "bad",
            &BmcOptions {
                depth: 4,
                init_state_zero: true,
            },
            &[("safe", true)],
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Proved, "{:?}", result.bmc);
        assert_eq!(
            result.assumed_invariants,
            vec![ProofInvariant::new("safe", true)]
        );
    }

    #[test]
    fn boolean_expression_tautology_is_proved() {
        let src = "module m(input a, output y); assign y = a; endmodule";
        let nl = netlist_of(src);
        let property = BoolExpr::Or(
            Box::new(BoolExpr::Signal("a".into())),
            Box::new(BoolExpr::Not(Box::new(BoolExpr::Signal("a".into())))),
        );
        let result = prove_expression(&nl, &property, &BmcOptions::default()).unwrap();
        assert_eq!(result.status, ProofStatus::Proved);
    }

    #[test]
    fn boolean_expression_counterexample_is_refuted() {
        let src = "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
         endmodule";
        let nl = netlist_of(src);
        let property = BoolExpr::Not(Box::new(BoolExpr::And(
            Box::new(BoolExpr::Signal("q".into())),
            Box::new(BoolExpr::Signal("d".into())),
        )));
        let result = prove_expression(&nl, &property, &BmcOptions::default()).unwrap();
        assert_eq!(result.status, ProofStatus::Refuted);
        assert!(result.counterexample.is_some());
    }

    #[test]
    fn boolean_expression_missing_signal_is_rejected() {
        let src = "module m(input a, output y); assign y = a; endmodule";
        let nl = netlist_of(src);
        let result = prove_expression(
            &nl,
            &BoolExpr::Signal("missing".into()),
            &BmcOptions::default(),
        );
        match result {
            Err(e) if e.contains("missing") => {}
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn expression_proof_uses_verified_net_invariant() {
        let src = "module m(input clk, enable, output reg held);
            always @(posedge clk) held <= held;
         endmodule";
        let nl = netlist_of(src);
        let property = BoolExpr::Not(Box::new(BoolExpr::And(
            Box::new(BoolExpr::Signal("held".into())),
            Box::new(BoolExpr::Signal("enable".into())),
        )));
        let opts = BmcOptions {
            depth: 4,
            init_state_zero: true,
        };
        assert_eq!(
            prove_expression(&nl, &property, &opts).unwrap().status,
            ProofStatus::Unknown
        );
        let result =
            prove_expression_with_value_invariants(&nl, &property, &opts, &[("held", false)])
                .unwrap();
        assert_eq!(result.status, ProofStatus::Proved, "{:?}", result.bmc);
        assert_eq!(
            result.assumed_invariants,
            vec![ProofInvariant::new("held", false)]
        );
    }

    #[test]
    fn expression_proof_uses_verified_expression_invariant() {
        let src = "module m(input clk, enable, output reg held, output safe, output bad);
            always @(posedge clk) held <= held;
            assign safe = ~held;
            assign bad = held & enable;
         endmodule";
        let nl = netlist_of(src);
        let property = BoolExpr::Not(Box::new(BoolExpr::Signal("bad".into())));
        let invariant = BoolExpr::Signal("safe".into());
        let opts = BmcOptions {
            depth: 4,
            init_state_zero: true,
        };
        assert_eq!(
            prove_expression(&nl, &property, &opts).unwrap().status,
            ProofStatus::Unknown
        );
        let result = prove_expression_with_expression_invariants(
            &nl,
            &property,
            &opts,
            &[("safe_is_one", &invariant)],
        )
        .unwrap();
        assert_eq!(result.status, ProofStatus::Proved, "{:?}", result.bmc);
        assert_eq!(
            result.assumed_invariants,
            vec![ProofInvariant::expression("safe_is_one", invariant.clone())]
        );
    }
}
