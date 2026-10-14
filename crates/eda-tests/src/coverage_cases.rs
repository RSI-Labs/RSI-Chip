//! Functional-coverage language and VCD scoring cases.

use crate::CaseList;

fn trace() -> eda_waveform::Vcd {
    eda_waveform::parse_vcd(
        r#"
$timescale 1ns $end
$var wire 1 ! clk $end
$var wire 1 " req $end
$var wire 2 # state [1:0] $end
$enddefinitions $end
#0
0!
0"
b00 #
#10
1!
1"
b01 #
#20
0!
0"
b01 #
#30
1!
1"
b11 #
"#,
    )
    .expect("valid VCD")
}

pub fn register(c: &mut CaseList) {
    c.add("coverage_parse_named_group", || {
        let plan = eda_coverage::parse_coverage(
            "covergroup cg @(posedge clk); cp: coverpoint req { bins hit = {1}; } endgroup",
        )?;
        if plan.groups.len() == 1
            && plan.groups[0].name == "cg"
            && plan.groups[0].clock.as_deref() == Some("clk")
        {
            Ok(())
        } else {
            Err(format!("{:?}", plan.groups))
        }
    });

    c.add("coverage_posedge_sampling", || {
        let plan = eda_coverage::parse_coverage(
            "covergroup g @(posedge clk); cp: coverpoint req { bins hit = {1}; } endgroup",
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        if result.samples == 2 && result.bins[0].hits == 2 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_negedge_sampling", || {
        let plan = eda_coverage::parse_coverage(
            "covergroup g @(negedge clk); cp: coverpoint req { bins hit = {1}; } endgroup",
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        if result.samples == 1 && result.bins[0].hits == 0 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_value_and_range_bins", || {
        let plan = eda_coverage::parse_coverage(
            r#"
covergroup g @(posedge clk);
  cp_state: coverpoint state[1:0] {
    bins zero = {0};
    bins low = {1};
    bins high = {[2:3]};
  }
endgroup
"#,
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        let hits: Vec<_> = result.bins.iter().map(|bin| bin.hits).collect();
        if hits == vec![0, 1, 1] {
            Ok(())
        } else {
            Err(format!("{:?}", hits))
        }
    });

    c.add("coverage_bit_select", || {
        let plan = eda_coverage::parse_coverage(
            "covergroup g @(posedge clk); cp: coverpoint state[0] { bins one = {1}; } endgroup",
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        if result.bins[0].hits == 2 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_cross_combinations", || {
        let plan = eda_coverage::parse_coverage(
            r#"
covergroup g @(posedge clk);
  cp_req: coverpoint req { bins on = {1}; }
  cp_state: coverpoint state[1:0] {
    bins low = {0,1};
    bins high = {[2:3]};
  }
  x: cross cp_req, cp_state;
endgroup
"#,
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        if result.crosses.len() == 2 && result.crosses[0].hits == 1 && result.crosses[1].hits == 1 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_toggle_count", || {
        let plan = eda_coverage::parse_coverage("toggle req;")?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        if result.toggles[0].transitions == 3 && result.toggles[0].samples == 4 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_unknown_values_do_not_hit", || {
        let vcd = eda_waveform::parse_vcd(
            "$var wire 1 ! x $end\n$enddefinitions $end\n#0\nx!\n#1\n1!\n",
        )?;
        let plan = eda_coverage::parse_coverage(
            "covergroup g; cp: coverpoint x { bins one = {1}; } endgroup",
        )?;
        let result = eda_coverage::check_coverage(&vcd, &plan)?;
        if result.bins[0].hits == 1 && result.samples == 2 {
            Ok(())
        } else {
            Err(eda_coverage::report(&result))
        }
    });

    c.add("coverage_percentage_and_report", || {
        let plan = eda_coverage::parse_coverage(
            r#"
covergroup g @(posedge clk);
  cp_state: coverpoint state[1:0] {
    bins zero = {0};
    bins high = {[2:3]};
  }
endgroup
"#,
        )?;
        let result = eda_coverage::check_coverage(&trace(), &plan)?;
        let text = eda_coverage::report(&result);
        if result.percentage == 50.0 && text.contains("Coverage report") && text.contains("BIN") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("coverage_api_parse_and_score", || {
        let plan = eda_api::parse_covergroups(
            "covergroup g @(posedge clk); cp: coverpoint req { bins on = {1}; } endgroup",
        )?;
        let result = eda_api::functional_coverage(&trace(), &plan)?;
        if result.bins[0].hits == 2 && eda_api::coverage_report(&result).contains("100.00%") {
            Ok(())
        } else {
            Err(eda_api::coverage_report(&result))
        }
    });

    for (name, source) in [
        ("empty", "// no coverage"),
        (
            "bad_range",
            "covergroup g; cp: coverpoint a { bins x = {[2:1]}; } endgroup",
        ),
        ("no_bins", "covergroup g; cp: coverpoint a { } endgroup"),
        (
            "missing_signal",
            "covergroup g; cp: coverpoint missing { bins x = {0}; } endgroup",
        ),
    ] {
        c.add(format!("coverage_err_{}", name), move || {
            if let Ok(plan) = eda_coverage::parse_coverage(source) {
                if eda_coverage::check_coverage(&trace(), &plan).is_ok() {
                    return Err(format!("{} unexpectedly passed", name));
                }
            }
            Ok(())
        });
    }
}
