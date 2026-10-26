module i8086_opcode_decoder(opcode, modrm, valid, opcode_class, width8,
                            has_modrm, immediate_size, group_operation,
                            string_operation, control_flow, repeatable);
  input [7:0] opcode;
  input [7:0] modrm;
  output valid;
  output [5:0] opcode_class;
  output width8;
  output has_modrm;
  output [1:0] immediate_size; // 0=none 1=byte 2=word 3=address-or-special
  output [2:0] group_operation;
  output string_operation;
  output control_flow;
  output repeatable;

  localparam CLASS_INVALID = 6'd0;
  localparam CLASS_ALU = 6'd1;
  localparam CLASS_SEGMENT_STACK = 6'd2;
  localparam CLASS_INC_DEC = 6'd3;
  localparam CLASS_PUSH_POP = 6'd4;
  localparam CLASS_CONDITIONAL_BRANCH = 6'd5;
  localparam CLASS_IMM_ALU = 6'd6;
  localparam CLASS_TEST_XCHG_MOV = 6'd7;
  localparam CLASS_ADDRESSING = 6'd8;
  localparam CLASS_ACCUMULATOR = 6'd9;
  localparam CLASS_STRING = 6'd10;
  localparam CLASS_MOFFS_TEST = 6'd11;
  localparam CLASS_MOVE_IMMEDIATE = 6'd12;
  localparam CLASS_CALL_RETURN = 6'd13;
  localparam CLASS_INTERRUPT = 6'd14;
  localparam CLASS_SHIFT_ROTATE = 6'd15;
  localparam CLASS_BCD_ADJUST = 6'd16;
  localparam CLASS_ESCAPE = 6'd17;
  localparam CLASS_LOOP_IO = 6'd18;
  localparam CLASS_NEAR_CONTROL = 6'd19;
  localparam CLASS_PORT_IO = 6'd20;
  localparam CLASS_PREFIX_FLAG = 6'd21;
  localparam CLASS_UNARY = 6'd22;
  localparam CLASS_RM_CONTROL = 6'd23;

  reg valid_r;
  reg [5:0] class_r;
  reg width8_r;
  reg has_modrm_r;
  reg [1:0] immediate_size_r;
  reg string_operation_r;
  reg control_flow_r;
  reg repeatable_r;

  always @* begin
    valid_r = 1'b1;
    class_r = CLASS_INVALID;
    width8_r = 1'b0;
    has_modrm_r = 1'b0;
    immediate_size_r = 2'b00;
    string_operation_r = 1'b0;
    control_flow_r = 1'b0;
    repeatable_r = 1'b0;

    if (opcode <= 8'h3d && opcode[2:0] <= 3'b101)
      begin
        class_r = CLASS_ALU;
        width8_r = ~opcode[0];
        has_modrm_r = opcode[2:0] <= 3'b011;
        immediate_size_r = opcode[2:0] == 3'b100 ? 2'b01 :
                           opcode[2:0] == 3'b101 ? 2'b10 : 2'b00;
      end
    else if (opcode == 8'h06 || opcode == 8'h07 || opcode == 8'h0e ||
             opcode == 8'h16 || opcode == 8'h17 || opcode == 8'h1e ||
             opcode == 8'h1f)
      begin class_r = CLASS_SEGMENT_STACK; control_flow_r = 1'b0; end
    else if (opcode >= 8'h40 && opcode <= 8'h4f)
      begin class_r = CLASS_INC_DEC; width8_r = 1'b0; end
    else if (opcode >= 8'h50 && opcode <= 8'h5f)
      begin class_r = CLASS_PUSH_POP; control_flow_r = opcode >= 8'h58; end
    else if (opcode >= 8'h70 && opcode <= 8'h7f)
      begin
        class_r = CLASS_CONDITIONAL_BRANCH;
        control_flow_r = 1'b1;
        immediate_size_r = 2'b01;
      end
    else if (opcode == 8'h80 || opcode == 8'h81 || opcode == 8'h82 || opcode == 8'h83)
      begin
        class_r = CLASS_IMM_ALU;
        has_modrm_r = 1'b1;
        width8_r = opcode == 8'h80 || opcode == 8'h82;
        immediate_size_r = opcode == 8'h81 ? 2'b10 : 2'b01;
      end
    else if (opcode >= 8'h84 && opcode <= 8'h8b)
      begin
        class_r = CLASS_TEST_XCHG_MOV;
        has_modrm_r = 1'b1;
        width8_r = opcode == 8'h84 || opcode == 8'h86 ||
                   opcode == 8'h88 || opcode == 8'h8a;
      end
    else if (opcode >= 8'h8c && opcode <= 8'h8f)
      begin class_r = CLASS_ADDRESSING; has_modrm_r = 1'b1; end
    else if (opcode >= 8'h90 && opcode <= 8'h9f)
      begin
        class_r = CLASS_ACCUMULATOR;
        control_flow_r = opcode == 8'h9c || opcode == 8'h9d;
      end
    else if ((opcode >= 8'ha4 && opcode <= 8'ha7) ||
             (opcode >= 8'haa && opcode <= 8'haf))
      begin
        class_r = CLASS_STRING;
        string_operation_r = 1'b1;
        repeatable_r = 1'b1;
        width8_r = ~opcode[0];
      end
    else if (opcode >= 8'ha0 && opcode <= 8'ha3)
      begin class_r = CLASS_MOFFS_TEST; immediate_size_r = 2'b11; width8_r = ~opcode[0]; end
    else if (opcode == 8'ha8 || opcode == 8'ha9)
      begin
        class_r = CLASS_MOFFS_TEST;
        immediate_size_r = opcode == 8'ha8 ? 2'b01 : 2'b10;
        width8_r = opcode == 8'ha8;
      end
    else if (opcode >= 8'hb0 && opcode <= 8'hbf)
      begin
        class_r = CLASS_MOVE_IMMEDIATE;
        width8_r = opcode <= 8'hb7;
        immediate_size_r = opcode <= 8'hb7 ? 2'b01 : 2'b10;
      end
    else if (opcode == 8'hc0 || opcode == 8'hc1)
      begin
        class_r = CLASS_SHIFT_ROTATE;
        has_modrm_r = 1'b1;
        width8_r = opcode == 8'hc0;
        immediate_size_r = 2'b01;
      end
    else if (opcode == 8'hc2 || opcode == 8'hc3 || opcode == 8'hca ||
             opcode == 8'hcb || opcode == 8'hc4 || opcode == 8'hc5 ||
             opcode == 8'hc6 || opcode == 8'hc7 || opcode == 8'hcf)
      begin
        class_r = CLASS_CALL_RETURN;
        control_flow_r = opcode != 8'hc6 && opcode != 8'hc7;
        has_modrm_r = opcode == 8'hc4 || opcode == 8'hc5 ||
                      opcode == 8'hc6 || opcode == 8'hc7;
        width8_r = opcode == 8'hc6;
        immediate_size_r = opcode == 8'hc2 || opcode == 8'hca ? 2'b10 :
                           opcode == 8'hc6 ? 2'b01 :
                           opcode == 8'hc7 ? 2'b10 : 2'b00;
      end
    else if (opcode >= 8'hcc && opcode <= 8'hce)
      begin
        class_r = CLASS_INTERRUPT;
        control_flow_r = 1'b1;
        immediate_size_r = opcode == 8'hcd ? 2'b01 : 2'b00;
      end
    else if (opcode >= 8'hd0 && opcode <= 8'hd3)
      begin
        class_r = CLASS_SHIFT_ROTATE;
        has_modrm_r = 1'b1;
        width8_r = ~opcode[0];
      end
    else if (opcode == 8'h27 || opcode == 8'h2f ||
             opcode == 8'h37 || opcode == 8'h3f)
      begin class_r = CLASS_BCD_ADJUST; end
    else if (opcode >= 8'hd4 && opcode <= 8'hd7)
      begin
        class_r = CLASS_BCD_ADJUST;
        immediate_size_r = opcode == 8'hd4 || opcode == 8'hd5 ? 2'b01 : 2'b00;
      end
    else if (opcode >= 8'hd8 && opcode <= 8'hdf)
      begin class_r = CLASS_ESCAPE; has_modrm_r = 1'b1; end
    else if (opcode >= 8'he0 && opcode <= 8'he3)
      begin
        class_r = CLASS_LOOP_IO;
        control_flow_r = 1'b1;
        immediate_size_r = 2'b01;
      end
    else if (opcode >= 8'he8 && opcode <= 8'heb)
      begin
        class_r = CLASS_NEAR_CONTROL;
        control_flow_r = 1'b1;
        immediate_size_r = opcode == 8'heb ? 2'b01 : 2'b10;
      end
    else if (opcode == 8'hec || opcode == 8'hed || opcode == 8'hee || opcode == 8'hef)
      begin class_r = CLASS_PORT_IO; width8_r = 1'b0; end
    else if (opcode == 8'he4 || opcode == 8'he5 || opcode == 8'he6 || opcode == 8'he7)
      begin
        class_r = CLASS_PORT_IO;
        width8_r = opcode == 8'he4 || opcode == 8'he6;
        immediate_size_r = 2'b01;
      end
    else if (opcode == 8'hf0 || opcode == 8'hf2 || opcode == 8'hf3 ||
             opcode == 8'hf4 || opcode == 8'hf5 || opcode == 8'hf8 ||
             opcode == 8'hf9 || opcode == 8'hfa || opcode == 8'hfb ||
             opcode == 8'hfc || opcode == 8'hfd)
      begin
        class_r = CLASS_PREFIX_FLAG;
        if (opcode == 8'hf4) control_flow_r = 1'b1;
      end
    else if (opcode == 8'hf6 || opcode == 8'hf7)
      begin
        class_r = CLASS_UNARY;
        has_modrm_r = 1'b1;
        width8_r = opcode == 8'hf6;
        immediate_size_r = modrm[5:3] == 3'b000 || modrm[5:3] == 3'b001 ?
                           (opcode == 8'hf6 ? 2'b01 : 2'b10) : 2'b00;
      end
    else if (opcode == 8'hfe || opcode == 8'hff)
      begin class_r = CLASS_RM_CONTROL; has_modrm_r = 1'b1; end
    else if (opcode == 8'hf1 || opcode == 8'h60 || opcode == 8'h61 ||
             opcode == 8'h62 || opcode == 8'h63 || opcode == 8'h64 ||
             opcode == 8'h65 || opcode == 8'h66 || opcode == 8'h67 ||
             opcode == 8'h68 || opcode == 8'h69 || opcode == 8'h6a ||
             opcode == 8'h6b || opcode == 8'hc0 || opcode == 8'hc1 ||
             opcode == 8'hc8 || opcode == 8'hc9)
      valid_r = 1'b0;
    else
      valid_r = 1'b0;
  end

  assign valid = valid_r;
  assign opcode_class = class_r;
  assign width8 = width8_r;
  assign has_modrm = has_modrm_r;
  assign immediate_size = immediate_size_r;
  assign group_operation = modrm[5:3];
  assign string_operation = string_operation_r;
  assign control_flow = control_flow_r;
  assign repeatable = repeatable_r;
endmodule
