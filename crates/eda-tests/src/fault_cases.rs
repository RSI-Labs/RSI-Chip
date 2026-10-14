//! Gate-level stuck-at fault simulation and ATPG pattern grading.

use crate::CaseList;
use eda_core::netlist::Netlist;
use std::collections::HashMap;

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

fn and() -> Netlist {
    synth("module m(input a, b, output y); assign y = a & b; endmodule")
}

fn pattern(a: u64, b: u64) -> eda_fault::Pattern {
    HashMap::from([("a".to_string(), a), ("b".to_string(), b)])
}

pub fn register(c: &mut CaseList) {
    c.add("fault_universe_lists_net_stuck_faults", || {
        let nl = and();
        let faults = eda_fault::fault_universe(&nl);
        // a/b/y each have SA0 and SA1.
        if faults.len() == 6
            && faults.iter().filter(|f| f.net_name == "a").count() == 2
            && faults.iter().filter(|f| f.net_name == "y").count() == 2
        {
            Ok(())
        } else {
            Err(format!("{:?}", faults))
        }
    });

    c.add("fault_exhaustive_and_full_coverage", || {
        let result = eda_fault::grade_automatic(&and(), &eda_fault::PatternOptions::default())?;
        if result.exhaustive
            && result.patterns.len() == 4
            && result.coverage_percent == 100.0
            && result.undetected_faults.is_empty()
        {
            Ok(())
        } else {
            Err(eda_fault::report(&result))
        }
    });

    c.add("fault_custom_pattern_detects_subset", || {
        let patterns = vec![pattern(0, 1), pattern(1, 0)];
        let result = eda_fault::grade_patterns(&and(), &patterns)?;
        let detected_names: Vec<_> = result
            .detections
            .iter()
            .filter(|detection| !detection.detecting_patterns.is_empty())
            .map(|detection| detection.fault.net_name.clone())
            .collect();
        // The first vector detects b-SA1/y-SA1; the second detects a-SA1/y-SA1.
        if result.detected_faults >= 3
            && result.detected_faults < result.total_faults
            && detected_names.contains(&"b".to_string())
            && detected_names.contains(&"a".to_string())
        {
            Ok(())
        } else {
            Err(eda_fault::report(&result))
        }
    });

    c.add("fault_redundant_unused_input_remains_undetected", || {
        let design = eda_hdl::compile(
            "module m(input a, b, output y); assign y = a & a; endmodule",
            None,
        )?;
        let nl = eda_synth::synthesize(
            &design,
            &eda_synth::SynthOptions {
                optimize: false,
                map_lib: false,
            },
        )?
        .netlist;
        let result = eda_fault::grade_automatic(&nl, &eda_fault::PatternOptions::default())?;
        if result
            .undetected_faults
            .iter()
            .any(|fault| fault.net_name == "b")
            && result.coverage_percent < 100.0
        {
            Ok(())
        } else {
            Err(eda_fault::report(&result))
        }
    });

    c.add("fault_adder_exhaustive_full_coverage", || {
        let nl = synth("module m(input [3:0] a, b, output [3:0] s); assign s = a + b; endmodule");
        let result = eda_fault::grade_automatic(&nl, &eda_fault::PatternOptions::default())?;
        if result.exhaustive && result.coverage_percent >= 95.0 {
            Ok(())
        } else {
            Err(eda_fault::report(&result))
        }
    });

    c.add("fault_random_generation_for_wide_inputs", || {
        let nl = synth("module m(input [7:0] a, b, output [7:0] y); assign y = a ^ b; endmodule");
        let options = eda_fault::PatternOptions {
            exhaustive_input_limit: 8,
            max_random_patterns: 64,
            random_seed: 12345,
        };
        let result = eda_fault::grade_automatic(&nl, &options)?;
        if !result.exhaustive && result.patterns.len() == 64 && result.coverage_percent > 50.0 {
            Ok(())
        } else {
            Err(eda_fault::report(&result))
        }
    });

    c.add("fault_sequential_design_rejected", || {
        let nl =
            synth("module m(input clk, d, output reg q); always @(posedge clk) q <= d; endmodule");
        if eda_fault::grade_automatic(&nl, &eda_fault::PatternOptions::default()).is_ok() {
            Err("sequential design unexpectedly graded".into())
        } else {
            Ok(())
        }
    });

    c.add("fault_report_content", || {
        let result = eda_fault::grade_automatic(&and(), &eda_fault::PatternOptions::default())?;
        let text = eda_fault::report(&result);
        if text.contains("Fault report") && text.contains("100.00%") && text.contains("exhaustive")
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_fault_pattern_generation_and_grading", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a & b; endmodule",
            None,
        )?;
        let patterns =
            eda_api::generate_patterns(&flow.netlist, &eda_fault::PatternOptions::default())?;
        let result = eda_api::grade_fault_patterns(&flow.netlist, &patterns)?;
        let automatic =
            eda_api::fault_coverage(&flow.netlist, &eda_fault::PatternOptions::default())?;
        if result.coverage_percent == 100.0
            && automatic.coverage_percent == 100.0
            && eda_api::fault_report(&automatic).contains("Fault report")
        {
            Ok(())
        } else {
            Err(eda_api::fault_report(&automatic))
        }
    });

    c.add("fault_stil_round_trip", || {
        let nl = and();
        let patterns = eda_fault::automatic_patterns(&nl, &eda_fault::PatternOptions::default())?;
        let text = eda_fault::write_stil(&nl, &patterns, "and_fault_patterns")?;
        if !text.contains("STIL 1.0")
            || !text.contains("PatternBurst \"and_fault_patterns\"")
            || !text.contains("a=1; b=1")
        {
            return Err(text);
        }
        let parsed = eda_fault::parse_stil(&text)?;
        if parsed.name != "and_fault_patterns"
            || parsed.input_signals != vec!["a".to_string(), "b".to_string()]
            || parsed.output_signals != vec!["y".to_string()]
            || parsed.vectors.len() != 4
        {
            Err(format!("{:?}", parsed))
        } else if parsed.vectors[3].inputs.get("a").copied() != Some(1)
            || parsed.vectors[3].inputs.get("b").copied() != Some(1)
            || parsed.vectors[3].outputs.get("y").copied() != Some(1)
        {
            Err(format!("{:?}", parsed.vectors[3]))
        } else {
            Ok(())
        }
    });

    c.add("fault_stil_vector_port_round_trip", || {
        let nl = synth("module m(input [3:0] a, b, output [3:0] s); assign s = a ^ b; endmodule");
        let patterns = vec![HashMap::from([
            ("a".to_string(), 0b1010u64),
            ("b".to_string(), 0b0110u64),
        ])];
        let text = eda_fault::write_stil(&nl, &patterns, "xor_vector")?;
        let parsed = eda_fault::parse_stil(&text)?;
        if parsed.vectors[0].inputs.get("a").copied() != Some(0b1010)
            || parsed.vectors[0].inputs.get("b").copied() != Some(0b0110)
            || parsed.vectors[0].outputs.get("s").copied() != Some(0b1100)
        {
            Err(format!("{:?}", parsed.vectors[0]))
        } else {
            Ok(())
        }
    });

    c.add("fault_stil_rejects_nonbinary_value", || {
        let source = r#"
STIL 1.0;
SignalGroups {
  inputs = 'a';
  outputs = 'y';
}
Pattern "vectors" {
  V { a=x; y=0; }
}
"#;
        if eda_fault::parse_stil(source).is_ok() {
            Err("nonbinary STIL value accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("api_fault_patterns_to_stil_and_back", || {
        let flow = eda_api::run_flow(
            "module m(input a, b, output y); assign y = a | b; endmodule",
            None,
        )?;
        let patterns =
            eda_api::generate_patterns(&flow.netlist, &eda_fault::PatternOptions::default())?;
        let text = eda_api::patterns_to_stil(&flow.netlist, &patterns, "or_patterns")?;
        let parsed = eda_api::parse_stil_patterns(&text)?;
        if parsed.name == "or_patterns" && parsed.vectors.len() == 4 {
            Ok(())
        } else {
            Err(format!("{} {}", parsed.name, parsed.vectors.len()))
        }
    });
}
