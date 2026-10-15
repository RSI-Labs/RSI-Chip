//! Base timing, path reconstruction, annotation, and derate cases.

use super::*;

pub(super) fn register(c: &mut CaseList) {
    c.add("sta_inv_chain_delay", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let mut prev = a;
        for i in 0..3 {
            let out = nl.add_net(format!("n{}", i));
            nl.add_cell(Cell {
                name: format!("u{}", i),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![prev],
                output: out,
            });
            prev = out;
        }
        let y = prev;
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        // 3 INV @ 0.4ns = 1.2ns combinational worst path.
        if (r.critical_path_delay - 1.2).abs() > 1e-9 {
            Err(format!("critical = {}, want 1.2", r.critical_path_delay))
        } else {
            Ok(())
        }
    });
    c.add("sta_and_or_two_levels", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("two");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        if (r.critical_path_delay - (1.0 + 0.4)).abs() > 1e-9 {
            Err(format!("critical = {}, want 1.4", r.critical_path_delay))
        } else {
            Ok(())
        }
    });
    // Register-to-register path: clk->q + comb + setup.
    c.add("sta_reg2reg", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("r2r");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let w = nl.add_net("w");
        let q2 = nl.add_net("q2");
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "inv".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: w,
        });
        nl.add_cell(Cell {
            name: "ff2".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, w],
            output: q2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("q2", Dir::Out, vec![q2]);
        let r = eda_sta::analyze(&nl)?;
        let want = 1.2 + 0.4 + 0.6; // clk->q + inv + setup
        if (r.reg2reg_critical - want).abs() > 1e-9 {
            Err(format!(
                "reg2reg = {}, want {}; in2reg={}; critical={}; arrivals={:?}",
                r.reg2reg_critical,
                want,
                r.in2reg_critical,
                r.critical_path_delay,
                eda_sta::arrival_by_name(&nl, &r)
            ))
        } else {
            Ok(())
        }
    });
    c.add("sta_hold_min_arrival", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("hold");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let w = nl.add_net("w");
        let q2 = nl.add_net("q2");
        let dff = lib.find("DFF").unwrap();
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "inv".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: w,
        });
        nl.add_cell(Cell {
            name: "ff2".into(),
            kind: CellKind::Lib(dff),
            inputs: vec![clk, w],
            output: q2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        nl.add_port("q2", Dir::Out, vec![q2]);
        let r = eda_sta::analyze(&nl)?;
        if (r.reg2reg_hold_critical - 1.6).abs() > 1e-9 {
            Err(format!(
                "reg2reg hold = {}, want 1.6",
                r.reg2reg_hold_critical
            ))
        } else if (r.in2reg_hold_critical - 0.0).abs() > 1e-9 || r.hold_endpoint_list().len() != 2 {
            Err(format!(
                "in2reg hold={}, endpoints={}",
                r.in2reg_hold_critical,
                r.hold_endpoint_list().len()
            ))
        } else {
            Ok(())
        }
    });
    // Slack at a given period.
    c.add("sta_slack", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("slack");
        nl.set_library(lib.clone());
        let clk = nl.add_net("clk");
        let d = nl.add_net("d");
        let q = nl.add_net("q");
        let n1 = nl.add_net("n1");
        let n2 = nl.add_net("n2");
        nl.add_cell(Cell {
            name: "ff1".into(),
            kind: CellKind::Lib(lib.find("DFF").unwrap()),
            inputs: vec![clk, d],
            output: q,
        });
        nl.add_cell(Cell {
            name: "i1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![q],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "i2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: n2,
        });
        nl.add_port("clk", Dir::In, vec![clk]);
        nl.add_port("d", Dir::In, vec![d]);
        nl.add_port("q", Dir::Out, vec![q]);
        let r = eda_sta::analyze(&nl)?;
        let period = 5.0;
        let slack = eda_sta::worst_slack(&r, period);
        let want = period - (1.2 + 0.8); // tail is unconnected: comb from q through 2 inv
        if (slack - want).abs() > 1e-9 {
            Err(format!("slack = {}, want {}", slack, want))
        } else {
            Ok(())
        }
    });
    // Synthesized designs produce finite, positive timing.
    for (name, src) in [
        ("add4", "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule"),
        ("mux", "module m(input s, a, b, output y); assign y = s ? b : a; endmodule"),
        ("counter", "module m(input clk, rst, output reg [3:0] q); always @(posedge clk or posedge rst) if (rst) q <= 4'd0; else q <= q + 4'd1; endmodule"),
    ] {
        c.add(format!("sta_synth_{}", name), move || {
            let nl = synth_cached(src);
            let r = eda_sta::analyze(nl)?;
            if !r.critical_path_delay.is_finite() || r.critical_path_delay <= 0.0 {
                Err(format!("bad critical path {}", r.critical_path_delay))
            } else {
                Ok(())
            }
        });
    }
    // Critical path reports the right cell count/sequence for a chain.
    c.add("sta_critical_path_cells", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("chain2");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        let path = r.critical_path_cells();
        if path.len() != 2 {
            Err(format!("path has {} cells, want 2", path.len()))
        } else {
            Ok(())
        }
    });

    c.add("sta_timing_paths_reconstruct_cells_and_nets", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("path_chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n1 = nl.add_net("n1");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![a],
            output: n1,
        });
        nl.add_cell(Cell {
            name: "u2".into(),
            kind: CellKind::Lib(lib.find("INV").unwrap()),
            inputs: vec![n1],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let r = eda_sta::analyze(&nl)?;
        let paths = r.timing_paths(&nl, 0);
        let report = eda_sta::timing_path_report(&nl, &paths);
        if paths.len() == 1
            && paths[0].kind == eda_sta::TimingPathKind::InputToOutput
            && paths[0].launch == eda_sta::TimingLaunch::PrimaryInput("a".into())
            && paths[0].cells.len() == 2
            && paths[0].nets.len() == 3
            && report.contains("cells: u1 -> u2")
            && report.contains("nets: a -> n1 -> y")
        {
            Ok(())
        } else {
            Err(report)
        }
    });

    c.add("sta_timing_paths_include_incremental_arcs", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("arc_chain");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let mut prev = a;
        for index in 0..3 {
            let out = nl.add_net(format!("n{}", index));
            nl.add_cell(Cell {
                name: format!("u{}", index),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![prev],
                output: out,
            });
            prev = out;
        }
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![prev]);
        let timing = eda_sta::analyze(&nl)?;
        let paths = timing.timing_paths(&nl, 1);
        let report = eda_sta::timing_path_report(&nl, &paths);
        let Some(path) = paths.first() else {
            return Err("no timing path".into());
        };
        let launch_ok = path.arcs.first().is_some_and(|arc| {
            arc.kind == eda_sta::TimingArcKind::PrimaryInputLaunch
                && arc.cell.is_none()
                && arc.from_net == Some(a)
                && arc.to_net == a
                && arc.from_arrival_ns.is_none()
                && (arc.to_arrival_ns - 0.0).abs() < 1e-12
                && (arc.delay_ns - 0.0).abs() < 1e-12
        });
        let combinational_ok = path.arcs.len() == 4
            && path.arcs[1..]
                .iter()
                .zip([0.4, 0.8, 1.2])
                .all(|(arc, arrival)| {
                    arc.kind == eda_sta::TimingArcKind::Combinational
                        && arc.cell.is_some()
                        && arc.from_arrival_ns.is_some()
                        && (arc.to_arrival_ns - arrival).abs() < 1e-12
                        && (arc.delay_ns - 0.4).abs() < 1e-12
                });
        let arc_sum = path.arcs.iter().map(|arc| arc.delay_ns).sum::<f64>();
        if launch_ok
            && combinational_ok
            && (arc_sum - path.arrival_ns).abs() < 1e-12
            && report.contains("input a -> a: launch delay 0.000000 ns")
            && report.contains("u0/a -> u0/y: combinational 0.400000 ns")
        {
            Ok(())
        } else {
            Err(format!(
                "sum={arc_sum}, arrival={}\n{}",
                path.arrival_ns, report
            ))
        }
    });

    c.add(
        "sta_timing_path_arcs_include_sdc_input_and_clk_launch",
        || {
            let src: &'static str = "module shift(input clk, d, output reg q1, q2);
                always @(posedge clk) q1 <= d;
                always @(posedge clk) q2 <= ~q1;
             endmodule";
            let nl = synth_cached(src);
            let constraints = eda_sta::parse_sdc(
                r#"
            create_clock -name clk -period 4.0 [get_ports clk]
            set_input_delay 1.0 -clock clk [get_ports d]
        "#,
            )?;
            let timing = eda_sta::analyze_with_sdc(nl, &constraints)?;
            let paths = eda_sta::constrained_timing_paths(nl, &timing, 0);
            let report = eda_sta::constrained_timing_path_report(nl, &paths);
            let input_launch = paths.iter().any(|path| {
                path.path.launch == eda_sta::TimingLaunch::PrimaryInput("d".into())
                    && path.path.arcs.first().is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::PrimaryInputLaunch
                            && (arc.delay_ns - 1.0).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.0).abs() < 1e-12
                    })
            });
            let clock_launch = paths.iter().any(|path| {
                matches!(path.path.launch, eda_sta::TimingLaunch::SequentialCell(_))
                    && path.path.arcs.first().is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::ClockToOutput
                            && (arc.delay_ns - 1.2).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.2).abs() < 1e-12
                    })
                    && path.path.arcs.get(1).is_some_and(|arc| {
                        arc.kind == eda_sta::TimingArcKind::Combinational
                            && (arc.delay_ns - 0.4).abs() < 1e-12
                            && (arc.to_arrival_ns - 1.6).abs() < 1e-12
                    })
            });
            if input_launch
                && clock_launch
                && report.contains("launch delay 1.000000 ns")
                && report.contains("clock-to-output 1.200000 ns")
            {
                Ok(())
            } else {
                Err(report)
            }
        },
    );

    c.add("sta_timing_path_arcs_reflect_sdf_annotation", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("sdf_arcs");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let n0 = nl.add_net("n0");
        let y = nl.add_net("y");
        for (name, input, output) in [("u0", a, n0), ("u1", n0, y)] {
            nl.add_cell(Cell {
                name: name.into(),
                kind: CellKind::Lib(lib.find("INV").unwrap()),
                inputs: vec![input],
                output,
            });
        }
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("y", Dir::Out, vec![y]);
        let sdf = eda_sdf::parse_sdf(
            r#"
            (DELAYFILE
              (SDFVERSION "3.0")
              (DESIGN "sdf_arcs")
              (TIMESCALE 1ns)
              (CELL (CELLTYPE "INV") (INSTANCE u0)
                (DELAY (ABSOLUTE (IOPATH A Y (0.8))))
              )
              (CELL (CELLTYPE "INV") (INSTANCE u1)
                (DELAY (ABSOLUTE (IOPATH A Y (0.9))))
              )
            )
            "#,
        )?;
        let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
        let timing = eda_sta::analyze(&annotated)?;
        let path = timing
            .timing_paths(&annotated, 1)
            .into_iter()
            .next()
            .ok_or("no annotated path")?;
        if path.arcs.len() == 3
            && (path.arcs[1].delay_ns - 0.8).abs() < 1e-12
            && (path.arcs[2].delay_ns - 0.9).abs() < 1e-12
            && (path.arrival_ns - 1.7).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!(
                "delays={},{}, arrival={}",
                path.arcs[1].delay_ns, path.arcs[2].delay_ns, path.arrival_ns
            ))
        }
    });

    c.add("sta_timing_path_arcs_reflect_cell_derate", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("derated_arc");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.2 -clock clk [get_ports a]
            set_timing_derate -early 0.9 -late 1.1
            set_timing_derate -cell_delay -early 0.5 -late 2.0 [get_cells u1]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 1);
        let Some(path) = paths.first() else {
            return Err("no derated path".into());
        };
        if path.path.arcs.len() == 2
            && (path.path.arcs[0].delay_ns - 0.22).abs() < 1e-12
            && (path.path.arcs[1].delay_ns - 2.0).abs() < 1e-12
            && (path.path.arrival_ns - 2.22).abs() < 1e-12
        {
            Ok(())
        } else {
            Err(format!(
                "launch={}, arc={}, arrival={}",
                path.path.arcs[0].delay_ns, path.path.arcs[1].delay_ns, path.path.arrival_ns
            ))
        }
    });

    c.add("sta_timing_path_arcs_select_derated_critical_pin", || {
        let lib = Arc::new(CellLibrary::std());
        let mut nl = Netlist::new("pin_derated_arc");
        nl.set_library(lib.clone());
        let a = nl.add_net("a");
        let b = nl.add_net("b");
        let y = nl.add_net("y");
        nl.add_cell(Cell {
            name: "u1".into(),
            kind: CellKind::Lib(lib.find("AND2").unwrap()),
            inputs: vec![a, b],
            output: y,
        });
        nl.add_port("a", Dir::In, vec![a]);
        nl.add_port("b", Dir::In, vec![b]);
        nl.add_port("y", Dir::Out, vec![y]);
        let constraints = eda_sta::parse_sdc(
            r#"
            create_clock -name clk -period 4.0 [get_ports a]
            set_input_delay 0.1 -clock clk [get_ports a]
            set_input_delay 0.1 -clock clk [get_ports b]
            set_timing_derate -cell_delay -late 2.0 [get_pins u1/a]
            set_timing_derate -cell_delay -late 1.0 [get_pins u1/b]
        "#,
        )?;
        let timing = eda_sta::analyze_with_sdc(&nl, &constraints)?;
        let paths = eda_sta::constrained_timing_paths(&nl, &timing, 1);
        let report = eda_sta::constrained_timing_path_report(&nl, &paths);
        let Some(path) = paths.first() else {
            return Err("no pin-derated path".into());
        };
        if path.path.arcs.len() == 2
            && path.path.arcs[1].from_net == Some(a)
            && (path.path.arcs[1].delay_ns - 2.0).abs() < 1e-12
            && report.contains("u1/a -> u1/y: combinational 2.000000 ns")
        {
            Ok(())
        } else {
            Err(report)
        }
    });
}
