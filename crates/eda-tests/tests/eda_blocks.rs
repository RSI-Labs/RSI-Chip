//! End-to-end EDA block regression:
//! RTL simulation, RTL-derived LEC against the mapped netlist, mapped
//! gate-level simulation, DRC, and SDF generation for the reference blocks.

use eda_api::Flow;
use eda_core::cancellation::FlowCancellation;
use eda_core::ir::Design;
use eda_core::library::CellLibrary;
use eda_core::logic::LogicWord;
use eda_core::to_design::netlist_to_design;
use eda_pnr::RouteOptions;
use eda_sim::Simulator;

const COUNTER: &str = include_str!("../../../platform/rtl/blocks/counter8.sv");
const SHIFT: &str = include_str!("../../../platform/rtl/blocks/shift_register32.sv");
const ALU: &str = include_str!("../../../platform/rtl/blocks/alu8.sv");
const UART: &str = include_str!("../../../platform/rtl/blocks/uart_tx.sv");
const SPI: &str = include_str!("../../../platform/rtl/blocks/spi_master.sv");
const PWM: &str = include_str!("../../../platform/rtl/blocks/pwm.sv");
const LFSR: &str = include_str!("../../../platform/rtl/blocks/lfsr8.sv");

struct Bench<'a> {
    design: &'a Design,
    sim: Simulator<'a>,
    now: u64,
    step: u64,
}

impl<'a> Bench<'a> {
    fn new(design: &'a Design) -> Self {
        let mut sim = Simulator::new(design).expect("simulator");
        if design.port("clk").is_some() {
            sim.set_input("clk", &LogicWord::from_u64(1, 0)).unwrap();
        }
        Self {
            design,
            sim,
            now: 0,
            step: 1,
        }
    }

    fn with_step(design: &'a Design, step: u64) -> Self {
        let mut bench = Self::new(design);
        bench.step = step;
        bench
    }

    fn set(&mut self, name: &str, value: u64) {
        let width = self
            .design
            .port(name)
            .unwrap_or_else(|| panic!("missing port {name}"))
            .width;
        self.sim
            .set_input(name, &LogicWord::from_u64(width, value))
            .unwrap();
    }

    fn get(&self, name: &str) -> u64 {
        self.sim
            .read_port(name)
            .and_then(|word| word.to_u64())
            .unwrap_or(u64::MAX)
    }

    fn advance(&mut self) {
        self.now += self.step;
        self.sim.run_until(self.now).unwrap();
    }

    fn tick(&mut self) {
        self.set("clk", 0);
        self.advance();
        self.set("clk", 1);
        self.advance();
        self.set("clk", 0);
        self.advance();
    }

    fn settle(&mut self) {
        self.advance();
    }
}

fn compile(src: &str, top: &str) -> Design {
    eda_api::compile(src, Some(top)).unwrap_or_else(|error| panic!("compile {top}: {error}"))
}

/// Run LEC against the mapped netlist, the full physical flow, and return it.
fn lec_and_flow(src: &str, top: &str) -> Flow {
    let lec = eda_api::check_rtl_equivalence(src, Some(top))
        .unwrap_or_else(|error| panic!("{top}: RTL-vs-mapped LEC failed to run: {error}"));
    assert!(
        lec.equivalent,
        "{top}: RTL-vs-mapped LEC is not equivalent: {:?}",
        lec.counterexample
    );
    let flow = eda_api::run_flow_with_rtl_reference_lec(
        src,
        Some(top),
        &CellLibrary::std(),
        &RouteOptions::default(),
        &FlowCancellation::new(),
    )
    .unwrap_or_else(|error| panic!("{top}: physical flow failed: {error}"));
    assert!(flow.equivalent, "{top}: flow LEC is not equivalent");
    assert!(
        !flow.drc.has_errors(),
        "{top}: DRC errors: {}",
        flow.drc.violations.len()
    );
    assert!(!flow.sdf.trim().is_empty(), "{top}: empty SDF");
    // The SDF must parse and round-trip through the delay-file reader.
    let sdf = eda_sdf::parse_sdf(&flow.sdf).expect("parse generated SDF");
    assert!(!sdf.iopaths.is_empty(), "{top}: SDF has no IOPATHs");
    flow
}

/// Compare an RTL trace with the mapped-gate trace and return it.
fn assert_rtl_and_gate_match(src: &str, top: &str, run: fn(&Design) -> Vec<u64>) -> Vec<u64> {
    let flow = lec_and_flow(src, top);
    let rtl = compile(src, top);
    let rtl_trace = run(&rtl);
    let gate_design = netlist_to_design(&flow.netlist);
    let gate_trace = run(&gate_design);
    assert_eq!(
        rtl_trace, gate_trace,
        "{top}: mapped gate-level trace differs from RTL"
    );
    rtl_trace
}

// ---------------------------------------------------------------------------
// 8-bit counter
// ---------------------------------------------------------------------------

fn trace_counter(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("en", 0);
    b.set("up", 1);
    b.set("load", 0);
    b.set("load_value", 0);
    b.tick();
    b.tick();
    let mut trace = Vec::new();
    trace.push(b.get("count"));
    assert_eq!(b.get("count"), 0);
    assert_eq!(b.get("zero"), 1);

    b.set("rst", 0);
    b.set("en", 1);
    b.set("up", 1);
    for _ in 0..5 {
        b.tick();
        trace.push(b.get("count"));
        trace.push(b.get("carry"));
        trace.push(b.get("zero"));
    }
    assert_eq!(b.get("count"), 5);

    b.set("load", 1);
    b.set("load_value", 0x80);
    b.tick();
    trace.push(b.get("count"));
    assert_eq!(b.get("count"), 0x80);
    b.set("load", 0);

    b.set("en", 0);
    b.tick();
    assert_eq!(b.get("count"), 0x80);

    b.set("en", 1);
    b.set("up", 0);
    for _ in 0..3 {
        b.tick();
        trace.push(b.get("count"));
        trace.push(b.get("carry"));
    }
    assert_eq!(b.get("count"), 0x7d);
    b.set("load", 1);
    b.set("load_value", 0x00);
    b.tick();
    b.set("load", 0);
    assert_eq!(b.get("count"), 0x00);
    assert_eq!(b.get("zero"), 1);
    b.set("en", 1);
    b.set("up", 0);
    b.settle();
    trace.push(b.get("carry"));
    assert_eq!(
        b.get("carry"),
        1,
        "carry must assert before the underflow edge"
    );
    b.tick();
    trace.push(b.get("count"));
    assert_eq!(b.get("count"), 0xff);
    trace
}

#[test]
fn counter8_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(COUNTER, "counter8", trace_counter);
}

#[test]
fn counter8_sdf_annotated_gate_runs() {
    let flow = lec_and_flow(COUNTER, "counter8");
    let sdf = eda_sdf::parse_sdf(&flow.sdf).expect("parse SDF");
    let annotated = eda_sdf::annotate_sdf(&flow.netlist, &sdf).expect("annotate SDF");
    let timed = eda_core::to_design::netlist_to_design_with_timing(&annotated);
    // Give the SDF-delayed combinational next-state a full clock period to
    // settle before each edge.
    let mut b = Bench::with_step(&timed, 1_000);
    b.set("rst", 1);
    b.set("en", 0);
    b.set("up", 1);
    b.set("load", 0);
    b.set("load_value", 0);
    b.tick();
    b.tick();
    assert_eq!(b.get("count"), 0, "reset did not clear the SDF counter");
    b.set("rst", 0);
    b.set("en", 1);
    b.set("up", 1);
    for cycle in 0..10 {
        b.tick();
        assert_eq!(
            b.get("count"),
            cycle + 1,
            "SDF-annotated counter diverged at cycle {cycle}"
        );
    }
    // Programmable load under SDF timing.
    b.set("load", 1);
    b.set("load_value", 0x80);
    b.tick();
    b.set("load", 0);
    assert_eq!(b.get("count"), 0x80);
    // Down-count through zero asserts carry exactly at the underflow.
    b.set("up", 0);
    b.set("en", 1);
    b.set("load", 1);
    b.set("load_value", 0);
    b.tick();
    b.set("load", 0);
    b.settle();
    assert_eq!(b.get("carry"), 1, "SDF counter underflow carry");
    b.tick();
    assert_eq!(b.get("count"), 0xff);
    assert_eq!(
        b.sim.severity_hits(eda_core::ir::SimulationSeverity::Error),
        0,
        "SDF-annotated counter reported a setup/hold violation"
    );
}

// ---------------------------------------------------------------------------
// 32-bit shift register
// ---------------------------------------------------------------------------

fn trace_shift(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("en", 0);
    b.set("load", 0);
    b.set("left", 0);
    b.set("arithmetic", 0);
    b.set("serial_in", 0);
    b.set("load_value", 0);
    b.tick();
    b.tick();
    let mut trace = Vec::new();
    assert_eq!(b.get("q"), 0);

    b.set("rst", 0);
    b.set("load", 1);
    b.set("load_value", 0x1234_5678);
    b.tick();
    trace.push(b.get("q"));
    assert_eq!(b.get("q"), 0x1234_5678);
    b.set("load", 0);

    b.set("en", 1);
    b.set("left", 1);
    b.set("serial_in", 1);
    b.tick();
    trace.push(b.get("q"));
    trace.push(b.get("serial_out"));
    assert_eq!(b.get("q"), 0x2468_acf1);
    assert_eq!(b.get("serial_out"), 0);

    b.set("left", 0);
    b.set("arithmetic", 1);
    b.set("load", 1);
    b.set("load_value", 0x8000_0001);
    b.tick();
    b.set("load", 0);
    b.set("en", 1);
    b.tick();
    trace.push(b.get("q"));
    assert_eq!(b.get("q"), 0xc000_0000);

    b.set("arithmetic", 0);
    b.tick();
    trace.push(b.get("q"));
    assert_eq!(b.get("q"), 0x6000_0000);
    trace
}

#[test]
fn shift_register32_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(SHIFT, "shift_register32", trace_shift);
}

// ---------------------------------------------------------------------------
// 8-bit ALU
// ---------------------------------------------------------------------------

fn alu_expected(op: u64, a: u64, b: u64) -> (u64, u64, u64, u64) {
    let a8 = a & 0xff;
    let b8 = b & 0xff;
    let full = if op == 0 {
        a8 + b8
    } else if op == 1 {
        (a8 as i64 - b8 as i64) as u64
    } else {
        0
    };
    let y = match op {
        0 => full & 0xff,
        1 => (a8.wrapping_sub(b8)) & 0xff,
        2 => a8 & b8,
        3 => a8 | b8,
        4 => a8 ^ b8,
        5 => (!a8) & 0xff,
        6 => (a8 << (b8 & 7)) & 0xff,
        _ => (a8 >> (b8 & 7)) & 0xff,
    };
    let zero = u64::from(y == 0);
    let negative = (y >> 7) & 1;
    let carry = if op == 0 {
        (full >> 8) & 1
    } else if op == 1 {
        // Borrow: a < b for the low byte.
        u64::from(a8 < b8)
    } else {
        0
    };
    let overflow = if op == 0 {
        u64::from(((a8 ^ b8) & 0x80) == 0 && ((a8 ^ y) & 0x80) != 0)
    } else if op == 1 {
        u64::from(((a8 ^ b8) & 0x80) != 0 && ((a8 ^ y) & 0x80) != 0)
    } else {
        0
    };
    (y, zero, negative, carry | (overflow << 1))
}

fn trace_alu(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    let mut trace = Vec::new();
    for op in 0..8u64 {
        for (av, bv) in [
            (0x00u64, 0x00u64),
            (0xff, 0x01),
            (0x80, 0x80),
            (0x55, 0xaa),
            (0x0f, 0x03),
            (0x7f, 0x01),
        ] {
            b.set("op", op);
            b.set("a", av);
            b.set("b", bv);
            b.settle();
            let (y, zero, negative, carry_overflow) = alu_expected(op, av, bv);
            assert_eq!(b.get("y"), y, "ALU op {op} a={av:#04x} b={bv:#04x}");
            assert_eq!(b.get("zero"), zero);
            assert_eq!(b.get("negative"), negative);
            assert_eq!(b.get("carry"), carry_overflow & 1);
            assert_eq!(b.get("overflow"), (carry_overflow >> 1) & 1);
            trace.extend([
                b.get("y"),
                b.get("zero"),
                b.get("negative"),
                b.get("carry"),
                b.get("overflow"),
            ]);
        }
    }
    trace
}

#[test]
fn alu8_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(ALU, "alu8", trace_alu);
}

// ---------------------------------------------------------------------------
// UART transmitter
// ---------------------------------------------------------------------------

fn trace_uart(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("start", 0);
    b.set("data", 0);
    b.tick();
    b.tick();
    let mut tx = Vec::new();
    b.set("rst", 0);
    b.set("data", 0xa5);
    b.set("start", 1);
    b.tick();
    b.set("start", 0);
    let mut saw_done = false;
    for _ in 0..400 {
        b.tick();
        tx.push(b.get("tx"));
        if b.get("done") == 1 {
            saw_done = true;
            break;
        }
    }
    assert!(saw_done, "UART TX did not assert done");
    assert_eq!(b.get("busy"), 0);
    // Decode the first frame: start (0), 8 data bits LSB first, stop (1).
    let start = tx.iter().position(|bit| *bit == 0).expect("no start bit");
    let sample = |index: usize| tx[start + 16 * index + 8];
    assert_eq!(sample(0), 0, "start bit");
    let mut value = 0u64;
    for bit in 0..8 {
        value |= sample(bit + 1) << bit;
    }
    assert_eq!(value, 0xa5, "decoded UART byte");
    assert_eq!(sample(9), 1, "stop bit");
    tx
}

#[test]
fn uart_tx_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(UART, "uart_tx", trace_uart);
}

// ---------------------------------------------------------------------------
// SPI master
// ---------------------------------------------------------------------------

fn trace_spi(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("start", 0);
    b.set("tx_data", 0);
    b.set("miso", 0);
    b.tick();
    b.tick();
    b.set("rst", 0);
    b.set("tx_data", 0x5a);
    // Slave response byte, MSB first. The master must sample MISO on each
    // rising SCLK edge (SPI mode 0).
    let rx_bits = [1u64, 1, 0, 0, 0, 0, 1, 1]; // 0xc3
    let mut rx_index = 0usize;
    b.set("miso", rx_bits[0]);
    b.set("start", 1);
    b.tick();
    b.set("start", 0);
    let mut mosi_bits = Vec::new();
    let mut previous_sclk = b.get("sclk");
    let mut saw_done = false;
    for _ in 0..400 {
        b.tick();
        let sclk = b.get("sclk");
        if previous_sclk == 0 && sclk == 1 {
            mosi_bits.push(b.get("mosi"));
            // Advance the slave response for the next sampled bit.
            rx_index += 1;
            if rx_index < rx_bits.len() {
                b.set("miso", rx_bits[rx_index]);
            }
        }
        previous_sclk = sclk;
        if b.get("done") == 1 {
            saw_done = true;
            break;
        }
    }
    assert!(saw_done, "SPI master did not assert done");
    assert_eq!(mosi_bits.len(), 8, "SPI MOSI bit count");
    let mut sent = 0u64;
    for bit in &mosi_bits {
        sent = (sent << 1) | bit;
    }
    assert_eq!(sent, 0x5a, "SPI MOSI data (MSB first)");
    assert_eq!(b.get("rx_data"), 0xc3, "SPI MISO sampled data (MSB first)");
    assert_eq!(b.get("busy"), 0);
    mosi_bits
}

#[test]
fn spi_master_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(SPI, "spi_master", trace_spi);
}

// ---------------------------------------------------------------------------
// PWM
// ---------------------------------------------------------------------------

fn trace_pwm(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("en", 0);
    b.set("duty", 50);
    b.set("period", 100);
    b.tick();
    b.tick();
    b.set("rst", 0);
    b.set("en", 1);
    let mut trace = Vec::new();
    let mut highs = 0u64;
    for _ in 0..200 {
        b.tick();
        let out = b.get("pwm_out");
        highs += out;
        trace.push(out);
    }
    // Two full 100-cycle periods with 50% duty.
    assert_eq!(highs, 100, "PWM duty cycle over two periods");
    trace
}

#[test]
fn pwm_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(PWM, "pwm", trace_pwm);
}

// ---------------------------------------------------------------------------
// LFSR
// ---------------------------------------------------------------------------

fn lfsr_next(state: u64) -> u64 {
    let feedback = ((state >> 7) ^ (state >> 5) ^ (state >> 4) ^ (state >> 3)) & 1;
    ((state << 1) & 0xff) | feedback
}

fn trace_lfsr(design: &Design) -> Vec<u64> {
    let mut b = Bench::new(design);
    b.set("rst", 1);
    b.set("en", 0);
    b.set("load", 0);
    b.set("seed", 0);
    b.tick();
    b.tick();
    let mut trace = Vec::new();
    assert_eq!(b.get("state"), 1);
    b.set("rst", 0);
    b.set("en", 1);
    let mut expected = 1u64;
    for _ in 0..64 {
        expected = lfsr_next(expected);
        b.tick();
        trace.push(b.get("state"));
        trace.push(b.get("bit_out"));
        assert_eq!(b.get("state"), expected, "LFSR state");
        assert_eq!(b.get("bit_out"), (expected >> 7) & 1);
    }
    // Reload a seed and verify the next value.
    b.set("load", 1);
    b.set("seed", 0x5a);
    b.tick();
    b.set("load", 0);
    trace.push(b.get("state"));
    assert_eq!(b.get("state"), 0x5a);
    b.tick();
    trace.push(b.get("state"));
    assert_eq!(b.get("state"), lfsr_next(0x5a));
    trace
}

#[test]
fn lfsr8_design_flow_lec_and_gate_match() {
    assert_rtl_and_gate_match(LFSR, "lfsr8", trace_lfsr);
}
