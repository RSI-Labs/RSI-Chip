//! # eda-formal — formal verification
//!
//! - [`sat`]: a CDCL SAT solver (watched literals, VSIDS, 1UIP learning,
//!   Luby restarts), validated against brute force on random CNFs.
//! - [`cnf`]: Tseitin encoding of gate netlists (generic + library cells).
//! - [`equiv`]: combinational and sequential equivalence checking via miter
//!   construction; flip-flops matched by name with next-state comparison.
//! - [`bmc`]: bounded model checking of `bad == 1` reachability with
//!   counterexample traces.
//! - [`induction`]: k-induction safety proofs over mapped sequential netlists.

pub mod bmc;
pub mod cnf;
pub mod equiv;
pub mod induction;
pub mod property;
pub mod sat;

pub use bmc::{
    bmc, bmc_cancellable, bmc_expression, bmc_expression_cancellable, BmcOptions, BmcResult,
};
pub use equiv::{check_equivalent, check_equivalent_cancellable, EquivResult};
pub use induction::{
    prove, prove_cancellable, prove_expression, prove_expression_cancellable,
    prove_expression_with_expression_invariants,
    prove_expression_with_expression_invariants_cancellable,
    prove_expression_with_proof_invariants, prove_expression_with_proof_invariants_cancellable,
    prove_expression_with_value_invariants, prove_expression_with_value_invariants_cancellable,
    prove_with_invariants, prove_with_invariants_cancellable, prove_with_value_invariants,
    prove_with_value_invariants_cancellable, report as proof_report, ProofInvariant, ProofResult,
    ProofStatus,
};
pub use property::{
    bmc_assertion, bmc_assertion_cancellable, bmc_assertions, bmc_assertions_cancellable,
    bmc_assertions_with_assumptions, bmc_assertions_with_assumptions_cancellable,
    property_bmc_report, replay_property_vcd, trace_input_vcd, trace_net_vcd, trace_property_vcd,
    verify_property_trace, PropertyBmcResult, PropertyBmcStatus, PropertyReplayResult,
};
pub use sat::{
    solve, solve_cancellable, solve_with_assumptions, solve_with_assumptions_cancellable, Cnf,
    IncrementalSolver, SatResult,
};
