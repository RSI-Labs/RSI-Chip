// 64-byte simulation memory attached to the full i8086 core.
module i8086_full_soc_64(clk, rst, init_ip, program_image, halted, trap, retired,
                         console_write, console_char,
                         ax, cx, dx, bx,
                         sp, bp, si, di, ip, flags);
  input clk;
  input rst;
  input [15:0] init_ip;
  input [511:0] program_image;
  output halted;
  output trap;
  output [63:0] retired;
  output console_write;
  output [7:0] console_char;
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

  wire [19:0] mem_addr;
  wire [15:0] mem_wdata;
  wire mem_we;
  wire [2:0] mem_width;
  wire [15:0] mem_rdata;
  wire [15:0] source_rdata;
  wire [19:0] source_addr;
  wire [15:0] io_rdata;
  wire [15:0] io_addr;
  wire io_we;
  wire [2:0] io_width;
  reg [7:0] b0, b1, b2, b3, b4, b5, b6, b7;
  reg [7:0] b8, b9, b10, b11, b12, b13, b14, b15;
  reg [7:0] b16, b17, b18, b19, b20, b21, b22, b23;
  reg [7:0] b24, b25, b26, b27, b28, b29, b30, b31;
  reg [7:0] b32, b33, b34, b35, b36, b37, b38, b39;
  reg [7:0] b40, b41, b42, b43, b44, b45, b46, b47;
  reg [7:0] b48, b49, b50, b51, b52, b53, b54, b55;
  reg [7:0] b56, b57, b58, b59, b60, b61, b62, b63;

  i8086_full_core core(
    .clk(clk), .rst(rst), .init_ip(init_ip), .init_sp(16'd56), .init_cs(16'b0),
    .init_ds(16'b0), .init_es(16'b0), .init_ss(16'b0),
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

  function [7:0] byte_read(input [7:0] index);
    begin
    case (index)
      8'd0: byte_read = b0;
      8'd1: byte_read = b1;
      8'd2: byte_read = b2;
      8'd3: byte_read = b3;
      8'd4: byte_read = b4;
      8'd5: byte_read = b5;
      8'd6: byte_read = b6;
      8'd7: byte_read = b7;
      8'd8: byte_read = b8;
      8'd9: byte_read = b9;
      8'd10: byte_read = b10;
      8'd11: byte_read = b11;
      8'd12: byte_read = b12;
      8'd13: byte_read = b13;
      8'd14: byte_read = b14;
      8'd15: byte_read = b15;
      8'd16: byte_read = b16;
      8'd17: byte_read = b17;
      8'd18: byte_read = b18;
      8'd19: byte_read = b19;
      8'd20: byte_read = b20;
      8'd21: byte_read = b21;
      8'd22: byte_read = b22;
      8'd23: byte_read = b23;
      8'd24: byte_read = b24;
      8'd25: byte_read = b25;
      8'd26: byte_read = b26;
      8'd27: byte_read = b27;
      8'd28: byte_read = b28;
      8'd29: byte_read = b29;
      8'd30: byte_read = b30;
      8'd31: byte_read = b31;
      8'd32: byte_read = b32;
      8'd33: byte_read = b33;
      8'd34: byte_read = b34;
      8'd35: byte_read = b35;
      8'd36: byte_read = b36;
      8'd37: byte_read = b37;
      8'd38: byte_read = b38;
      8'd39: byte_read = b39;
      8'd40: byte_read = b40;
      8'd41: byte_read = b41;
      8'd42: byte_read = b42;
      8'd43: byte_read = b43;
      8'd44: byte_read = b44;
      8'd45: byte_read = b45;
      8'd46: byte_read = b46;
      8'd47: byte_read = b47;
      8'd48: byte_read = b48;
      8'd49: byte_read = b49;
      8'd50: byte_read = b50;
      8'd51: byte_read = b51;
      8'd52: byte_read = b52;
      8'd53: byte_read = b53;
      8'd54: byte_read = b54;
      default: byte_read = b55;
    endcase
    end
  endfunction

  function [7:0] byte_read_high(input [7:0] index);
    byte_read_high = byte_read(index + 8'd1);
  endfunction

  assign mem_rdata = {byte_read_high(mem_addr[7:0]), byte_read(mem_addr[7:0])};
  assign source_rdata = {byte_read_high(source_addr[7:0]), byte_read(source_addr[7:0])};

  always @(posedge clk) begin
    if (rst) begin
      b0 <= program_image[7:0]; b1 <= program_image[15:8]; b2 <= program_image[23:16]; b3 <= program_image[31:24];
      b4 <= program_image[39:32]; b5 <= program_image[47:40]; b6 <= program_image[55:48]; b7 <= program_image[63:56];
      b8 <= program_image[71:64]; b9 <= program_image[79:72]; b10 <= program_image[87:80]; b11 <= program_image[95:88];
      b12 <= program_image[103:96]; b13 <= program_image[111:104]; b14 <= program_image[119:112]; b15 <= program_image[127:120];
      b16 <= program_image[135:128]; b17 <= program_image[143:136]; b18 <= program_image[151:144]; b19 <= program_image[159:152];
      b20 <= program_image[167:160]; b21 <= program_image[175:168]; b22 <= program_image[183:176]; b23 <= program_image[191:184];
      b24 <= program_image[199:192]; b25 <= program_image[207:200]; b26 <= program_image[215:208]; b27 <= program_image[223:216];
      b28 <= program_image[231:224]; b29 <= program_image[239:232]; b30 <= program_image[247:240]; b31 <= program_image[255:248];
      b32 <= program_image[263:256]; b33 <= program_image[271:264]; b34 <= program_image[279:272]; b35 <= program_image[287:280];
      b36 <= program_image[295:288]; b37 <= program_image[303:296]; b38 <= program_image[311:304]; b39 <= program_image[319:312];
      b40 <= program_image[327:320]; b41 <= program_image[335:328]; b42 <= program_image[343:336]; b43 <= program_image[351:344];
      b44 <= program_image[359:352]; b45 <= program_image[367:360]; b46 <= program_image[375:368]; b47 <= program_image[383:376];
      b48 <= program_image[391:384]; b49 <= program_image[399:392]; b50 <= program_image[407:400]; b51 <= program_image[415:408];
      b52 <= program_image[423:416]; b53 <= program_image[431:424]; b54 <= program_image[439:432]; b55 <= program_image[447:440];
      b56 <= program_image[455:448]; b57 <= program_image[463:456]; b58 <= program_image[471:464]; b59 <= program_image[479:472];
      b60 <= program_image[487:480]; b61 <= program_image[495:488]; b62 <= program_image[503:496]; b63 <= program_image[511:504];
    end else if (mem_we) begin
      if (mem_width == 3'd1) begin
        case (mem_addr[5:0])
          6'd0: begin b0 <= mem_wdata[7:0]; b1 <= mem_wdata[15:8]; end
          6'd2: begin b2 <= mem_wdata[7:0]; b3 <= mem_wdata[15:8]; end
          6'd4: begin b4 <= mem_wdata[7:0]; b5 <= mem_wdata[15:8]; end
          6'd6: begin b6 <= mem_wdata[7:0]; b7 <= mem_wdata[15:8]; end
          6'd8: begin b8 <= mem_wdata[7:0]; b9 <= mem_wdata[15:8]; end
          6'd10: begin b10 <= mem_wdata[7:0]; b11 <= mem_wdata[15:8]; end
          6'd12: begin b12 <= mem_wdata[7:0]; b13 <= mem_wdata[15:8]; end
          6'd14: begin b14 <= mem_wdata[7:0]; b15 <= mem_wdata[15:8]; end
          6'd16: begin b16 <= mem_wdata[7:0]; b17 <= mem_wdata[15:8]; end
          6'd18: begin b18 <= mem_wdata[7:0]; b19 <= mem_wdata[15:8]; end
          6'd20: begin b20 <= mem_wdata[7:0]; b21 <= mem_wdata[15:8]; end
          6'd22: begin b22 <= mem_wdata[7:0]; b23 <= mem_wdata[15:8]; end
          6'd24: begin b24 <= mem_wdata[7:0]; b25 <= mem_wdata[15:8]; end
          6'd26: begin b26 <= mem_wdata[7:0]; b27 <= mem_wdata[15:8]; end
          6'd28: begin b28 <= mem_wdata[7:0]; b29 <= mem_wdata[15:8]; end
          6'd30: begin b30 <= mem_wdata[7:0]; b31 <= mem_wdata[15:8]; end
          6'd32: begin b32 <= mem_wdata[7:0]; b33 <= mem_wdata[15:8]; end
          6'd34: begin b34 <= mem_wdata[7:0]; b35 <= mem_wdata[15:8]; end
          6'd36: begin b36 <= mem_wdata[7:0]; b37 <= mem_wdata[15:8]; end
          6'd38: begin b38 <= mem_wdata[7:0]; b39 <= mem_wdata[15:8]; end
          6'd40: begin b40 <= mem_wdata[7:0]; b41 <= mem_wdata[15:8]; end
          6'd42: begin b42 <= mem_wdata[7:0]; b43 <= mem_wdata[15:8]; end
          6'd44: begin b44 <= mem_wdata[7:0]; b45 <= mem_wdata[15:8]; end
          6'd46: begin b46 <= mem_wdata[7:0]; b47 <= mem_wdata[15:8]; end
          6'd48: begin b48 <= mem_wdata[7:0]; b49 <= mem_wdata[15:8]; end
          6'd50: begin b50 <= mem_wdata[7:0]; b51 <= mem_wdata[15:8]; end
          6'd52: begin b52 <= mem_wdata[7:0]; b53 <= mem_wdata[15:8]; end
          6'd54: begin b54 <= mem_wdata[7:0]; b55 <= mem_wdata[15:8]; end
          6'd56: begin b56 <= mem_wdata[7:0]; b57 <= mem_wdata[15:8]; end
          6'd58: begin b58 <= mem_wdata[7:0]; b59 <= mem_wdata[15:8]; end
          6'd60: begin b60 <= mem_wdata[7:0]; b61 <= mem_wdata[15:8]; end
          default: begin b62 <= mem_wdata[7:0]; b63 <= mem_wdata[15:8]; end
        endcase
      end else begin
        case (mem_addr[5:0])
          6'd0: b0 <= mem_wdata[7:0]; 6'd1: b1 <= mem_wdata[7:0]; 6'd2: b2 <= mem_wdata[7:0]; 6'd3: b3 <= mem_wdata[7:0];
          6'd4: b4 <= mem_wdata[7:0]; 6'd5: b5 <= mem_wdata[7:0]; 6'd6: b6 <= mem_wdata[7:0]; 6'd7: b7 <= mem_wdata[7:0];
          6'd8: b8 <= mem_wdata[7:0]; 6'd9: b9 <= mem_wdata[7:0]; 6'd10: b10 <= mem_wdata[7:0]; 6'd11: b11 <= mem_wdata[7:0];
          6'd12: b12 <= mem_wdata[7:0]; 6'd13: b13 <= mem_wdata[7:0]; 6'd14: b14 <= mem_wdata[7:0]; 6'd15: b15 <= mem_wdata[7:0];
          6'd16: b16 <= mem_wdata[7:0]; 6'd17: b17 <= mem_wdata[7:0]; 6'd18: b18 <= mem_wdata[7:0]; 6'd19: b19 <= mem_wdata[7:0];
          6'd20: b20 <= mem_wdata[7:0]; 6'd21: b21 <= mem_wdata[7:0]; 6'd22: b22 <= mem_wdata[7:0]; 6'd23: b23 <= mem_wdata[7:0];
          6'd24: b24 <= mem_wdata[7:0]; 6'd25: b25 <= mem_wdata[7:0]; 6'd26: b26 <= mem_wdata[7:0]; 6'd27: b27 <= mem_wdata[7:0];
          6'd28: b28 <= mem_wdata[7:0]; 6'd29: b29 <= mem_wdata[7:0]; 6'd30: b30 <= mem_wdata[7:0]; 6'd31: b31 <= mem_wdata[7:0];
          6'd32: b32 <= mem_wdata[7:0]; 6'd33: b33 <= mem_wdata[7:0]; 6'd34: b34 <= mem_wdata[7:0]; 6'd35: b35 <= mem_wdata[7:0];
          6'd36: b36 <= mem_wdata[7:0]; 6'd37: b37 <= mem_wdata[7:0]; 6'd38: b38 <= mem_wdata[7:0]; 6'd39: b39 <= mem_wdata[7:0];
          6'd40: b40 <= mem_wdata[7:0]; 6'd41: b41 <= mem_wdata[7:0]; 6'd42: b42 <= mem_wdata[7:0]; 6'd43: b43 <= mem_wdata[7:0];
          6'd44: b44 <= mem_wdata[7:0]; 6'd45: b45 <= mem_wdata[7:0]; 6'd46: b46 <= mem_wdata[7:0]; 6'd47: b47 <= mem_wdata[7:0];
          6'd48: b48 <= mem_wdata[7:0]; 6'd49: b49 <= mem_wdata[7:0]; 6'd50: b50 <= mem_wdata[7:0]; 6'd51: b51 <= mem_wdata[7:0];
          6'd52: b52 <= mem_wdata[7:0]; 6'd53: b53 <= mem_wdata[7:0]; 6'd54: b54 <= mem_wdata[7:0]; default: b55 <= mem_wdata[7:0];
        endcase
      end
    end
  end
endmodule
