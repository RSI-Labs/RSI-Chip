//! Temporary probe for the i8086 full-core PDK signoff flow.

fn main() {
    let source = format!(
        "{}\n{}\n{}{}",
        include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
        include_str!("../../../platform/rtl/i8086_full_core.sv"),
    );
    let manifest = r#"pdk i8086-signoff-pdk
version 1.0
layer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14 tracks 4096
corner fast process FF voltage_v 1.9 temperature_c -40 delay_scale 0.8
corner slow process SS voltage_v 1.6 temperature_c 125 delay_scale 1.25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#;
    let pdk = eda_api::parse_pdk(manifest).expect("parse pdk manifest");
    match eda_api::run_pdk_signoff(&source, Some("i8086_full_core"), &pdk) {
        Ok(result) => {
            println!("passed={}", result.passed);
            println!("equivalent={}", result.equivalent);
            println!("drc_errors={}", result.drc_errors);
            println!("delay_ns={}", result.critical_path_delay_ns);
            println!("{}", eda_api::report_pdk_signoff(&result));
        }
        Err(error) => {
            eprintln!("signoff failed: {error}");
            std::process::exit(1);
        }
    }
}
