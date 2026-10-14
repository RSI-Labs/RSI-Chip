// ---------------------------------------------------------------------------
// cpu8086 — 8 x 16 位通用寄存器堆（8086 EU 寄存器组）
// 索引: 0=AX 1=CX 2=DX 3=BX 4=SP 5=BP 6=SI 7=DI
// 两个组合读端口 + 一个同步写端口；复位清零。
// 硬化数组不可综合为动态索引，因此用 case 多路器实现
// （这本身就是硬件中寄存器堆读端口的真实结构）。
// ---------------------------------------------------------------------------
module regfile16(
    input  wire        clk,
    input  wire        rst,
    input  wire [2:0]  rs1,
    output reg  [15:0] q1,
    input  wire [2:0]  rs2,
    output reg  [15:0] q2,
    input  wire [2:0]  wa,
    input  wire [15:0] wd,
    input  wire        we
);
    reg [15:0] r0, r1, r2, r3, r4, r5, r6, r7;

    always @(posedge clk) begin
        if (rst) begin
            r0 <= 16'h0000; r1 <= 16'h0000; r2 <= 16'h0000; r3 <= 16'h0000;
            r4 <= 16'h0000; r5 <= 16'h0000; r6 <= 16'h0000; r7 <= 16'h0000;
        end else if (we) begin
            case (wa)
                3'd0: r0 <= wd;
                3'd1: r1 <= wd;
                3'd2: r2 <= wd;
                3'd3: r3 <= wd;
                3'd4: r4 <= wd;
                3'd5: r5 <= wd;
                3'd6: r6 <= wd;
                3'd7: r7 <= wd;
            endcase
        end
    end

    always @(*) begin
        case (rs1)
            3'd0: q1 = r0;
            3'd1: q1 = r1;
            3'd2: q1 = r2;
            3'd3: q1 = r3;
            3'd4: q1 = r4;
            3'd5: q1 = r5;
            3'd6: q1 = r6;
            3'd7: q1 = r7;
        endcase
    end

    always @(*) begin
        case (rs2)
            3'd0: q2 = r0;
            3'd1: q2 = r1;
            3'd2: q2 = r2;
            3'd3: q2 = r3;
            3'd4: q2 = r4;
            3'd5: q2 = r5;
            3'd6: q2 = r6;
            3'd7: q2 = r7;
        endcase
    end
endmodule
