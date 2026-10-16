// Full multicycle RV32I core with external instruction/data memories.
// The core implements RV32I base plus the machine-mode CSR/trap subset needed
// to execute freestanding programs. Instruction and data words are supplied
// combinationally; stores present a merged 32-bit word and byte-enable size.
module rv32i_full_core(clk, rst, init_pc, imem_rdata, dmem_rdata, imem_addr,
                       dmem_addr, dmem_wdata, dmem_width, dmem_req, dmem_we,
                       halted, trap, trap_cause, dbg_pc, dbg_x, dbg_x10);
  input clk;
  input rst;
  input [31:0] init_pc;
  input [31:0] imem_rdata;
  input [31:0] dmem_rdata;
  output [31:0] imem_addr;
  output [31:0] dmem_addr;
  output [31:0] dmem_wdata;
  output [2:0] dmem_width;
  output dmem_req;
  output dmem_we;
  output halted;
  output trap;
  output [31:0] trap_cause;
  output [31:0] dbg_pc;
  output [127:0] dbg_x;
  output [31:0] dbg_x10;

  localparam S_FETCH = 2'd0;
  localparam S_EXECUTE = 2'd1;
  localparam S_HALTED = 2'd2;

  reg [1:0] state;
  reg [31:0] pc;
  reg [31:0] instr;
  reg halted_r;
  reg trap_r;
  reg [31:0] trap_cause_r;
  reg [31:0] x1, x2, x3, x4, x5, x6, x7, x8, x9, x10, x11, x12, x13, x14;
  reg [31:0] x15, x16, x17, x18, x19, x20, x21, x22, x23, x24, x25, x26;
  reg [31:0] x27, x28, x29, x30, x31;
  reg [31:0] csr_mstatus, csr_misa, csr_medeleg, csr_mideleg, csr_mie;
  reg [31:0] csr_mtvec, csr_mscratch, csr_mepc, csr_mcause, csr_mtval, csr_mip;
  reg [63:0] cycle_count;
  reg [63:0] instret_count;

  wire [6:0] opcode = instr[6:0];
  wire [4:0] rd = instr[11:7];
  wire [2:0] funct3 = instr[14:12];
  wire [4:0] rs1 = instr[19:15];
  wire [4:0] rs2 = instr[24:20];
  wire [6:0] funct7 = instr[31:25];
  wire [31:0] imm_i = {{20{instr[31]}}, instr[31:20]};
  wire [31:0] imm_s = {{20{instr[31]}}, instr[31:25], instr[11:7]};
  wire [31:0] imm_b = {{19{instr[31]}}, instr[31], instr[7],
                       instr[30:25], instr[11:8], 1'b0};
  wire [31:0] imm_u = {instr[31:12], 12'b0};
  wire [31:0] imm_j = {{11{instr[31]}}, instr[31], instr[19:12],
                       instr[20], instr[30:21], 1'b0};

  reg [31:0] va;
  reg [31:0] vb;
  reg [31:0] next_pc;
  reg [31:0] rf_wdata;
  reg rf_we;
  reg exception;
  reg [31:0] exception_cause;
  reg [31:0] exception_tval;
  reg csr_write;
  reg [31:0] csr_wdata;
  reg [31:0] stored_word;
  reg [31:0] loaded_word;

  function [31:0] sra32(input [31:0] value, input [4:0] shift);
    begin
      sra32 = (value < 32'h80000000)
        ? (value >> shift)
        : ((value >> shift) | ((shift == 0) ? 32'b0 :
             (32'hffffffff << (32 - shift))));
    end
  endfunction

  function logic csr_address_legal(input logic [11:0] address);
    begin
      csr_address_legal = address == 12'h300 || address == 12'h301 ||
                          address == 12'h302 || address == 12'h303 ||
                          address == 12'h304 || address == 12'h305 ||
                          address == 12'h306 || address == 12'h340 ||
                          address == 12'h341 || address == 12'h342 ||
                          address == 12'h343 || address == 12'h344 ||
                          address == 12'hc00 || address == 12'hc01 ||
                          address == 12'hc02;
    end
  endfunction

  function [31:0] csr_read_value(input [11:0] address);
    begin
      case (address)
        12'h300: csr_read_value = csr_mstatus;
        12'h301: csr_read_value = csr_misa;
        12'h302: csr_read_value = csr_medeleg;
        12'h303: csr_read_value = csr_mideleg;
        12'h304: csr_read_value = csr_mie;
        12'h305: csr_read_value = csr_mtvec;
        12'h306: csr_read_value = 32'b0;
        12'h340: csr_read_value = csr_mscratch;
        12'h341: csr_read_value = csr_mepc;
        12'h342: csr_read_value = csr_mcause;
        12'h343: csr_read_value = csr_mtval;
        12'h344: csr_read_value = csr_mip;
        12'hc00: csr_read_value = cycle_count[31:0];
        12'hc01: csr_read_value = cycle_count[31:0];
        12'hc02: csr_read_value = instret_count[31:0];
        default: csr_read_value = 32'b0;
      endcase
    end
  endfunction

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

  wire [31:0] ls_load_addr = va + imm_i;
  wire [31:0] ls_store_addr = va + imm_s;
  wire [31:0] branch_target = pc + imm_b;
  wire [31:0] jalr_target = (va + imm_i) & 32'hfffffffe;

  always @* begin
    loaded_word = dmem_rdata;
    stored_word = vb;
    case (funct3)
      3'b000: stored_word = {24'b0, vb[7:0]};
      3'b001: stored_word = {16'b0, vb[15:0]};
      3'b010: stored_word = vb;
      default: stored_word = vb;
    endcase
  end

  always @* begin
    next_pc = pc + 32'd4;
    rf_we = 1'b0;
    rf_wdata = 32'b0;
    exception = 1'b0;
    exception_cause = 32'd2;
    exception_tval = instr;
    csr_write = 1'b0;
    csr_wdata = 32'b0;

    case (opcode)
      7'b0110111: begin rf_we = 1'b1; rf_wdata = imm_u; end
      7'b0010111: begin rf_we = 1'b1; rf_wdata = pc + imm_u; end
      7'b1101111: begin
        rf_we = 1'b1;
        rf_wdata = pc + 32'd4;
        next_pc = pc + imm_j;
      end
      7'b1100111: begin
        rf_we = 1'b1;
        rf_wdata = pc + 32'd4;
        next_pc = jalr_target;
      end
      7'b1100011: begin
        case (funct3)
          3'b000: next_pc = (va == vb) ? branch_target : pc + 32'd4;
          3'b001: next_pc = (va != vb) ? branch_target : pc + 32'd4;
          3'b100: next_pc = (va[31] != vb[31]) ? (va[31] ? branch_target : pc + 4)
                                                 : ((va < vb) ? branch_target : pc + 4);
          3'b101: next_pc = (va[31] != vb[31]) ? (va[31] ? pc + 4 : branch_target)
                                                 : ((va >= vb) ? pc + 4 : branch_target);
          3'b110: next_pc = (va < vb) ? branch_target : pc + 4;
          3'b111: next_pc = (va >= vb) ? pc + 4 : branch_target;
          default: exception = 1'b1;
        endcase
        if (branch_target[1:0] != 2'b00) begin
          exception = 1'b1;
          exception_cause = 32'd0;
          exception_tval = branch_target;
        end
      end
      7'b0000011: begin
        rf_we = 1'b1;
        case (funct3)
          3'b000:
            rf_wdata = ls_load_addr[1:0] == 2'd0
              ? {{24{loaded_word[7]}}, loaded_word[7:0]}
              : ls_load_addr[1:0] == 2'd1
                ? {{24{loaded_word[15]}}, loaded_word[15:8]}
                : ls_load_addr[1:0] == 2'd2
                  ? {{24{loaded_word[23]}}, loaded_word[23:16]}
                  : {{24{loaded_word[31]}}, loaded_word[31:24]};
          3'b001:
            rf_wdata = ls_load_addr[1] == 1'b0
              ? {{16{loaded_word[15]}}, loaded_word[15:0]}
              : {{16{loaded_word[31]}}, loaded_word[31:16]};
          3'b010: rf_wdata = loaded_word;
          3'b100:
            rf_wdata = ls_load_addr[1:0] == 2'd0 ? {24'b0, loaded_word[7:0]} :
                        ls_load_addr[1:0] == 2'd1 ? {24'b0, loaded_word[15:8]} :
                        ls_load_addr[1:0] == 2'd2 ? {24'b0, loaded_word[23:16]} :
                                                    {24'b0, loaded_word[31:24]};
          3'b101:
            rf_wdata = ls_load_addr[1] == 1'b0
              ? {16'b0, loaded_word[15:0]}
              : {16'b0, loaded_word[31:16]};
          default: exception = 1'b1;
        endcase
      end
      7'b0100011: rf_we = 1'b0;
      7'b0010011: begin
        rf_we = 1'b1;
        case (funct3)
          3'b000: rf_wdata = va + imm_i;
          3'b010: rf_wdata = ({va[31], va} < {imm_i[31], imm_i}) ? 32'd1 : 32'd0;
          3'b011: rf_wdata = (va < imm_i) ? 32'd1 : 32'd0;
          3'b100: rf_wdata = va ^ imm_i;
          3'b110: rf_wdata = va | imm_i;
          3'b111: rf_wdata = va & imm_i;
          3'b001: rf_wdata = (funct7 == 7'b0000000) ? (va << imm_i[4:0]) : 32'b0;
          3'b101: rf_wdata = (funct7 == 7'b0000000) ? (va >> imm_i[4:0])
                        : (funct7 == 7'b0100000 ? sra32(va, imm_i[4:0]) : 32'b0);
          default: rf_wdata = 32'b0;
        endcase
        if ((funct3 == 3'b001 && funct7 != 7'b0000000) ||
            (funct3 == 3'b101 && funct7 != 7'b0000000 && funct7 != 7'b0100000))
          exception = 1'b1;
      end
      7'b0110011: begin
        rf_we = 1'b1;
        case ({funct7, funct3})
          {7'b0000000, 3'b000}: rf_wdata = va + vb;
          {7'b0100000, 3'b000}: rf_wdata = va - vb;
          {7'b0000000, 3'b001}: rf_wdata = va << vb[4:0];
          {7'b0000000, 3'b010}: rf_wdata = (va[31] != vb[31]) ? {va[31], 31'b0}
                                  : ((va < vb) ? 32'd1 : 32'd0);
          {7'b0000000, 3'b011}: rf_wdata = (va < vb) ? 32'd1 : 32'd0;
          {7'b0000000, 3'b100}: rf_wdata = va ^ vb;
          {7'b0000000, 3'b101}: rf_wdata = va >> vb[4:0];
          {7'b0100000, 3'b101}: rf_wdata = sra32(va, vb[4:0]);
          {7'b0000000, 3'b110}: rf_wdata = va | vb;
          {7'b0000000, 3'b111}: rf_wdata = va & vb;
          default: begin
            rf_wdata = 32'b0;
            exception = 1'b1;
          end
        endcase
      end
      7'b0001111: begin
        if (funct3 > 3'b001) exception = 1'b1;
      end
      7'b1110011: begin
        if (funct3 == 3'b000) begin
          if (instr[31:20] == 12'b0 || instr[31:20] == 12'b1) begin
            exception = 1'b1;
            exception_cause = instr[20] ? 32'd3 : 32'd11;
            exception_tval = 32'b0;
          end
        end else if (funct3 == 3'b001 || funct3 == 3'b010 || funct3 == 3'b011 ||
                     funct3 == 3'b101 || funct3 == 3'b110 || funct3 == 3'b111) begin
          rf_we = 1'b1;
          rf_wdata = csr_read_value(instr[31:20]);
            if (!csr_address_legal(instr[31:20])) begin
            exception = 1'b1;
            exception_cause = 32'd2;
            rf_we = 1'b0;
          end else begin
            csr_write = 1'b1;
            if (funct3[2])
              csr_wdata = {27'b0, instr[19:15]};
            else
              csr_wdata = va;
            if ((funct3[1:0] == 2'b10 || funct3[1:0] == 2'b11) &&
                (funct3[2] ? instr[19:15] == 5'b0 : rs1 == 5'b0))
              csr_write = 1'b0;
          end
        end
      end
      default: exception = 1'b1;
    endcase

    if (opcode == 7'b1100111 && jalr_target[1:0] != 2'b00) begin
      exception = 1'b1;
      exception_cause = 32'd0;
      exception_tval = jalr_target;
    end
  end

  assign imem_addr = pc;
  assign dmem_addr = (opcode == 7'b0100011) ? ls_store_addr : ls_load_addr;
  assign dmem_width = funct3;
  assign dmem_wdata = stored_word;
  assign dmem_req = (state == S_EXECUTE) &&
                    (opcode == 7'b0000011 || opcode == 7'b0100011);
  assign dmem_we = (state == S_EXECUTE) && opcode == 7'b0100011;
  assign halted = halted_r;
  assign trap = trap_r;
  assign trap_cause = trap_cause_r;
  assign dbg_pc = pc;
  assign dbg_x = {x4,x3,x2,x1};
  assign dbg_x10 = x10;

  always @(posedge clk) begin
    if (rst) begin
      state <= S_FETCH;
      pc <= init_pc;
      instr <= 32'b0;
      halted_r <= 1'b0;
      trap_r <= 1'b0;
      trap_cause_r <= 32'b0;
      x1 <= 0; x2 <= 0; x3 <= 0; x4 <= 0; x5 <= 0; x6 <= 0; x7 <= 0;
      x8 <= 0; x9 <= 0; x10 <= 0; x11 <= 0; x12 <= 0; x13 <= 0; x14 <= 0;
      x15 <= 0; x16 <= 0; x17 <= 0; x18 <= 0; x19 <= 0; x20 <= 0; x21 <= 0;
      x22 <= 0; x23 <= 0; x24 <= 0; x25 <= 0; x26 <= 0; x27 <= 0; x28 <= 0;
      x29 <= 0; x30 <= 0; x31 <= 0;
      csr_mstatus <= 32'b0;
      csr_misa <= 32'h40000000;
      csr_medeleg <= 32'b0;
      csr_mideleg <= 32'b0;
      csr_mie <= 32'b0;
      csr_mtvec <= 32'b0;
      csr_mscratch <= 32'b0;
      csr_mepc <= 32'b0;
      csr_mcause <= 32'b0;
      csr_mtval <= 32'b0;
      csr_mip <= 32'b0;
      cycle_count <= 64'b0;
      instret_count <= 64'b0;
    end else begin
      cycle_count <= cycle_count + 64'd1;
      case (state)
        S_FETCH: begin
          instr <= imem_rdata;
          state <= S_EXECUTE;
        end
        S_EXECUTE: begin
          instret_count <= instret_count + 64'd1;
          if (exception) begin
            trap_r <= 1'b1;
            trap_cause_r <= exception_cause;
            csr_mepc <= pc;
            csr_mcause <= exception_cause;
            csr_mtval <= exception_tval;
            if (csr_mtvec != 32'b0) begin
              pc <= csr_mtvec;
              state <= S_FETCH;
            end else begin
              halted_r <= 1'b1;
              state <= S_HALTED;
            end
          end else begin
            pc <= next_pc;
            state <= S_FETCH;

            if (rf_we) begin
              case (rd)
                5'd1: x1 <= rf_wdata;
                5'd2: x2 <= rf_wdata;
                5'd3: x3 <= rf_wdata;
                5'd4: x4 <= rf_wdata;
                5'd5: x5 <= rf_wdata;
                5'd6: x6 <= rf_wdata;
                5'd7: x7 <= rf_wdata;
                5'd8: x8 <= rf_wdata;
                5'd9: x9 <= rf_wdata;
                5'd10: x10 <= rf_wdata;
                5'd11: x11 <= rf_wdata;
                5'd12: x12 <= rf_wdata;
                5'd13: x13 <= rf_wdata;
                5'd14: x14 <= rf_wdata;
                5'd15: x15 <= rf_wdata;
                5'd16: x16 <= rf_wdata;
                5'd17: x17 <= rf_wdata;
                5'd18: x18 <= rf_wdata;
                5'd19: x19 <= rf_wdata;
                5'd20: x20 <= rf_wdata;
                5'd21: x21 <= rf_wdata;
                5'd22: x22 <= rf_wdata;
                5'd23: x23 <= rf_wdata;
                5'd24: x24 <= rf_wdata;
                5'd25: x25 <= rf_wdata;
                5'd26: x26 <= rf_wdata;
                5'd27: x27 <= rf_wdata;
                5'd28: x28 <= rf_wdata;
                5'd29: x29 <= rf_wdata;
                5'd30: x30 <= rf_wdata;
                5'd31: x31 <= rf_wdata;
                default: x1 <= x1;
              endcase
            end

            if (csr_write) begin
              case (instr[31:20])
                12'h300: csr_mstatus <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mstatus | csr_wdata) :
                            (csr_mstatus & ~csr_wdata);
                12'h304: csr_mie <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mie | csr_wdata) :
                            (csr_mie & ~csr_wdata);
                12'h305: csr_mtvec <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mtvec | csr_wdata) :
                            (csr_mtvec & ~csr_wdata);
                12'h340: csr_mscratch <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mscratch | csr_wdata) :
                            (csr_mscratch & ~csr_wdata);
                12'h341: csr_mepc <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mepc | csr_wdata) :
                            (csr_mepc & ~csr_wdata);
                12'h342: csr_mcause <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mcause | csr_wdata) :
                            (csr_mcause & ~csr_wdata);
                12'h343: csr_mtval <= (funct3[1:0] == 2'b01) ? csr_wdata :
                            (funct3[1:0] == 2'b10) ? (csr_mtval | csr_wdata) :
                            (csr_mtval & ~csr_wdata);
                default: x1 <= x1;
              endcase
            end

            if (opcode == 7'b1110011 && funct3 == 3'b000 && instr[31:20] == 12'h302) begin
              pc <= csr_mepc;
              trap_r <= 1'b0;
              trap_cause_r <= 32'b0;
              csr_mstatus <= {csr_mstatus[31:8], 1'b1, csr_mstatus[6:4],
                              csr_mstatus[7], csr_mstatus[2:0]};
              state <= S_FETCH;
            end
          end
        end
        S_HALTED: state <= S_HALTED;
        default: state <= S_FETCH;
      endcase
    end
  end
endmodule
