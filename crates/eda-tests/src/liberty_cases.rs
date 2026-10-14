//! Liberty import and custom-library flow cases.

use crate::CaseList;
use eda_core::library::CellLibrary;
use eda_core::logic::LogicValue as LV;

const BASE_LIB: &str = r#"
library (mini) {
    time_unit : "1ns";
    cell (INV) {
        area : 1.2;
        cell_leakage_power : 0.03;
        pin (A) { direction : input; capacitance : 0.012; }
        pin (Y) {
            direction : output;
            function : "!A";
            max_transition : 0.30;
            max_capacitance : 0.12;
            timing () { related_pin : "A"; cell_rise(values("0.35,0.42")); }
        }
    }
    cell (AND2) {
        area : 3.1;
        cell_leakage_power : 0.04;
        pin (A) { direction : input; capacitance : 0.014; }
        pin (B) { direction : input; capacitance : 0.016; }
        pin (Y) {
            direction : output;
            function : "(A*B)";
            max_transition : 0.40;
            max_capacitance : 0.20;
            timing () { related_pin : "A"; cell_rise(values("1.05")); }
            timing () { related_pin : "B"; cell_rise(values("1.10")); }
        }
    }
}
"#;

pub fn register(c: &mut CaseList) {
    parse_basics(c);
    boolean_functions(c);
    custom_flow(c);
    errors(c);
}

fn parse_basics(c: &mut CaseList) {
    c.add("liberty_library_name_and_cells", || {
        let lib = eda_core::liberty::parse_liberty(BASE_LIB)?;
        if lib.name != "mini" || lib.cells.len() != 2 {
            Err(format!("name={} cells={}", lib.name, lib.cells.len()))
        } else {
            let one_d = eda_api::LibertyLut {
                index_1: vec![0.0, 0.2],
                index_2: None,
                values: vec![0.10, 0.30],
            };
            if (one_d.evaluate(0.1, 0.0)? - 0.20).abs() > 1e-12
                || (one_d.evaluate(-1.0, 0.0)? - 0.10).abs() > 1e-12
            {
                Err("one-dimensional Liberty LUT interpolation failed".into())
            } else {
                Ok(())
            }
        }
    });
    c.add("liberty_area_width_and_leakage", || {
        let lib = eda_core::liberty::parse_liberty(BASE_LIB)?;
        let inv = lib.get(lib.find("INV").ok_or("no INV")?);
        let meta = lib.metadata("INV");
        if (meta.area - 1.2).abs() > 1e-12
            || inv.width_sites != 2
            || (meta.leakage_power - 0.03).abs() > 1e-12
        {
            Err("area/width/leakage metadata".into())
        } else {
            Ok(())
        }
    });
    c.add("liberty_timing_and_capacitance", || {
        let lib = eda_core::liberty::parse_liberty(BASE_LIB)?;
        let and = lib.get(lib.find("AND2").ok_or("no AND2")?);
        let meta = lib.metadata("AND2");
        if and.arcs != vec![(0, 1.05), (1, 1.10)]
            || (meta.input_capacitance_pf["A"] - 0.014).abs() > 1e-12
            || (meta.max_transition_ns["Y"] - 0.40).abs() > 1e-12
            || (meta.max_capacitance_pf["Y"] - 0.20).abs() > 1e-12
        {
            Err(format!(
                "arcs={:?} caps={:?}",
                and.arcs, meta.input_capacitance_pf
            ))
        } else {
            Ok(())
        }
    });
    c.add("liberty_ps_time_unit_scales_delays", || {
        let src = r#"
library(ps) {
    time_unit : "1ps";
    cell (INV) { area : 1; pin(A){direction:input;} pin(Y){direction:output;function:"!A";
        timing(){related_pin:"A";cell_rise(values("350"));} } }
}
"#;
        let lib = eda_core::liberty::parse_liberty(src)?;
        let inv = lib.get(lib.find("INV").ok_or("no INV")?);
        if (inv.arcs[0].1 - 0.35).abs() > 1e-12 {
            Err(format!("delay {}", inv.arcs[0].1))
        } else {
            Ok(())
        }
    });
    c.add("liberty_ps_nldm_scales_time_axis_not_capacitance", || {
        let src = r#"
library(ps_nldm) {
    time_unit : "1ps";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(ps_template) {
                    index_1("0, 200");
                    index_2("0, 0.02");
                    values("100, 120", "140, 180");
                }
                rise_transition(ps_template) {
                    index_1("0, 200");
                    index_2("0, 0.02");
                    values("40, 60", "80, 100");
                }
            }
        }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
        let lib = eda_core::liberty::parse_liberty(src)?;
        let id = lib.find("INV").ok_or("missing ps INV")?;
        let tables = lib
            .get(id)
            .timing_tables
            .first()
            .ok_or("missing ps timing tables")?;
        let delay = tables.rise.as_ref().ok_or("missing delay LUT")?;
        let transition = tables
            .rise_transition
            .as_ref()
            .ok_or("missing transition LUT")?;
        let cap_axis = delay.index_2.clone().ok_or("missing capacitance axis")?;
        if delay.index_1 != vec![0.0, 0.2]
            || cap_axis != vec![0.0, 0.02]
            || (delay.values[3] - 0.18).abs() > 1e-12
            || transition.index_1 != vec![0.0, 0.2]
            || transition.index_2 != Some(vec![0.0, 0.02])
            || (transition.values[3] - 0.10).abs() > 1e-12
        {
            Err(format!("delay={delay:?} transition={transition:?}"))
        } else {
            Ok(())
        }
    });
    c.add("liberty_ff_capacitance_unit_scales_pin_and_nldm", || {
        let src = r#"
library(ff_cap) {
    time_unit : "1ns";
    capacitance_unit : "1ff";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 10;
            timing () {
                related_pin : "A";
                cell_rise(ff_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.10, 0.12", "0.14, 0.18");
                }
            }
        }
        pin (Y) {
            direction : output;
            function : "!A";
            max_transition : 0.30;
            max_capacitance : 20;
        }
    }
}
"#;
        let lib = eda_api::parse_liberty(src)?;
        let id = lib.find("INV").ok_or("missing ff INV")?;
        let metadata = lib.metadata("INV");
        let tables = lib
            .get(id)
            .timing_tables
            .first()
            .ok_or("missing ff timing tables")?;
        let cap_axis = tables
            .rise
            .as_ref()
            .ok_or("missing ff delay table")?
            .index_2
            .clone()
            .ok_or("missing ff capacitance axis")?;
        if (metadata.input_capacitance_pf["A"] - 0.01).abs() > 1e-12
            || (metadata.max_capacitance_pf["Y"] - 0.02).abs() > 1e-12
            || cap_axis != vec![0.0, 0.02]
        {
            Err(format!(
                "input={:?} max={:?} axis={cap_axis:?}",
                metadata.input_capacitance_pf, metadata.max_capacitance_pf
            ))
        } else {
            Ok(())
        }
    });
    c.add("liberty_capacitive_load_unit_scales_pin_and_nldm", || {
        let src = r#"
library(standard_cap) {
    time_unit : "1ns";
    capacitive_load_unit (1, ff);
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 10;
            timing () {
                related_pin : "A";
                cell_rise(standard_cap_template) {
                    index_1("0.0, 0.2");
                    index_2("0, 20");
                    values("0.10, 0.12", "0.14, 0.18");
                }
            }
        }
        pin (Y) {
            direction : output;
            function : "!A";
            max_capacitance : 20;
        }
    }
}
"#;
        let lib = eda_api::parse_liberty(src)?;
        let metadata = lib.metadata("INV");
        let cap_axis = lib
            .get(lib.find("INV").ok_or("missing standard-cap INV")?)
            .timing_tables
            .first()
            .and_then(|tables| tables.rise.as_ref())
            .and_then(|lut| lut.index_2.clone())
            .ok_or("missing standard-cap LUT axis")?;
        if (metadata.input_capacitance_pf["A"] - 0.01).abs() > 1e-12
            || (metadata.max_capacitance_pf["Y"] - 0.02).abs() > 1e-12
            || cap_axis != vec![0.0, 0.02]
        {
            Err(format!(
                "input={:?} max={:?} axis={cap_axis:?}",
                metadata.input_capacitance_pf, metadata.max_capacitance_pf
            ))
        } else {
            Ok(())
        }
    });
    c.add("liberty_leakage_power_unit_scales_cell_and_states", || {
        let src = r#"
library(leakage_units) {
    time_unit : "1ns";
    leakage_power_unit : "1nW";
    cell (INV) {
        area : 1;
        cell_leakage_power : 30;
        leakage_power () { when : "!A"; value : 10; }
        leakage_power () { when : "A"; value : 80; }
        pin (A) { direction : input; }
        pin (Y) { direction : output; function : "!A"; }
    }
}
"#;
        let lib = eda_api::parse_liberty(src)?;
        let metadata = lib.metadata("INV");
        let low = metadata
            .leakage_states
            .first()
            .ok_or("missing low leakage state")?;
        let high = metadata
            .leakage_states
            .get(1)
            .ok_or("missing high leakage state")?;
        if (metadata.leakage_power - 30.0e-9).abs() > 1e-18
            || (low.power_w - 10.0e-9).abs() > 1e-18
            || (high.power_w - 80.0e-9).abs() > 1e-18
        {
            Err(format!(
                "base={:e} low={:e} high={:e}",
                metadata.leakage_power, low.power_w, high.power_w
            ))
        } else {
            Ok(())
        }
    });

    c.add("liberty_state_and_path_dependent_power", || {
        let src = r#"
library(power) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        cell_leakage_power : 0.03;
        leakage_power () { when : "!A"; value : 0.01; }
        leakage_power () { when : "A"; value : 0.09; }
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
        let lib = eda_core::liberty::parse_liberty(src)?;
        let meta = lib.metadata("INV");
        let low = meta.leakage_states.first().ok_or("low state missing")?;
        let high = meta.leakage_states.get(1).ok_or("high state missing")?;
        let arc = meta
            .internal_power
            .first()
            .ok_or("internal-power arc missing")?;
        if meta.leakage_states.len() != 2
            || low.when.eval(&[LV::Zero]) != LV::One
            || (low.power_w - 0.01).abs() > 1e-12
            || high.when.eval(&[LV::One]) != LV::One
            || (high.power_w - 0.09).abs() > 1e-12
            || arc.related_pin != "A"
            || arc.when.is_none()
            || (arc.rise_power_pj - 0.011).abs() > 1e-12
            || (arc.fall_power_pj - 0.007).abs() > 1e-12
        {
            Err(format!("{meta:?}"))
        } else {
            Ok(())
        }
    });

    c.add("liberty_indexed_timing_tables", || {
        let src = r#"
library(nldm) {
    time_unit : "1ns";
    cell (INV) {
        area : 1;
        pin (A) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "A";
                cell_rise(nldm_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.10, 0.12", "0.14, 0.18");
                }
                cell_fall(nldm_template) {
                    index_1("0.0, 0.2");
                    index_2("0.0, 0.02");
                    values("0.08, 0.09", "0.10, 0.11");
                }
            }
        }
        pin (Y) {
            direction : output;
            function : "!A";
        }
    }
}
"#;
        let lib = eda_core::liberty::parse_liberty(src)?;
        let id = lib.find("INV").ok_or("missing indexed INV")?;
        let def = lib.get(id);
        let tables = def
            .timing_tables
            .first()
            .ok_or("missing indexed timing tables")?;
        let rise = tables.rise.as_ref().ok_or("missing rise LUT")?;
        let fall = tables.fall.as_ref().ok_or("missing fall LUT")?;
        let rise_mid = rise.evaluate(0.1, 0.01)?;
        let fall_mid = fall.evaluate(0.1, 0.01)?;
        let rise_low = rise.evaluate(-1.0, -1.0)?;
        let fall_high = fall.evaluate(1.0, 1.0)?;
        if (rise_mid - 0.135).abs() > 1e-12
            || (fall_mid - 0.095).abs() > 1e-12
            || (rise_low - 0.10).abs() > 1e-12
            || (fall_high - 0.11).abs() > 1e-12
            || (def.arcs[0].1 - 0.10).abs() > 1e-12
        {
            Err(format!(
                "rise_mid={rise_mid} fall_mid={fall_mid} rise_low={rise_low} fall_high={fall_high} arc={}",
                def.arcs[0].1
            ))
        } else {
            Ok(())
        }
    });
    c.add("liberty_indexed_setup_hold_constraint_tables", || {
        let src = r#"
library(constraints) {
    time_unit : "1ns";
    cell (DFF) {
        area : 5;
        ff (IQ, IQN) {
            clocked_on : "CLK";
            next_state : "D";
        }
        pin (D) {
            direction : input;
            capacitance : 0.01;
            timing () {
                related_pin : "CLK";
                timing_type : setup_rising;
                rise_constraint(setup_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.20, 0.24", "0.28, 0.32");
                }
                fall_constraint(setup_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.18, 0.22", "0.26, 0.30");
                }
            }
            timing () {
                related_pin : "CLK";
                timing_type : hold_rising;
                rise_constraint(hold_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.02, 0.04", "0.06, 0.08");
                }
                fall_constraint(hold_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.01, 0.02", "0.03, 0.04");
                }
            }
        }
        pin (CLK) { direction : input; capacitance : 0.02; }
        pin (Q) {
            direction : output;
            timing () {
                related_pin : "CLK";
                timing_type : rising_edge;
                cell_rise(values("0.10"));
            }
        }
    }
}
"#;
        let lib = eda_api::parse_liberty(src)?;
        let id = lib.find("DFF").ok_or("missing constraint DFF")?;
        let def = lib.get(id);
        let seq = def.seq.as_ref().ok_or("missing sequential timing")?;
        let setup = seq.setup_tables.as_ref().ok_or("missing setup tables")?;
        let hold = seq.hold_tables.as_ref().ok_or("missing hold tables")?;
        let setup_value = setup
            .evaluate_worst(0.05, 0.06)?
            .ok_or("empty setup table result")?;
        let hold_value = hold
            .evaluate_worst(0.05, 0.06)?
            .ok_or("empty hold table result")?;
        if (setup_value - 0.264).abs() > 1e-12
            || (hold_value - 0.052).abs() > 1e-12
            || setup.rise.is_none()
            || setup.fall.is_none()
            || hold.rise.is_none()
            || hold.fall.is_none()
        {
            Err(format!("setup={setup_value} hold={hold_value}"))
        } else {
            Ok(())
        }
    });
    c.add("liberty_indexed_recovery_removal_constraint_tables", || {
        let src = r#"
library(async_constraints) {
    time_unit : "1ns";
    cell (DFFR) {
        area : 5;
        ff (IQ, IQN) {
            clocked_on : "CLK";
            next_state : "D";
            clear : "RST";
        }
        pin (RST) {
            direction : input;
            capacitance : 0.02;
            timing () {
                related_pin : "CLK";
                timing_type : recovery_rising;
                rise_constraint(recovery_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.30, 0.34", "0.38, 0.42");
                }
                fall_constraint(recovery_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.29, 0.33", "0.37, 0.41");
                }
            }
            timing () {
                related_pin : "CLK";
                timing_type : removal_rising;
                rise_constraint(removal_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.10, 0.14", "0.18, 0.22");
                }
                fall_constraint(removal_template) {
                    index_1("0.0, 0.1");
                    index_2("0.0, 0.1");
                    values("0.09, 0.13", "0.17, 0.21");
                }
            }
        }
        pin (CLK) { direction : input; capacitance : 0.02; }
        pin (D) { direction : input; capacitance : 0.01; }
        pin (Q) {
            direction : output;
            timing () {
                related_pin : "CLK";
                timing_type : rising_edge;
                cell_rise(values("0.10"));
            }
        }
    }
}
"#;
        let lib = eda_api::parse_liberty(src)?;
        let id = lib.find("DFFR").ok_or("missing asynchronous DFF")?;
        let def = lib.get(id);
        let seq = def.seq.as_ref().ok_or("missing sequential timing")?;
        let recovery = seq
            .recovery_tables
            .as_ref()
            .ok_or("missing recovery tables")?;
        let removal = seq
            .removal_tables
            .as_ref()
            .ok_or("missing removal tables")?;
        let recovery_value = recovery
            .evaluate_worst(0.06, 0.06)?
            .ok_or("empty recovery table result")?;
        let removal_value = removal
            .evaluate_worst(0.06, 0.06)?
            .ok_or("empty removal table result")?;
        if (recovery_value - 0.372).abs() > 1e-12
            || (removal_value - 0.172).abs() > 1e-12
            || recovery.rise.is_none()
            || recovery.fall.is_none()
            || removal.rise.is_none()
            || removal.fall.is_none()
            || (seq.recovery - 0.30).abs() > 1e-12
            || (seq.removal - 0.10).abs() > 1e-12
        {
            Err(format!(
                "recovery={recovery_value} removal={removal_value} scalars=({},{})",
                seq.recovery, seq.removal
            ))
        } else {
            Ok(())
        }
    });
}

fn boolean_functions(c: &mut CaseList) {
    let funcs: Vec<(&str, &str, [LV; 2], LV)> = vec![
        ("and", "(A*B)", [LV::One, LV::One], LV::One),
        ("nand", "!(A*B)", [LV::One, LV::One], LV::Zero),
        ("or", "A+B", [LV::Zero, LV::One], LV::One),
        ("nor", "!(A+B)", [LV::Zero, LV::Zero], LV::One),
        ("xor", "A^B", [LV::One, LV::One], LV::Zero),
        ("xnor", "!(A^B)", [LV::One, LV::Zero], LV::Zero),
        ("aoi", "!((A*B)+(A))", [LV::One, LV::Zero], LV::Zero),
        ("alt_and", "A&B", [LV::One, LV::One], LV::One),
        ("alt_or", "A|B", [LV::Zero, LV::Zero], LV::Zero),
    ];
    for (i, (name, function, ins, want)) in funcs.into_iter().enumerate() {
        c.add(format!("liberty_func_{}_{}", i, name), move || {
            let src = format!(
                r#"library(f){{cell({}{}){{area:1;pin(A){{direction:input;}}pin(B){{direction:input;}}pin(Y){{direction:output;function:"{}";}}}}}}"#,
                name.to_uppercase(),
                i,
                function
            );
            let lib = eda_core::liberty::parse_liberty(&src)?;
            let id = lib
                .find(&format!("{}{}", name.to_uppercase(), i))
                .ok_or("cell missing")?;
            if lib.eval(id, &ins) != want {
                Err(format!("{} evaluated wrong", function))
            } else {
                Ok(())
            }
        });
    }
}

fn custom_flow(c: &mut CaseList) {
    c.add("liberty_api_import", || {
        let lib = eda_api::parse_liberty(BASE_LIB)?;
        if lib.find("AND2").is_none() || lib.metadata("INV").area != 1.2 {
            Err("API Liberty import".into())
        } else {
            Ok(())
        }
    });
    c.add("liberty_custom_library_synthesis_flow", || {
        let lib: CellLibrary = eda_api::parse_liberty(BASE_LIB)?;
        let flow = eda_api::run_flow_with_library(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
            &lib,
        )?;
        if !flow.equivalent || flow.netlist.stats().lib_cells != 1 {
            Err(format!(
                "equivalent={} stats={:?}",
                flow.equivalent,
                flow.netlist.stats()
            ))
        } else if (flow.timing.critical_path_delay - 1.1).abs() > 1e-9 {
            Err(format!("custom delay {}", flow.timing.critical_path_delay))
        } else {
            Ok(())
        }
    });
    c.add("liberty_quality_limits_flow", || {
        let lib: CellLibrary = eda_api::parse_liberty(BASE_LIB)?;
        let flow = eda_api::run_flow_with_library(
            "module m(input a, b, c, output y); assign y = (a & b) & c; endmodule",
            None,
            &lib,
        )?;
        let quality =
            eda_api::analyze_signal_quality(&flow.netlist, &eda_api::QualityOptions::default())?;
        if quality.transitions_checked == 0
            || !quality
                .violations
                .iter()
                .any(|v| v.kind == eda_api::QualityKind::MaxTransition)
        {
            Err(eda_api::signal_quality_report(&quality))
        } else {
            Ok(())
        }
    });
}

fn errors(c: &mut CaseList) {
    for (name, src) in [
        ("missing_library", "cell(X){area:1;}"),
        ("missing_area", "library(x){cell(A){pin(Y){direction:output;function:\"1\";}}}"),
        (
            "unknown_function_pin",
            "library(x){cell(A){area:1;pin(Y){direction:output;function:\"ZZ\";}}}",
        ),
        (
            "bad_time_unit",
            "library(x){time_unit:\"1fs\";cell(A){area:1;pin(Y){direction:output;function:\"1\";}}}",
        ),
        (
            "bad_leakage_power_unit",
            "library(x){leakage_power_unit:\"1fw\";cell(A){area:1;pin(Y){direction:output;function:\"1\";}}}",
        ),
        (
            "conflicting_capacitive_units",
            "library(x){capacitance_unit:\"1ff\";capacitive_load_unit(1,pf);cell(A){area:1;pin(Y){direction:output;function:\"1\";}}}",
        ),
        (
            "bad_capacitive_load_unit",
            "library(x){capacitive_load_unit(1,gf);cell(A){area:1;pin(Y){direction:output;function:\"1\";}}}",
        ),
        (
            "duplicate_capacitive_load_unit",
            "library(x){capacitive_load_unit(1,ff);capacitive_load_unit(1,ff);cell(A){area:1;pin(Y){direction:output;function:\"1\";}}}",
        ),
        (
            "unterminated",
            "library(x){cell(A){area:1;pin(Y){direction:output;function:\"1\";}",
        ),
    ] {
        c.add(format!("liberty_err_{}", name), move || {
            if eda_core::liberty::parse_liberty(src).is_ok() {
                Err(format!("{} accepted", name))
            } else {
                Ok(())
            }
        });
    }
}
