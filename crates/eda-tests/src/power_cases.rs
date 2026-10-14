//! Gate-level switching-power regression cases.

use crate::CaseList;
use eda_core::netlist::Netlist;
use eda_power::{analyze, InputVector, PowerOptions};
use std::sync::OnceLock;

fn synth_cached(src: &'static str) -> &'static Netlist {
    static NETS: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Netlist>>,
    > = OnceLock::new();
    let lock = NETS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = lock.lock().unwrap();
    guard.entry(src).or_insert_with(|| {
        Box::leak(Box::new(
            eda_synth::synthesize(
                Box::leak(Box::new(eda_hdl::compile(src, None).expect("compile"))),
                &eda_synth::SynthOptions::default(),
            )
            .expect("synth")
            .netlist,
        ))
    })
}

fn vector(values: &[(&str, u64)]) -> InputVector {
    values
        .iter()
        .map(|(name, value)| {
            (
                (*name).to_string(),
                eda_core::logic::LogicWord::from_u64(1, *value),
            )
        })
        .collect()
}

pub fn register(c: &mut CaseList) {
    let src: &'static str = "module m(input a, b, output y); assign y = a ^ b; endmodule";
    let nl = synth_cached(src);

    c.add("power_constant_inputs_leakage", || {
        let r = analyze(
            nl,
            &[vector(&[("a", 0), ("b", 0)]), vector(&[("a", 0), ("b", 0)])],
            &PowerOptions::default(),
        )?;
        if r.dynamic_power_uw != 0.0 {
            Err(format!("constant dynamic power {}", r.dynamic_power_uw))
        } else if r.static_power_uw <= 0.0 {
            Err("no leakage estimate".into())
        } else {
            Ok(())
        }
    });

    c.add("power_xor_toggle_dynamic", || {
        let r = analyze(
            nl,
            &[vector(&[("a", 0), ("b", 0)]), vector(&[("a", 1), ("b", 0)])],
            &PowerOptions::default(),
        )?;
        if r.dynamic_power_uw <= 0.0 || r.activity_by_net["y"].transitions != 1 {
            Err(format!(
                "dynamic={}, y transitions={}",
                r.dynamic_power_uw, r.activity_by_net["y"].transitions
            ))
        } else {
            Ok(())
        }
    });

    c.add("power_frequency_scales_dynamic", || {
        let vectors = vec![vector(&[("a", 0), ("b", 0)]), vector(&[("a", 1), ("b", 0)])];
        let low = analyze(
            nl,
            &vectors,
            &PowerOptions {
                frequency_mhz: 50.0,
                ..PowerOptions::default()
            },
        )?;
        let high = analyze(
            nl,
            &vectors,
            &PowerOptions {
                frequency_mhz: 200.0,
                ..PowerOptions::default()
            },
        )?;
        if (high.dynamic_power_uw - 4.0 * low.dynamic_power_uw).abs() > 1e-12 {
            Err(format!(
                "{} did not scale linearly from {}",
                high.dynamic_power_uw, low.dynamic_power_uw
            ))
        } else {
            Ok(())
        }
    });

    c.add("power_static_probability", || {
        let r = analyze(
            nl,
            &[
                vector(&[("a", 0), ("b", 0)]),
                vector(&[("a", 1), ("b", 0)]),
                vector(&[("a", 1), ("b", 0)]),
            ],
            &PowerOptions::default(),
        )?;
        let a = r.activity_by_net["a"];
        if (a.static_probability() - 2.0 / 3.0).abs() > 1e-12 {
            Err(format!("P(a=1) = {}", a.static_probability()))
        } else {
            Ok(())
        }
    });

    c.add("power_missing_vector_input_rejected", || {
        if analyze(nl, &[vector(&[("a", 0)])], &PowerOptions::default()).is_ok() {
            Err("missing b accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("power_report_content", || {
        let r = analyze(
            nl,
            &[vector(&[("a", 0), ("b", 0)]), vector(&[("a", 1), ("b", 1)])],
            &PowerOptions::default(),
        )?;
        let text = eda_power::report(&r);
        if text.contains("dynamic") && text.contains("leakage") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_power_uses_liberty_state_and_path_power", || {
        let source = r#"
library(state_power) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        cell_leakage_power : 0.02;
        leakage_power () { when : "!A"; value : 0.01; }
        leakage_power () { when : "A"; value : 0.08; }
        pin (A) {
            direction : input;
            capacitance : 0.01;
            internal_power () {
                related_pin : "A";
                when : "!A";
                rise_power(values("0.011"));
                fall_power(values("0.007"));
            }
        }
        pin (Y) {
            direction : output;
            function : "!A";
            timing () { related_pin : "A"; cell_rise(values("0.2")); }
        }
    }
}
"#;
        let library = std::sync::Arc::new(eda_api::parse_liberty(source)?);
        let mut nl = Netlist::new("state_power");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(eda_core::netlist::Cell {
            name: "u1".into(),
            kind: eda_core::netlist::CellKind::Lib(library.find("INV").ok_or("missing INV")?),
            inputs: vec![a],
            output: y,
        });
        nl.add_port("a", eda_core::netlist::Dir::In, vec![a]);
        nl.add_port("y", eda_core::netlist::Dir::Out, vec![y]);
        let result = eda_api::analyze_power(
            &nl,
            &[
                vector(&[("a", 0)]),
                vector(&[("a", 1)]),
                vector(&[("a", 0)]),
            ],
            &PowerOptions::default(),
        )?;
        let expected_static_uw = (0.01 + 0.08 + 0.01) / 3.0 * 1_000_000.0;
        let expected_dynamic_uw = 0.011 / 2.0 * 100.0;
        if (result.static_power_uw - expected_static_uw).abs() > 1e-9
            || (result.dynamic_power_uw - expected_dynamic_uw).abs() > 1e-12
            || (result.total_power_uw - (result.static_power_uw + result.dynamic_power_uw)).abs()
                > 1e-15
        {
            Err(format!(
                "{result:?} expected static={expected_static_uw} dynamic={expected_dynamic_uw}"
            ))
        } else {
            Ok(())
        }
    });

    c.add("api_power_uses_liberty_leakage_power_unit", || {
        let source = r#"
library(leakage_unit_power) {
    time_unit : "1ns";
    leakage_power_unit : "1nW";
    cell (INV) {
        area : 1;
        leakage_power () { when : "!A"; value : 10; }
        leakage_power () { when : "A"; value : 80; }
        pin (A) { direction : input; }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
        let library = std::sync::Arc::new(eda_api::parse_liberty(source)?);
        let mut nl = Netlist::new("leakage_unit_power");
        nl.set_library(library.clone());
        let a = nl.add_net("a");
        let y = nl.add_net("y");
        nl.add_cell(eda_core::netlist::Cell {
            name: "u1".into(),
            kind: eda_core::netlist::CellKind::Lib(
                library.find("INV").ok_or("missing leakage INV")?,
            ),
            inputs: vec![a],
            output: y,
        });
        nl.add_port("a", eda_core::netlist::Dir::In, vec![a]);
        nl.add_port("y", eda_core::netlist::Dir::Out, vec![y]);
        let result = eda_api::analyze_power(
            &nl,
            &[vector(&[("a", 0)]), vector(&[("a", 1)])],
            &PowerOptions::default(),
        )?;
        // Average state leakage is (10 nW + 80 nW) / 2 = 45 nW = 0.045 uW.
        if (result.static_power_uw - 0.045).abs() > 1e-15 {
            Err(format!(
                "static={:?} dynamic={:?} total={:?}",
                result.static_power_uw, result.dynamic_power_uw, result.total_power_uw
            ))
        } else {
            Ok(())
        }
    });

    for k in 0..16u64 {
        let a = k & 0xF;
        let b = (k * 7 + 3) & 0xF;
        c.add(format!("power_adder_vector_{}", k), move || {
            let adder: &'static str =
                "module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule";
            let nl = synth_cached(adder);
            let r = analyze(
                nl,
                &[vector(&[("a", 0), ("b", 0)]), vector(&[("a", a), ("b", b)])],
                &PowerOptions::default(),
            )?;
            if r.total_power_uw < r.dynamic_power_uw || r.total_power_uw < r.static_power_uw {
                Err("power accounting".into())
            } else {
                Ok(())
            }
        });
    }
}
