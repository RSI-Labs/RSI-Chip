module rv32i_isa_decoder(instruction, legal, kind, rd, rs1, rs2, funct3,
                         funct7, imm_i, imm_s, imm_b, imm_u, imm_j);
  input [31:0] instruction;
  output legal;
  output [6:0] kind;
  output [4:0] rd;
  output [4:0] rs1;
  output [4:0] rs2;
  output [2:0] funct3;
  output [6:0] funct7;
  output [31:0] imm_i;
  output [31:0] imm_s;
  output [31:0] imm_b;
  output [31:0] imm_u;
  output [31:0] imm_j;
  localparam KIND_LUI    = 7'd0;
  localparam KIND_AUIPC  = 7'd1;
  localparam KIND_JAL    = 7'd2;
  localparam KIND_JALR   = 7'd3;
  localparam KIND_BRANCH = 7'd4;
  localparam KIND_LOAD   = 7'd5;
  localparam KIND_STORE  = 7'd6;
  localparam KIND_OPIMM  = 7'd7;
  localparam KIND_OP     = 7'd8;
  localparam KIND_FENCE  = 7'd9;
  localparam KIND_SYSTEM = 7'd10;

  wire [6:0] opcode = instruction[6:0];
  assign rd = instruction[11:7];
  assign funct3 = instruction[14:12];
  assign rs1 = instruction[19:15];
  assign rs2 = instruction[24:20];
  assign funct7 = instruction[31:25];
  assign imm_i = {{20{instruction[31]}}, instruction[31:20]};
  assign imm_s = {{20{instruction[31]}}, instruction[31:25], instruction[11:7]};
  assign imm_b = {{19{instruction[31]}}, instruction[31], instruction[7],
                  instruction[30:25], instruction[11:8], 1'b0};
  assign imm_u = {instruction[31:12], 12'b0};
  assign imm_j = {{11{instruction[31]}}, instruction[31], instruction[19:12],
                  instruction[20], instruction[30:21], 1'b0};

  reg legal_r;
  reg [6:0] kind_r;
  always @* begin
    legal_r = 1'b0;
    kind_r = KIND_SYSTEM;
    case (opcode)
      7'b0110111: begin legal_r = 1'b1; kind_r = KIND_LUI; end
      7'b0010111: begin legal_r = 1'b1; kind_r = KIND_AUIPC; end
      7'b1101111: begin legal_r = 1'b1; kind_r = KIND_JAL; end
      7'b1100111:
        if (funct3 == 3'b000 && funct7 == 7'b0000000) begin
          legal_r = 1'b1;
          kind_r = KIND_JALR;
        end
      7'b1100011:
        if (funct3 == 3'b000 || funct3 == 3'b001 || funct3 >= 3'b100) begin
          legal_r = 1'b1;
          kind_r = KIND_BRANCH;
        end
      7'b0000011:
        if (funct3 == 3'b000 || funct3 == 3'b001 || funct3 == 3'b010 ||
            funct3 == 3'b100 || funct3 == 3'b101) begin
          legal_r = 1'b1;
          kind_r = KIND_LOAD;
        end
      7'b0100011:
        if (funct3 <= 3'b010) begin
          legal_r = 1'b1;
          kind_r = KIND_STORE;
        end
      7'b0010011:
        if (funct3 <= 3'b101 && funct3 != 3'b011) begin
          legal_r = 1'b1;
          kind_r = KIND_OPIMM;
        end
      7'b0110011:
        if (funct7 == 7'b0000000 || funct7 == 7'b0100000) begin
          legal_r = 1'b1;
          kind_r = KIND_OP;
        end
      7'b0001111:
        if (funct3 == 3'b000 || funct3 == 3'b001) begin
          legal_r = 1'b1;
          kind_r = KIND_FENCE;
        end
      7'b1110011: begin
        legal_r = 1'b1;
        kind_r = KIND_SYSTEM;
      end
      default: begin
        legal_r = 1'b0;
        kind_r = KIND_SYSTEM;
      end
    endcase

    // Narrow illegal funct combinations that the broad opcode classes above
    // cannot express directly.
    if (opcode == 7'b0010011 && funct3 == 3'b001 && funct7 != 7'b0000000)
      legal_r = 1'b0;
    if (opcode == 7'b0010011 && funct3 == 3'b101 && funct7 != 7'b0000000 &&
        funct7 != 7'b0100000)
      legal_r = 1'b0;
    if (opcode == 7'b0110011 && funct7 == 7'b0100000 &&
        !(funct3 == 3'b000 || funct3 == 3'b101))
      legal_r = 1'b0;
    if (opcode == 7'b0110011 && funct3 == 3'b101 && funct7 == 7'b0100000)
      legal_r = 1'b1;
  end
  assign legal = legal_r;
  assign kind = kind_r;
endmodule
