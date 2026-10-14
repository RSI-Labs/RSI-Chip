// ---------------------------------------------------------------------------
// cpu8086 — 门级加法器 vs 行为级参考的交叉验证载荷
// add16_gate: 门级行波进位加法器（16 个全加器 = 约 80 个逻辑门）
// add16_ref : 行为级 a + b + cin
// 两者对所有输入逐位等价（由回归 cpu8086_adder_crosscheck 驱动验证）。
// ---------------------------------------------------------------------------
module add16_gate(
    input  wire [15:0] a,
    input  wire [15:0] b,
    input  wire        cin,
    output wire [15:0] sum,
    output wire        cout,
    output wire        c14,
    output wire        c3
);
    ripple_adder16 u(
        .a(a), .b(b), .cin(cin),
        .sum(sum), .cout(cout), .c14(c14), .c3(c3)
    );
endmodule

module add16_ref(
    input  wire [15:0] a,
    input  wire [15:0] b,
    input  wire        cin,
    output wire [15:0] sum,
    output wire        cout
);
    wire [16:0] r;
    assign r = a + b + cin;
    assign sum  = r[15:0];
    assign cout = r[16];
endmodule
