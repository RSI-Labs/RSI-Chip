// ---------------------------------------------------------------------------
// cpu8086 — 16 位行波进位加法器
// 由 16 个门级全加器级联构成。除最终结果外还导出中间进位：
//   c3  — bit3 向 bit4 的进位（x86 AF 辅助进位的物理来源）
//   c14 — bit14 向 bit15 的进位（与 cout 异或得到 x86 OF 溢出标志）
// 这些输出证明标志位不是"软件模拟"，而是真实加法器内部信号。
// ---------------------------------------------------------------------------
module ripple_adder16(
    input  wire [15:0] a,
    input  wire [15:0] b,
    input  wire        cin,
    output wire [15:0] sum,
    output wire        cout,
    output wire        c14,
    output wire        c3
);
    wire [15:0] c; // c[i] = 第 i 位的进位输出

    full_adder fa00(.a(a[0]),  .b(b[0]),  .cin(cin),   .sum(sum[0]),  .cout(c[0]));
    full_adder fa01(.a(a[1]),  .b(b[1]),  .cin(c[0]),  .sum(sum[1]),  .cout(c[1]));
    full_adder fa02(.a(a[2]),  .b(b[2]),  .cin(c[1]),  .sum(sum[2]),  .cout(c[2]));
    full_adder fa03(.a(a[3]),  .b(b[3]),  .cin(c[2]),  .sum(sum[3]),  .cout(c[3]));
    full_adder fa04(.a(a[4]),  .b(b[4]),  .cin(c[3]),  .sum(sum[4]),  .cout(c[4]));
    full_adder fa05(.a(a[5]),  .b(b[5]),  .cin(c[4]),  .sum(sum[5]),  .cout(c[5]));
    full_adder fa06(.a(a[6]),  .b(b[6]),  .cin(c[5]),  .sum(sum[6]),  .cout(c[6]));
    full_adder fa07(.a(a[7]),  .b(b[7]),  .cin(c[6]),  .sum(sum[7]),  .cout(c[7]));
    full_adder fa08(.a(a[8]),  .b(b[8]),  .cin(c[7]),  .sum(sum[8]),  .cout(c[8]));
    full_adder fa09(.a(a[9]),  .b(b[9]),  .cin(c[8]),  .sum(sum[9]),  .cout(c[9]));
    full_adder fa10(.a(a[10]), .b(b[10]), .cin(c[9]),  .sum(sum[10]), .cout(c[10]));
    full_adder fa11(.a(a[11]), .b(b[11]), .cin(c[10]), .sum(sum[11]), .cout(c[11]));
    full_adder fa12(.a(a[12]), .b(b[12]), .cin(c[11]), .sum(sum[12]), .cout(c[12]));
    full_adder fa13(.a(a[13]), .b(b[13]), .cin(c[12]), .sum(sum[13]), .cout(c[13]));
    full_adder fa14(.a(a[14]), .b(b[14]), .cin(c[13]), .sum(sum[14]), .cout(c[14]));
    full_adder fa15(.a(a[15]), .b(b[15]), .cin(c[14]), .sum(sum[15]), .cout(c[15]));

    assign cout = c[15];
    assign c14  = c[14];
    assign c3   = c[3];
endmodule
