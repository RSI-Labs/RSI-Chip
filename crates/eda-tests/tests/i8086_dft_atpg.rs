//! Named i8086 DFT/ATPG gate.
//!
//! Full-core scan insertion is cheap (one mux per flop). Fault grading is
//! O(faults * cells), so the ATPG/coverage pass runs on the opcode decoder
//! (a representative control block) while scan insertion is validated on the
//! whole core.

use eda_core::library::CellLibrary;
use eda_core::netlist::Netlist;

const I8086_SOURCE: &str = concat!(
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    "\n",
    include_str!("../../../platform/rtl/i8086_full_core.sv"),
);

const OPCODE_DECODER: &str = include_str!("../../../platform/rtl/i8086_opcode_decoder.sv");

fn synthesize(source: &str, top: &str) -> Netlist {
    let rtl = eda_hdl::compile(source, Some(top)).expect("compile");
    eda_synth::synthesize_with_library(
        &rtl,
        &eda_synth::SynthOptions::default(),
        &CellLibrary::std(),
    )
    .expect("synthesize")
    .netlist
}

#[test]
fn i8086_full_core_scan_insertion_and_atpg_pass() {
    // 1. Full-core scan insertion and structural audit.
    let core = synthesize(I8086_SOURCE, "i8086_full_core");
    let scanned = eda_dft::insert_scan_chain(&core, &eda_dft::ScanOptions::default())
        .expect("insert i8086 scan chain");
    assert!(
        scanned.chain.len() > 400,
        "expected a large scan chain, got {}",
        scanned.chain.len()
    );
    eda_dft::check_scan_structure(
        &scanned.netlist,
        &scanned.chain,
        scanned.scan_en,
        scanned.scan_in,
        scanned.scan_out,
    )
    .expect("i8086 scan chain structure");
    let report = eda_dft::analyze(&scanned.netlist).expect("analyze scan netlist");
    eprintln!(
        "i8086 scan: chain={} flops, sequential={} scan_cells={} chains={}",
        scanned.chain.len(),
        report.sequential_cells,
        report.scan_cells,
        report.scan_chains
    );
    assert_eq!(
        report.unscanned_sequential.len(),
        0,
        "unscanned flops: {:?}",
        &report.unscanned_sequential[..report.unscanned_sequential.len().min(8)]
    );
    assert_eq!(report.scan_cells, report.sequential_cells);
    assert_eq!(report.scan_chains, 1);

    // 2. ATPG + stuck-at coverage on the opcode decoder.
    let decoder = synthesize(OPCODE_DECODER, "i8086_opcode_decoder");
    let options = eda_fault::PatternOptions {
        max_random_patterns: 24,
        // The decoder has 16 input bits; keep ATPG at 24 pseudo-random
        // vectors instead of 2^16 exhaustive combinations.
        exhaustive_input_limit: 8,
        ..Default::default()
    };
    let coverage =
        eda_fault::grade_automatic(&decoder, &options).expect("grade i8086 opcode decoder faults");
    eprintln!(
        "i8086 opcode-decoder ATPG: faults={} detected={} coverage={:.2}%",
        coverage.total_faults, coverage.detected_faults, coverage.coverage_percent
    );
    assert!(coverage.total_faults > 0, "no testable faults");
    assert!(
        coverage.coverage_percent >= 50.0,
        "ATPG coverage too low: {:.2}% ({}/{})",
        coverage.coverage_percent,
        coverage.detected_faults,
        coverage.total_faults
    );

    // 3. STIL round trip for the generated vectors.
    let stil =
        eda_fault::write_stil(&decoder, &coverage.patterns, "i8086_opcode").expect("write STIL");
    let parsed = eda_fault::parse_stil(&stil).expect("parse STIL");
    assert_eq!(parsed.vectors.len(), coverage.patterns.len());
}
