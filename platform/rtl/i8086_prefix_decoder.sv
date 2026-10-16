module i8086_prefix_decoder(first_byte, is_prefix, is_segment_override,
                            segment_index, segment_number, repeat_active,
                            repeat_while_zero, lock);
  input [7:0] first_byte;
  output is_prefix;
  output is_segment_override;
  output segment_index;
  output [1:0] segment_number;
  output repeat_active;
  output repeat_while_zero;
  output lock;
  assign is_prefix = first_byte == 8'hf0 || first_byte == 8'hf2 ||
                     first_byte == 8'hf3 || first_byte == 8'h26 ||
                     first_byte == 8'h2e || first_byte == 8'h36 ||
                     first_byte == 8'h3e;
  assign is_segment_override = first_byte == 8'h26 || first_byte == 8'h2e ||
                               first_byte == 8'h36 || first_byte == 8'h3e;
  assign segment_index = first_byte == 8'h36;
  assign segment_number = first_byte == 8'h26 ? 2'b00 :
                          first_byte == 8'h2e ? 2'b01 :
                          first_byte == 8'h36 ? 2'b10 : 2'b11;
  assign repeat_active = first_byte == 8'hf2 || first_byte == 8'hf3;
  assign repeat_while_zero = first_byte == 8'hf3;
  assign lock = first_byte == 8'hf0;
endmodule
