//! Structural layout-versus-schematic regression cases.

use crate::CaseList;
use eda_core::netlist::Netlist;

fn reference() -> Netlist {
    synth("module m(input a, b, output y); assign y = a & b; endmodule")
}

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

pub fn register(c: &mut CaseList) {
    c.add("lvs_identical_netlists_match", || {
        let layout = synth("module m(input a, b, output y); assign y = a & b; endmodule");
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        if !result.matches {
            Err(eda_lvs::report(&result))
        } else {
            Ok(())
        }
    });

    c.add("lvs_renamed_instances_and_internal_nets", || {
        let mut layout = synth("module m(input a, b, output y); assign y = a & b; endmodule");
        for (index, cell) in layout.cells_mut().iter_mut().enumerate() {
            cell.name = format!("layout_cell_{index}");
        }
        // Preserve primary-port names; only internal net names are changed.
        let port_nets: std::collections::HashSet<_> = layout
            .ports
            .iter()
            .flat_map(|port| port.bits.iter().copied())
            .collect();
        for net in 0..layout.net_count() {
            if !port_nets.contains(&net) {
                let name = layout.net(net).name.clone();
                layout.set_net_name(net, format!("layout_{name}"));
            }
        }
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        if !result.matches {
            Err(eda_lvs::report(&result))
        } else {
            Ok(())
        }
    });

    c.add("lvs_connectivity_mismatch_detected", || {
        let layout = synth("module m(input a, b, output y); assign y = a & a; endmodule");
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        if result.matches {
            Err("wrong input connectivity accepted".into())
        } else {
            Ok(())
        }
    });

    c.add("lvs_device_mismatch_detected", || {
        let layout = synth("module m(input a, b, output y); assign y = a | b; endmodule");
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        if result.matches || !result.cell_matches.is_empty() {
            Err(format!(
                "device mismatch accepted: {:?}",
                result.cell_matches
            ))
        } else {
            Ok(())
        }
    });

    c.add("lvs_port_mismatch_detected", || {
        let layout = synth("module m(input a, b, c, output y); assign y = a & b; endmodule");
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        if result.matches || !result.notes.iter().any(|note| note.contains("port count")) {
            Err(format!("port mismatch accepted: {:?}", result.notes))
        } else {
            Ok(())
        }
    });

    c.add("lvs_report_content", || {
        let layout = reference();
        let result = eda_lvs::check_lvs(&reference(), &layout)?;
        let text = eda_lvs::report(&result);
        if text.contains("LVS report") && text.contains("MATCH") {
            Ok(())
        } else {
            Err(text)
        }
    });

    c.add("api_layout_vs_schematic", || {
        let layout = reference();
        let result = eda_api::layout_vs_schematic(&reference(), &layout)?;
        if !result.matches || result.cell_matches.len() != reference().cell_count() {
            Err(eda_api::lvs_report(&result))
        } else {
            Ok(())
        }
    });
}
