// ---------------------------------------------------------------------------
// cpu8086 — ALU 测试台（组合激励）
// 也可由 Rust 回归直接驱动（见 crates/eda-tests/src/cpu8086_cases.rs）。
// ---------------------------------------------------------------------------
module tb_alu_top(
    input  wire [15:0] a,
    input  wire [15:0] b,
    input  wire [3:0]  sel,
    output wire [15:0] y,
    output wire        cf,
    output wire        pf,
    output wire        af,
    output wire        zf,
    output wire        sf,
    output wire        of
);
    alu16 u_alu(
        .a(a), .b(b), .sel(sel),
        .y(y), .cf(cf), .pf(pf), .af(af), .zf(zf), .sf(sf), .of(of)
    );
endmodule
