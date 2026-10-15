//! CLI integration-test domain.

use super::common::{run_edu, TempFile};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::Command;

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
