//! Data-driven regression suite: 1000+ named circuit/signal test cases
//! across every layer of the toolchain (logic, netlist, library, HDL,
//! simulation, synthesis, formal, STA, PnR, API/CLI).

pub mod ams_cases;
pub mod api_cases;
pub mod assertion_cases;
pub mod cdc_cases;
pub mod coverage_cases;
pub mod cpu8086_cases;
pub mod cts_cases;
pub mod dft_cases;
pub mod eco_cases;
pub mod fault_cases;
pub mod flow_cache_cases;
pub mod formal_cases;
pub mod hdl_cases;
pub mod ir_cases;
pub mod layout_cases;
pub mod liberty_cases;
pub mod library_cases;
pub mod logic_cases;
pub mod lvs_cases;
pub mod netlist_cases;
pub mod platform_cases;
pub mod pnr_cases;
pub mod power_cases;
pub mod rv32i_cases;
pub mod schematic_cases;
pub mod sdf_cases;
pub mod sim_comb;
pub mod sim_misc;
pub mod sim_seq;
pub mod spef_cases;
pub mod sta_cases;
pub mod synth_cases;
pub mod upf_cases;
pub mod waveform_cases;

/// One named test case with its check.
pub struct Case {
    pub name: String,
    pub run: Box<dyn FnOnce() -> Result<(), String>>,
}

pub struct CaseList(pub Vec<Case>);

impl Default for CaseList {
    fn default() -> Self {
        Self::new()
    }
}

impl CaseList {
    pub fn new() -> Self {
        CaseList(Vec::new())
    }

    pub fn add(
        &mut self,
        name: impl Into<String>,
        f: impl FnOnce() -> Result<(), String> + 'static,
    ) {
        self.0.push(Case {
            name: name.into(),
            run: Box::new(f),
        });
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Run every case; return (total count, failures with messages).
    /// Panics inside a case are caught and reported as failures.
    pub fn run_all(self) -> (usize, Vec<String>) {
        let total = self.0.len();
        let mut failures = Vec::new();
        for c in self.0 {
            let name = c.name.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (c.run)()));
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("{}: {}", name, e)),
                Err(p) => {
                    let msg = p
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "panic".into());
                    failures.push(format!("{}: PANIC {}", name, msg));
                }
            }
        }
        (total, failures)
    }
}

/// Collect the full suite from every category.
pub fn all_cases() -> CaseList {
    let mut all = CaseList::new();
    logic_cases::register(&mut all);
    lvs_cases::register(&mut all);
    netlist_cases::register(&mut all);
    library_cases::register(&mut all);
    liberty_cases::register(&mut all);
    hdl_cases::register(&mut all);
    sim_comb::register(&mut all);
    sim_seq::register(&mut all);
    sim_misc::register(&mut all);
    synth_cases::register(&mut all);
    formal_cases::register(&mut all);
    sta_cases::register(&mut all);
    sdf_cases::register(&mut all);
    schematic_cases::register(&mut all);
    spef_cases::register(&mut all);
    pnr_cases::register(&mut all);
    power_cases::register(&mut all);
    waveform_cases::register(&mut all);
    api_cases::register(&mut all);
    ams_cases::register(&mut all);
    assertion_cases::register(&mut all);
    coverage_cases::register(&mut all);
    cdc_cases::register(&mut all);
    dft_cases::register(&mut all);
    fault_cases::register(&mut all);
    flow_cache_cases::register(&mut all);
    eco_cases::register(&mut all);
    upf_cases::register(&mut all);
    cpu8086_cases::register(&mut all);
    rv32i_cases::register(&mut all);
    cts_cases::register(&mut all);
    ir_cases::register(&mut all);
    layout_cases::register(&mut all);
    platform_cases::register(&mut all);
    all
}
