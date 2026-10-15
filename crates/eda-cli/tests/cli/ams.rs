//! CLI integration-test domain.

use super::common::{run_edu, TempFile};

#[test]
fn ams_runs_spice_analyses() {
    let input = TempFile::new(
        "input.spice",
        r#"mixed-signal smoke
Vin in 0 DC 1 AC 1
R1 in out 1k
C1 out 0 1u
.op
.ac lin 2 1 1meg
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("DC operating point: converged=true")
            && report.contains("AC sweep: 2 points (linear)")
            && report.contains("V(out)=0.999980"),
        "{report}"
    );
}

#[test]
fn ams_reports_noise_analysis() {
    let input = TempFile::new(
        "noise.spice",
        r#"noise smoke
Iin 0 out DC 0 AC 1
R1 out 0 1k
.noise V(out) Iin lin 1 1 1
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Noise analysis: output=V(out), input=Iin") && report.contains("V/sqrtHz"),
        "{report}"
    );
}

#[test]
fn ams_reports_monte_carlo_analysis() {
    let input = TempFile::new(
        "monte-carlo.spice",
        r#"monte-carlo smoke
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 16 op V(out) seed=9 rtol=0.1
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=16")
            && report.contains("statistics: min=")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_runs_explicit_monte_carlo_correlation_matrix() {
    let input = TempFile::new(
        "monte-carlo-matrix.spice",
        r#"monte-carlo matrix
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 64 op V(out) seed=19 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 matrix=R1.R|R2.R|-0.75
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=64")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_runs_spatial_monte_carlo_correlation() {
    let input = TempFile::new(
        "monte-carlo-spatial.spice",
        r#"monte-carlo spatial
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.location R1 0 0
.location R2 2 0
.mc 128 op V(out) seed=29 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr=0.8 spatialscale=2
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=128")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_runs_lognormal_monte_carlo_distribution() {
    let input = TempFile::new(
        "monte-carlo-lognormal.spice",
        r#"monte-carlo lognormal
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=31 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=lognormal
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=256")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_runs_full_covariance_monte_carlo() {
    let input = TempFile::new(
        "monte-carlo-covariance.spice",
        r#"monte-carlo covariance
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=321 rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 dist=normal cov=R1.R:2500,R2.R:10000,R1.R|R2.R:-2500
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=256")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_imports_vendor_statistical_model_file() {
    let input = TempFile::new(
        "monte-carlo-vendor.spice",
        r#"monte-carlo vendor
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=77 rtol=0 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0
.end
"#,
    );
    let model = TempFile::new(
        "vendor.stats",
        r#"model vendor_n
param R1.R dist=normal relative=true sigma=0.05
param R2.R dist=normal relative=true sigma=0.05
corr R1.R R2.R -0.5
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path(), "--stats", model.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=256")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_imports_def_locations_for_spatial_monte_carlo() {
    let input = TempFile::new(
        "monte-carlo-def-locations.spice",
        r#"monte-carlo DEF locations
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.mc 256 op V(out) seed=91 rtol=0.05 ctol=0 ltol=0 vtotol=0 kptol=0 bftol=0 spatialcorr=0.7 spatialscale=0.04
.end
"#,
    );
    let def = TempFile::new(
        "locations.def",
        r#"DESIGN spatial_layout ;
ROW ROW_0 core 0 0 N DO 20 BY 1 STEP 100 0 ;
COMPONENTS 2 ;
- R1 RES + PLACED ( 100 0 ) N ;
- R2 RES + PLACED ( 500 0 ) N ;
END COMPONENTS
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path(), "--def-locations", def.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Monte Carlo: target=op, output=V(out), samples=256")
            && report.contains("sigma="),
        "{report}"
    );
}

#[test]
fn ams_imports_resistor_layer_model_table() {
    let input = TempFile::new(
        "layer-resistors.spice",
        r#"layer resistor import
Vin in 0 1
R1 in out LAYER=POLY L=2u W=70n
R2 out 0 LAYER=POLY L=2u W=70n
.op
.end
"#,
    );
    let table = TempFile::new(
        "poly.rmodels",
        r#".rmodel POLY SHEET=50 TC1=0 WOFF=0 MINW=40n MAXW=100n BINS=40n:50:0,100n:60:-5n
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path(), "--rmodels", table.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("DC operating point: converged=true")
            && report.contains("V(out) = 0.500000000 V"),
        "{report}"
    );
}

#[test]
fn ams_reports_sensitivity_analysis() {
    let input = TempFile::new(
        "sensitivity.spice",
        r#"sensitivity smoke
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.sens V(out)
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Sensitivity: target=op, output=V(out)")
            && report.contains("R1.R")
            && report.contains("R2.R"),
        "{report}"
    );
}

#[test]
fn ams_reports_analytic_derivatives() {
    let input = TempFile::new(
        "derivatives.spice",
        r#"derivatives smoke
Vin in 0 1.8
R1 in out 1k
R2 out 0 2k
.deriv V(out) params=R1.R,R2.R,Vin.DC
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: output=V(out)")
            && report.contains("R1.R:")
            && report.contains("gradient=-0.000400000")
            && report.contains("hessian=[0.000000267")
            && report.contains("Vin.DC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_analytic_ac_derivatives() {
    let input = TempFile::new(
        "ac-derivatives.spice",
        r#"AC derivatives smoke
Vin in 0 DC 0 AC 1
R1 in out 1k
C1 out 0 1u
.deriv ac V(out) freq=1k params=R1.R,C1.C,Vin.AC
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: target=ac, frequency=1000 Hz")
            && report.contains("R1.R:")
            && report.contains("C1.C:")
            && report.contains("Vin.AC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_mos_derivatives() {
    let input = TempFile::new(
        "mos-derivatives.spice",
        r#"MOS derivatives smoke
Vin vdd 0 DC 1.8
R1 vdd out 4.7k
M1 out out 0 0 NM W=2u L=180n
.model NM NMOS(KP=120u VTO=450m)
.deriv V(out) params=R1.R,NM.VTO,NM.KP,Vin.DC
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: output=V(out)")
            && report.contains("R1.R:")
            && report.contains("NM.VTO:")
            && report.contains("NM.KP:")
            && report.contains("Vin.DC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_bjt_derivatives() {
    let input = TempFile::new(
        "bjt-derivatives.spice",
        r#"BJT derivatives smoke
Vin vdd 0 DC 1.8
R1 vdd out 4.7k
Q1 out out 0 NPN
.model NPN NPN(IS=1p BF=100 BR=1)
.deriv V(out) params=R1.R,NPN.IS,NPN.BF,Vin.DC,Q1.AREA
.op
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: output=V(out)")
            && report.contains("R1.R:")
            && report.contains("NPN.IS:")
            && report.contains("NPN.BF:")
            && report.contains("Q1.AREA:")
            && report.contains("Vin.DC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_ac_optimization() {
    let input = TempFile::new(
        "ac-optimization.spice",
        r#"AC optimization smoke
Vin in 0 DC 0 AC 1
R1 in out 1k
C1 out 0 1u
.ac lin 2 1k 1k
.optim match ac V(out) goal=0.5 freq=1k params=R1.R:100:10000 population=5 iterations=1 seed=17 tol=1e-9
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Optimization: objective=match, target=ac")
            && report.contains("converged=true")
            && report.contains("r1.r:"),
        "{report}"
    );
}

#[test]
fn ams_reports_nonlinear_diode_ac_derivatives() {
    let input = TempFile::new(
        "nonlinear-ac-derivatives.spice",
        r#"nonlinear AC diode derivatives
Vin in 0 DC 0.8 AC 0.7
R1 in out 1
D1 out 0 DN
.model DN D(IS=1n N=1 TT=10m CJO=10p VJ=700m M=0.5 FC=0.5)
.temp 26.85
.ac lin 1 20 20
.deriv ac V(out) freq=20 params=R1.R,DN.IS,Vin.DC,Vin.AC,DN.TT,DN.CJO,DN.VJ,DN.M,DN.FC
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: target=ac, frequency=20 Hz")
            && report.contains("R1.R:")
            && report.contains("DN.IS:")
            && report.contains("Vin.DC:")
            && report.contains("Vin.AC:")
            && report.contains("DN.TT:")
            && report.contains("DN.CJO:")
            && report.contains("DN.VJ:")
            && report.contains("DN.M:")
            && report.contains("DN.FC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_transient_derivatives() {
    let input = TempFile::new(
        "transient-derivatives.spice",
        r#"transient derivatives smoke
Vin in 0 DC 1
R1 in out 1k
C1 out 0 1u IC=0
.tran 1u 5m method=be
.deriv tran V(out) params=R1.R,C1.C,Vin.DC
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Analytic derivatives: target=tran")
            && report.contains("R1.R:")
            && report.contains("C1.C:")
            && report.contains("Vin.DC:"),
        "{report}"
    );
}

#[test]
fn ams_reports_global_sensitivity_analysis() {
    let input = TempFile::new(
        "sobol.spice",
        r#"sobol smoke
Vin in 0 1.8
R1 in out 1k
R2 out 0 1k
.sobol V(out) samples=64 seed=9
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Global sensitivity: target=op, output=V(out)")
            && report.contains("first-order=")
            && report.contains("total-order="),
        "{report}"
    );
}

#[test]
fn ams_reports_optimization_analysis() {
    let input = TempFile::new(
        "optimization.spice",
        r#"optimization smoke
Vin in 0 1.8
R1 in out 1k
R2 out 0 1k
.optim match V(out) goal=1.2 params=R2.R:100:10000 population=8 iterations=16
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Optimization: objective=match, target=op")
            && report.contains("converged=")
            && report.contains("r2.r"),
        "{report}"
    );
}

#[test]
fn ams_reports_quantized_optimization_grid() {
    let input = TempFile::new(
        "quantized-optimization.spice",
        r#"optimization grid
Vin in 0 1.8
R1 in out 1k
R2 out 0 1k
.optim match V(out) goal=1.2 params=R2.R:100:4777:1000:100 population=8 iterations=16
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("quantum=100")
            && report.contains("final=2000")
            && report.contains("bounds=[100,4777]"),
        "{report}"
    );
}

#[test]
fn ams_reports_pareto_optimization_front() {
    let input = TempFile::new(
        "pareto.spice",
        r#"pareto smoke
Vin in 0 1.8
R1 in a 1k
R2 a b 1k
R3 b 0 1k
.pareto V(a) V(b) sense=min,max front=nsga3 divisions=3 adaptive=true constraints=(V(a)+2*V(b))<=3 params=R1.R:500:5000,R3.R:500:5000 population=12 iterations=16 seed=11 points=4
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Pareto optimization: target=op")
            && report.contains("senses=min,max")
            && report.contains("front=reference")
            && report.contains("adaptive=true")
            && report.contains("constraints=(V(a)+(2*V(b)))<=3")
            && report.contains("constraint_violation=")
            && report.contains("crowding=")
            && report.contains("point 0:")
            && report.contains("r1.r=")
            && report.contains("r3.r="),
        "{report}"
    );
}

#[test]
fn ams_reports_periodic_steady_state_analysis() {
    let input = TempFile::new(
        "pss.spice",
        r#"pss smoke
Vin in 0 SIN(0 1 1k)
R1 in out 1k
C1 out 0 1u
.tran 1u 10m method=be
.pss V(out) fund=1k min=8 max=32 samples=64 harmonics=4
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("PSS: output=V(out), fundamental=1000")
            && report.contains("harmonic 1")
            && report.contains("magnitude="),
        "{report}"
    );
}

#[test]
fn ams_reports_envelope_analysis() {
    let input = TempFile::new(
        "envelope.spice",
        r#"envelope smoke
Vin out 0 AM(0 1 100k 1k 0.8)
.tran 100n 10m method=be
.env V(out) carrier=100k cycles=8 points=128
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Envelope: output=V(out), carrier=100000")
            && report.contains("envelope=")
            && report.contains("phase="),
        "{report}"
    );
}
