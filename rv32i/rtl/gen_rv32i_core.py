#!/usr/bin/env python3
"""Generate rv32i_core.v in the frontend's proven synthesizable style:
individual registers + case multiplexers (no memory arrays / dynamic
indexing), with a one-cycle 2048-bit program-image load port."""

def regs(prefix, n):
    return ", ".join(f"{prefix}{i}" for i in range(n))

def case_mux(sel_bits, value_fmt, n, indent="    "):
    lines = []
    for i in range(n):
        lines.append(f"{indent}{sel_bits}'d{i}: {value_fmt.format(i=i)}")
    return "\n".join(lines)

def gen():
    s = []
    a = s.append
    a("// rv32i_core.v — 由 gen_rv32i_core.py 生成；请勿手工编辑。")
    a("// 单周期 RV32I 子集核：全部 OP/OP-IMM、LUI/AUIPC、JAL/JALR、全部分支、")
    a("// LW/SW；EBREAK/ECALL 停机，FENCE 当 nop。存储用独立寄存器 + case")
    a("// 多路器实现（即综合后寄存器堆的真实门级结构）。x0 恒零。")
    a("// IMEM/DMEM 各 16 词（512 位程序映像）。")
    a("")
    a("module rv32i_core (")
    a("    input clk,")
    a("    input rst,")
    a("    input prog_ld,")
    a("    input [511:0] prog,")
    a("    output halted,")
    a("    output [1:0] trap_cause,")
    a("    output [31:0] dbg_pc,")
    a("    output [31:0] dbg_x10,")
    a("    output [31:0] dbg_x11,")
    a("    output [31:0] dbg_mepc,")
    a("    output [31:0] dbg_mcause")
    a(");")
    a(f"  reg [31:0] {regs('i', 16)};")
    a(f"  reg [31:0] {regs('d', 16)};")
    a(f"  reg [31:0] {regs('x', 32)};")
    a("  reg [31:0] pc;")
    a("  reg halted_r;")
    a("  reg [1:0] trap_cause_r;")
    a("  // Zicsr 机器模式 CSR（裸金属子集）。")
    a("  reg [31:0] csr_mstatus;")
    a("  reg [31:0] csr_mie;")
    a("  reg [31:0] csr_mtvec;")
    a("  reg [31:0] csr_mscratch;")
    a("  reg [31:0] csr_mepc;")
    a("  reg [31:0] csr_mcause;")
    a("")
    a("  // 算术右移（无 `>>>`）：按 shamt 每一位旋转进符号位。")
    a("  // 算术右移（无 `>>>`，只用全字运算，避免对参数做位选择）：")
    a("  //   非负数 = 逻辑右移；负数额外在顶部 sh 位填充 1。")
    a("  function [31:0] sra32(input [31:0] v, input [4:0] sh);")
    a("    begin")
    a("      sra32 = (v < 32'h80000000)")
    a("        ? (v >> sh)")
    a("        : ((v >> sh) | ((sh == 0) ? 32'b0")
    a("             : (32'hffffffff << (32 - sh))));")
    a("    end")
    a("  endfunction")
    a("")
    a("  reg [31:0] instr;")
    a("  reg [31:0] va;")
    a("  reg [31:0] vb;")
    a("  reg [31:0] load_word;")
    a("  reg [15:0] half_sel;")
    a("  reg [7:0] byte_sel;")
    a("  reg [31:0] store_word;")
    a("  reg [31:0] store_dword;")
    a("  always @* begin")
    a("    instr = i0; // 默认值，避免不完整 case 推断出锁存器")
    a("    case (pc[5:2])")
    a(case_mux("4", "instr = i{i};", 16, "      "))
    a("    endcase")
    a("  end")
    a("  always @* begin")
    a("    va = 32'b0;")
    a("    case (rs1)")
    a(case_mux("5", "va = x{i};", 32, "      ").replace("      5'd0: va = x0;", "      5'd0: va = 32'b0;"))
    a("    endcase")
    a("  end")
    a("  always @* begin")
    a("    vb = 32'b0;")
    a("    case (rs2)")
    a(case_mux("5", "vb = x{i};", 32, "      ").replace("      5'd0: vb = x0;", "      5'd0: vb = 32'b0;"))
    a("    endcase")
    a("  end")
    a("""
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
      csr_mcause <= 32'b0;""")
    for i in range(16):
        a(f"      i{i} <= prog[{32*i+31}:{32*i}];")
    a("""    end else if (!halted_r) begin
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
        if (rf_we) begin""")
    for i in range(1, 32):
        a(f"          if (rd == 5'd{i}) x{i} <= rf_wdata;")
    a("""        end
        if (dmem_we) begin
          case (ls_addr_s[5:2])""")
    a(case_mux("4", "d{i} <= store_word;", 16, "            "))
    a("            default: d0 <= store_word;")
    a("""          endcase
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
endmodule""")
    with open("rv32i/rtl/rv32i_core.v", "w") as f:
        f.write("\n".join(s) + "\n")
    print("generated rv32i/rtl/rv32i_core.v")

if __name__ == "__main__":
    gen()
