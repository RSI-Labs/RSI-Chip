// rv32i_core.v — 由 gen_rv32i_core.py 生成；请勿手工编辑。
// 单周期 RV32I 子集核：全部 OP/OP-IMM、LUI/AUIPC、JAL/JALR、全部分支、
// LW/SW；EBREAK/ECALL 停机，FENCE 当 nop。存储用独立寄存器 + case
// 多路器实现（即综合后寄存器堆的真实门级结构）。x0 恒零。
// IMEM/DMEM 各 16 词（512 位程序映像）。

module rv32i_core (
    input clk,
    input rst,
    input prog_ld,
    input [511:0] prog,
    output halted,
    output [1:0] trap_cause,
    output [31:0] dbg_pc,
    output [31:0] dbg_x10,
    output [31:0] dbg_x11,
    output [31:0] dbg_mepc,
    output [31:0] dbg_mcause
);
  reg [31:0] i0, i1, i2, i3, i4, i5, i6, i7, i8, i9, i10, i11, i12, i13, i14, i15;
  reg [31:0] d0, d1, d2, d3, d4, d5, d6, d7, d8, d9, d10, d11, d12, d13, d14, d15;
  reg [31:0] x0, x1, x2, x3, x4, x5, x6, x7, x8, x9, x10, x11, x12, x13, x14, x15, x16, x17, x18, x19, x20, x21, x22, x23, x24, x25, x26, x27, x28, x29, x30, x31;
  reg [31:0] pc;
  reg halted_r;
  reg [1:0] trap_cause_r;
  // Zicsr 机器模式 CSR（裸金属子集）。
  reg [31:0] csr_mstatus;
  reg [31:0] csr_mie;
  reg [31:0] csr_mtvec;
  reg [31:0] csr_mscratch;
  reg [31:0] csr_mepc;
  reg [31:0] csr_mcause;

  // 算术右移（无 `>>>`）：按 shamt 每一位旋转进符号位。
  // 算术右移（无 `>>>`，只用全字运算，避免对参数做位选择）：
  //   非负数 = 逻辑右移；负数额外在顶部 sh 位填充 1。
  function [31:0] sra32(input [31:0] v, input [4:0] sh);
    begin
      sra32 = (v < 32'h80000000)
        ? (v >> sh)
        : ((v >> sh) | ((sh == 0) ? 32'b0
             : (32'hffffffff << (32 - sh))));
    end
  endfunction

  reg [31:0] instr;
  reg [31:0] va;
  reg [31:0] vb;
  reg [31:0] load_word;
  reg [15:0] half_sel;
  reg [7:0] byte_sel;
  reg [31:0] store_word;
  reg [31:0] store_dword;
  always @* begin
    instr = i0; // 默认值，避免不完整 case 推断出锁存器
    case (pc[5:2])
      4'd0: instr = i0;
      4'd1: instr = i1;
      4'd2: instr = i2;
      4'd3: instr = i3;
      4'd4: instr = i4;
      4'd5: instr = i5;
      4'd6: instr = i6;
      4'd7: instr = i7;
      4'd8: instr = i8;
      4'd9: instr = i9;
      4'd10: instr = i10;
      4'd11: instr = i11;
      4'd12: instr = i12;
      4'd13: instr = i13;
      4'd14: instr = i14;
      4'd15: instr = i15;
    endcase
  end
  always @* begin
    va = 32'b0;
    case (rs1)
      5'd0: va = 32'b0;
      5'd1: va = x1;
      5'd2: va = x2;
      5'd3: va = x3;
      5'd4: va = x4;
      5'd5: va = x5;
      5'd6: va = x6;
      5'd7: va = x7;
      5'd8: va = x8;
      5'd9: va = x9;
      5'd10: va = x10;
      5'd11: va = x11;
      5'd12: va = x12;
      5'd13: va = x13;
      5'd14: va = x14;
      5'd15: va = x15;
      5'd16: va = x16;
      5'd17: va = x17;
      5'd18: va = x18;
      5'd19: va = x19;
      5'd20: va = x20;
      5'd21: va = x21;
      5'd22: va = x22;
      5'd23: va = x23;
      5'd24: va = x24;
      5'd25: va = x25;
      5'd26: va = x26;
      5'd27: va = x27;
      5'd28: va = x28;
      5'd29: va = x29;
      5'd30: va = x30;
      5'd31: va = x31;
    endcase
  end
  always @* begin
    vb = 32'b0;
    case (rs2)
      5'd0: vb = 32'b0;
      5'd1: vb = x1;
      5'd2: vb = x2;
      5'd3: vb = x3;
      5'd4: vb = x4;
      5'd5: vb = x5;
      5'd6: vb = x6;
      5'd7: vb = x7;
      5'd8: vb = x8;
      5'd9: vb = x9;
      5'd10: vb = x10;
      5'd11: vb = x11;
      5'd12: vb = x12;
      5'd13: vb = x13;
      5'd14: vb = x14;
      5'd15: vb = x15;
      5'd16: vb = x16;
      5'd17: vb = x17;
      5'd18: vb = x18;
      5'd19: vb = x19;
      5'd20: vb = x20;
      5'd21: vb = x21;
      5'd22: vb = x22;
      5'd23: vb = x23;
      5'd24: vb = x24;
      5'd25: vb = x25;
      5'd26: vb = x26;
      5'd27: vb = x27;
      5'd28: vb = x28;
      5'd29: vb = x29;
      5'd30: vb = x30;
      5'd31: vb = x31;
    endcase
  end

  wire [6:0] opcode = instr[6:0];
  wire [4:0] rd     = instr[11:7];
  wire [2:0] funct3 = instr[14:12];
  wire [4:0] rs1    = instr[19:15];
  wire [4:0] rs2    = instr[24:20];
  wire [6:0] funct7 = instr[31:25];

  wire [31:0] imm_i = {{20{instr[31]}}, instr[31:20]};
  wire [31:0] imm_s = {{20{instr[31]}}, instr[31:25], instr[11:7]};
  wire [31:0] imm_b = {{19{instr[31]}}, instr[31], instr[7], instr[30:25], instr[11:8], 1'b0};
  wire [31:0] imm_u = {instr[31:12], 12'b0};
  wire [31:0] imm_j = {{11{instr[31]}}, instr[31], instr[19:12], instr[20], instr[30:21], 1'b0};

  // 有符号比较：符号相同时位模式序 = 有符号序；符号不同时负者小。
  wire lt_signed = (va[31] != vb[31]) ? va[31] : (va < vb);
  wire ltu       = (va < vb);
  wire imm_lt_s  = (va[31] != imm_i[31]) ? va[31] : (va < imm_i);
  wire imm_ltu   = (va < imm_i);

  wire [4:0] shamt_i = imm_i[4:0];
  wire [4:0] shamt_r = vb[4:0];

  wire [31:0] ls_addr   = va + imm_i;
  wire [31:0] ls_addr_s = va + imm_s;

  // CSR 读-修改-写组合逻辑。
  wire [11:0] csr_addr = instr[31:20];
  reg [31:0] csr_rdata;
  reg csr_we;
  always @* begin
    csr_rdata = 32'b0;
    case (csr_addr)
      12'h300: csr_rdata = csr_mstatus;
      12'h304: csr_rdata = csr_mie;
      12'h305: csr_rdata = csr_mtvec;
      12'h340: csr_rdata = csr_mscratch;
      12'h341: csr_rdata = csr_mepc;
      12'h342: csr_rdata = csr_mcause;
      default: csr_rdata = 32'b0;
    endcase
  end
  // CSR 支持：地址集合 + funct3（W=001/S=010/C=011）。
  function logic csr_supported(input logic [11:0] addr);
    csr_supported = (addr == 12'h300) || (addr == 12'h304) ||
                    (addr == 12'h305) || (addr == 12'h340) ||
                    (addr == 12'h341) || (addr == 12'h342);
  endfunction
  wire csr_is_imm = funct3 == 3'b101 || funct3 == 3'b110 || funct3 == 3'b111;
  wire csrLegal = (funct3 == 3'b001 || funct3 == 3'b010 || funct3 == 3'b011
                   || csr_is_imm)
                  && csr_supported(csr_addr);
  // CSR 写数据：寄存器形式 = rs1(va)；立即数形式 = 零扩展 zimm[4:0]。
  wire [31:0] csr_wdata = csr_is_imm ? {27'b0, instr[19:15]} : va;

  // 装载字/子字选择/存储字：独立 @* 块，避免同块读写自身触发零延迟环路。
  always @* begin
    case (ls_addr[5:2])
      4'd0: load_word = d0;
      4'd1: load_word = d1;
      4'd2: load_word = d2;
      4'd3: load_word = d3;
      4'd4: load_word = d4;
      4'd5: load_word = d5;
      4'd6: load_word = d6;
      4'd7: load_word = d7;
      4'd8: load_word = d8;
      4'd9: load_word = d9;
      4'd10: load_word = d10;
      4'd11: load_word = d11;
      4'd12: load_word = d12;
      4'd13: load_word = d13;
      4'd14: load_word = d14;
      4'd15: load_word = d15;
      default: load_word = 32'b0;
    endcase
  end
  always @* begin
    case (ls_addr[1])
      1'b0: half_sel = load_word[15:0];
      default: half_sel = load_word[31:16];
    endcase
    case (ls_addr[1:0])
      2'd0: byte_sel = load_word[7:0];
      2'd1: byte_sel = load_word[15:8];
      2'd2: byte_sel = load_word[23:16];
      default: byte_sel = load_word[31:24];
    endcase
  end
  always @* begin
    case (ls_addr_s[5:2])
      4'd0: store_dword = d0;
      4'd1: store_dword = d1;
      4'd2: store_dword = d2;
      4'd3: store_dword = d3;
      4'd4: store_dword = d4;
      4'd5: store_dword = d5;
      4'd6: store_dword = d6;
      4'd7: store_dword = d7;
      4'd8: store_dword = d8;
      4'd9: store_dword = d9;
      4'd10: store_dword = d10;
      4'd11: store_dword = d11;
      4'd12: store_dword = d12;
      4'd13: store_dword = d13;
      4'd14: store_dword = d14;
      4'd15: store_dword = d15;
      default: store_dword = 32'b0;
    endcase
  end
  always @* begin
    case (funct3)
      3'b000: begin // SB
        case (ls_addr_s[1:0])
          2'd0: store_word = {store_dword[31:8], vb[7:0]};
          2'd1: store_word = {store_dword[31:16], vb[7:0], store_dword[7:0]};
          2'd2: store_word = {store_dword[31:24], vb[7:0], store_dword[15:0]};
          default: store_word = {vb[7:0], store_dword[23:0]};
        endcase
      end
      3'b001: begin // SH
        if (ls_addr_s[1] == 1'b0)
          store_word = {store_dword[31:16], vb[15:0]};
        else
          store_word = {vb[15:0], store_dword[15:0]};
      end
      default: store_word = vb; // SW
    endcase
  end

  reg [31:0] next_pc;
  reg [31:0] rf_wdata;
  reg rf_we;
  reg dmem_we;

  always @* begin
    next_pc = pc + 4;
    rf_wdata = 32'b0;
    rf_we = 1'b0;
    dmem_we = 1'b0;
    case (opcode)
      7'b0110011: begin
        rf_we = 1'b1;
        case ({funct7, funct3})
          10'b0000000000: rf_wdata = va + vb;
          10'b0100000000: rf_wdata = va - vb;
          10'b0000000001: rf_wdata = va << shamt_r;
          10'b0000000010: rf_wdata = {31'b0, lt_signed};
          10'b0000000011: rf_wdata = {31'b0, ltu};
          10'b0000000100: rf_wdata = va ^ vb;
          10'b0000000101: rf_wdata = va >> shamt_r;
          10'b0100000101: rf_wdata = sra32(va, shamt_r);
          10'b0000000110: rf_wdata = va | vb;
          10'b0000000111: rf_wdata = va & vb;
          default: begin rf_we = 1'b0; rf_wdata = 32'b0; end
        endcase
      end
      7'b0010011: begin
        rf_we = 1'b1;
        case (funct3)
          3'b000: rf_wdata = va + imm_i;
          3'b010: rf_wdata = {31'b0, imm_lt_s};
          3'b011: rf_wdata = {31'b0, imm_ltu};
          3'b100: rf_wdata = va ^ imm_i;
          3'b110: rf_wdata = va | imm_i;
          3'b111: rf_wdata = va & imm_i;
          3'b001: rf_wdata = va << shamt_i;
          3'b101: begin
            if (instr[30]) rf_wdata = sra32(va, shamt_i);
            else           rf_wdata = va >> shamt_i;
          end
          default: begin rf_we = 1'b0; rf_wdata = 32'b0; end
        endcase
      end
      7'b0110111: begin rf_we = 1'b1; rf_wdata = imm_u; end
      7'b0010111: begin rf_we = 1'b1; rf_wdata = pc + imm_u; end
      7'b1101111: begin
        rf_we = 1'b1;
        rf_wdata = pc + 4;
        next_pc = pc + imm_j;
      end
      7'b1100111: begin
        rf_we = 1'b1;
        rf_wdata = pc + 4;
        next_pc = (va + imm_i) & 32'hfffffffe;
      end
      7'b1100011: begin
        case (funct3)
          3'b000: if (va == vb)   next_pc = pc + imm_b;
          3'b001: if (va != vb)   next_pc = pc + imm_b;
          3'b100: if (lt_signed)  next_pc = pc + imm_b;
          3'b101: if (!lt_signed) next_pc = pc + imm_b;
          3'b110: if (ltu)        next_pc = pc + imm_b;
          3'b111: if (!ltu)       next_pc = pc + imm_b;
          default: next_pc = pc + 4;
        endcase
      end
      7'b0000011: begin
        rf_we = 1'b1;
        if (funct3 == 3'b001)
          rf_wdata = {{16{half_sel[15]}}, half_sel};
        else if (funct3 == 3'b101)
          rf_wdata = {16'b0, half_sel};
        else if (funct3 == 3'b000)
          rf_wdata = {{24{byte_sel[7]}}, byte_sel};
        else if (funct3 == 3'b100)
          rf_wdata = {24'b0, byte_sel};
        else
          rf_wdata = load_word;
      end
      7'b0100011: dmem_we = 1'b1;
      7'b1110011: begin
        if (funct3 == 3'b000 && instr[31:20] == 12'h302 && rd == 5'd0) begin
          // MRET：返回 mepc。
          next_pc = csr_mepc;
        end else if (funct3 == 3'b000) begin
          // ECALL/EBREAK：MIE=1 时跳 mtvec（陷入使能）；否则记录后停机。
          if (csr_mstatus[3])
            next_pc = {csr_mtvec[31:2], 2'b00};
          else
            next_pc = pc;
        end else if (csrLegal) begin
          // Zicsr 读-修改-写：rd <= 旧值。
          rf_we = 1'b1;
          rf_wdata = csr_rdata;
          next_pc = pc + 4;
        end else begin
          // 未知/不支持的 CSR：非法，停机标记 3。
          next_pc = pc;
        end
      end
      7'b0001111: begin
        // FENCE / FENCE.I：单核顺序模型下为完整 nop。
        next_pc = pc + 4;
      end
      default: next_pc = pc + 4;
    endcase
  end

  always @(posedge clk) begin
    if (rst || prog_ld) begin
      pc <= 32'b0;
      halted_r <= 1'b0;
      trap_cause_r <= 2'b0;
      csr_mstatus <= 32'b0;
      csr_mie <= 32'b0;
      csr_mtvec <= 32'b0;
      csr_mscratch <= 32'b0;
      csr_mepc <= 32'b0;
      csr_mcause <= 32'b0;
      i0 <= prog[31:0];
      i1 <= prog[63:32];
      i2 <= prog[95:64];
      i3 <= prog[127:96];
      i4 <= prog[159:128];
      i5 <= prog[191:160];
      i6 <= prog[223:192];
      i7 <= prog[255:224];
      i8 <= prog[287:256];
      i9 <= prog[319:288];
      i10 <= prog[351:320];
      i11 <= prog[383:352];
      i12 <= prog[415:384];
      i13 <= prog[447:416];
      i14 <= prog[479:448];
      i15 <= prog[511:480];
    end else if (!halted_r) begin
      if (opcode == 7'b1110011 && funct3 == 3'b000
          && instr[31:20] == 12'h302 && rd == 5'd0) begin
        // MRET：MIE <- MPIE，MPIE <- 1，返回 mepc。
        pc <= next_pc;
        csr_mstatus <= {csr_mstatus[31:8], 1'b1, csr_mstatus[6:4],
                        csr_mstatus[7], csr_mstatus[2:0]};
      end else if (opcode == 7'b1110011 && funct3 == 3'b000 && csr_mstatus[3]) begin
        // 陷入使能：mepc 指向陷入后的下一条指令，返回时跳过 ECALL。
        pc <= next_pc;
        csr_mepc <= pc + 4;
        csr_mcause <= instr[20] ? 32'd3 : 32'd11;
        csr_mstatus <= {csr_mstatus[31:8], 1'b0, csr_mstatus[6:4],
                        csr_mstatus[3], csr_mstatus[2:0]};
      end else if (opcode == 7'b1110011 && funct3 == 3'b000) begin
        // 陷入关闭（裸金属子集）：记录后停机。
        halted_r <= 1'b1;
        csr_mepc <= pc;
        csr_mcause <= instr[20] ? 32'd3 : 32'd11;
        trap_cause_r <= instr[20] ? 2'd2 : 2'd1;
      end else if (opcode == 7'b1110011 && !csrLegal) begin
        halted_r <= 1'b1;
        trap_cause_r <= 2'd3; // CSR unsupported
      end else begin
        if (opcode == 7'b1110011 && csrLegal) begin
          if (funct3 == 3'b001 || funct3 == 3'b101
              || (!csr_is_imm && rs1 != 5'd0)
              || (csr_is_imm && instr[19:15] != 5'd0)) begin
            case (csr_addr)
              12'h300: csr_mstatus <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mstatus | csr_wdata) : (csr_mstatus & ~csr_wdata);
              12'h304: csr_mie <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mie | csr_wdata) : (csr_mie & ~csr_wdata);
              12'h305: csr_mtvec <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mtvec | csr_wdata) : (csr_mtvec & ~csr_wdata);
              12'h340: csr_mscratch <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mscratch | csr_wdata) : (csr_mscratch & ~csr_wdata);
              12'h341: csr_mepc <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mepc | csr_wdata) : (csr_mepc & ~csr_wdata);
              12'h342: csr_mcause <= (funct3 == 3'b001 || funct3 == 3'b101) ? csr_wdata :
                        (funct3 == 3'b010 || funct3 == 3'b110) ? (csr_mcause | csr_wdata) : (csr_mcause & ~csr_wdata);
            endcase
          end
        end
        pc <= next_pc;
        if (rf_we) begin
          if (rd == 5'd1) x1 <= rf_wdata;
          if (rd == 5'd2) x2 <= rf_wdata;
          if (rd == 5'd3) x3 <= rf_wdata;
          if (rd == 5'd4) x4 <= rf_wdata;
          if (rd == 5'd5) x5 <= rf_wdata;
          if (rd == 5'd6) x6 <= rf_wdata;
          if (rd == 5'd7) x7 <= rf_wdata;
          if (rd == 5'd8) x8 <= rf_wdata;
          if (rd == 5'd9) x9 <= rf_wdata;
          if (rd == 5'd10) x10 <= rf_wdata;
          if (rd == 5'd11) x11 <= rf_wdata;
          if (rd == 5'd12) x12 <= rf_wdata;
          if (rd == 5'd13) x13 <= rf_wdata;
          if (rd == 5'd14) x14 <= rf_wdata;
          if (rd == 5'd15) x15 <= rf_wdata;
          if (rd == 5'd16) x16 <= rf_wdata;
          if (rd == 5'd17) x17 <= rf_wdata;
          if (rd == 5'd18) x18 <= rf_wdata;
          if (rd == 5'd19) x19 <= rf_wdata;
          if (rd == 5'd20) x20 <= rf_wdata;
          if (rd == 5'd21) x21 <= rf_wdata;
          if (rd == 5'd22) x22 <= rf_wdata;
          if (rd == 5'd23) x23 <= rf_wdata;
          if (rd == 5'd24) x24 <= rf_wdata;
          if (rd == 5'd25) x25 <= rf_wdata;
          if (rd == 5'd26) x26 <= rf_wdata;
          if (rd == 5'd27) x27 <= rf_wdata;
          if (rd == 5'd28) x28 <= rf_wdata;
          if (rd == 5'd29) x29 <= rf_wdata;
          if (rd == 5'd30) x30 <= rf_wdata;
          if (rd == 5'd31) x31 <= rf_wdata;
        end
        if (dmem_we) begin
          case (ls_addr_s[5:2])
            4'd0: d0 <= store_word;
            4'd1: d1 <= store_word;
            4'd2: d2 <= store_word;
            4'd3: d3 <= store_word;
            4'd4: d4 <= store_word;
            4'd5: d5 <= store_word;
            4'd6: d6 <= store_word;
            4'd7: d7 <= store_word;
            4'd8: d8 <= store_word;
            4'd9: d9 <= store_word;
            4'd10: d10 <= store_word;
            4'd11: d11 <= store_word;
            4'd12: d12 <= store_word;
            4'd13: d13 <= store_word;
            4'd14: d14 <= store_word;
            4'd15: d15 <= store_word;
            default: d0 <= store_word;
          endcase
        end
      end
    end
  end

  assign halted   = halted_r;
  assign trap_cause = trap_cause_r;
  assign dbg_mepc = csr_mepc;
  assign dbg_mcause = csr_mcause;
  assign dbg_pc   = pc;
  assign dbg_x10  = x10;
  assign dbg_x11  = x11;
endmodule
