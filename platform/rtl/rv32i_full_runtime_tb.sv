// Event-driven RV32I harness with host-backed sparse instruction/data memory.
module rv32i_full_runtime_tb(init_pc, imem_rdata, dmem_rdata,
                             clock, halted, trap, trap_cause,
                             imem_addr, dmem_addr, dmem_wdata,
                             dmem_width, dmem_req, dmem_we,
                             dbg_pc, dbg_x, dbg_x10);
  input [31:0] init_pc;
  input [31:0] imem_rdata;
  input [31:0] dmem_rdata;
  output clock;
  output halted;
  output trap;
  output [31:0] trap_cause;
  output [31:0] imem_addr;
  output [31:0] dmem_addr;
  output [31:0] dmem_wdata;
  output [2:0] dmem_width;
  output dmem_req;
  output dmem_we;
  output [31:0] dbg_pc;
  output [127:0] dbg_x;
  output [31:0] dbg_x10;
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

  rv32i_full_core core(
    .clk(clk), .rst(rst), .init_pc(init_pc),
    .imem_rdata(imem_rdata), .dmem_rdata(dmem_rdata),
    .imem_addr(imem_addr), .dmem_addr(dmem_addr),
    .dmem_wdata(dmem_wdata), .dmem_width(dmem_width),
    .dmem_req(dmem_req), .dmem_we(dmem_we),
    .halted(halted), .trap(trap), .trap_cause(trap_cause),
    .dbg_pc(dbg_pc), .dbg_x(dbg_x), .dbg_x10(dbg_x10)
  );
endmodule
