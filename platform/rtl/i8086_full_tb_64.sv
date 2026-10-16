// Self-clocking simulation harness for the 64-byte i8086 SOC.
module i8086_full_tb_64(program_image, halted, trap, retired,
                        console_write, console_char,
                        clock,
                        ax, cx, dx, bx, sp, bp,
                        si, di, ip, flags);
  input [511:0] program_image;
  output halted;
  output trap;
  output [63:0] retired;
  output console_write;
  output [7:0] console_char;
  output clock;
  output [15:0] ax;
  output [15:0] cx;
  output [15:0] dx;
  output [15:0] bx;
  output [15:0] sp;
  output [15:0] bp;
  output [15:0] si;
  output [15:0] di;
  output [15:0] ip;
  output [15:0] flags;
  parameter INIT_IP = 16'd0;
  reg clk;
  reg rst;
  assign clock = clk;

  initial begin
    clk = 1'b0;
    rst = 1'b1;
    #1 clk = 1'b1;
    #1 clk = 1'b0;
    rst = 1'b0;
    forever #1 clk = ~clk;
  end

  always @(posedge clk) begin
    if (halted)
      $finish;
  end

  i8086_full_soc_64 dut(
    .clk(clk), .rst(rst), .init_ip(INIT_IP), .program_image(program_image), .halted(halted), .trap(trap),
    .retired(retired), .console_write(console_write), .console_char(console_char),
    .ax(ax), .cx(cx), .dx(dx), .bx(bx), .sp(sp), .bp(bp),
    .si(si), .di(di), .ip(ip), .flags(flags)
  );
endmodule
