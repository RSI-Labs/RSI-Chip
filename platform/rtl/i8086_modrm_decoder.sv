// Complete 8086 ModRM address/register decoder used by the full-core project.
module i8086_modrm_decoder(modrm, displacement, override_present, override_segment,
                           bx, bp, si, di, memory_operand, default_segment,
                           effective_offset, register_index, register_memory_index);
  input [7:0] modrm;
  input [15:0] displacement;
  input override_present;
  input [1:0] override_segment;
  input [15:0] bx;
  input [15:0] bp;
  input [15:0] si;
  input [15:0] di;
  output memory_operand;
  output [1:0] default_segment;
  output [15:0] effective_offset;
  output [2:0] register_index;
  output [2:0] register_memory_index;

  wire [1:0] mode = modrm[7:6];
  wire [2:0] rm = modrm[2:0];
  reg memory_operand_r;
  reg [1:0] default_segment_r;
  reg [15:0] base_offset;
  reg [15:0] effective_offset_r;

  assign register_index = modrm[5:3];
  assign register_memory_index = rm;
  assign memory_operand_r = mode != 2'b11;

  always @* begin
    default_segment_r = 2'b11;
    if (mode != 2'b11 && (rm == 3'b010 || rm == 3'b011 ||
                          (rm == 3'b110 && mode != 2'b00)))
      default_segment_r = 2'b10;
  end

  always @* begin
    base_offset = 16'b0;
    if (mode != 2'b11) begin
      case (rm)
        3'b000: base_offset = bx + si;
        3'b001: base_offset = bx + di;
        3'b010: base_offset = bp + si;
        3'b011: base_offset = bp + di;
        3'b100: base_offset = si;
        3'b101: base_offset = di;
        3'b110: base_offset = mode == 2'b00 ? displacement : bp;
        default: base_offset = bx;
      endcase
    end
  end

  always @* begin
    if (mode == 2'b11)
      effective_offset_r = 16'b0;
    else if (mode == 2'b01 || mode == 2'b10)
      effective_offset_r = base_offset + displacement;
    else
      effective_offset_r = base_offset;
  end

  assign memory_operand = memory_operand_r;
  assign default_segment = override_present ? override_segment : default_segment_r;
  assign effective_offset = effective_offset_r;
endmodule
