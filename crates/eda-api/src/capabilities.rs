//! Machine-readable tool capability audit for agent-driven flows.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityStatus {
    Implemented,
    Partial,
    Missing,
}

impl CapabilityStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Partial => "partial",
            Self::Missing => "missing",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Capability {
    pub domain: &'static str,
    pub name: &'static str,
    pub status: CapabilityStatus,
    pub evidence: &'static str,
    pub limitation: &'static str,
}

const CAPABILITIES: &[Capability] = &[
    Capability {
        domain: "frontend",
        name: "verilog_parse_elaborate",
        status: CapabilityStatus::Implemented,
        evidence: "eda_hdl::compile, module hierarchy metadata, parameter packages/imports/scoped constants, local/package enum, packed struct, and packed union typedefs with member selection, simple local/package elaboration-inline functions including non-recursive nested-call inlining, recursion rejection, scalar, packed struct/union, fixed multidimensional unpacked-array locals, fixed multidimensional unpacked module nets and whole-vector/module-array/one-dimensional-slice port connections, constant-base/runtime-module-read/runtime-function-read/runtime-blocking-write/runtime-NBA-write/reversed-packed-module-function-continuous-read-write/scalar-runtime-packed-base-continuous-write/runtime-array-element-constant-packed-base-continuous-write/procedural-runtime-module-array-element-read-write-and-synthesis/runtime-function-array-element-read-write indexed part-selects, if/else assignment merging, exact/wildcard-case assignment merging, bounded canonical for-loop unrolling, constant-repeat unrolling, constant-condition bounded while-loop unrolling, shared-interface signal connectivity with explicit modport direction checks, fixed multidimensional interface/module arrays, and static virtual ports, immediate assertions and clocked assert/assume/cover properties with fixed/bounded-range delays, fixed/range bounded consecutive, goto, and nonconsecutive repetition, bounded fixed-delay two-sequence and/or composition, and disable-iff aborts, signed arithmetic/casts/literals, parameterized generate-if/generate-for/generate-case elaboration, SystemVerilog variable types/default widths, and four-state casez/casex wildcard matching",
        limitation: "targeted Verilog/SystemVerilog subset rather than a complete commercial language surface",
    },
    Capability {
        domain: "simulation",
        name: "digital_event_and_combinational",
        status: CapabilityStatus::Implemented,
        evidence: "eda_sim simulator plus signed arithmetic/shifts/division/modulo, procedural immediate assertions, clocked assert/assume/cover properties with fixed/bounded-range delays, fixed/range bounded consecutive, goto, and nonconsecutive repetition, bounded fixed-delay two-sequence and/or composition, disable-iff aborts, and named coverage hits, VCD edge, clock period/duty, pulse-width, and signal-comparison queries",
        limitation: "",
    },
    Capability {
        domain: "simulation",
        name: "spice_dc_ac_transient",
        status: CapabilityStatus::Implemented,
        evidence: "eda_ams MNA, phasor, BE/trapezoidal integration",
        limitation: "compact device subset",
    },
    Capability {
        domain: "simulation",
        name: "nonlinear_diode",
        status: CapabilityStatus::Implemented,
        evidence: "Newton solve with emission-coefficient continuation",
        limitation: "fixed temperature; shot noise but no temperature-dependent model",
    },
    Capability {
        domain: "simulation",
        name: "small_signal_noise",
        status: CapabilityStatus::Implemented,
        evidence: "thermal, shot, flicker, and input-referred noise",
        limitation: "no advanced foundry noise/correlation model",
    },
    Capability {
        domain: "simulation",
        name: "temperature_dependent_models",
        status: CapabilityStatus::Implemented,
        evidence: ".temp, SHEET/L/W, imported .rmodel layer tables, width-bin sheet/WOFF interpolation, and MOS mobility scaling",
        limitation: "no certified tool-specific foundry file, self-heating, or spatial thermal network",
    },
    Capability {
        domain: "simulation",
        name: "power_law_reliability_aging",
        status: CapabilityStatus::Implemented,
        evidence: ".aging power-law stress with resistor increase and MOS VTO/KP degradation",
        limitation: "no calibrated BTI/HCI physics, stochastic aging, or foundry lifetime model",
    },
    Capability {
        domain: "simulation",
        name: "monte_carlo_statistical",
        status: CapabilityStatus::Implemented,
        evidence: "deterministic seeded .mc with same-type, pairwise, covariance, imported-model, and DEF-derived spatial correlations",
        limitation: "DEF locations match by stable element name; no certified vendor file, automatic name mapping, or correlated stochastic aging",
    },
    Capability {
        domain: "simulation",
        name: "sensitivity_analysis",
        status: CapabilityStatus::Implemented,
        evidence: "central-difference .sens for OP/transient/AC RCL/source/model parameters",
        limitation: "local one-at-a-time derivatives",
    },
    Capability {
        domain: "simulation",
        name: "analytic_multidomain_derivatives",
        status: CapabilityStatus::Implemented,
        evidence: ".deriv adjoint/implicit/closed-form/tangent-recurrence gradients and full Hessians for linear DC, nonlinear diode DC/AC with transit-time and CJO/VJ/M/FC depletion capacitance, diode-connected ideal-NMOS DC, diode-connected ideal-NPN DC with AREA, linear AC magnitude, and backward-Euler/trapezoidal linear RCL transient-final targets",
        limitation: "nonlinear AC derivatives cover one source-R-diode path; MOS/BJT DC derivatives each cover one ideal diode-connected path; general transient derivatives require linear RCL/DC-source circuits; no time-varying-source adjoint Hessian",
    },
    Capability {
        domain: "simulation",
        name: "global_variance_sensitivity",
        status: CapabilityStatus::Implemented,
        evidence: "seeded .sobol with correlation-aware first/total variance indices",
        limitation: "correlation-aware indices use a local linear covariance surrogate",
    },
    Capability {
        domain: "simulation",
        name: "mosfet_bjt_behavioral",
        status: CapabilityStatus::Partial,
        evidence: "Level-1 MOS; BSIM3 LEVEL=8 core with VTH0/K1/K2/TOX/U0/UA/UB/UC/VSAT, DVT0/DVT1/DVT2 short-channel/DIBL reduction, DVT0W/DVT1W/W0 narrow-width threshold shift, KT1/KT2/KT1L/UTE temperature scaling, and WMIN/WMAX/LMIN/LMAX geometry-bin selection; local/global parameter sensitivity, MOS charge; Ebers-Moll BJT and diode/BJT transit charge",
        limitation: "BSIM3 uses simplified geometry and temperature subsets with rectangular W/L bins only; full DVT, full charge, NQS, self-heating, full noise, and PSP/HiSIM/VBIC/HICUM-class models are not implemented",
    },
    Capability {
        domain: "simulation",
        name: "bounded_global_optimization",
        status: CapabilityStatus::Implemented,
        evidence: "seeded .optim differential evolution with quantization and exact linear-DC/AC plus diode/MOS/BJT-DC adjoint Hessian polish, including BJT AREA",
        limitation: "analytic nonlinear Hessian polish covers DC diode IS and one ideal diode-connected NMOS/NPN path only; no nonlinear-AC or transient Hessian optimizer",
    },
    Capability {
        domain: "simulation",
        name: "pareto_multi_objective_optimization",
        status: CapabilityStatus::Implemented,
        evidence: "seeded .pareto differential evolution with 2-8 outputs, algebraic constraints, adaptive NSGA-III-style reference directions, and per-point netlists",
        limitation: "constraints have a fixed arithmetic/transcendental/min-max function set; no user-defined or differential constraint functions",
    },
    Capability {
        domain: "simulation",
        name: "periodic_steady_state",
        status: CapabilityStatus::Implemented,
        evidence: ".pss cycle convergence, harmonic spectrum, automatic frequency estimation, and autonomous ring-oscillator startup",
        limitation: "autonomous startup requires a user-supplied IC/source and full nonlinear/multi-tone harmonic balance is not implemented",
    },
    Capability {
        domain: "simulation",
        name: "envelope_following",
        status: CapabilityStatus::Implemented,
        evidence: ".env quadrature demodulation of transient carrier waveforms",
        limitation: "single carrier only; no autonomous envelope-step solver",
    },
    Capability {
        domain: "synthesis",
        name: "rtl_optimization_mapping",
        status: CapabilityStatus::Implemented,
        evidence: "eda_synth optimization, library mapping, Verilog writer, runtime-indexed scalar and unpacked-array-element blocking/NBA field-update mux/DFF lowering, and signed add/subtract/multiply/comparison/arithmetic-shift/cast lowering with RTL-versus-gate tests",
        limitation: "targeted RTL construct coverage rather than a complete commercial synthesis language surface",
    },
    Capability {
        domain: "formal",
        name: "equivalence_sat_bmc_induction",
        status: CapabilityStatus::Implemented,
        evidence: "eda_formal SAT engine and proof reports; CDCL propagation/search, generic bad-state/expression BMC, equivalence encoding/miter solving, k-induction proof/unrolling, and temporal-property unrolling accept cooperative cancellation tokens",
        limitation: "property language is a targeted subset",
    },
    Capability {
        domain: "timing",
        name: "sta_constraints_mmmc_waivers",
        status: CapabilityStatus::Implemented,
        evidence: "eda_sta constrained timing, generated clocks, clock groups, independent min/max clock transitions, design-wide/object-scoped transition/capacitance/fanout plus input-transition/load SDC quality rules, MMMC, violation registry",
        limitation: "not a full commercial SDC parser",
    },
    Capability {
        domain: "physical",
        name: "placement_routing_drc_def_lef",
        status: CapabilityStatus::Implemented,
        evidence: "eda_pnr placement, maze/rip-up routing, LEF/DEF writers, typed rectilinear polygon/via layout editing, exact Manhattan boolean operations, width/area/spacing/enclosure DRC, typed fix suggestions, arbitrary-angle polygon area/bounds/containment/exact-spacing primitives, arbitrary-angle layout-document hit-test/SVG/movement/DRC/editor-tool/vertex-drag integration, and direct arbitrary-angle two-pin routing",
        limitation: "arbitrary-angle Boolean operations and a general arbitrary-angle maze/rip-up router are absent, and a complete foundry rule deck is missing",
    },
    Capability {
        domain: "physical",
        name: "parasitic_sdf_back_annotation",
        status: CapabilityStatus::Implemented,
        evidence: "eda_spef and eda_sdf readers/writers and API annotation",
        limitation: "",
    },
    Capability {
        domain: "power",
        name: "power_intent_ir_em_mesh",
        status: CapabilityStatus::Implemented,
        evidence: "eda_upf, eda_ir, eda_power, coupled mesh APIs",
        limitation: "",
    },
    Capability {
        domain: "verification",
        name: "sva_coverage_cdc_dft_fault",
        status: CapabilityStatus::Partial,
        evidence: "eda_assert, coverage, CDC, DFT and fault crates",
        limitation: "language/device coverage remains narrower than commercial tools",
    },
    Capability {
        domain: "integration",
        name: "stable_rust_api_facade",
        status: CapabilityStatus::Implemented,
        evidence: "eda-api re-exports one-call flows and typed results",
        limitation: "API is pre-1.0 and may still evolve",
    },
    Capability {
        domain: "integration",
        name: "cli_agent_surface",
        status: CapabilityStatus::Implemented,
        evidence: "eda CLI including ams, flow, signoff reports, and daemon startup",
        limitation: "",
    },
    Capability {
        domain: "integration",
        name: "persistent_agent_daemon",
        status: CapabilityStatus::Partial,
        evidence: "line-delimited JSON TCP RPC with ping/capabilities/SPICE-DC/all-AMS-analysis/functional-coverage/waveform-assertions/waveform-clock-pulse-comparison/layout-polygon-via-DRC/RTL-equivalence/RTL-standalone-STA/RTL-standalone-PnR/RTL-DFT-scan-insertion/RTL-CTS-clock-tree-synthesis/RTL-structural-ECO/RTL-CDC/RTL-combinational-stuck-at-fault-grading-STIL/RTL-timed-simulation/RTL-power/RTL-power-grid-IR-EM/RTL-temporal-property-BMC/RTL-flow-with-structured-STA-PnR-DRC/cached-RTL-flow/cache-stats/cache-clear/PDK-signoff plus remote jobSubmit/jobStatus/jobCancel/jobWait, typed asynchronous job manager with service-owned flow-cache dispatch, cancellation-before-start, cooperative RTL/PDK stage, SAT/BMC/equivalence/k-induction search, simulator delta scheduling, global/maze/rip-up routing, single-corner physical-timing extraction/STA/antenna/hold/optimization stages, and AMS transient/Monte-Carlo/Sobol/PSS/optimizer/quantized-refine loop cancellation, and concurrent clients",
        limitation: "daemon exposes RTL, AMS, waveform/layout, and PDK signoff flows but not every specialized toolchain API; cancellation is not yet supported inside every nested multi-corner physical-timing stage",
    },
    Capability {
        domain: "integration",
        name: "bounded_regression_cache",
        status: CapabilityStatus::Implemented,
        evidence: "content-addressed source/top/library flow keys, bounded LRU flow cache, hit/miss/insert/eviction statistics, and mutex-shared concurrent agent cache",
        limitation: "the cache is shared inside one Rust daemon process only; a distributed/cloud backend is not bundled",
    },
    Capability {
        domain: "user_experience",
        name: "schematic_layout_gui",
        status: CapabilityStatus::Partial,
        evidence: "dependency-free schematic document database with symbols, transforms, wires, ports, hit testing, net extraction, SVG rendering, undo/redo, pointer-driven select/drag/wire/delete session events, bundled schematic/layout browser hosts with zoom/pan controls and typed selection property grids, plus rectangle/polygon/via placement, polygon vertex dragging, DRC events, and SVG rendering",
        limitation: "no native desktop shell is bundled and browser layout editing remains focused on the typed Manhattan geometry subset",
    },
    Capability {
        domain: "signoff",
        name: "foundry_calibrated_pdk_signoff",
        status: CapabilityStatus::Partial,
        evidence: "typed PDK manifests with layers, PVT-tagged delay-scale corners, checksummed inline per-corner Liberty views, model correlations, and required checks; deterministic per-corner RTL-to-equivalence/timing/DRC/LEF-DEF-SDF MMMC signoff",
        limitation: "manifest is portable but not certified by an external foundry; Liberty SHA-256 detects accidental corruption but does not establish foundry provenance, and advanced compact-model correlation is not vendor-calibrated",
    },
];

pub fn capabilities() -> &'static [Capability] {
    CAPABILITIES
}

pub fn capability_report() -> String {
    let mut out = String::from("EDA capability audit\n");
    let mut implemented = 0;
    let mut partial = 0;
    let mut missing = 0;
    for capability in CAPABILITIES {
        match capability.status {
            CapabilityStatus::Implemented => implemented += 1,
            CapabilityStatus::Partial => partial += 1,
            CapabilityStatus::Missing => missing += 1,
        }
        out.push_str(&format!(
            "{:<12} {:<34} {:<12} {}\n",
            capability.domain,
            capability.name,
            capability.status.as_str(),
            if capability.limitation.is_empty() {
                capability.evidence
            } else {
                capability.limitation
            }
        ));
    }
    out.push_str(&format!(
        "summary: implemented={}, partial={}, missing={}\n",
        implemented, partial, missing
    ));
    out
}

pub fn missing_capabilities() -> Vec<&'static Capability> {
    CAPABILITIES
        .iter()
        .filter(|capability| capability.status == CapabilityStatus::Missing)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_is_nonempty_and_counts_match() {
        assert!(!CAPABILITIES.is_empty());
        assert_eq!(missing_capabilities().len(), 0);
        let report = capability_report();
        assert!(report.contains("spice_dc_ac_transient") && report.contains("implemented"));
        assert!(report.contains("summary: implemented="));
    }
}
