// tb_class — class 组合式 testbench（Monitor / Scoreboard / Env）
//
// 用 SystemVerilog class 组合能力搭建 UVM 风格验证环境：
//   Monitor    采样 DUT 调试端口
//   Scoreboard 持有预期值并比对
//   Env        组合两者，run() 完成 采样→比对→置 pass

class Monitor;
  int sample_x10 = 0;
  int halted_seen = 0;
  function new();
  endfunction
  function void sample(int x10, int halted);
    this.sample_x10 = x10;
    this.halted_seen = halted;
  endfunction
endclass

class Scoreboard;
  int expected = 0;
  int errors = 0;
  function new(int exp);
    this.expected = exp;
  endfunction
  function void check(int actual);
    if (actual != this.expected) this.errors = this.errors + 1;
  endfunction
endclass

class Env;
  Monitor mon;
  Scoreboard sb;
  function new(int exp);
    this.mon = new();
    this.sb = new(exp);
  endfunction
  function void run(int x10, int halted);
    this.mon.sample(x10, halted);
    if (this.mon.halted_seen) this.sb.check(this.mon.sample_x10);
  endfunction
endclass

module tb_class(
  output reg tb_done,
  output reg tb_ok
);
  reg clk;
  reg rst;
  reg prog_ld;
  reg [511:0] prog;

  wire halted;
  wire [1:0] trap_cause;
  wire [31:0] dbg_pc;
  wire [31:0] dbg_x10;
  wire [31:0] dbg_x11;
  wire [31:0] dbg_mepc;
  wire [31:0] dbg_mcause;

  rv32i_core dut(.clk(clk), .rst(rst), .prog_ld(prog_ld), .prog(prog),
                 .halted(halted), .trap_cause(trap_cause), .dbg_pc(dbg_pc), .dbg_x10(dbg_x10),
                 .dbg_x11(dbg_x11), .dbg_mepc(dbg_mepc), .dbg_mcause(dbg_mcause));

  Env env;
  initial begin
    clk = 0; rst = 1; prog_ld = 1; tb_done = 0; tb_ok = 0;
    // 程序：x10 = 6 + 20 = 26 后 EBREAK
    prog = {
      32'h00000000, 32'h00000000, 32'h00000000,
      32'h00000000, 32'h00000000, 32'h00000000,
      32'h00000000, 32'h00000000, 32'h00100073,
      32'h00208153,                    // add x10, x1, x2
      32'h01400113,                    // addi x2, x0, 20
      32'h00600093                     // addi x1, x0, 6
    };
    env = new(26);
    #12 rst = 0; prog_ld = 0;
    #80;
    env.run(dbg_x10, halted);
    if (env.sb.errors == 0) tb_ok = 1;
    tb_done = 1;
    #10 $finish;
  end

  always #5 clk = !tb_done && ~clk;
endmodule
