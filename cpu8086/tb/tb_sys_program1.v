// ---------------------------------------------------------------------------
// cpu8086 — 系统级测试台：运行算术演示程序
//   MVI AX,5 ; MVI BX,3 ; ADD AX,BX ; OUT AX ; HLT
// 预期: io_out = 8, halted = 1
// ---------------------------------------------------------------------------
`timescale 1ns/1ps
module tb_sys_program1;
    reg clk;
    reg rst;
    reg prog_ld;
    reg [1023:0] prog;
    wire io_wr;
    wire [15:0] io_data;
    wire [15:0] io_out;
    wire io_valid;
    wire halted;

    sys8086 dut(
        .clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog),
        .io_wr(io_wr), .io_data(io_data),
        .io_out(io_out), .io_valid(io_valid), .halted(halted)
    );

    initial clk = 1'b0;
    always #5 clk = ~clk;

    initial begin
        // 程序 1 机器码（见 programs/prog1.md）
        prog[15:0]    = 16'h1000;  // MVI AX, imm
        prog[31:16]   = 16'h0005;  //         5
        prog[47:32]   = 16'h1600;  // MVI BX, imm
        prog[63:48]   = 16'h0003;  //         3
        prog[79:64]   = 16'h20C0;  // ADD AX, BX
        prog[95:80]   = 16'hD000;  // OUT port0, AX
        prog[111:96]  = 16'hE000;  // HLT
        rst     = 1'b1;
        prog_ld = 1'b1;
        #10;                       // 第 1 个上升沿: 复位 + 装载程序
        rst     = 1'b0;
        prog_ld = 1'b0;
        #200;                      // 运行
        $display("io_out=%h io_valid=%b halted=%b", io_out, io_valid, halted);
        if (io_out == 16'h0008 && halted == 1'b1)
            $display("PASS: 5 + 3 = 8");
        else
            $display("FAIL");
        $finish;
    end
endmodule
