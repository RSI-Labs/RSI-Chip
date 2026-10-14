//! Scan-chain insertion and shift/function-mode regression cases.

use crate::CaseList;
use eda_core::logic::LogicWord as LW;
use eda_core::netlist::Netlist;
use eda_core::to_design::netlist_to_design;
use eda_sim::Simulator;

fn synth(src: &'static str) -> Netlist {
    let design = eda_hdl::compile(src, None).expect("compile");
    eda_synth::synthesize(
        &design,
        &eda_synth::SynthOptions {
            optimize: true,
            map_lib: true,
        },
    )
    .expect("synthesis")
    .netlist
}

fn flop_netlist() -> Netlist {
    synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule")
}

fn counter_netlist() -> Netlist {
    synth(
        "module m(input clk, rst, output reg [2:0] q);
            always @(posedge clk or posedge rst) if (rst) q <= 3'd0; else q <= q + 3'd1;
         endmodule",
    )
}

pub fn register(c: &mut CaseList) {
    c.add("dft_analyze_unscanned_design", || {
        let report = eda_dft::analyze(&flop_netlist())?;
        if report.sequential_cells == 1
            && report.scan_cells == 0
            && report.unscanned_sequential.len() == 1
        {
            Ok(())
        } else {
            Err(format!("{:?}", report))
        }
    });

    c.add("dft_insert_single_scan_cell", || {
        let nl = flop_netlist();
        let result = eda_dft::insert_scan_chain(&nl, &eda_dft::ScanOptions::default())?;
        if result.chain.len() != 1 || result.netlist.cell_count() != nl.cell_count() + 2 {
            Err(format!(
                "chain={} cells={} original={}",
                result.chain.len(),
                result.netlist.cell_count(),
                nl.cell_count()
            ))
        } else if result.netlist.port_by_name("scan_en").is_none()
            || result.netlist.port_by_name("scan_in").is_none()
            || result.netlist.port_by_name("scan_out").is_none()
        {
            Err("scan ports missing".into())
        } else {
            Ok(())
        }
    });

    c.add("dft_functional_mode_preserves_capture", || {
        let scanned =
            eda_dft::insert_scan_chain(&flop_netlist(), &eda_dft::ScanOptions::default())?;
        let design = netlist_to_design(&scanned.netlist);
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("scan_en", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        for d in [0u64, 1, 0, 1] {
            sim.set_input("d", &LW::from_u64(1, d))
                .map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 0))
                .map_err(|e| e.to_string())?;
            sim.set_input("clk", &LW::from_u64(1, 1))
                .map_err(|e| e.to_string())?;
            if sim.read_port("q").unwrap().to_u64() != Some(d) {
                return Err(format!("functional capture d={} failed", d));
            }
        }
        Ok(())
    });

    c.add("dft_shift_mode_moves_scan_value", || {
        let scanned =
            eda_dft::insert_scan_chain(&flop_netlist(), &eda_dft::ScanOptions::default())?;
        let design = netlist_to_design(&scanned.netlist);
        let mut sim = Simulator::new(&design).map_err(|e| e.to_string())?;
        sim.set_input("scan_en", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("scan_in", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        if sim.read_port("q").unwrap().to_u64() != Some(1) {
            return Err("flop did not load scan_in".into());
        }
        if sim.read_port("scan_out").unwrap().to_u64() != Some(1) {
            return Err("scan_out did not observe captured value".into());
        }
        sim.set_input("scan_in", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 0))
            .map_err(|e| e.to_string())?;
        sim.set_input("clk", &LW::from_u64(1, 1))
            .map_err(|e| e.to_string())?;
        Ok(())
    });

    c.add("dft_counter_chain_and_structure", || {
        let nl = counter_netlist();
        let scanned = eda_dft::insert_scan_chain(&nl, &eda_dft::ScanOptions::default())?;
        eda_dft::check_scan_structure(
            &scanned.netlist,
            &scanned.chain,
            scanned.scan_en,
            scanned.scan_in,
            scanned.scan_out,
        )?;
        if scanned.chain.len() != 3 {
            Err(format!("chain length {}", scanned.chain.len()))
        } else {
            Ok(())
        }
    });

    c.add("dft_analyze_scanned_design", || {
        let scanned =
            eda_dft::insert_scan_chain(&counter_netlist(), &eda_dft::ScanOptions::default())?;
        let report = eda_dft::analyze(&scanned.netlist)?;
        if report.scan_cells == 3
            && report.scan_chains == 1
            && report.unscanned_sequential.is_empty()
        {
            Ok(())
        } else {
            Err(format!("{:?}", report))
        }
    });

    c.add("dft_report_content", || {
        let scanned =
            eda_dft::insert_scan_chain(&counter_netlist(), &eda_dft::ScanOptions::default())?;
        let text = eda_dft::report(&scanned);
        if text.contains("DFT scan report") && text.contains("chain length 3") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_scan_insertion_and_analysis", || {
        let flow = eda_api::run_flow(
            "module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule",
            None,
        )?;
        let scanned = eda_api::insert_scan(&flow.netlist, &eda_dft::ScanOptions::default())?;
        let report = eda_api::dft_analysis(&scanned.netlist)?;
        if report.scan_cells == 1 && eda_api::dft_report(&scanned).contains("scan_en") {
            Ok(())
        } else {
            Err(eda_api::dft_report(&scanned))
        }
    });

    c.add("dft_rejects_combinational_design", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        if eda_api::insert_scan(&flow.netlist, &eda_dft::ScanOptions::default()).is_ok() {
            Err("combinational design unexpectedly scanned".into())
        } else {
            Ok(())
        }
    });
}
