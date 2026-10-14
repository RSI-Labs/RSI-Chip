// ---------------------------------------------------------------------------
// cpu8086 — 系统顶层：CPU + RAM + 输出 I/O 端口
//   prog_ld / prog[1023:0] — 一拍并行加载 64 字程序映像
//   io_wr / io_data        — CPU OUT 指令脉冲
//   io_out / io_valid      — 锁存的输出端口（模拟 8255 风格并行口）
//   halted                 — HLT 停机
// ---------------------------------------------------------------------------
module sys8086(
    input  wire         clk,
    input  wire         rst,
    input  wire         prog_ld,
    input  wire [1023:0] prog,
    output wire         io_wr,
    output wire [15:0]  io_data,
    output reg  [15:0]  io_out,
    output reg          io_valid,
    output wire         halted
);
    wire [5:0]  mem_addr;
    wire        mem_we;
    wire [15:0] mem_wd, mem_rd;

    cpu8086 u_cpu(
        .clk(clk), .rst(rst),
        .mem_addr(mem_addr), .mem_we(mem_we), .mem_wd(mem_wd), .mem_rd(mem_rd),
        .io_wr(io_wr), .io_data(io_data), .halted(halted)
    );

    ram64x16 u_ram(
        .clk(clk), .ld(prog_ld), .prog(prog),
        .addr(mem_addr), .we(mem_we), .wd(mem_wd), .rd(mem_rd)
    );

    // 输出端口寄存器: OUT 指令把寄存器值锁存到 io_out
    always @(posedge clk) begin
        if (rst) begin
            io_out  <= 16'h0000;
            io_valid <= 1'b0;
        end else if (io_wr) begin
            io_out  <= io_data;
            io_valid <= 1'b1;
        end
    end
endmodule
