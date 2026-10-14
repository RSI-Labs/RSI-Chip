//! SPEF parsing, parasitic analysis, and RC-to-SDF conversion cases.

use crate::CaseList;
use eda_core::netlist::Netlist;

fn synth() -> Netlist {
    let design = eda_hdl::compile(
        "module m(input a, b, output y); assign y = a & b; endmodule",
        None,
    )
    .expect("compile");
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

fn spef_text() -> String {
    r#"
*SPEF "IEEE 1481-1998"
*DESIGN "m"
*T_UNIT 1 NS
*C_UNIT 1 PF
*R_UNIT 1 OHM
*D_NET a 0.012
*CONN
*P a I
*CAP
1 a 0.010
*RES
1 a u1:A 20.0
*END
*D_NET b 0.014
*CONN
*P b I
*CAP
1 b 0.014
*RES
1 b u1:B 30.0
*END
*D_NET y 0.006
*CONN
*I u1:Y O
*P y O
*CAP
1 y 0.006
2 y u2:A 0.004
*RES
1 y u1:Y 10.0
*END
"#
    .to_string()
}

pub fn register(c: &mut CaseList) {
    c.add("spef_parse_units_and_nets", || {
        let spef = eda_spef::parse_spef(&spef_text())?;
        if spef.design != "m" || spef.nets.len() != 3 {
            Err(format!("design={} nets={}", spef.design, spef.nets.len()))
        } else {
            Ok(())
        }
    });

    c.add("spef_grounded_coupling_and_resistance", || {
        let spef = eda_spef::parse_spef(&spef_text())?;
        let y = &spef.nets["y"];
        let grounded = y.grounded_capacitance(spef.units);
        let coupling = y.coupling_capacitance(spef.units);
        let resistance = y.total_resistance(spef.units);
        if (grounded - 0.006e-12).abs() > 1e-20
            || (coupling - 0.004e-12).abs() > 1e-20
            || (resistance - 10.0).abs() > 1e-12
        {
            Err(format!("{grounded} {coupling} {resistance}"))
        } else {
            Ok(())
        }
    });

    c.add("spef_analyze_mapped_netlist", || {
        let nl = synth();
        let spef = eda_spef::parse_spef(&spef_text())?;
        let report = eda_spef::analyze(&nl, &spef)?;
        if report.net_count != 3 || !report.unmatched_netlist_nets.is_empty() {
            Err(eda_spef::report(&report))
        } else {
            Ok(())
        }
    });

    c.add("spef_rc_delay_estimation", || {
        let spef = eda_spef::parse_spef(&spef_text())?;
        let a = &spef.nets["a"];
        let delay_ns = a.estimated_delay_seconds(spef.units) * 1e9;
        let want = 0.69 * 20.0 * 0.010e-12 * 1e9;
        if (delay_ns - want).abs() > 1e-12 {
            Err(format!("delay={delay_ns} want={want}"))
        } else {
            Ok(())
        }
    });

    c.add("spef_report_content", || {
        let nl = synth();
        let spef = eda_spef::parse_spef(&spef_text())?;
        let report = eda_spef::analyze(&nl, &spef)?;
        let text = eda_spef::report(&report);
        if text.contains("SPEF report") && text.contains("NET a") && text.contains("NET y") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("spef_to_sdf_parses_and_annotates", || {
        let nl = synth();
        let spef = eda_spef::parse_spef(&spef_text())?;
        let sdf_text = eda_spef::write_sdf(&nl, &spef)?;
        let sdf = eda_sdf::parse_sdf(&sdf_text)?;
        if sdf.interconnects.is_empty() {
            Err(sdf_text)
        } else {
            let annotated = eda_sdf::annotate_sdf(&nl, &sdf)?;
            let timing = eda_sta::analyze(&annotated)?;
            if timing.critical_path_delay > 0.0 {
                Ok(())
            } else {
                Err("RC annotation did not affect timing".into())
            }
        }
    });

    c.add("api_spef_parse_analyze_and_convert", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        let spef = eda_api::parse_parasitics(&spef_text())?;
        let report = eda_api::parasitic_analysis(&flow.netlist, &spef)?;
        let sdf_text = eda_api::parasitics_to_sdf(&flow.netlist, &spef)?;
        if report.net_count > 0
            && eda_api::parasitic_report(&report).contains("SPEF")
            && sdf_text.contains("INTERCONNECT")
        {
            Ok(())
        } else {
            Err("API SPEF flow incomplete".into())
        }
    });

    for (name, text) in [
        ("missing_design", "*D_NET a 1 *CAP 1 a 1 *END"),
        ("no_nets", "*DESIGN \"x\""),
        (
            "bad_unit",
            "*DESIGN \"x\"\n*T_UNIT 1 SEC\n*D_NET a 1\n*CAP\n1 a 1\n*END",
        ),
        (
            "duplicate_net",
            "*DESIGN \"x\"\n*D_NET a 1\n*END\n*D_NET a 1\n*END",
        ),
        ("entry_outside_net", "*DESIGN \"x\"\n*CAP\n1 a 1"),
    ] {
        c.add(format!("spef_err_{}", name), move || {
            if eda_spef::parse_spef(text).is_ok() {
                Err(format!("{} accepted", name))
            } else {
                Ok(())
            }
        });
    }
}
