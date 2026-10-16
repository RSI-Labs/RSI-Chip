// Small synthesizable program-memory SOC used for architectural simulation
// and RTL-versus-golden-model differential validation.
module rv32i_full_soc_16(clk, rst, prog, halted, trap, trap_cause, dbg_pc,
                         dbg_x, dbg_x10);
  input clk;
  input rst;
  input [511:0] prog;
  output halted;
  output trap;
  output [31:0] trap_cause;
  output [31:0] dbg_pc;
  output [127:0] dbg_x;
  output [31:0] dbg_x10;

  wire [31:0] imem_addr;
  wire [31:0] dmem_addr;
  wire [31:0] dmem_wdata;
  wire [2:0] dmem_width;
  wire dmem_req;
  wire dmem_we;
  reg [31:0] mem0, mem1, mem2, mem3, mem4, mem5, mem6, mem7;
  reg [31:0] mem8, mem9, mem10, mem11, mem12, mem13, mem14, mem15;
  reg [31:0] imem_word;
  reg [31:0] dmem_word;
  reg [31:0] merged_word;

  rv32i_full_core core(
    .clk(clk), .rst(rst), .init_pc(32'b0), .imem_rdata(imem_word), .dmem_rdata(dmem_word),
    .imem_addr(imem_addr), .dmem_addr(dmem_addr), .dmem_wdata(dmem_wdata),
    .dmem_width(dmem_width), .dmem_req(dmem_req), .dmem_we(dmem_we),
    .halted(halted), .trap(trap), .trap_cause(trap_cause),
    .dbg_pc(dbg_pc), .dbg_x(dbg_x), .dbg_x10(dbg_x10)
  );

  always @* begin
    imem_word = mem0;
    case (imem_addr[5:2])
      4'd0: imem_word = mem0;
      4'd1: imem_word = mem1;
      4'd2: imem_word = mem2;
      4'd3: imem_word = mem3;
      4'd4: imem_word = mem4;
      4'd5: imem_word = mem5;
      4'd6: imem_word = mem6;
      4'd7: imem_word = mem7;
      4'd8: imem_word = mem8;
      4'd9: imem_word = mem9;
      4'd10: imem_word = mem10;
      4'd11: imem_word = mem11;
      4'd12: imem_word = mem12;
      4'd13: imem_word = mem13;
      4'd14: imem_word = mem14;
      4'd15: imem_word = mem15;
      default: imem_word = mem0;
    endcase
  end

  always @* begin
    dmem_word = mem0;
    case (dmem_addr[5:2])
      4'd0: dmem_word = mem0;
      4'd1: dmem_word = mem1;
      4'd2: dmem_word = mem2;
      4'd3: dmem_word = mem3;
      4'd4: dmem_word = mem4;
      4'd5: dmem_word = mem5;
      4'd6: dmem_word = mem6;
      4'd7: dmem_word = mem7;
      4'd8: dmem_word = mem8;
      4'd9: dmem_word = mem9;
      4'd10: dmem_word = mem10;
      4'd11: dmem_word = mem11;
      4'd12: dmem_word = mem12;
      4'd13: dmem_word = mem13;
      4'd14: dmem_word = mem14;
      4'd15: dmem_word = mem15;
      default: dmem_word = mem0;
    endcase
  end

  always @(posedge clk) begin
    if (rst) begin
      mem0 <= prog[31:0];
      mem1 <= prog[63:32];
      mem2 <= prog[95:64];
      mem3 <= prog[127:96];
      mem4 <= prog[159:128];
      mem5 <= prog[191:160];
      mem6 <= prog[223:192];
      mem7 <= prog[255:224];
      mem8 <= prog[287:256];
      mem9 <= prog[319:288];
      mem10 <= prog[351:320];
      mem11 <= prog[383:352];
      mem12 <= prog[415:384];
      mem13 <= prog[447:416];
      mem14 <= prog[479:448];
      mem15 <= prog[511:480];
    end else if (dmem_req && dmem_we) begin
      case (dmem_addr[5:2])
        4'd0: mem0 <= merged_word;
        4'd1: mem1 <= merged_word;
        4'd2: mem2 <= merged_word;
        4'd3: mem3 <= merged_word;
        4'd4: mem4 <= merged_word;
        4'd5: mem5 <= merged_word;
        4'd6: mem6 <= merged_word;
        4'd7: mem7 <= merged_word;
        4'd8: mem8 <= merged_word;
        4'd9: mem9 <= merged_word;
        4'd10: mem10 <= merged_word;
        4'd11: mem11 <= merged_word;
        4'd12: mem12 <= merged_word;
        4'd13: mem13 <= merged_word;
        4'd14: mem14 <= merged_word;
        4'd15: mem15 <= merged_word;
        default: mem0 <= mem0;
      endcase
    end
  end
  always @* begin
    case (dmem_width)
      3'b000: merged_word = dmem_addr[1:0] == 2'd0 ? {dmem_word[31:8], dmem_wdata[7:0]} : dmem_addr[1:0] == 2'd1 ? {dmem_word[31:16], dmem_wdata[7:0], dmem_word[7:0]} : dmem_addr[1:0] == 2'd2 ? {dmem_word[31:24], dmem_wdata[7:0], dmem_word[15:0]} : {dmem_wdata[7:0], dmem_word[23:0]};
      3'b001: merged_word = dmem_addr[1] == 1'b0 ? {dmem_word[31:16], dmem_wdata[15:0]} : {dmem_wdata[15:0], dmem_word[15:0]};
      default: merged_word = dmem_wdata;
    endcase
  end

endmodule
