use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::Command;

struct TempFile {
    path: std::path::PathBuf,
}

impl TempFile {
    fn new(name: &str, contents: &str) -> Self {
        let path = std::env::temp_dir().join(format!("eda-cli-{}-{}", std::process::id(), name));
        fs::write(&path, contents).expect("write temporary CLI input");
        Self { path }
    }

    fn path(&self) -> &str {
        self.path.to_str().expect("temporary path is UTF-8")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn run_edu(args: &[&str]) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_eda"))
        .args(args)
        .output()
        .expect("run eda binary");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

#[test]
fn sta_accepts_waiver_file() {
    let input = TempFile::new(
        "input.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "input.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let waivers = TempFile::new(
        "waivers.waivers",
        r#"waiver "accepted" "clk" "y" "-1.0"
kind output
owner "timing"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing violation report for 'clk'")
            && report.contains("waived violations: 1"),
        "{report}"
    );
}

#[test]
fn sta_accepts_hierarchy_scoped_waiver_file() {
    let input = TempFile::new(
        "hierarchy-waiver.v",
        "module inv(input a, output y); assign y = ~a; endmodule
         module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
         module top(input a, output y1, y2);
            sub u1(.a(a), .y(y1));
            sub u2(.a(a), .y(y2));
         endmodule",
    );
    let sdc = TempFile::new(
        "hierarchy-waiver.sdc",
        r#"
create_clock -name clk -period 0.1 [get_ports a]
set_output_delay 0.2 -clock clk [get_ports y1]
set_output_delay 0.2 -clock clk [get_ports y2]
"#,
    );
    let waivers = TempFile::new(
        "hierarchy-waiver.waivers",
        r#"
waiver "one-instance" "clk" "*" "-10.0"
instance "top.u1.u_leaf"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--top",
        "top",
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing violation report for 'clk'")
            && report.contains("waived violations: 1")
            && report.contains("hierarchy: top.u2.u_leaf"),
        "{report}"
    );
}

#[test]
fn info_reports_elaborated_hierarchy() {
    let input = TempFile::new(
        "info-hierarchy.v",
        "module leaf(input a, output y); assign y = ~a; endmodule
         module sub(input a, output y); leaf u_leaf(.a(a), .y(y)); endmodule
         module top(input a, output y); sub u1(.a(a), .y(y)); endmodule",
    );
    let (ok, report) = run_edu(&["info", input.path(), "--top", "top"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("hierarchy: 3 instance(s)")
            && report.contains("top.u1")
            && report.contains("top.u1.u_leaf"),
        "{report}"
    );
}

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

#[test]
fn capabilities_report_is_available_without_a_design() {
    let (ok, report) = run_edu(&["capabilities"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("EDA capability audit")
            && report.contains("mosfet_bjt_behavioral")
            && report.contains("summary: implemented="),
        "{report}"
    );
}

#[test]
fn daemon_command_serves_json_rpc() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_eda"))
        .args(["daemon", "--listen", "127.0.0.1:0"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn EDA daemon");
    let stdout = child.stdout.take().expect("daemon stdout");
    let mut banner = String::new();
    BufReader::new(stdout)
        .read_line(&mut banner)
        .expect("read daemon banner");
    let port = banner
        .rsplit(':')
        .next()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .expect("daemon banner contains a port");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon");
    stream
        .write_all(b"{\"id\":41,\"method\":\"ping\",\"params\":{}}\n")
        .expect("write daemon request");
    stream.flush().expect("flush daemon request");
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("close daemon write side");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    child.kill().expect("stop daemon");
    let _ = child.wait();
    assert!(banner.contains("EDA daemon listening on"), "{banner}");
    assert!(response.contains("\"ok\":true"), "{response}");
}

#[test]
fn pdk_signoff_reports_correlation_and_implementation_checks() {
    let input = TempFile::new(
        "pdk-input.v",
        "module top(input a, output y); assign y = ~a; endmodule",
    );
    let pdk = TempFile::new(
        "input.pdk",
        r#"pdk test-pdk
version 1.2
layer met1 routing pitch_um 0.34 min_width_um 0.14 min_spacing_um 0.14
corner tt process TT voltage_v 1.8 temperature_c 25
correlate nmos VTO nominal 0.45 golden 0.452 tolerance 0.01
signoff model-correlation
signoff equivalence
signoff timing
signoff drc
signoff physical-database
.end
"#,
    );
    let (ok, report) = run_edu(&["pdk", input.path(), "--top", "top", "--pdk", pdk.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Model correlation nmos.VTO")
            && report.contains("PDK corner result tt")
            && report.contains("Check equivalence: PASS")
            && report.contains("Check physical-database: PASS")
            && report.contains("Overall signoff: PASS"),
        "{report}"
    );
}

#[test]
fn sta_accepts_mmsc_waiver_audit_file() {
    let input = TempFile::new(
        "mmsc-input.v",
        "module inv(input a, output y); assign y = ~a; endmodule
         module sub(input a, output y); inv u_leaf(.a(a), .y(y)); endmodule
         module top(input a, output y1, y2);
            sub u1(.a(a), .y(y1));
            sub u2(.a(a), .y(y2));
         endmodule",
    );
    let mmsc = TempFile::new(
        "input.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.2 -clock clk [get_ports y1]
    set_output_delay 0.2 -clock clk [get_ports y2]
endscenario
scenario fast
    create_clock -name clk -period 0.2 [get_ports a]
    set_output_delay 0.2 -clock clk [get_ports y1]
    set_output_delay 0.2 -clock clk [get_ports y2]
endscenario
"#,
    );
    let audit = TempFile::new(
        "waiver-audit.audit",
        r#"
audit 1 added 1 "engineer"
waiver "fast-only" "fast" "*" "-10.0"
instance "top.u1.u_leaf"
end
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--top",
        "top",
        "--mmsc",
        mmsc.path(),
        "--waiver-audit",
        audit.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner timing violation report")
            && report.contains("Timing waiver audit: 1 event(s)")
            && report.contains("waived violations: 1")
            && report.contains("Timing waiver audit hierarchy distribution: 1 instance scope(s)")
            && report.contains("'top.u1.u_leaf': events=1, added=1, removed=0, active=1"),
        "{report}"
    );
}

#[test]
fn sta_accepts_min_pulse_waiver_file() {
    let input = TempFile::new(
        "min-pulse.v",
        "module shift(input clk, d, output reg q1, q2);
            always @(posedge clk) q1 <= d;
            always @(posedge clk) q2 <= q1;
        endmodule",
    );
    let sdc = TempFile::new(
        "min-pulse.sdc",
        r#"
create_clock -name clk -period 4.0 -fall 0.1 [get_ports clk]
set_input_delay 1.0 -clock clk [get_ports d]
"#,
    );
    let waivers = TempFile::new(
        "min-pulse.waivers",
        r#"waiver "short-clock" "clk" "*" "-1.0"
kind min_pulse_high
owner "timing"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("min pulse high violation") && report.contains("waived violations: 2"),
        "{report}"
    );
}

#[test]
fn sta_accepts_explicit_delay_limit_waiver_file() {
    let input = TempFile::new(
        "delay-limit.v",
        "module m(input a, b, output y); assign y = a & b; endmodule",
    );
    let sdc = TempFile::new(
        "delay-limit.sdc",
        r#"
create_clock -name clk -period 4.0 [get_ports clk]
set_max_delay 0.5 -from [get_ports a] -to [get_ports y]
set_min_delay 1.5 -from [get_ports a] -to [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "delay-limit.waivers",
        r#"waiver "accepted-max" "clk" "y" "-1.0"
kind max_delay
owner "timing"
"#,
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("min_delay clk:min_delay:y") && report.contains("waived violations: 1"),
        "{report}"
    );
}

#[test]
fn sta_exports_waiver_audit_and_active_waivers() {
    let input = TempFile::new(
        "waiver-export.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "waiver-export.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let source = TempFile::new(
        "waiver-export.waivers",
        r#"waiver "accepted" "clk" "y" "-1.0"
kind output
owner "timing"
"#,
    );
    let audit_output = TempFile::new("waiver-export.audit", "");
    let waiver_output = TempFile::new("waiver-export-round.waivers", "");

    let (first_ok, first_report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        source.path(),
        "--write-waiver-audit",
        audit_output.path(),
    ]);
    assert!(first_ok, "{first_report}");
    let audit_text = fs::read_to_string(audit_output.path()).expect("read exported waiver audit");
    let audit = eda_api::parse_timing_waiver_audit(&audit_text).expect("parse exported audit");
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].waiver_id, "accepted");

    let (second_ok, second_report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waiver-audit",
        audit_output.path(),
        "--write-waivers",
        waiver_output.path(),
    ]);
    assert!(second_ok, "{second_report}");
    let waiver_text =
        fs::read_to_string(waiver_output.path()).expect("read exported active waivers");
    let waivers =
        eda_api::parse_timing_violation_waivers(&waiver_text).expect("parse exported waivers");
    assert_eq!(waivers.len(), 1);
    assert_eq!(waivers[0].id, "accepted");
    assert_eq!(waivers[0].kind, Some(eda_api::TimingViolationKind::Output));
}

#[test]
fn ams_reports_power_law_aging_analysis() {
    let input = TempFile::new(
        "aging.spice",
        r#"aging smoke
Iin out 0 1m
R1 out 0 2k
.aging V(out) years=9 exp=0.5 rinc=0.1
.end
"#,
    );
    let (ok, report) = run_edu(&["ams", input.path()]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Aging: target=op, output=V(out)")
            && report.contains("stress=3.000000000")
            && report.contains("nominal=-2.000000000")
            && report.contains("aged=-2.600000000")
            && report.contains("delta=-0.600000000"),
        "{report}"
    );
}

#[test]
fn eco_repairs_hold_violations() {
    let input = TempFile::new(
        "hold-repair.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let sdc = TempFile::new(
        "hold-repair.sdc",
        r#"
create_clock -name clk -period 8.0 [get_ports clk]
set_input_delay 0.0 -clock clk [get_ports d]
set_clock_uncertainty 1.2 -hold [get_clocks clk]
"#,
    );
    let (ok, report) = run_edu(&["eco", input.path(), "--sdc", sdc.path(), "--repair-hold"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Hold repair: 2 buffer(s), 1 endpoint(s), status PASS")
            && report.contains("slack -1.400000 -> 0.200000 ns"),
        "{report}"
    );
}

#[test]
fn eco_repairs_multicorner_hold_violations() {
    let input = TempFile::new(
        "hold-repair-mmsc.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "hold-repair-mmsc.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 12.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 2.0 -hold [get_clocks clk]
endscenario
scenario fast
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 1.2 -hold [get_clocks clk]
endscenario
"#,
    );
    let (ok, report) = run_edu(&["eco", input.path(), "--mmsc", mmsc.path(), "--repair-hold"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner hold repair:")
            && report.contains("status PASS")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_hold_repair() {
    let input = TempFile::new(
        "flow-mmsc-hold.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-hold.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 12.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 1.0 -hold [get_clocks clk]
endscenario
scenario fast
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.0 -clock clk [get_ports d]
    set_clock_uncertainty 0.8 -hold [get_clocks clk]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--repair-hold",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("Multi-corner hold repair:")
            && report.contains("status PASS"),
        "{report}"
    );
}

#[test]
fn flow_accepts_single_corner_hold_repair() {
    let input = TempFile::new(
        "flow-hold.v",
        "module m(input clk, d, output reg q, output y);
            always @(posedge clk) q <= d;
            assign y = ~d;
        endmodule",
    );
    let sdc = TempFile::new(
        "flow-hold.sdc",
        r#"
create_clock -name clk -period 8.0 [get_ports clk]
set_input_delay 0.0 -clock clk [get_ports d]
set_clock_uncertainty 1.2 -hold [get_clocks clk]
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--physical-timing",
        "--repair-hold",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("Hold repair:")
            && report.contains("status PASS"),
        "{report}"
    );
}

#[test]
fn flow_exports_best_physical_timing_views() {
    let input = TempFile::new(
        "flow-physical-export.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let lef = TempFile::new("flow-physical-export.lef", "");
    let def = TempFile::new("flow-physical-export.def", "");
    let spef = TempFile::new("flow-physical-export.spef", "");
    let sdf = TempFile::new("flow-physical-export.sdf", "");
    let verilog = TempFile::new("flow-physical-export.best.v", "");
    let upf = TempFile::new(
        "flow-physical-export.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
set_active_power_mode active
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--upf",
        upf.path(),
        "--all-power-modes",
        "--timing-ripgup",
        "1",
        "--lef",
        lef.path(),
        "--def",
        def.path(),
        "--extract-spef",
        spef.path(),
        "--write-sdf",
        sdf.path(),
        "--write-verilog",
        verilog.path(),
    ]);
    assert!(ok, "{report}");
    let lef_text = fs::read_to_string(lef.path()).expect("read best LEF");
    let def_text = fs::read_to_string(def.path()).expect("read best DEF");
    let spef_text = fs::read_to_string(spef.path()).expect("read best SPEF");
    let sdf_text = fs::read_to_string(sdf.path()).expect("read best SDF");
    let verilog_text = fs::read_to_string(verilog.path()).expect("read best Verilog");
    let parsed_def = eda_api::parse_physical_def(&def_text).expect("parse best DEF");
    let parsed_spef = eda_api::parse_parasitics(&spef_text).expect("parse best SPEF");
    let parsed_sdf = eda_api::parse_delay_file(&sdf_text).expect("parse best SDF");
    let reparsed_flow =
        eda_api::run_flow(&verilog_text, Some("m")).expect("verify best physical Verilog");
    assert!(
        report.contains("Physical timing flow:")
            && lef_text.contains("MACRO ")
            && parsed_def.tracks_per_channel > 0
            && !parsed_spef.nets.is_empty()
            && !parsed_sdf.interconnects.is_empty()
            && report.contains("Best physical Verilog export:")
            && report.contains("UPF IR report")
            && report.contains("DOMAIN 'TOP'")
            && report.contains("UPF power-mode IR report:")
            && report.contains("power mode 'active'")
            && report.contains("power mode 'sleep'")
            && reparsed_flow.equivalent,
        "{report}\n{lef_text}\n{def_text}\n{spef_text}\n{verilog_text}"
    );
}

#[test]
fn flow_write_sdf_requires_physical_timing_and_mmsc_template() {
    let input = TempFile::new(
        "flow-write-sdf-requires.v",
        "module m(input a, output y); assign y = a; endmodule",
    );
    let sdf = TempFile::new("flow-write-sdf-requires.sdf", "");
    let verilog = TempFile::new("flow-write-sdf-requires.best.v", "");
    let (no_timing_ok, no_timing_report) =
        run_edu(&["flow", input.path(), "--write-sdf", sdf.path()]);
    assert!(!no_timing_ok);
    assert!(
        no_timing_report.contains("--write-sdf requires --physical-timing"),
        "{no_timing_report}"
    );

    let (no_verilog_timing_ok, no_verilog_timing_report) =
        run_edu(&["flow", input.path(), "--write-verilog", verilog.path()]);
    assert!(!no_verilog_timing_ok);
    assert!(
        no_verilog_timing_report.contains("--write-verilog requires --physical-timing"),
        "{no_verilog_timing_report}"
    );

    let (no_post_route_timing_ok, no_post_route_timing_report) =
        run_edu(&["flow", input.path(), "--post-route-power"]);
    assert!(!no_post_route_timing_ok);
    assert!(
        no_post_route_timing_report.contains("--post-route-power requires --physical-timing"),
        "{no_post_route_timing_report}"
    );

    let (all_modes_no_upf_ok, all_modes_no_upf_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--all-power-modes",
    ]);
    assert!(!all_modes_no_upf_ok);
    assert!(
        all_modes_no_upf_report.contains("--all-power-modes requires --upf"),
        "{all_modes_no_upf_report}"
    );

    let mmsc = TempFile::new(
        "flow-write-sdf-requires.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
"#,
    );
    let (mmmc_ok, mmmc_report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--write-sdf",
        sdf.path(),
    ]);
    assert!(!mmmc_ok);
    assert!(
        mmmc_report.contains("--write-sdf with --mmsc requires a '{corner}' template"),
        "{mmmc_report}"
    );
}

#[test]
fn ir_accepts_power_mesh_analysis() {
    let input = TempFile::new(
        "ir-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&["ir", input.path(), "--power-mesh"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Power-mesh IR report:")
            && report.contains("NODE (")
            && report.contains("CELL '"),
        "{report}"
    );

    let (wrong_command_ok, wrong_command_report) = run_edu(&["pnr", input.path(), "--power-mesh"]);
    assert!(!wrong_command_ok);
    assert!(
        wrong_command_report.contains("--power-mesh requires the ir or flow command"),
        "{wrong_command_report}"
    );

    let (missing_power_ok, missing_power_report) =
        run_edu(&["flow", input.path(), "--physical-timing", "--power-mesh"]);
    assert!(!missing_power_ok);
    assert!(
        missing_power_report.contains("--power-mesh in flow requires --post-route-power"),
        "{missing_power_report}"
    );

    let (flow_ok, flow_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--power-mesh",
        "--timing-ripgup",
        "1",
    ]);
    assert!(flow_ok, "{flow_report}");
    assert!(
        flow_report.contains("Post-route power/IR:")
            && flow_report.contains("Power-mesh IR report:")
            && flow_report.contains("worst segment"),
        "{flow_report}"
    );
}

#[test]
fn ir_accepts_multilayer_power_mesh_analysis() {
    let input = TempFile::new(
        "ir-multilayer-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&["ir", input.path(), "--multilayer-power-mesh"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multilayer power-mesh IR report:")
            && report.contains("via Vdd")
            && report.contains("via Gnd")
            && report.contains("worst drop:"),
        "{report}"
    );

    let (wrong_command_ok, wrong_command_report) =
        run_edu(&["pnr", input.path(), "--multilayer-power-mesh"]);
    assert!(!wrong_command_ok);
    assert!(
        wrong_command_report.contains("--multilayer-power-mesh requires the ir or flow command"),
        "{wrong_command_report}"
    );

    let (conflict_ok, conflict_report) = run_edu(&[
        "ir",
        input.path(),
        "--power-mesh",
        "--multilayer-power-mesh",
    ]);
    assert!(!conflict_ok);
    assert!(
        conflict_report.contains("choose either --power-mesh or --multilayer-power-mesh"),
        "{conflict_report}"
    );
}

#[test]
fn flow_accepts_multilayer_post_route_power_mesh() {
    let input = TempFile::new(
        "flow-multilayer-power-mesh.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--post-route-power",
        "--multilayer-power-mesh",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Post-route power/IR:")
            && report.contains("Multilayer power-mesh IR report:")
            && report.contains("via Vdd")
            && report.contains("worst drop:"),
        "{report}"
    );
}

#[test]
fn ir_accepts_coupled_power_domain_mesh() {
    let input = TempFile::new(
        "ir-coupled-power-domains.v",
        "module m(input a, b, c, d, output y, z);
            assign y = a & b;
            assign z = c | d;
         endmodule",
    );
    let upf = TempFile::new(
        "ir-coupled-power-domains.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode active
set_active_power_mode active
"#,
    );
    let domains = TempFile::new(
        "ir-coupled-power-domains.mesh",
        r#"
supply DOMAIN_A 0.8
tap 0 0 0 vdd
end
supply DOMAIN_B 0.65
tap 0 0 2 vdd
end
"#,
    );
    let (ok, report) = run_edu(&[
        "ir",
        input.path(),
        "--upf",
        upf.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multilayer power-mesh IR report:")
            && report.contains("domain 'DOMAIN_A' V=0.800000")
            && report.contains("domain 'DOMAIN_B' V=0.650000")
            && report.contains("worst EM"),
        "{report}"
    );

    let (missing_upf_ok, missing_upf_report) = run_edu(&[
        "ir",
        input.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(!missing_upf_ok);
    assert!(
        missing_upf_report.contains("--power-mesh-domains requires --upf"),
        "{missing_upf_report}"
    );
}

#[test]
fn mmsc_flow_accepts_coupled_power_domain_mesh() {
    let input = TempFile::new(
        "mmsc-coupled-power-domains.v",
        "module m(input a, b, c, output z);
            wire n;
            assign n = a & b;
            assign z = c | n;
         endmodule",
    );
    let mmsc = TempFile::new(
        "mmsc-coupled-power-domains.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
endscenario
"#,
    );
    let upf = TempFile::new(
        "mmsc-coupled-power-domains.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain DOMAIN_A -elements {u1_and2}
create_power_domain DOMAIN_B -elements {u2_or2}
create_supply_net vdda -domain DOMAIN_A
create_supply_net vssa -domain DOMAIN_A
create_supply_net vddb -domain DOMAIN_B
create_supply_net vssb -domain DOMAIN_B
set_domain_supply_net DOMAIN_A -primary_power_net vdda -primary_ground_net vssa
set_domain_supply_net DOMAIN_B -primary_power_net vddb -primary_ground_net vssb
create_power_mode active
set_power_mode_domain_activity active DOMAIN_A 1.0
set_power_mode_domain_activity active DOMAIN_B 0.5
set_active_power_mode active
"#,
    );
    let domains = TempFile::new(
        "mmsc-coupled-power-domains.mesh",
        r#"
supply DOMAIN_A 0.8
tap 0 0 0 vdd
end
supply DOMAIN_B 0.65
tap 0 0 2 vdd
end
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--post-route-power",
        "--all-power-modes",
        "--upf",
        upf.path(),
        "--multilayer-power-mesh",
        "--power-mesh-domains",
        domains.path(),
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner UPF power-mode IR report:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("coupled_multilayer_mesh=true")
            && report.contains("domain 'DOMAIN_A' V=0.800000")
            && report.contains("domain 'DOMAIN_B' V=0.650000"),
        "{report}"
    );
}

#[test]
fn sta_paths_include_hierarchy_grouping() {
    let input = TempFile::new(
        "sta-hierarchy-paths.v",
        "module leaf(input a, b, output n1, n2);
            assign n1 = a & b;
            assign n2 = ~(a | b);
         endmodule
         module sub(input a, b, output y);
            wire n1, n2;
            leaf u_leaf(.a(a), .b(b), .n1(n1), .n2(n2));
            assign y = n1 & n2;
         endmodule
         module top(input a, b, output y);
            sub u1(.a(a), .b(b), .y(y));
         endmodule",
    );
    let (ok, report) = run_edu(&["sta", input.path(), "--top", "top", "--paths", "2"]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing path report:")
            && report.contains("Hierarchical timing path report:")
            && report.contains("instance 'top.u1.u_leaf'"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_import_def_and_exports_best() {
    let input = TempFile::new(
        "flow-mmsc-import.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let imported = TempFile::new("flow-mmsc-import.in.def", "");
    let (export_ok, export_report) = run_edu(&["pnr", input.path(), "--def", imported.path()]);
    assert!(export_ok, "{export_report}");
    let mmsc = TempFile::new(
        "flow-mmsc-import.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
    set_scenario_property post_route.voltage_v 0.90
    set_scenario_property post_route.vdd_ohm_per_site 0.05
    set_scenario_property post_route.gnd_ohm_per_site 0.05
    set_scenario_property post_route.domain.TOP.voltage_v 0.85
    set_scenario_property post_route.domain.TOP.vdd_ohm_per_site 0.07
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let lef = TempFile::new("flow-mmsc-import.lef", "");
    let def = TempFile::new("flow-mmsc-import.out.def", "");
    let spef = TempFile::new("flow-mmsc-import.spef", "");
    let verilog = TempFile::new("flow-mmsc-import.best.v", "");
    let upf = TempFile::new(
        "flow-mmsc-import.upf",
        r#"
upf_version 2.1
set_design_top m
create_power_domain TOP -elements {m}
create_supply_port vdd -domain TOP
create_supply_port vss -domain TOP
create_supply_net vdd -domain TOP
create_supply_net vss -domain TOP
set_domain_supply_net TOP -primary_power_net vdd -primary_ground_net vss
create_power_switch sw -domain TOP -input_supply_port vdd -output_supply_port vdd
set_power_switch_on_resistance sw 10.0
set_power_switch_off_leakage sw 1e-9
create_power_mode active
set_power_mode_switch active sw on
create_power_mode sleep
set_power_mode_switch sleep sw off
set_active_power_mode active
"#,
    );
    let slow_sdf = TempFile::new("flow-mmsc-import.slow.sdf", "");
    let fast_sdf = TempFile::new("flow-mmsc-import.fast.sdf", "");
    let sdf_template = slow_sdf.path().replace(".slow.sdf", ".{corner}.sdf");
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--post-route-power",
        "--upf",
        upf.path(),
        "--all-power-modes",
        "--power-mesh",
        "--import-def",
        imported.path(),
        "--timing-ripgup",
        "1",
        "--lef",
        lef.path(),
        "--def",
        def.path(),
        "--extract-spef",
        spef.path(),
        "--write-verilog",
        verilog.path(),
        "--write-sdf",
        &sdf_template,
    ]);
    assert!(ok, "{report}");
    let lef_text = fs::read_to_string(lef.path()).expect("read best MMMC LEF");
    let def_text = fs::read_to_string(def.path()).expect("read best MMMC DEF");
    let spef_text = fs::read_to_string(spef.path()).expect("read best MMMC SPEF");
    let verilog_text = fs::read_to_string(verilog.path()).expect("read best MMMC Verilog");
    let slow_sdf_text = fs::read_to_string(slow_sdf.path()).expect("read best MMMC slow SDF");
    let fast_sdf_text = fs::read_to_string(fast_sdf.path()).expect("read best MMMC fast SDF");
    let parsed_def = eda_api::parse_physical_def(&def_text).expect("parse best MMMC DEF");
    let parsed_spef = eda_api::parse_parasitics(&spef_text).expect("parse best MMMC SPEF");
    let parsed_slow_sdf =
        eda_api::parse_delay_file(&slow_sdf_text).expect("parse best MMMC slow SDF");
    let parsed_fast_sdf =
        eda_api::parse_delay_file(&fast_sdf_text).expect("parse best MMMC fast SDF");
    let reparsed_flow =
        eda_api::run_flow(&verilog_text, Some("m")).expect("verify best MMMC Verilog");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("MMMC SDF exports: slow=")
            && report.contains(", fast=")
            && report.contains("Best physical Verilog export:")
            && report.contains("Multi-corner UPF power-mode IR report:")
            && report.contains("UPF IR report")
            && report.contains("DOMAIN 'TOP'")
            && report.contains("power mode 'active'")
            && report.contains("power mode 'sleep'")
            && report.contains("Power-mesh IR report:")
            && report.contains("worst drop 'slow' / 'active'")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && reparsed_flow.equivalent
            && lef_text.contains("MACRO ")
            && parsed_def.tracks_per_channel > 0
            && !parsed_spef.nets.is_empty()
            && !parsed_slow_sdf.interconnects.is_empty()
            && !parsed_fast_sdf.interconnects.is_empty(),
        "{report}\n{lef_text}\n{def_text}\n{spef_text}\n{verilog_text}"
    );
}

#[test]
fn waiver_export_requires_a_waiver_source() {
    let input = TempFile::new(
        "waiver-export-source.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "waiver-export-source.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let output = TempFile::new("waiver-export-source.waivers", "");
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--write-waivers",
        output.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("waiver export requires --waivers or --waiver-audit"),
        "{report}"
    );
}

#[test]
fn sta_rejects_conflicting_waiver_inputs() {
    let input = TempFile::new(
        "conflict.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "conflict.sdc",
        "create_clock -name clk -period 0.2 [get_ports a]",
    );
    let waiver = TempFile::new("conflict.waivers", "waiver x clk y -1\n");
    let audit = TempFile::new(
        "conflict.audit",
        "audit 1 added 1 \"x\"\nwaiver x clk y -1\nend\n",
    );
    let (ok, report) = run_edu(&[
        "sta",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waiver.path(),
        "--waiver-audit",
        audit.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("choose either --waivers or --waiver-audit"),
        "{report}"
    );
}

#[test]
fn pnr_accepts_waiver_aware_timing_routing() {
    let input = TempFile::new(
        "pnr-waiver.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let sdc = TempFile::new(
        "pnr-waiver.sdc",
        r#"
create_clock -name clk -period 0.1 [get_ports a]
set_output_delay 0.1 -clock clk [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "pnr-waiver.waivers",
        r#"waiver "physical" "clk" "y" "-10.0"
kind output
"#,
    );
    let (ok, report) = run_edu(&[
        "pnr",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing violation report for 'clk'")
            && report.contains("waived violations: 1")
            && report.contains("Routing report"),
        "{report}"
    );
}

#[test]
fn flow_accepts_physical_timing_waivers() {
    let input = TempFile::new(
        "flow-waiver.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let sdc = TempFile::new(
        "flow-waiver.sdc",
        r#"
create_clock -name clk -period 0.1 [get_ports a]
set_output_delay 0.1 -clock clk [get_ports y]
"#,
    );
    let waivers = TempFile::new(
        "flow-waiver.waivers",
        r#"waiver "physical" "clk" "y" "-10.0"
kind output
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("constrained slack:")
            && report.contains("best iteration"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_physical_timing() {
    let input = TempFile::new(
        "flow-mmsc.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'")
            && report.contains("common HPWL"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_timing_driven_cts() {
    let input = TempFile::new(
        "flow-mmsc-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-cts.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 10.0 [get_ports clk]
    set_input_delay 0.5 -clock clk [get_ports d]
endscenario
scenario fast
    create_clock -name clk -period 8.0 [get_ports clk]
    set_input_delay 0.4 -clock clk [get_ports d]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--timing-driven-cts",
        "--cts-target-skew",
        "0.2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("Timing-driven CTS search:")
            && report.contains("target MET")
            && report.contains("scenario 'slow'")
            && report.contains("scenario 'fast'"),
        "{report}"
    );
}

#[test]
fn flow_accepts_mmsc_antenna_repair() {
    let input = TempFile::new(
        "flow-mmsc-antenna.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let mmsc = TempFile::new(
        "flow-mmsc-antenna.mmsc",
        r#"
scenario slow
    create_clock -name clk -period 4.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
scenario fast
    create_clock -name clk -period 3.0 [get_ports a]
    set_output_delay 0.3 -clock clk [get_ports y]
endscenario
"#,
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--mmsc",
        mmsc.path(),
        "--physical-timing",
        "--repair-antennas",
        "--antenna-max-diodes",
        "2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Multi-corner physical timing flow:")
            && report.contains("antenna diodes 0, antenna warnings 0"),
        "{report}"
    );
}

#[test]
fn flow_accepts_and_requires_antenna_repair() {
    let input = TempFile::new(
        "flow-antenna.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--repair-antennas",
        "--antenna-max-diodes",
        "2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("antenna diodes 0, antenna warnings 0")
            && report.contains("Physical timing flow:"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["flow", input.path(), "--repair-antennas"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--repair-antennas requires --physical-timing"),
        "{rejected_report}"
    );

    let (limit_rejected, limit_report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--antenna-max-diodes",
        "2",
    ]);
    assert!(!limit_rejected, "{limit_report}");
    assert!(
        limit_report.contains("--antenna-max-diodes requires --repair-antennas"),
        "{limit_report}"
    );
}

#[test]
fn cli_imports_def_physical_views() {
    let input = TempFile::new(
        "import-def.v",
        "module m(input a, b, c, output y); assign y = (a & b) | c; endmodule",
    );
    let exported = TempFile::new("import-def-output.def", "");
    let (export_ok, export_report) = run_edu(&["pnr", input.path(), "--def", exported.path()]);
    assert!(export_ok, "{export_report}");
    let def_text = fs::read_to_string(exported.path()).expect("read exported DEF");
    assert!(
        def_text.contains("TRACKS ") && def_text.contains("COMPONENTS"),
        "{def_text}"
    );

    let (drc_ok, drc_report) = run_edu(&["drc", input.path(), "--import-def", exported.path()]);
    assert!(drc_ok, "{drc_report}");
    assert!(
        drc_report.contains("DRC report: 0 error(s)") && drc_report.contains("0 warning(s)"),
        "{drc_report}"
    );

    let (pnr_ok, pnr_report) = run_edu(&[
        "pnr",
        input.path(),
        "--import-def",
        exported.path(),
        "--maze",
    ]);
    assert!(pnr_ok, "{pnr_report}");
    assert!(
        pnr_report.contains("PnR report") && pnr_report.contains("Routing report"),
        "{pnr_report}"
    );

    let (flow_ok, flow_report) = run_edu(&[
        "flow",
        input.path(),
        "--import-def",
        exported.path(),
        "--physical-timing",
        "--timing-ripgup",
        "1",
    ]);
    assert!(flow_ok, "{flow_report}");
    assert!(
        flow_report.contains("Physical timing flow:") && flow_report.contains("best iteration"),
        "{flow_report}"
    );

    let (rejected, rejected_report) =
        run_edu(&["flow", input.path(), "--import-def", exported.path()]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--import-def requires --physical-timing"),
        "{rejected_report}"
    );
}

#[test]
fn cts_accepts_timing_driven_search() {
    let input = TempFile::new(
        "timing-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let (ok, report) = run_edu(&[
        "cts",
        input.path(),
        "--timing-driven-cts",
        "--cts-target-skew",
        "1.0",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Timing-driven CTS search:")
            && report.contains("selected 2")
            && report.contains("target MET"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["cts", input.path(), "--cts-target-skew", "1.0"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--cts-target-skew requires --timing-driven-cts"),
        "{rejected_report}"
    );
}

#[test]
fn flow_accepts_timing_driven_cts() {
    let input = TempFile::new(
        "flow-cts.v",
        "module m(input clk, d, output reg q);
            always @(posedge clk) q <= d;
        endmodule",
    );
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--physical-timing",
        "--timing-driven-cts",
        "--cts-target-skew",
        "0.2",
        "--timing-ripgup",
        "1",
    ]);
    assert!(ok, "{report}");
    assert!(
        report.contains("Physical timing flow:")
            && report.contains("Timing-driven CTS search:")
            && report.contains("target MET"),
        "{report}"
    );

    let (rejected, rejected_report) = run_edu(&["flow", input.path(), "--timing-driven-cts"]);
    assert!(!rejected, "{rejected_report}");
    assert!(
        rejected_report.contains("--timing-driven-cts in flow requires --physical-timing"),
        "{rejected_report}"
    );
}

#[test]
fn flow_rejects_waivers_without_physical_timing() {
    let input = TempFile::new(
        "flow-waiver-required.v",
        "module m(input a, output y); assign y = ~a; endmodule",
    );
    let sdc = TempFile::new(
        "flow-waiver-required.sdc",
        "create_clock -name clk -period 0.1 [get_ports a]",
    );
    let waivers = TempFile::new("flow-waiver-required.waivers", "waiver x clk y -1\n");
    let (ok, report) = run_edu(&[
        "flow",
        input.path(),
        "--sdc",
        sdc.path(),
        "--waivers",
        waivers.path(),
    ]);
    assert!(!ok, "{report}");
    assert!(
        report.contains("flow waivers require --physical-timing"),
        "{report}"
    );
}
