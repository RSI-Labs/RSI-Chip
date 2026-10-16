// Event-driven runtime harness. The host supplies architectural-memory reads
// and observes bus writes around each rising clock edge.
module i8086_full_runtime_tb(mem_rdata, source_rdata, io_rdata,
                             clock, halted, trap, retired,
                             console_write, console_char,
                             mem_addr, mem_wdata, mem_we, mem_width,
                             source_addr, io_addr, io_we, io_width,
                             ax, cx, dx, bx, sp, bp, si, di, ip, flags);
  input [15:0] mem_rdata;
  input [15:0] source_rdata;
  input [15:0] io_rdata;
  output clock;
  output halted;
  output trap;
  output [63:0] retired;
  output console_write;
  output [7:0] console_char;
  output [19:0] mem_addr;
  output [15:0] mem_wdata;
  output mem_we;
  output [2:0] mem_width;
  output [19:0] source_addr;
  output [15:0] io_addr;
  output io_we;
  output [2:0] io_width;
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
  parameter INIT_SP = 16'hfffe;
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

  i8086_full_core core(
    .clk(clk), .rst(rst), .init_ip(INIT_IP), .init_sp(INIT_SP),
    .init_cs(16'b0), .init_ds(16'b0), .init_es(16'b0), .init_ss(16'b0),
    .mem_rdata(mem_rdata), .mem_addr(mem_addr), .mem_wdata(mem_wdata),
    .mem_we(mem_we), .mem_width(mem_width), .source_rdata(source_rdata),
    .source_addr(source_addr), .io_rdata(io_rdata),
    .io_addr(io_addr), .io_we(io_we), .io_width(io_width),
    .console_write(console_write), .console_char(console_char),
    .irq(1'b0), .nmi(1'b0), .interrupt_vector(8'b0), .interrupt_ack(),
    .halted(halted), .trap(trap), .instructions_retired(retired),
    .dbg_ax(ax), .dbg_cx(cx), .dbg_dx(dx), .dbg_bx(bx), .dbg_sp(sp),
    .dbg_bp(bp), .dbg_si(si), .dbg_di(di), .dbg_es(), .dbg_cs(),
    .dbg_ss(), .dbg_ds(), .dbg_ip(ip), .dbg_flags(flags)
  );
endmodule
