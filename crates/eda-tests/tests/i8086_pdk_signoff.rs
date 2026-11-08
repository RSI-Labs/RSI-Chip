//! Named i8086 PDK signoff gate: LEC + timing + DRC + physical DB (LEF/DEF/GDS/SDF).

const SOURCE_PARTS: [&str; 4] = [
    include_str!("../../../platform/rtl/i8086_prefix_decoder.sv"),
    include_str!("../../../platform/rtl/i8086_opcode_decoder.sv"),
    include_str!("../../../platform/rtl/i8086_modrm_decoder.sv"),
    include_str!("../../../platform/rtl/i8086_full_core.sv"),
];

const MANIFEST: &str = r#"pdk i8086-signoff-pdk
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

#[test]
fn i8086_pdk_signoff_mmmc_pass() {
    if cfg!(debug_assertions) {
        eprintln!("skipping: run the signoff gate in release");
        return;
    }
    let source = SOURCE_PARTS.join("\n");
    let pdk = eda_api::parse_pdk(MANIFEST).expect("parse i8086 signoff PDK");
    let result = eda_api::run_pdk_signoff(&source, Some("i8086_full_core"), &pdk)
        .expect("run i8086 PDK signoff");
    let report = eda_api::report_pdk_signoff(&result);
    assert!(result.passed, "i8086 signoff failed:\n{report}");
    assert!(result.equivalent, "i8086 signoff LEC failed:\n{report}");
    assert_eq!(result.drc_errors, 0, "i8086 signoff DRC errors:\n{report}");
    assert!(
        result
            .corner_results
            .iter()
            .all(|corner| corner.physical_database_complete),
        "i8086 physical DB incomplete:\n{report}"
    );
    assert!(
        result
            .corner_results
            .iter()
            .all(|corner| corner.gds_bytes > 0),
        "i8086 GDSII export missing:\n{report}"
    );
    assert!(
        result
            .corner_results
            .iter()
            .all(|corner| corner.oasis_bytes > 0),
        "i8086 OASIS export missing:\n{report}"
    );
    assert!(
        result.corner_results.len() == 2,
        "expected fast+slow corners:\n{report}"
    );
}
