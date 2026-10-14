//! UPF power-intent parsing and static validation cases.

use crate::CaseList;
use eda_core::ir::Design;

fn design() -> Design {
    eda_hdl::compile(
        "module m(input sleep, a, output y); assign y = a & sleep; endmodule",
        None,
    )
    .expect("compile")
}

fn upf_text() -> String {
    r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
connect_supply_net vdd -ports {vdd}
connect_supply_net vss -ports {vss}
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch main_sw -domain TOP -input_supply_port vdd -output_supply_port vdd -control_port sleep
set_isolation iso -domain TOP -signal sleep -isolation_power_net vdd -isolation_ground_net vss
set_retention ret -domain TOP -signal sleep -retention_power_net vdd -retention_ground_net vss
set_level_shifter ls -domain TOP -input vdd -output vdd -location self
"#
    .to_string()
}

pub fn register(c: &mut CaseList) {
    c.add("upf_parse_domains_supplies_and_strategies", || {
        let upf = eda_upf::parse_upf(&upf_text())?;
        if upf.version == "2.1"
            && upf.design_top.as_deref() == Some("m")
            && upf.domains.len() == 1
            && upf.supply_ports.len() == 2
            && upf.supply_nets.len() == 2
            && upf.switches.len() == 1
            && upf.isolations.len() == 1
            && upf.retentions.len() == 1
            && upf.level_shifters.len() == 1
        {
            Ok(())
        } else {
            Err(format!("{:?}", upf))
        }
    });

    c.add("upf_supply_net_connection", || {
        let upf = eda_upf::parse_upf(&upf_text())?;
        if upf.supply_nets["vdd"].connected_ports == vec!["vdd".to_string()] {
            Ok(())
        } else {
            Err(format!("{:?}", upf.supply_nets["vdd"]))
        }
    });

    c.add("upf_valid_intent_has_no_errors", || {
        let upf = eda_upf::parse_upf(&upf_text())?;
        let result = eda_upf::check(&design(), &upf);
        if result.has_errors() {
            Err(eda_upf::report(&result))
        } else {
            Ok(())
        }
    });

    c.add("upf_top_mismatch_detected", || {
        let mut source = upf_text();
        source = source.replace("set_design_top m", "set_design_top wrong");
        let upf = eda_upf::parse_upf(&source)?;
        let result = eda_upf::check(&design(), &upf);
        if result.findings.iter().any(|f| f.code == "UPF-TOP") {
            Ok(())
        } else {
            Err(eda_upf::report(&result))
        }
    });

    c.add("upf_unknown_domain_element_detected", || {
        let mut source = upf_text();
        source = source.replace("-elements {m}", "-elements {missing_instance}");
        let upf = eda_upf::parse_upf(&source)?;
        let result = eda_upf::check(&design(), &upf);
        if result.findings.iter().any(|f| f.code == "UPF-ELEMENT") {
            Ok(())
        } else {
            Err(eda_upf::report(&result))
        }
    });

    c.add("upf_missing_primary_supply_detected", || {
        let source = r#"
create_power_domain TOP -elements {m}
create_supply_net vdd -domain TOP
"#;
        let upf = eda_upf::parse_upf(source)?;
        let result = eda_upf::check(&design(), &upf);
        if result.findings.iter().any(|f| f.code == "UPF-GND") {
            Ok(())
        } else {
            Err(eda_upf::report(&result))
        }
    });

    c.add("upf_unknown_strategy_signal_detected", || {
        let mut source = upf_text();
        source = source.replace("-signal sleep", "-signal missing_sleep");
        let upf = eda_upf::parse_upf(&source)?;
        let result = eda_upf::check(&design(), &upf);
        if result.findings.iter().any(|f| f.code == "UPF-SIGNAL") {
            Ok(())
        } else {
            Err(eda_upf::report(&result))
        }
    });

    c.add("upf_report_summarizes_intent", || {
        let upf = eda_upf::parse_upf(&upf_text())?;
        let result = eda_upf::check(&design(), &upf);
        let text = eda_upf::report(&result);
        if text.contains("UPF report")
            && text.contains("1 domain(s)")
            && text.contains("1 isolation(s)")
            && text.contains("0 error(s)")
        {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_upf_parse_and_check", || {
        let design = eda_api::compile(
            "module m(input sleep, a, output y); assign y = a & sleep; endmodule",
            None,
        )?;
        let intent = eda_api::parse_power_intent(&upf_text())?;
        let result = eda_api::power_intent_check(&design, &intent);
        if result.domain_count == 1 && !result.has_errors() {
            Ok(())
        } else {
            Err(eda_api::upf_report(&result))
        }
    });

    for (name, source) in [
        ("empty", "// no UPF"),
        ("bad_command", "create_nonsense x;"),
        (
            "duplicate_domain",
            "create_power_domain A; create_power_domain A;",
        ),
    ] {
        c.add(format!("upf_err_{}", name), move || {
            if eda_upf::parse_upf(source).is_ok() {
                Err(format!("{} accepted", name))
            } else {
                Ok(())
            }
        });
    }
}
