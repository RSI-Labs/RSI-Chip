//! VCD reader, waveform query, and waveform-driven power cases.

use crate::CaseList;
use eda_core::netlist::Netlist;
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

fn inverter_vcd() -> String {
    let nl = synth_cached("module m(input a, output y); assign y = ~a; endmodule");
    let design = eda_core::to_design::netlist_to_design(nl);
    let mut sim = eda_sim::Simulator::new(&design).unwrap();
    sim.enable_vcd();
    sim.set_input("a", &eda_core::logic::LogicWord::from_u64(1, 0))
        .unwrap();
    let first = sim.vcd().unwrap();
    sim.set_input("a", &eda_core::logic::LogicWord::from_u64(1, 1))
        .unwrap();
    let mut second = first.clone();
    // The API deliberately exposes only quiescent simulation here; add an
    // explicit later timestamp so this test covers temporal VCD queries too.
    second.push_str("#1\n1!\n0\"\n");
    assert!(first.contains("$var"));
    second
}

fn measured_clock_vcd(period: u64, high: u64) -> String {
    format!(
        "$timescale 1ns $end\n$var wire 1 ! clk $end\n$enddefinitions $end\n\
         #0\n0!\n#1\n1!\n#{fall_1}\n0!\n#{rise_2}\n1!\n#{fall_2}\n0!\n#{rise_3}\n1!\n",
        fall_1 = 1 + high,
        rise_2 = 1 + period,
        fall_2 = 1 + period + high,
        rise_3 = 1 + 2 * period,
    )
}

fn compared_vector_vcd(pattern: u64) -> String {
    let first = pattern & 0xF;
    let second = (pattern >> 4) & 0xF;
    let other_first = ((pattern * 7) >> 2) & 0xF;
    let other_second = ((pattern * 13) >> 4) & 0xF;
    format!(
        "$timescale 1ns $end\n$var wire 4 ! a [3:0] $end\n$var wire 4 \" b [3:0] $end\n\
         $enddefinitions $end\n#0\nb0000 !\nb0000 \"\n#1\nb{:04b} !\nb{:04b} \"\n\
         #2\nb{:04b} !\nb{:04b} \"\n",
        first, other_first, second, other_second
    )
}

fn expected_hamming(a: u64, b: u64) -> usize {
    (a ^ b).count_ones() as usize
}

pub fn register(c: &mut CaseList) {
    c.add("vcd_parse_writer_output", || {
        let text = inverter_vcd();
        let vcd = eda_waveform::parse_vcd(&text)?;
        if vcd.signal("a").is_none() || vcd.signal("top.y").is_none() {
            Err(format!("signals missing in {}", text))
        } else {
            Ok(())
        }
    });
    c.add("vcd_timescale", || {
        let vcd = eda_waveform::parse_vcd(
            "$timescale 10ps $end\n$scope module top $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\n",
        )?;
        if (vcd.timescale_ns - 0.01).abs() > 1e-12 {
            Err(format!("timescale {}", vcd.timescale_ns))
        } else {
            Ok(())
        }
    });
    c.add("vcd_scalar_query_and_transition", || {
        let vcd = eda_waveform::parse_vcd(&inverter_vcd())?;
        let a = vcd.signal("a").ok_or("no a")?;
        let y = vcd.signal("y").ok_or("no y")?;
        if a.value_at(0).unwrap().to_u64() != Some(0)
            || a.final_value().unwrap().to_u64() != Some(1)
            || a.transition_count() != 1
            || y.transition_count() != 1
        {
            Err("scalar query/transition".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_vector_and_indexed_query", || {
        let vcd = eda_waveform::parse_vcd(
            "$timescale 1ns $end\n$var wire 4 ! q [3:0] $end\n$enddefinitions $end\n#0\nb0000 !\n#5\nb1010 !\n",
        )?;
        let q = vcd.signal("q").ok_or("no q")?;
        if q.width != 4
            || q.value_at(5).unwrap().to_u64() != Some(10)
            || q.transition_count() != 1
        {
            Err("vector query".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_x_value_is_not_known", || {
        let vcd = eda_waveform::parse_vcd(
            "$timescale 1ns $end\n$var wire 2 ! q [1:0] $end\n$enddefinitions $end\n#0\nb01 !\n#1\nbxx !\n#2\nb11 !\n",
        )?;
        assert!(vcd.signal("q").unwrap().value_at(1).unwrap().to_u64().is_none());
        Ok(())
    });
    c.add("vcd_duration_and_time_points", || {
        let vcd = eda_waveform::parse_vcd(&inverter_vcd())?;
        if vcd.duration().is_none() || vcd.time_points().is_empty() {
            Err("no waveform duration".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_write_round_trip", || {
        let source = eda_waveform::parse_vcd(
            "$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! a $end\n$var wire 4 \" q [3:0] $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\nb0000 \"\n#4\n1!\n#8\nb1010 \"\n",
        )?;
        let signals: Vec<_> = source.signals().cloned().collect();
        let exported = eda_api::write_vcd(&signals)?;
        let round_trip = eda_waveform::parse_vcd(&exported)?;
        if round_trip.signal("a").unwrap().value_at(4).unwrap().to_u64() != Some(1)
            || round_trip
                .signal("top.q")
                .unwrap()
                .value_at(8)
                .unwrap()
                .to_u64()
                != Some(10)
        {
            Err(exported)
        } else {
            Ok(())
        }
    });
    c.add("vcd_write_rejects_duplicate_time", || {
        let source = eda_waveform::parse_vcd(
            "$timescale 1ns $end\n$var wire 1 ! a $end\n$enddefinitions $end\n#0\n0!\n#1\n1!\n",
        )?;
        let mut signals: Vec<_> = source.signals().cloned().collect();
        let duplicate_value = signals[0].changes[0].1.clone();
        signals[0].changes.push((1, duplicate_value));
        if eda_api::write_vcd(&signals).is_ok() {
            Err("duplicate timestamp accepted".into())
        } else {
            Ok(())
        }
    });
    c.add("vcd_power_from_waveform", || {
        let nl = synth_cached("module m(input a, output y); assign y = ~a; endmodule");
        let vcd = eda_waveform::parse_vcd(&inverter_vcd())?;
        let report = eda_waveform_power(nl, &vcd)?;
        if report.activity_by_net["a"].transitions != 1
            || report.activity_by_net["y"].transitions != 1
            || report.dynamic_power_uw <= 0.0
        {
            Err(format!(
                "a={:?} y={:?} dynamic={}",
                report.activity_by_net["a"], report.activity_by_net["y"], report.dynamic_power_uw
            ))
        } else {
            Ok(())
        }
    });
    c.add("vcd_api_parse_and_power", || {
        let text = inverter_vcd();
        let vcd = eda_api::parse_vcd(&text)?;
        let src: &'static str = "module m(input a, output y); assign y = ~a; endmodule";
        let flow = eda_api::run_flow(src, None)?;
        let report =
            eda_api::power_from_waveform(&flow.netlist, &vcd, &eda_api::PowerOptions::default())?;
        if report.vectors < 2 || report.total_power_uw <= report.static_power_uw {
            Err(format!(
                "samples={} total={} leakage={}",
                report.vectors, report.total_power_uw, report.static_power_uw
            ))
        } else {
            Ok(())
        }
    });
    for period in 2_u64..=101 {
        let high = 1 + (period - 2) / 3;
        c.add(
            format!("waveform_measure_clock_period_{period:03}"),
            move || {
                let vcd = eda_waveform::parse_vcd(&measured_clock_vcd(period, high))?;
                let metrics = vcd.clock_metrics("clk")?;
                let expected_duty = 100.0 * high as f64 / period as f64;
                if metrics.period_count != 2
                    || metrics.rise_count != 3
                    || metrics.fall_count != 2
                    || metrics.min_period_ns != period as f64
                    || metrics.max_period_ns != period as f64
                    || metrics.min_high_ns != high as f64
                    || metrics.max_high_ns != high as f64
                    || metrics.min_low_ns != (period - high) as f64
                    || metrics.max_low_ns != (period - high) as f64
                    || (metrics.duty_cycle_percent - expected_duty).abs() > 1e-12
                {
                    Err(format!(
                        "period={} high={} metrics={:?}",
                        period, high, metrics
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }
    for pattern in 0_u64..100 {
        c.add(
            format!("waveform_compare_vectors_{pattern:03}"),
            move || {
                let a_first = pattern & 0xF;
                let a_second = (pattern >> 4) & 0xF;
                let b_first = ((pattern * 7) >> 2) & 0xF;
                let b_second = ((pattern * 13) >> 4) & 0xF;
                let expected =
                    expected_hamming(a_first, b_first) + expected_hamming(a_second, b_second);
                let vcd = eda_waveform::parse_vcd(&compared_vector_vcd(pattern))?;
                let comparison = vcd.compare_signals("a", "b")?;
                if comparison.sample_count != 3
                    || comparison.unknown_samples != 0
                    || comparison.hamming_distance != expected
                    || comparison.matching_samples + comparison.differing_samples != 3
                {
                    Err(format!("pattern={} comparison={:?}", pattern, comparison))
                } else {
                    Ok(())
                }
            },
        );
    }
    for (name, text) in [
        ("no_definitions", "#0\n0!\n"),
        (
            "bad_scalar",
            "$var wire 1 ! a $end\n$enddefinitions $end\n#0\nq!\n",
        ),
        (
            "bad_timescale",
            "$timescale 1sec $end\n$var wire 1 ! a $end\n$enddefinitions $end\n#0\n0!\n",
        ),
    ] {
        c.add(format!("vcd_err_{}", name), move || {
            if eda_waveform::parse_vcd(text).is_ok() {
                Err(format!("{} accepted", name))
            } else {
                Ok(())
            }
        });
    }
}

fn eda_waveform_power(
    nl: &Netlist,
    vcd: &eda_waveform::Vcd,
) -> Result<eda_power::PowerReport, String> {
    eda_power::analyze_waveform(nl, vcd, &eda_power::PowerOptions::default())
}
