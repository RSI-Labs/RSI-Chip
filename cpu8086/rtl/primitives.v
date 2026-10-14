// ---------------------------------------------------------------------------
// cpu8086 — 门级原语层
// 最底层的组合/时序构件：全加器（纯逻辑门实现）与 D 触发器。
// 全加器使用 Verilog 门原语 xor/and/or，对应真实 CMOS 标准单元；
// 可被 eda-synth 映射到 Liberty 单元库并用 eda-formal 做等价检查。
// ---------------------------------------------------------------------------

// 一位全加器: sum = a ^ b ^ cin, cout = a&b | (a^b)&cin
module full_adder(
    input  wire a,
    input  wire b,
    input  wire cin,
    output wire sum,
    output wire cout
);
    wire axb;
    wire ab;
    wire cxa;
    xor g_sum1(axb, a, b);
    xor g_sum2(sum, axb, cin);
    and g_ab(ab, a, b);
    and g_cxa(cxa, axb, cin);
    or  g_cout(cout, ab, cxa);
endmodule

// D 触发器（上升沿）
module dff(
    input  wire clk,
    input  wire d,
    output reg  q
);
    always @(posedge clk) q <= d;
endmodule
