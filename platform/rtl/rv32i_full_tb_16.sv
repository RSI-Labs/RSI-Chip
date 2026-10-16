// Self-clocking simulation harness for the 16-word full-core SOC.
module rv32i_full_tb_16(prog, halted, trap, trap_cause, dbg_pc, dbg_x, dbg_x10);
  input [511:0] prog;
  output halted;
  output trap;
  output [31:0] trap_cause;
  output [31:0] dbg_pc;
  output [127:0] dbg_x;
  output [31:0] dbg_x10;
  reg clk;
  reg rst;

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

  rv32i_full_soc_16 dut(
    .clk(clk), .rst(rst), .prog(prog), .halted(halted), .trap(trap),
    .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x(dbg_x), .dbg_x10(dbg_x10)
  );

endmodule
