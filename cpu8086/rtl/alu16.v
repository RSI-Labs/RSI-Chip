// ---------------------------------------------------------------------------
// cpu8086 — 16 位算术逻辑单元
// 加/减路径直接例化门级行波进位加法器（减法 = a + ~b + 1）。
// 完整产生 8086 六个标志：CF PF AF ZF SF OF。
// sel 编码: 0=ADD 1=SUB 2=AND 3=OR 4=XOR 5=SHL1 6=SHR1 7=ROL1 8=ROR1
// ---------------------------------------------------------------------------
module alu16(
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
    // --- 算术路径：门级加法器 ---
    wire sub_mode = (sel == 4'h1);
    wire [15:0] b_eff = sub_mode ? ~b : b;
    wire [15:0] sum16;
    wire cout, c14, c3;
    ripple_adder16 u_add(
        .a(a), .b(b_eff), .cin(sub_mode),
        .sum(sum16), .cout(cout), .c14(c14), .c3(c3)
    );

    // --- 结果与标志选择 ---
    reg [15:0] r;
    reg f_cf, f_af, f_of;
    always @(*) begin
        // 算术默认（ADD）
        r    = sum16;
        f_cf = cout;
        f_af = c3;
        f_of = cout ^ c14;
        case (sel)
            4'h0: begin end // ADD — 使用默认值
            4'h1: begin // SUB — 借价 = 取反内部进位
                f_cf = ~cout;
                f_af = ~c3;
                f_of = cout ^ c14;
            end
            4'h2: begin r = a & b; f_cf = 1'b0; f_af = 1'b0; f_of = 1'b0; end
            4'h3: begin r = a | b; f_cf = 1'b0; f_af = 1'b0; f_of = 1'b0; end
            4'h4: begin r = a ^ b; f_cf = 1'b0; f_af = 1'b0; f_of = 1'b0; end
            4'h5: begin // SHL 1: OF = a15 ^ a14（避免读回本进程信号）
                r = {a[14:0], 1'b0};
                f_cf = a[15];
                f_of = a[15] ^ a[14];
                f_af = 1'b0;
            end
            4'h6: begin // SHR 1: OF = 原操作数 MSB
                r = {1'b0, a[15:1]};
                f_cf = a[0];
                f_of = a[15];
                f_af = 1'b0;
            end
            4'h7: begin // ROL 1: OF = a14 ^ a15
                r = {a[14:0], a[15]};
                f_cf = a[15];
                f_of = a[14] ^ a[15];
                f_af = 1'b0;
            end
            4'h8: begin // ROR 1: OF = 原操作数高两位异或
                r = {a[0], a[15:1]};
                f_cf = a[0];
                f_of = a[15] ^ a[14];
                f_af = 1'b0;
            end
            default: begin
                r = 16'h0000; f_cf = 1'b0; f_af = 1'b0; f_of = 1'b0;
            end
        endcase
    end

    assign y  = r;
    assign cf = f_cf;
    assign pf = ~^r[7:0];  // 低 8 位 1 的个数为偶数时 PF=1
    assign af = f_af;
    assign zf = ~|r;
    assign sf = r[15];
    assign of = f_of;
endmodule
