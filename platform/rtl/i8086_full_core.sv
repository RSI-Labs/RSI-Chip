// Full real-mode 8086 core under active construction.
// The interface models a byte-addressable 1 MiB external memory. Reads are
// combinational; writes are observed by the memory on the rising clock.
module i8086_full_core(clk, rst, init_ip, init_sp, init_cs, init_ds, init_es, init_ss,
                       mem_rdata, mem_addr, mem_wdata, mem_we, mem_width,
                       source_rdata, source_addr,
                       io_rdata, io_addr, io_we, io_width,
                       console_write, console_char,
                       irq, nmi, interrupt_vector, interrupt_ack,
                       halted, trap, instructions_retired,
                       dbg_ax, dbg_cx, dbg_dx, dbg_bx, dbg_sp, dbg_bp, dbg_si, dbg_di,
                       dbg_es, dbg_cs, dbg_ss, dbg_ds, dbg_ip, dbg_flags);
  input clk;
  input rst;
  input [15:0] init_ip;
  input [15:0] init_sp;
  input [15:0] init_cs;
  input [15:0] init_ds;
  input [15:0] init_es;
  input [15:0] init_ss;
  input [15:0] mem_rdata;
  input [15:0] source_rdata;
  output [19:0] mem_addr;
  output [15:0] mem_wdata;
  output mem_we;
  output [2:0] mem_width;
  output [19:0] source_addr;
  input [15:0] io_rdata;
  output [15:0] io_addr;
  output io_we;
  output [2:0] io_width;
  output console_write;
  output [7:0] console_char;
  input irq;
  input nmi;
  input [7:0] interrupt_vector;
  output interrupt_ack;
  output halted;
  output trap;
  output [63:0] instructions_retired;
  output [15:0] dbg_ax;
  output [15:0] dbg_cx;
  output [15:0] dbg_dx;
  output [15:0] dbg_bx;
  output [15:0] dbg_sp;
  output [15:0] dbg_bp;
  output [15:0] dbg_si;
  output [15:0] dbg_di;
  output [15:0] dbg_es;
  output [15:0] dbg_cs;
  output [15:0] dbg_ss;
  output [15:0] dbg_ds;
  output [15:0] dbg_ip;
  output [15:0] dbg_flags;

  localparam S_FETCH = 5'd0;
  localparam S_MODRM = 5'd1;
  localparam S_DISP8 = 5'd2;
  localparam S_DISP16_LOW = 5'd3;
  localparam S_DISP16_HIGH = 5'd4;
  localparam S_IMM_LOW = 5'd5;
  localparam S_IMM_HIGH = 5'd6;
  localparam S_FAR_LOW = 5'd7;
  localparam S_FAR_HIGH = 5'd8;
  localparam S_EXECUTE = 5'd9;
  localparam S_HALTED = 5'd10;
  localparam S_INT_PUSH_FLAGS = 5'd11;
  localparam S_INT_PUSH_CS = 5'd12;
  localparam S_INT_PUSH_IP = 5'd13;
  localparam S_INT_READ_TARGET = 5'd14;
  localparam S_IRET_POP_IP = 5'd15;
  localparam S_IRET_POP_CS = 5'd16;
  localparam S_IRET_POP_FLAGS = 5'd17;
  localparam S_FAR_CALL_PUSH_CS = 5'd18;
  localparam S_FAR_CALL_PUSH_IP = 5'd19;
  localparam S_DOS_PRINT = 5'd20;
  localparam S_DIVIDE = 5'd21;
  localparam S_DIVIDE_INIT = 5'd22;

  localparam SZ_BYTE = 3'd0;
  localparam SZ_WORD = 3'd1;

  reg [4:0] state;
  reg [7:0] interrupt_vector_r;
  reg [7:0] interrupt_vector_input;
  reg [7:0] opcode_r;
  reg [7:0] modrm_r;
  reg [15:0] displacement_r;
  reg [15:0] immediate_r;
  reg [15:0] far_r;
  reg [15:0] far_call_segment;
  reg [15:0] far_jmp_segment;
  reg [15:0] far_call_return_ip;
  reg [15:0] next_far_call_return_ip;
  reg seg_override_present;
  reg [1:0] seg_override;
  reg rep_none;
  reg rep_while_equal;
  reg rep_while_not_equal;
  reg [15:0] ax, cx, dx, bx, sp, bp, si, di;
  reg [15:0] es, cs, ss, ds;
  reg [15:0] ip;
  reg [15:0] flags;
  reg halted_r;
  reg trap_r;
  reg [63:0] retired;

  wire [7:0] fetch_byte = mem_rdata[7:0];
  wire [7:0] al = ax[7:0];
  wire [7:0] ah = ax[15:8];
  wire [7:0] dl = dx[7:0];
  wire fetch_prefix;
  wire fetch_segment_override;
  wire fetch_segment_index;
  wire [1:0] fetch_segment_number;
  wire fetch_repeat;
  wire fetch_repeat_equal;
  wire fetch_lock;
  i8086_prefix_decoder prefix_decode(
    .first_byte(fetch_byte), .is_prefix(fetch_prefix),
    .is_segment_override(fetch_segment_override),
    .segment_index(fetch_segment_index), .segment_number(fetch_segment_number),
    .repeat_active(fetch_repeat), .repeat_while_zero(fetch_repeat_equal), .lock(fetch_lock)
  );

  wire fetch_opcode_valid;
  wire [5:0] fetch_opcode_class;
  wire fetch_opcode_width8;
  wire fetch_opcode_has_modrm;
  wire [1:0] fetch_opcode_imm_size;
  i8086_opcode_decoder fetch_opcode_decode(
    .opcode(fetch_byte), .modrm(modrm_r), .valid(fetch_opcode_valid),
    .opcode_class(fetch_opcode_class), .width8(fetch_opcode_width8),
    .has_modrm(fetch_opcode_has_modrm), .immediate_size(fetch_opcode_imm_size),
    .group_operation(), .string_operation(), .control_flow(), .repeatable()
  );

  wire execute_opcode_valid;
  wire [5:0] execute_opcode_class;
  wire execute_width8;
  wire execute_has_modrm;
  wire [1:0] execute_imm_size;
  wire [2:0] execute_group_operation;
  wire execute_string_operation;
  wire execute_control_flow;
  wire execute_repeatable;
  i8086_opcode_decoder execute_opcode_decode(
    .opcode(opcode_r), .modrm(modrm_r), .valid(execute_opcode_valid),
    .opcode_class(execute_opcode_class), .width8(execute_width8),
    .has_modrm(execute_has_modrm), .immediate_size(execute_imm_size),
    .group_operation(execute_group_operation),
    .string_operation(execute_string_operation),
    .control_flow(execute_control_flow), .repeatable(execute_repeatable)
  );

  wire modrm_fetch_needs_immediate =
    (opcode_r == 8'h80 || opcode_r == 8'h82 || opcode_r == 8'h83 || opcode_r == 8'hc6) ||
    (opcode_r == 8'hc0 || opcode_r == 8'hc1) ||
    (opcode_r == 8'hf6 && fetch_byte[5:3] <= 3'b001) ||
    (opcode_r == 8'hf7 && fetch_byte[5:3] <= 3'b001) ||
    opcode_r == 8'h81 || opcode_r == 8'hc7;
  wire modrm_memory;
  wire [1:0] modrm_segment;
  wire [15:0] modrm_offset;
  wire [2:0] modrm_register;
  wire [2:0] modrm_rm;
  wire divide_will_be_signed =
    opcode_r != 8'hd4 && modrm_register == 3'd7;
  i8086_modrm_decoder modrm_decode(
    .modrm(modrm_r), .displacement(displacement_r),
    .override_present(seg_override_present), .override_segment(seg_override),
    .bx(bx), .bp(bp), .si(si), .di(di),
    .memory_operand(modrm_memory), .default_segment(modrm_segment),
    .effective_offset(modrm_offset), .register_index(modrm_register),
    .register_memory_index(modrm_rm)
  );

  function [19:0] physical_address(input [1:0] segment, input [15:0] offset);
    case (segment)
      2'b00: physical_address = {es, 4'b0000} + {4'b0000, offset};
      2'b01: physical_address = {cs, 4'b0000} + {4'b0000, offset};
      2'b10: physical_address = {ss, 4'b0000} + {4'b0000, offset};
      default: physical_address = {ds, 4'b0000} + {4'b0000, offset};
    endcase
  endfunction

  function [15:0] segment_result_value(input [1:0] segment);
    case (segment)
      2'b00: segment_result_value = es;
      2'b01: segment_result_value = cs;
      2'b10: segment_result_value = ss;
      default: segment_result_value = ds;
    endcase
  endfunction

  function [15:0] register_word(input [2:0] index);
    case (index)
      3'd0: register_word = ax;
      3'd1: register_word = cx;
      3'd2: register_word = dx;
      3'd3: register_word = bx;
      3'd4: register_word = sp;
      3'd5: register_word = bp;
      3'd6: register_word = si;
      default: register_word = di;
    endcase
  endfunction

  function [7:0] register_byte(input [2:0] index);
    case (index)
      3'd0: register_byte = ax[7:0];
      3'd1: register_byte = cx[7:0];
      3'd2: register_byte = dx[7:0];
      3'd3: register_byte = bx[7:0];
      3'd4: register_byte = ax[15:8];
      3'd5: register_byte = cx[15:8];
      3'd6: register_byte = dx[15:8];
      default: register_byte = bx[15:8];
    endcase
  endfunction

  function logic parity_even(input [7:0] result_value);
    parity_even = ^result_value == 1'b0;
  endfunction

  // Architectural next-state result_values. They are distinct nets from current state.
  reg [4:0] next_state;
  reg [15:0] next_ax, next_cx, next_dx, next_bx, next_sp, next_bp, next_si, next_di;
  reg [15:0] next_es, next_cs, next_ss, next_ds;
  reg [15:0] next_ip;
  reg [15:0] next_flags;
  reg next_halted;
  reg next_trap;
  reg [63:0] next_retired;
  reg next_seg_override_present;
  reg [1:0] next_seg_override;
  reg next_rep_none;
  reg next_rep_while_equal;
  reg next_rep_while_not_equal;
  reg [19:0] execute_mem_addr;
  reg [15:0] execute_mem_wdata;
  reg execute_mem_we;
  reg [2:0] execute_mem_width;
  reg [19:0] execute_io_addr;
  reg [15:0] execute_io_wdata;
  reg execute_io_we;
  reg [2:0] execute_io_width;
  reg console_write_r;
  reg [7:0] console_char_r;
  reg [19:0] fetch_address;
  reg [15:0] operand_a;
  reg [15:0] operand_b;
  reg [15:0] operand_result;
  reg [15:0] incdec_result;
  reg [15:0] simple_flags;
  reg [15:0] test_byte_flags;
  reg [15:0] test_word_flags;
  reg [15:0] group_result;
  reg [15:0] rm_incdec_result;
  reg [2:0] alu_flag_operation;
  reg [15:0] alu_flag_a;
  reg [15:0] alu_flag_b;
  reg [15:0] alu_flag_result;
  reg alu_flag_byte;
  reg alu_flag_preserve_carry;
  reg [15:0] alu_flag_value;
  reg [15:0] test_rm_result;
  reg [4:0] shift_count;
  reg [4:0] rotate_count;
  reg [4:0] through_carry_count;
  reg shift_byte;
  reg [15:0] shift_value;
  reg [15:0] shift_result;
  reg [15:0] shift_flags;
  reg shift_carry;
  reg shift_overflow;
  reg [15:0] bcd_result;
  reg [15:0] bcd_flags;
  reg [15:0] unary_result;
  reg [15:0] unary_flags;
  reg [31:0] mul_product;
  wire [15:0] mul_flags;
  reg mul_overflow;
  reg [31:0] divide_dividend;
  reg [31:0] divide_divisor;
  reg [31:0] divide_remainder;
  reg [31:0] divide_quotient;
  reg [5:0] divide_bits_left;
  reg divide_byte;
  reg divide_signed;
  reg divide_dividend_negative;
  reg divide_divisor_negative;
  reg divide_start;
  reg divide_aam;
  reg divide_dividend_msb;
  reg unary_result_sign;
  reg [16:0] rcl_input;
  reg [16:0] rcl_rotated;
  reg [16:0] rcr_input;
  reg [16:0] rcr_rotated;
  reg shift_result_sign;
  reg shift_result_next_sign;
  reg shift_input_sign;
  reg [2:0] alu_operation;
  reg alu_byte;
  reg [15:0] rm_result_value;
  reg [15:0] source_register_result_value;
  reg condition_true;
  reg [15:0] string_delta;
  reg [1:0] string_source_segment;

  assign fetch_address = physical_address(2'b01, ip);
  assign source_addr =
    (state == S_INT_READ_TARGET)
      ? {{10{1'b0}}, interrupt_vector_r, 2'b00} + 20'd2
    :
    (opcode_r == 8'hc4 || opcode_r == 8'hc5)
      ? physical_address(modrm_segment, modrm_offset + 16'd2)
    : (opcode_r == 8'hca || opcode_r == 8'hcb)
      ? physical_address(2'b10, sp + 16'd2)
    : opcode_r == 8'h8f
      ? physical_address(2'b10, sp)
    : (opcode_r == 8'hff && (modrm_register == 3'd3 || modrm_register == 3'd5))
      ? physical_address(modrm_segment, modrm_offset + 16'd2)
      : physical_address(string_source_segment, si);
  assign rm_result_value = modrm_memory ? mem_rdata :
                    (execute_width8 ? {8'b0, register_byte(modrm_rm)} : register_word(modrm_rm));
  assign source_register_result_value = execute_width8 ? {8'b0, register_byte(modrm_register)} : register_word(modrm_register);

  always @* begin
    condition_true = 1'b0;
    case (opcode_r[3:0])
      4'h0: condition_true = flags[11];
      4'h1: condition_true = ~flags[11];
      4'h2: condition_true = flags[0];
      4'h3: condition_true = ~flags[0];
      4'h4: condition_true = flags[6];
      4'h5: condition_true = ~flags[6];
      4'h6: condition_true = flags[0] | flags[6];
      4'h7: condition_true = ~(flags[0] | flags[6]);
      4'h8: condition_true = flags[7];
      4'h9: condition_true = ~flags[7];
      4'ha: condition_true = flags[2];
      4'hb: condition_true = ~flags[2];
      4'hc: condition_true = flags[7] != flags[11];
      4'hd: condition_true = flags[7] == flags[11];
      4'he: condition_true = flags[6] | (flags[7] != flags[11]);
      default: condition_true = ~flags[6] & (flags[7] == flags[11]);
    endcase
  end

  always @* begin
    divide_dividend_msb = divide_aam ? divide_dividend[7] :
                          divide_byte ? divide_dividend[15] : divide_dividend[31];
  end

  always @* begin
    far_jmp_segment = far_r;
  end

  always @* begin
    if (opcode_r == 8'h9a)
      far_call_segment = far_r;
    else
      far_call_segment = source_rdata;
  end

  assign mul_flags = {
    flags[15:12],
    mul_overflow,
    flags[10:1],
    mul_overflow
  };

  always @* begin
    if (modrm_register == 3'd5 && !execute_width8)
      mul_product = ({16'b0, ax} * {16'b0, rm_result_value}) -
                    (ax[15] ? {rm_result_value, 16'b0} : 32'b0) -
                    (rm_result_value[15] ? {ax, 16'b0} : 32'b0);
    else if (modrm_register == 3'd5)
      mul_product = {{16{al[7]}}, al} * {{16{rm_result_value[7]}}, rm_result_value[7:0]};
    else if (execute_width8)
      mul_product = {24'b0, al} * {24'b0, rm_result_value[7:0]};
    else
      mul_product = {16'b0, ax} * {16'b0, rm_result_value};
  end

  always @* begin
    unary_result_sign = execute_width8 ? unary_result[7] : unary_result[15];
  end

  always @* begin
    if (modrm_register == 3'd0 || modrm_register == 3'd1)
      unary_result = rm_result_value & immediate_r;
    else if (modrm_register == 3'd2)
      unary_result = ~rm_result_value;
    else if (modrm_register == 3'd3)
      unary_result = 16'b0 - rm_result_value;
    else
      unary_result = rm_result_value;
  end

  always @* begin
    if (modrm_register == 3'd0 || modrm_register == 3'd1) begin
      unary_flags = {
        flags[15:12], 1'b0, flags[10:8], unary_result_sign,
        (unary_result & (execute_width8 ? 16'h00ff : 16'hffff)) == 0,
        flags[5:4], 1'b0, parity_even(unary_result), flags[1], 1'b0
      };
    end else if (modrm_register == 3'd2) begin
      unary_flags = flags;
    end else if (modrm_register == 3'd3) begin
      unary_flags = {
        flags[15:12],
        rm_result_value == (execute_width8 ? 16'h0080 : 16'h8000),
        flags[10:8], unary_result_sign,
        (unary_result & (execute_width8 ? 16'h00ff : 16'hffff)) == 0,
        flags[5:4],
        ((rm_result_value ^ unary_result) & 16'h0010) != 0,
        parity_even(unary_result), flags[1], rm_result_value != 16'b0
      };
    end else begin
      unary_flags = flags;
    end
  end

  always @* begin
    if (flags[10])
      string_delta = ~(execute_width8 ? 16'd1 : 16'd2) + 16'd1;
    else
      string_delta = execute_width8 ? 16'd1 : 16'd2;
    string_source_segment = seg_override_present ? seg_override : 2'b11;
  end

  always @* begin
    alu_operation = opcode_r[5:3];
    alu_byte = execute_width8;
    if (opcode_r == 8'ha6 || opcode_r == 8'ha7 || opcode_r == 8'hae || opcode_r == 8'haf) begin
      alu_operation = 3'd7;
      if (opcode_r == 8'ha6 || opcode_r == 8'ha7) begin
        operand_a = source_rdata;
        operand_b = mem_rdata;
      end else begin
        operand_a = ax;
        operand_b = mem_rdata;
      end
    end else case (opcode_r[2:0])
      3'd0: begin
        operand_a = rm_result_value;
        operand_b = source_register_result_value;
      end
      3'd1: begin
        // The low opcode bit selects width; direction is bit 1. Thus 01h,
        // like 00h, computes r/m <- register.
        operand_a = rm_result_value;
        operand_b = source_register_result_value;
      end
      3'd2: begin
        operand_a = source_register_result_value;
        operand_b = rm_result_value;
      end
      3'd3: begin
        // Likewise 03h, like 02h, computes register <- r/m.
        operand_a = source_register_result_value;
        operand_b = rm_result_value;
      end
      3'd4: begin
        operand_a = ax;
        operand_b = {8'b0, immediate_r[7:0]};
      end
      default: begin
        operand_a = ax;
        operand_b = immediate_r;
      end
    endcase
  end

  always @* begin
    if (alu_byte)
      case (alu_operation)
        3'd0: operand_result = {8'b0, operand_a[7:0] + operand_b[7:0]};
        3'd1: operand_result = {8'b0, operand_a[7:0] | operand_b[7:0]};
        3'd2: operand_result = {8'b0, operand_a[7:0] + operand_b[7:0] + {7'b0, flags[0]}};
        3'd3: operand_result = {8'b0, operand_a[7:0] - operand_b[7:0] - {7'b0, flags[0]}};
        3'd4: operand_result = {8'b0, operand_a[7:0] & operand_b[7:0]};
        3'd5: operand_result = {8'b0, operand_a[7:0] - operand_b[7:0]};
        3'd6: operand_result = {8'b0, operand_a[7:0] ^ operand_b[7:0]};
        default: operand_result = {8'b0, operand_a[7:0] - operand_b[7:0]};
      endcase
    else
      case (alu_operation)
        3'd0: operand_result = operand_a + operand_b;
        3'd1: operand_result = operand_a | operand_b;
        3'd2: operand_result = operand_a + operand_b + {15'b0, flags[0]};
        3'd3: operand_result = operand_a - operand_b - {15'b0, flags[0]};
        3'd4: operand_result = operand_a & operand_b;
        3'd5: operand_result = operand_a - operand_b;
        3'd6: operand_result = operand_a ^ operand_b;
        default: operand_result = operand_a - operand_b;
      endcase
  end

  always @* begin
    if (opcode_r[3])
      incdec_result = register_word(opcode_r[2:0]) - 16'd1;
    else
      incdec_result = register_word(opcode_r[2:0]) + 16'd1;
  end

  always @* begin
    case (opcode_r)
      8'hf5: simple_flags = {flags[15:1], !flags[0]};
      8'hf8: simple_flags = {flags[15:1], 1'b0};
      8'hf9: simple_flags = {flags[15:1], 1'b1};
      8'hfa: simple_flags = {flags[15:10], 1'b0, flags[8:0]};
      8'hfb: simple_flags = {flags[15:10], 1'b1, flags[8:0]};
      8'hfc: simple_flags = {flags[15:11], 1'b0, flags[9:0]};
      8'hfd: simple_flags = {flags[15:11], 1'b1, flags[9:0]};
      default: simple_flags = flags;
    endcase
  end

  always @* begin
    test_byte_flags = {
      flags[15:12], 1'b0, flags[10:8],
      (al & immediate_r[7:0]) & 8'h80,
      (al & immediate_r[7:0]) == 0,
      {flags[5], 1'b0},
      parity_even(al & immediate_r[7:0]),
      flags[1], 1'b0
    };
  end

  always @* begin
    test_word_flags = {
      flags[15:12], 1'b0, flags[10:8],
      (ax & immediate_r) & 16'h8000,
      (ax & immediate_r) == 0,
      {flags[5], 1'b0},
      parity_even(ax & immediate_r),
      flags[1], 1'b0
    };
  end

  always @* begin
    case (modrm_register)
      3'd1: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value | (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r)) :
                           {8'b0, rm_result_value[7:0] | immediate_r[7:0]};
      3'd2: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value + (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r) + {15'b0, flags[0]})
                           : {8'b0, rm_result_value[7:0] + immediate_r[7:0] + {7'b0, flags[0]}};
      3'd3: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value - (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r) - {15'b0, flags[0]}) :
                           {8'b0, rm_result_value[7:0] - immediate_r[7:0] - {7'b0, flags[0]}};
      3'd4: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value & (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r)) :
                           {8'b0, rm_result_value[7:0] & immediate_r[7:0]};
      3'd5: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value - (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r))
                           : {8'b0, rm_result_value[7:0] - immediate_r[7:0]};
      3'd6: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value ^ (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r)) :
                           {8'b0, rm_result_value[7:0] ^ immediate_r[7:0]};
      3'd7: group_result = (opcode_r == 8'h81 || opcode_r == 8'h83)
                           ? (rm_result_value - (opcode_r == 8'h83
                              ? {{8{immediate_r[7]}}, immediate_r[7:0]}
                              : immediate_r)) :
                           {8'b0, rm_result_value[7:0] - immediate_r[7:0]};
      default: group_result = opcode_r == 8'h81 ? (rm_result_value + immediate_r) :
                              opcode_r == 8'h83
                              ? (rm_result_value + {{8{immediate_r[7]}}, immediate_r[7:0]})
                              : {8'b0, rm_result_value[7:0] + immediate_r[7:0]};
    endcase
  end

  always @* begin
    test_rm_result = rm_result_value & source_register_result_value;
  end

  always @* begin
    if (modrm_register == 3'd0)
      rm_incdec_result = rm_result_value + 16'd1;
    else
      rm_incdec_result = rm_result_value - 16'd1;
  end

  always @* begin
    alu_flag_operation = alu_operation;
    alu_flag_a = operand_a;
    alu_flag_b = operand_b;
    alu_flag_result = operand_result;
    alu_flag_byte = alu_byte;
    alu_flag_preserve_carry = 1'b0;
    if (opcode_r == 8'h80 || opcode_r == 8'h81 || opcode_r == 8'h82 || opcode_r == 8'h83) begin
      alu_flag_operation = modrm_register;
      alu_flag_a = rm_result_value;
      alu_flag_b = opcode_r == 8'h81 ? immediate_r :
                   opcode_r == 8'h83 ? {{8{immediate_r[7]}}, immediate_r[7:0]} :
                                       {8'b0, immediate_r[7:0]};
      alu_flag_result = group_result;
      alu_flag_byte = execute_width8;
    end else if (opcode_r >= 8'h40 && opcode_r <= 8'h4f) begin
      alu_flag_operation = opcode_r[3] ? 3'd3 : 3'd0;
      alu_flag_a = register_word(opcode_r[2:0]);
      alu_flag_b = 16'd1;
      alu_flag_result = incdec_result;
      alu_flag_byte = 1'b0;
      alu_flag_preserve_carry = 1'b1;
    end else if (opcode_r == 8'h84 || opcode_r == 8'h85) begin
      alu_flag_operation = 3'd4;
      alu_flag_a = rm_result_value;
      alu_flag_b = source_register_result_value;
      alu_flag_result = test_rm_result;
      alu_flag_byte = execute_width8;
    end else if (opcode_r == 8'hfe || opcode_r == 8'hff) begin
      alu_flag_operation = modrm_register == 3'd0 ? 3'd0 : 3'd3;
      alu_flag_a = rm_result_value;
      alu_flag_b = 16'd1;
      alu_flag_result = rm_incdec_result;
      alu_flag_byte = opcode_r == 8'hfe;
      alu_flag_preserve_carry = 1'b1;
    end
  end

  always @* begin
    alu_flag_value = {
      flags[15:12],
      alu_flag_operation == 3'd0 ? (~(alu_flag_a ^ alu_flag_b) & (alu_flag_a ^ alu_flag_result) & (alu_flag_byte ? 16'h0080 : 16'h8000)) != 0 :
      alu_flag_operation == 3'd2 ? (~(alu_flag_a ^ alu_flag_b) & (alu_flag_a ^ alu_flag_result) & (alu_flag_byte ? 16'h0080 : 16'h8000)) != 0 :
      (alu_flag_operation == 3'd3 || alu_flag_operation == 3'd5 || alu_flag_operation == 3'd7) ? ((alu_flag_a ^ alu_flag_b) & (alu_flag_a ^ alu_flag_result) & (alu_flag_byte ? 16'h0080 : 16'h8000)) != 0 : 1'b0,
      flags[10:8],
      (alu_flag_result & (alu_flag_byte ? 16'h0080 : 16'h8000)) != 0,
      (alu_flag_result & (alu_flag_byte ? 16'h00ff : 16'hffff)) == 0,
      flags[5:4],
      (alu_flag_operation == 3'd1 || alu_flag_operation == 3'd4 ||
       alu_flag_operation == 3'd6)
        ? 1'b0
        : ((alu_flag_a ^ alu_flag_b ^ alu_flag_result) & 16'h0010) != 0,
      parity_even(alu_flag_result),
      flags[1],
      alu_flag_preserve_carry ? flags[0] :
      alu_flag_operation == 3'd0
        ? ({1'b0, alu_flag_a} + {1'b0, alu_flag_b}) >
          (alu_flag_byte ? 17'h0000_00ff : 17'h0001_0000) :
      alu_flag_operation == 3'd2
        ? ({1'b0, alu_flag_a} + {1'b0, alu_flag_b} + {16'b0, flags[0]}) >
          (alu_flag_byte ? 17'h0000_00ff : 17'h0001_0000) :
      alu_flag_operation == 3'd3
        ? (alu_flag_a < alu_flag_b ||
           (alu_flag_a == alu_flag_b && flags[0])) :
      (alu_flag_operation == 3'd5 || alu_flag_operation == 3'd7) ? alu_flag_a < alu_flag_b : 1'b0
    };
  end

  always @* begin
    shift_count = (opcode_r == 8'hd2 || opcode_r == 8'hd3) ? cx[4:0] :
                  (opcode_r == 8'hc0 || opcode_r == 8'hc1) ? immediate_r[4:0] : 5'd1;
    shift_byte = opcode_r[0] == 1'b0;
    shift_value = rm_result_value;
  end

  always @* begin
    if (shift_byte)
      rotate_count = shift_count & 5'd7;
    else
      rotate_count = shift_count & 5'd15;
    if (shift_byte)
      case (shift_count)
        5'd0: through_carry_count = 5'd0;
        5'd1: through_carry_count = 5'd1;
        5'd2: through_carry_count = 5'd2;
        5'd3: through_carry_count = 5'd3;
        5'd4: through_carry_count = 5'd4;
        5'd5: through_carry_count = 5'd5;
        5'd6: through_carry_count = 5'd6;
        5'd7: through_carry_count = 5'd7;
        5'd8: through_carry_count = 5'd8;
        5'd9: through_carry_count = 5'd0;
        5'd10: through_carry_count = 5'd1;
        5'd11: through_carry_count = 5'd2;
        5'd12: through_carry_count = 5'd3;
        5'd13: through_carry_count = 5'd4;
        5'd14: through_carry_count = 5'd5;
        5'd15: through_carry_count = 5'd6;
        5'd16: through_carry_count = 5'd7;
        5'd17: through_carry_count = 5'd8;
        5'd18: through_carry_count = 5'd0;
        5'd19: through_carry_count = 5'd1;
        5'd20: through_carry_count = 5'd2;
        5'd21: through_carry_count = 5'd3;
        5'd22: through_carry_count = 5'd4;
        5'd23: through_carry_count = 5'd5;
        5'd24: through_carry_count = 5'd6;
        5'd25: through_carry_count = 5'd7;
        5'd26: through_carry_count = 5'd8;
        5'd27: through_carry_count = 5'd0;
        5'd28: through_carry_count = 5'd1;
        5'd29: through_carry_count = 5'd2;
        5'd30: through_carry_count = 5'd3;
        5'd31: through_carry_count = 5'd4;
        default: through_carry_count = 5'd0;
      endcase
    else
      case (shift_count)
        5'd0: through_carry_count = 5'd0;
        5'd1: through_carry_count = 5'd1;
        5'd2: through_carry_count = 5'd2;
        5'd3: through_carry_count = 5'd3;
        5'd4: through_carry_count = 5'd4;
        5'd5: through_carry_count = 5'd5;
        5'd6: through_carry_count = 5'd6;
        5'd7: through_carry_count = 5'd7;
        5'd8: through_carry_count = 5'd8;
        5'd9: through_carry_count = 5'd9;
        5'd10: through_carry_count = 5'd10;
        5'd11: through_carry_count = 5'd11;
        5'd12: through_carry_count = 5'd12;
        5'd13: through_carry_count = 5'd13;
        5'd14: through_carry_count = 5'd14;
        5'd15: through_carry_count = 5'd15;
        5'd16: through_carry_count = 5'd16;
        5'd17: through_carry_count = 5'd0;
        5'd18: through_carry_count = 5'd1;
        5'd19: through_carry_count = 5'd2;
        5'd20: through_carry_count = 5'd3;
        5'd21: through_carry_count = 5'd4;
        5'd22: through_carry_count = 5'd5;
        5'd23: through_carry_count = 5'd6;
        5'd24: through_carry_count = 5'd7;
        5'd25: through_carry_count = 5'd8;
        5'd26: through_carry_count = 5'd9;
        5'd27: through_carry_count = 5'd10;
        5'd28: through_carry_count = 5'd11;
        5'd29: through_carry_count = 5'd12;
        5'd30: through_carry_count = 5'd13;
        5'd31: through_carry_count = 5'd14;
        default: through_carry_count = 5'd0;
      endcase
  end
  always @* begin
    rcl_input = shift_byte
      ? {8'b0, flags[0], shift_value[7:0]}
      : {flags[0], shift_value};
    rcr_input = shift_byte
      ? {8'b0, flags[0], shift_value[7:0]}
      : {flags[0], shift_value};
  end

  always @* begin
    if (shift_byte) begin
      rcl_rotated = {8'b0,
        ((rcl_input[8:0] << through_carry_count) |
         (rcl_input[8:0] >> (9 - through_carry_count)))};
      rcr_rotated = {8'b0,
        ((rcr_input[8:0] >> through_carry_count) |
         (rcr_input[8:0] << (9 - through_carry_count)))};
    end else begin
      rcl_rotated = (rcl_input << through_carry_count) |
                    (rcl_input >> (17 - through_carry_count));
      rcr_rotated = (rcr_input >> through_carry_count) |
                    (rcr_input << (17 - through_carry_count));
    end
  end

  always @* begin
    if (shift_count == 5'd0)
      shift_result = shift_value;
    else if (modrm_register == 3'd0)
      shift_result = shift_byte
        ? {8'b0, ((shift_value[7:0] << rotate_count) |
                  (shift_value[7:0] >> (8 - rotate_count)))}
        : ((shift_value << rotate_count) | (shift_value >> (16 - rotate_count)));
    else if (modrm_register == 3'd1)
      shift_result = shift_byte
        ? {8'b0, ((shift_value[7:0] >> rotate_count) |
                  (shift_value[7:0] << (8 - rotate_count)))}
        : ((shift_value >> rotate_count) | (shift_value << (16 - rotate_count)));
    else if (modrm_register == 3'd2)
      shift_result = shift_byte ? {8'b0, rcl_rotated[7:0]} : rcl_rotated[15:0];
    else if (modrm_register == 3'd3)
      shift_result = shift_byte ? {8'b0, rcr_rotated[7:0]} : rcr_rotated[15:0];
    else if (modrm_register == 3'd4 || modrm_register == 3'd6)
      shift_result = shift_byte
        ? {8'b0, shift_value[7:0] << shift_count}
        : shift_value << shift_count;
    else if (modrm_register == 3'd5)
      shift_result = shift_value >> shift_count;
    else if (shift_byte) begin
      if (shift_count >= 5'd8)
        shift_result = {8'b0, {8{shift_value[7]}}};
      else if (shift_value[7])
        shift_result = {8'b0, (shift_value[7:0] >> shift_count) |
                            (8'hff << (8 - shift_count))};
      else
        shift_result = {8'b0, shift_value[7:0] >> shift_count};
    end else if (shift_count >= 5'd16)
      shift_result = {16{shift_value[15]}};
    else if (shift_value[15])
      shift_result = (shift_value >> shift_count) |
                      (16'hffff << (16 - shift_count));
    else
      shift_result = shift_value >> shift_count;
  end

  always @* begin
    shift_result_sign = shift_byte ? shift_result[7] : shift_result[15];
    shift_result_next_sign = shift_byte ? shift_result[6] : shift_result[14];
    shift_input_sign = shift_byte ? shift_value[7] : shift_value[15];
  end

  always @* begin
    if (shift_count != 5'd1)
      shift_overflow = flags[11];
    else if (modrm_register == 3'd0)
      shift_overflow = shift_result[0] != shift_result_sign;
    else if (modrm_register == 3'd1)
      shift_overflow = shift_result_sign != shift_result_next_sign;
    else if (modrm_register == 3'd2)
      shift_overflow = (shift_byte ? rcl_rotated[8] : rcl_rotated[16]) !=
                       shift_result_sign;
    else if (modrm_register == 3'd3)
      shift_overflow = shift_result_sign != shift_result_next_sign;
    else if (modrm_register == 3'd4 || modrm_register == 3'd6)
      shift_overflow = ((shift_value >> (shift_byte ? 8 - shift_count : 16 - shift_count)) & 16'b1) !=
                       shift_result_sign;
    else if (modrm_register == 3'd5)
      shift_overflow = shift_input_sign;
    else
      shift_overflow = 1'b0;
  end

  always @* begin
    if (modrm_register == 3'd0)
      shift_carry = shift_result[0];
    else if (modrm_register == 3'd1)
      shift_carry = shift_result_sign;
    else if (modrm_register == 3'd2)
      shift_carry = shift_byte ? rcl_rotated[8] : rcl_rotated[16];
    else if (modrm_register == 3'd3)
      shift_carry = shift_byte ? rcr_rotated[8] : rcr_rotated[16];
    else if (modrm_register == 3'd4 || modrm_register == 3'd6)
      shift_carry = (shift_value >>
        (shift_byte ? 8 - shift_count : 16 - shift_count)) & 16'b1;
    else if (modrm_register == 3'd5)
      shift_carry = (shift_value >> (shift_count - 5'd1)) & 16'b1;
    else
      shift_carry = (shift_count > (shift_byte ? 5'd8 : 5'd16)) ?
                    shift_input_sign :
                    ((shift_value >> (shift_count - 5'd1)) & 16'b1);
  end

  always @* begin
    shift_flags = flags;
    if (shift_count != 5'd0) begin
      shift_flags[11] = shift_overflow;
      if (modrm_register >= 3'd4) begin
        shift_flags[7] = shift_result_sign;
        shift_flags[6] = (shift_result &
                          (shift_byte ? 16'h00ff : 16'hffff)) == 0;
        shift_flags[2] = parity_even(shift_result);
      end
      shift_flags[0] = shift_carry;
    end
  end

  always @* begin
    if (opcode_r == 8'h27) begin
      if ((al & 8'h0f) > 8'h09 || flags[3]) begin
        if (al > 8'h99 || flags[0])
          bcd_result = {ah + 8'h01, al + 8'h66};
        else
          bcd_result = {ah, al + 8'h06};
      end else if (al > 8'h99 || flags[0])
        bcd_result = {ah + 8'h01, al + 8'h60};
      else
        bcd_result = {ah, al};
    end else if (opcode_r == 8'h2f) begin
      if ((al & 8'h0f) > 8'h09 || flags[3]) begin
        if (al > 8'h99 || flags[0])
          bcd_result = {ah - 8'h01, al - 8'h66};
        else
          bcd_result = {ah, al - 8'h06};
      end else if (al > 8'h99 || flags[0])
        bcd_result = {ah - 8'h01, al - 8'h60};
      else
        bcd_result = {ah, al};
    end else if (opcode_r == 8'h37) begin
      if ((al & 8'h0f) > 8'h09 || flags[3])
        bcd_result = {(ax + 16'h0106) >> 8, (ax + 16'h0106) & 16'h000f};
      else
        bcd_result = {ah, 4'b0000};
    end else if (opcode_r == 8'h3f) begin
      if ((al & 8'h0f) > 8'h09 || flags[3])
        bcd_result = {(ax - 16'h0106) >> 8, (ax - 16'h0106) & 16'h000f};
      else
        bcd_result = {ah, 4'b0000};
    end else
      bcd_result = ax;
  end

  always @* begin
    if (opcode_r == 8'h27 || opcode_r == 8'h2f) begin
      bcd_flags = {
        flags[15:12],
        flags[11],
        flags[10:8],
        bcd_result[7],
        bcd_result[7:0] == 8'h00,
        flags[5:4],
        (al & 8'h0f) > 8'h09 || flags[3],
        parity_even(bcd_result[7:0]),
        flags[2:1],
        al > 8'h99 || flags[0]
      };
    end else if (opcode_r == 8'h37 || opcode_r == 8'h3f) begin
      bcd_flags = flags;
      bcd_flags[3] = (al & 8'h0f) > 8'h09 || flags[3];
      bcd_flags[0] = (al & 8'h0f) > 8'h09 || flags[3];
    end else
      bcd_flags = flags;
  end

  always @* begin
    next_state = state;
    next_ax = ax;
    next_cx = cx;
    next_dx = dx;
    next_bx = bx;
    next_sp = sp;
    next_bp = bp;
    next_si = si;
    next_di = di;
    next_es = es;
    next_cs = cs;
    next_ss = ss;
    next_ds = ds;
    next_ip = ip;
    next_flags = flags;
    next_halted = halted_r;
    next_trap = trap_r;
    next_retired = retired;
    next_seg_override_present = seg_override_present;
    next_seg_override = seg_override;
    next_rep_none = rep_none;
    next_rep_while_equal = rep_while_equal;
    next_rep_while_not_equal = rep_while_not_equal;
    interrupt_vector_input = 8'b0;
    next_far_call_return_ip = far_call_return_ip;
    execute_mem_addr = fetch_address;
    execute_mem_wdata = 16'b0;
    execute_mem_we = 1'b0;
    execute_mem_width = SZ_BYTE;
    execute_io_addr = {4'b0000, dx};
    execute_io_wdata = 16'b0;
    execute_io_we = 1'b0;
    execute_io_width = SZ_WORD;
    console_write_r = 1'b0;
    console_char_r = 8'b0;
    divide_start = 1'b0;
    mul_overflow = execute_width8 ? mul_product > 32'h0000_00ff :
                                  mul_product > 32'h0000_ffff;

    if (state == S_EXECUTE) begin
      // Defaults for a completed instruction.
      next_state = S_FETCH;
      next_seg_override_present = 1'b0;
      next_rep_none = 1'b0;
      next_rep_while_equal = 1'b0;
      next_rep_while_not_equal = 1'b0;
      next_retired = retired + 64'd1;
      execute_mem_addr = physical_address(modrm_segment, modrm_offset);

      if ((opcode_r < 8'h80 || opcode_r > 8'h83)
          && (opcode_r <= 8'h3d)
          && (opcode_r[2:0] <= 3'b101))
      begin
        next_flags = alu_flag_value;
        if (alu_operation != 3'd7) begin
          if (opcode_r[2:0] == 3'd0 || opcode_r[2:0] == 3'd1) begin
            if (modrm_memory) begin
              execute_mem_we = 1'b1;
              execute_mem_wdata = operand_result;
              execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
            end else if (execute_width8) begin
              case (modrm_rm)
                3'd0: next_ax = {ax[15:8], operand_result[7:0]};
                3'd1: next_cx = {cx[15:8], operand_result[7:0]};
                3'd2: next_dx = {dx[15:8], operand_result[7:0]};
                3'd3: next_bx = {bx[15:8], operand_result[7:0]};
                3'd4: next_ax = {operand_result[7:0], ax[7:0]};
                3'd5: next_cx = {operand_result[7:0], cx[7:0]};
                3'd6: next_dx = {operand_result[7:0], dx[7:0]};
                default: next_bx = {operand_result[7:0], bx[7:0]};
              endcase
            end else begin
              case (modrm_rm)
                3'd0: next_ax = operand_result;
                3'd1: next_cx = operand_result;
                3'd2: next_dx = operand_result;
                3'd3: next_bx = operand_result;
                3'd4: next_sp = operand_result;
                3'd5: next_bp = operand_result;
                3'd6: next_si = operand_result;
                default: next_di = operand_result;
              endcase
            end
          end else if (opcode_r[2:0] == 3'd2 || opcode_r[2:0] == 3'd3) begin
            if (opcode_r == 8'hd4 || execute_width8) begin
              case (modrm_register)
                3'd0: next_ax = {ax[15:8], operand_result[7:0]};
                3'd1: next_cx = {cx[15:8], operand_result[7:0]};
                3'd2: next_dx = {dx[15:8], operand_result[7:0]};
                3'd3: next_bx = {bx[15:8], operand_result[7:0]};
                3'd4: next_ax = {operand_result[7:0], ax[7:0]};
                3'd5: next_cx = {operand_result[7:0], cx[7:0]};
                3'd6: next_dx = {operand_result[7:0], dx[7:0]};
                default: next_bx = {operand_result[7:0], bx[7:0]};
              endcase
            end else begin
              case (modrm_register)
                3'd0: next_ax = operand_result;
                3'd1: next_cx = operand_result;
                3'd2: next_dx = operand_result;
                3'd3: next_bx = operand_result;
                3'd4: next_sp = operand_result;
                3'd5: next_bp = operand_result;
                3'd6: next_si = operand_result;
                default: next_di = operand_result;
              endcase
            end
          end else begin
            if (execute_width8)
              next_ax = {ax[15:8], operand_result[7:0]};
            else
              next_ax = operand_result;
          end
        end
      end else begin
        case (opcode_r)
          8'h06, 8'h0e, 8'h16, 8'h1e: begin
            next_sp = sp - 16'd2;
            execute_mem_addr = physical_address(2'b10, sp - 16'd2);
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_WORD;
            case (opcode_r)
              8'h06: execute_mem_wdata = es;
              8'h0e: execute_mem_wdata = cs;
              8'h16: execute_mem_wdata = ss;
              default: execute_mem_wdata = ds;
            endcase
          end
          8'h07: begin next_es = mem_rdata; next_sp = sp + 16'd2; end
          8'h17: begin next_ss = mem_rdata; next_sp = sp + 16'd2; end
          8'h1f: begin next_ds = mem_rdata; next_sp = sp + 16'd2; end
          8'h27, 8'h2f, 8'h37, 8'h3f: begin
            next_ax = bcd_result;
            next_flags = bcd_flags;
          end
          8'h80, 8'h81, 8'h82, 8'h83: begin
            next_flags = alu_flag_value;
            if (modrm_register != 3'd7) begin
              if (modrm_memory) begin
                execute_mem_we = 1'b1;
                execute_mem_width =
                  (opcode_r == 8'h81 || opcode_r == 8'h83) ? SZ_WORD : SZ_BYTE;
                execute_mem_wdata = group_result;
              end else if (opcode_r == 8'h81 || opcode_r == 8'h83) begin
                case (modrm_rm)
                  3'd0: next_ax = group_result;
                  3'd1: next_cx = group_result;
                  3'd2: next_dx = group_result;
                  3'd3: next_bx = group_result;
                  3'd4: next_sp = group_result;
                  3'd5: next_bp = group_result;
                  3'd6: next_si = group_result;
                  default: next_di = group_result;
                endcase
              end else begin
                case (modrm_rm)
                  3'd0: next_ax = {ax[15:8], group_result[7:0]};
                  3'd1: next_cx = {cx[15:8], group_result[7:0]};
                  3'd2: next_dx = {dx[15:8], group_result[7:0]};
                  3'd3: next_bx = {bx[15:8], group_result[7:0]};
                  3'd4: next_ax = {group_result[7:0], ax[7:0]};
                  3'd5: next_cx = {group_result[7:0], cx[7:0]};
                  3'd6: next_dx = {group_result[7:0], dx[7:0]};
                  default: next_bx = {group_result[7:0], bx[7:0]};
                endcase
              end
            end
          end
          8'h40, 8'h41, 8'h42, 8'h43, 8'h44, 8'h45, 8'h46, 8'h47: begin
            next_flags = alu_flag_value;
            case (opcode_r[2:0])
              3'd0: next_ax = incdec_result;
              3'd1: next_cx = incdec_result;
              3'd2: next_dx = incdec_result;
              3'd3: next_bx = incdec_result;
              3'd4: next_sp = incdec_result;
              3'd5: next_bp = incdec_result;
              3'd6: next_si = incdec_result;
              default: next_di = incdec_result;
            endcase
          end
          8'h48, 8'h49, 8'h4a, 8'h4b, 8'h4c, 8'h4d, 8'h4e, 8'h4f: begin
            next_flags = alu_flag_value;
            case (opcode_r[2:0])
              3'd0: next_ax = incdec_result;
              3'd1: next_cx = incdec_result;
              3'd2: next_dx = incdec_result;
              3'd3: next_bx = incdec_result;
              3'd4: next_sp = incdec_result;
              3'd5: next_bp = incdec_result;
              3'd6: next_si = incdec_result;
              default: next_di = incdec_result;
            endcase
          end
          8'h50, 8'h51, 8'h52, 8'h53, 8'h54, 8'h55, 8'h56, 8'h57: begin
            next_sp = sp - 16'd2;
            execute_mem_addr = physical_address(2'b10, sp - 16'd2);
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_WORD;
            case (opcode_r[2:0])
              3'd0: execute_mem_wdata = ax;
              3'd1: execute_mem_wdata = cx;
              3'd2: execute_mem_wdata = dx;
              3'd3: execute_mem_wdata = bx;
              3'd4: execute_mem_wdata = sp;
              3'd5: execute_mem_wdata = bp;
              3'd6: execute_mem_wdata = si;
              default: execute_mem_wdata = di;
            endcase
          end
          8'h58, 8'h59, 8'h5a, 8'h5b, 8'h5c, 8'h5d, 8'h5e, 8'h5f: begin
            case (opcode_r[2:0])
              3'd0: next_ax = mem_rdata;
              3'd1: next_cx = mem_rdata;
              3'd2: next_dx = mem_rdata;
              3'd3: next_bx = mem_rdata;
              3'd4: next_sp = mem_rdata;
              3'd5: next_bp = mem_rdata;
              3'd6: next_si = mem_rdata;
              default: next_di = mem_rdata;
            endcase
            next_sp = sp + 16'd2;
            execute_mem_addr = physical_address(2'b10, sp);
          end
          8'h70, 8'h71, 8'h72, 8'h73, 8'h74, 8'h75, 8'h76, 8'h77,
          8'h78, 8'h79, 8'h7a, 8'h7b, 8'h7c, 8'h7d, 8'h7e, 8'h7f: begin
            if (condition_true)
              next_ip = ip + {{8{immediate_r[7]}}, immediate_r[7:0]};
          end
          8'h88: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = SZ_BYTE;
            execute_mem_wdata = {8'b0, register_byte(modrm_register)};
            if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = {ax[15:8], register_byte(modrm_register)};
                3'd1: next_cx = {cx[15:8], register_byte(modrm_register)};
                3'd2: next_dx = {dx[15:8], register_byte(modrm_register)};
                3'd3: next_bx = {bx[15:8], register_byte(modrm_register)};
                3'd4: next_ax = {register_byte(modrm_register), ax[7:0]};
                3'd5: next_cx = {register_byte(modrm_register), cx[7:0]};
                3'd6: next_dx = {register_byte(modrm_register), dx[7:0]};
                default: next_bx = {register_byte(modrm_register), bx[7:0]};
              endcase
            end
          end
          8'h89: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = register_word(modrm_register);
            if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = register_word(modrm_register);
                3'd1: next_cx = register_word(modrm_register);
                3'd2: next_dx = register_word(modrm_register);
                3'd3: next_bx = register_word(modrm_register);
                3'd4: next_sp = register_word(modrm_register);
                3'd5: next_bp = register_word(modrm_register);
                3'd6: next_si = register_word(modrm_register);
                default: next_di = register_word(modrm_register);
              endcase
            end
          end
          8'h8a: begin
            if (execute_width8) begin
              case (modrm_register)
                3'd0: next_ax = {ax[15:8], rm_result_value[7:0]};
                3'd1: next_cx = {cx[15:8], rm_result_value[7:0]};
                3'd2: next_dx = {dx[15:8], rm_result_value[7:0]};
                3'd3: next_bx = {bx[15:8], rm_result_value[7:0]};
                3'd4: next_ax = {rm_result_value[7:0], ax[7:0]};
                3'd5: next_cx = {rm_result_value[7:0], cx[7:0]};
                3'd6: next_dx = {rm_result_value[7:0], dx[7:0]};
                default: next_bx = {rm_result_value[7:0], bx[7:0]};
              endcase
            end else begin
              case (modrm_register)
                3'd0: next_ax = rm_result_value;
                3'd1: next_cx = rm_result_value;
                3'd2: next_dx = rm_result_value;
                3'd3: next_bx = rm_result_value;
                3'd4: next_sp = rm_result_value;
                3'd5: next_bp = rm_result_value;
                3'd6: next_si = rm_result_value;
                default: next_di = rm_result_value;
              endcase
            end
          end
          8'h8b: begin
            case (modrm_register)
              3'd0: next_ax = rm_result_value;
              3'd1: next_cx = rm_result_value;
              3'd2: next_dx = rm_result_value;
              3'd3: next_bx = rm_result_value;
              3'd4: next_sp = rm_result_value;
              3'd5: next_bp = rm_result_value;
              3'd6: next_si = rm_result_value;
              default: next_di = rm_result_value;
            endcase
          end
          8'h84, 8'h85:
            next_flags = alu_flag_value;
          8'h86, 8'h87: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
            execute_mem_wdata = source_register_result_value;
            if (execute_width8) begin
              case (modrm_register)
                3'd0: next_ax = {ax[15:8], rm_result_value[7:0]};
                3'd1: next_cx = {cx[15:8], rm_result_value[7:0]};
                3'd2: next_dx = {dx[15:8], rm_result_value[7:0]};
                3'd3: next_bx = {bx[15:8], rm_result_value[7:0]};
                3'd4: next_ax = {rm_result_value[7:0], ax[7:0]};
                3'd5: next_cx = {rm_result_value[7:0], cx[7:0]};
                3'd6: next_dx = {rm_result_value[7:0], dx[7:0]};
                default: next_bx = {rm_result_value[7:0], bx[7:0]};
              endcase
            end else begin
              case (modrm_register)
                3'd0: next_ax = rm_result_value;
                3'd1: next_cx = rm_result_value;
                3'd2: next_dx = rm_result_value;
                3'd3: next_bx = rm_result_value;
                3'd4: next_sp = rm_result_value;
                3'd5: next_bp = rm_result_value;
                3'd6: next_si = rm_result_value;
                default: next_di = rm_result_value;
              endcase
            end
          end
          8'h8c: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = segment_result_value(modrm_register[1:0]);
            if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = segment_result_value(modrm_register[1:0]);
                3'd1: next_cx = segment_result_value(modrm_register[1:0]);
                3'd2: next_dx = segment_result_value(modrm_register[1:0]);
                3'd3: next_bx = segment_result_value(modrm_register[1:0]);
                3'd4: next_sp = segment_result_value(modrm_register[1:0]);
                3'd5: next_bp = segment_result_value(modrm_register[1:0]);
                3'd6: next_si = segment_result_value(modrm_register[1:0]);
                default: next_di = segment_result_value(modrm_register[1:0]);
              endcase
            end
          end
          8'h8d: begin
            case (modrm_register)
              3'd0: next_ax = modrm_offset;
              3'd1: next_cx = modrm_offset;
              3'd2: next_dx = modrm_offset;
              3'd3: next_bx = modrm_offset;
              3'd4: next_sp = modrm_offset;
              3'd5: next_bp = modrm_offset;
              3'd6: next_si = modrm_offset;
              default: next_di = modrm_offset;
            endcase
          end
          8'h8e: begin
            if (modrm_register[1:0] == 2'b00)
              next_es = rm_result_value;
            else if (modrm_register[1:0] == 2'b01)
              next_cs = rm_result_value;
            else if (modrm_register[1:0] == 2'b10)
              next_ss = rm_result_value;
            else
              next_ds = rm_result_value;
          end
          8'h8f: begin
            next_sp = sp + 16'd2;
            execute_mem_we = modrm_memory;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = source_rdata;
            if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = source_rdata;
                3'd1: next_cx = source_rdata;
                3'd2: next_dx = source_rdata;
                3'd3: next_bx = source_rdata;
                3'd4: next_sp = source_rdata;
                3'd5: next_bp = source_rdata;
                3'd6: next_si = source_rdata;
                default: next_di = source_rdata;
              endcase
            end
          end
          8'hb0, 8'hb1, 8'hb2, 8'hb3, 8'hb4, 8'hb5, 8'hb6, 8'hb7: begin
            case (opcode_r[2:0])
              3'd0: next_ax = {ax[15:8], immediate_r[7:0]};
              3'd1: next_cx = {cx[15:8], immediate_r[7:0]};
              3'd2: next_dx = {dx[15:8], immediate_r[7:0]};
              3'd3: next_bx = {bx[15:8], immediate_r[7:0]};
              3'd4: next_ax = {immediate_r[7:0], ax[7:0]};
              3'd5: next_cx = {immediate_r[7:0], cx[7:0]};
              3'd6: next_dx = {immediate_r[7:0], dx[7:0]};
              default: next_bx = {immediate_r[7:0], bx[7:0]};
            endcase
          end
          8'hb8, 8'hb9, 8'hba, 8'hbb, 8'hbc, 8'hbd, 8'hbe, 8'hbf: begin
            case (opcode_r[2:0])
              3'd0: next_ax = immediate_r;
              3'd1: next_cx = immediate_r;
              3'd2: next_dx = immediate_r;
              3'd3: next_bx = immediate_r;
              3'd4: next_sp = immediate_r;
              3'd5: next_bp = immediate_r;
              3'd6: next_si = immediate_r;
              default: next_di = immediate_r;
            endcase
          end
          8'hc2: begin
            execute_mem_addr = physical_address(2'b10, sp);
            next_ip = mem_rdata;
            next_sp = sp + 16'd2 + immediate_r;
          end
          8'hc3: begin
            execute_mem_addr = physical_address(2'b10, sp);
            next_ip = mem_rdata;
            next_sp = sp + 16'd2;
          end
          8'hc4, 8'hc5: begin
            execute_mem_addr = physical_address(modrm_segment, modrm_offset);
            if (opcode_r == 8'hc4)
              next_es = source_rdata;
            else
              next_ds = source_rdata;
            case (modrm_register)
              3'd0: next_ax = mem_rdata;
              3'd1: next_cx = mem_rdata;
              3'd2: next_dx = mem_rdata;
              3'd3: next_bx = mem_rdata;
              3'd4: next_sp = mem_rdata;
              3'd5: next_bp = mem_rdata;
              3'd6: next_si = mem_rdata;
              default: next_di = mem_rdata;
            endcase
          end
          8'hca: begin
            execute_mem_addr = physical_address(2'b10, sp);
            next_ip = mem_rdata;
            next_cs = source_rdata;
            next_sp = sp + 16'd4 + immediate_r;
          end
          8'hcb: begin
            execute_mem_addr = physical_address(2'b10, sp);
            next_ip = mem_rdata;
            next_cs = source_rdata;
            next_sp = sp + 16'd4;
          end
          8'h9a: begin
            next_ip = immediate_r;
            next_cs = far_call_segment;
            next_far_call_return_ip = ip;
            next_state = S_FAR_CALL_PUSH_CS;
          end
          8'hcc: begin
              interrupt_vector_input = 8'd3;
            next_state = S_INT_PUSH_FLAGS;
          end
          8'hcd: begin
            interrupt_vector_input = immediate_r[7:0];
            if (immediate_r[7:0] == 8'h20) begin
              next_halted = 1'b1;
              next_state = S_HALTED;
            end else if (immediate_r[7:0] == 8'h21 && ah == 8'h4c) begin
              next_halted = 1'b1;
              next_state = S_HALTED;
            end else if (immediate_r[7:0] == 8'h21 && ah == 8'h09) begin
              next_state = S_DOS_PRINT;
            end else if (immediate_r[7:0] == 8'h21 && ah == 8'h02) begin
              console_write_r = 1'b1;
              console_char_r = dl;
            end else if (immediate_r[7:0] == 8'h10 && ah == 8'h0e) begin
              console_write_r = 1'b1;
              console_char_r = al;
            end else
              next_state = S_INT_PUSH_FLAGS;
          end
          8'hce: begin
            if (flags[11]) begin
              interrupt_vector_input = 8'd4;
              next_state = S_INT_PUSH_FLAGS;
            end
          end
          8'hcf:
            next_state = S_IRET_POP_IP;
          8'hc6, 8'hc7: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = opcode_r == 8'hc6 ? SZ_BYTE : SZ_WORD;
            execute_mem_wdata = immediate_r;
            if (!modrm_memory && opcode_r == 8'hc6) begin
              case (modrm_rm)
                3'd0: next_ax = {ax[15:8], immediate_r[7:0]};
                3'd1: next_cx = {cx[15:8], immediate_r[7:0]};
                3'd2: next_dx = {dx[15:8], immediate_r[7:0]};
                3'd3: next_bx = {bx[15:8], immediate_r[7:0]};
                3'd4: next_ax = {immediate_r[7:0], ax[7:0]};
                3'd5: next_cx = {immediate_r[7:0], cx[7:0]};
                3'd6: next_dx = {immediate_r[7:0], dx[7:0]};
                default: next_bx = {immediate_r[7:0], bx[7:0]};
              endcase
            end else if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = immediate_r;
                3'd1: next_cx = immediate_r;
                3'd2: next_dx = immediate_r;
                3'd3: next_bx = immediate_r;
                3'd4: next_sp = immediate_r;
                3'd5: next_bp = immediate_r;
                3'd6: next_si = immediate_r;
                default: next_di = immediate_r;
              endcase
            end
          end
          8'hc0, 8'hc1, 8'hd0, 8'hd1, 8'hd2, 8'hd3: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
            execute_mem_wdata = shift_result;
            next_flags = shift_flags;
            if (!modrm_memory) begin
              if (execute_width8) begin
                case (modrm_rm)
                  3'd0: next_ax = {ax[15:8], shift_result[7:0]};
                  3'd1: next_cx = {cx[15:8], shift_result[7:0]};
                  3'd2: next_dx = {dx[15:8], shift_result[7:0]};
                  3'd3: next_bx = {bx[15:8], shift_result[7:0]};
                  3'd4: next_ax = {shift_result[7:0], ax[7:0]};
                  3'd5: next_cx = {shift_result[7:0], cx[7:0]};
                  3'd6: next_dx = {shift_result[7:0], dx[7:0]};
                  default: next_bx = {shift_result[7:0], bx[7:0]};
                endcase
              end else begin
                case (modrm_rm)
                  3'd0: next_ax = shift_result;
                  3'd1: next_cx = shift_result;
                  3'd2: next_dx = shift_result;
                  3'd3: next_bx = shift_result;
                  3'd4: next_sp = shift_result;
                  3'd5: next_bp = shift_result;
                  3'd6: next_si = shift_result;
                  default: next_di = shift_result;
                endcase
              end
            end
          end
          8'hd4: begin
            if (immediate_r[7:0] == 8'b0)
              next_trap = 1'b1;
            else begin
              divide_start = 1'b1;
              next_state = S_DIVIDE;
             end
           end
          8'hd5:
            next_ax = {8'b0, al + (ah * immediate_r[7:0])};
          8'hd6:
            next_ax = {ah, flags[0] ? 8'hff : 8'h00};
          8'hd7: begin
            execute_mem_addr = physical_address(2'b11, bx + {8'b0, al});
            next_ax = {ah, mem_rdata[7:0]};
          end
          8'hd8, 8'hd9, 8'hda, 8'hdb, 8'hdc, 8'hdd, 8'hde, 8'hdf:
            next_ax = ax;
          8'hf6, 8'hf7: begin
            if (modrm_register == 3'd0 || modrm_register == 3'd1) begin
              next_flags = unary_flags;
            end
            if (modrm_register == 3'd2 || modrm_register == 3'd3) begin
              execute_mem_we = modrm_memory;
              execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
              execute_mem_wdata = unary_result;
              next_flags = unary_flags;
              if (!modrm_memory) begin
                if (execute_width8) begin
                  case (modrm_rm)
                    3'd0: next_ax = {ax[15:8], unary_result[7:0]};
                    3'd1: next_cx = {cx[15:8], unary_result[7:0]};
                    3'd2: next_dx = {dx[15:8], unary_result[7:0]};
                    3'd3: next_bx = {bx[15:8], unary_result[7:0]};
                    3'd4: next_ax = {unary_result[7:0], ax[7:0]};
                    3'd5: next_cx = {unary_result[7:0], cx[7:0]};
                    3'd6: next_dx = {unary_result[7:0], dx[7:0]};
                    default: next_bx = {unary_result[7:0], bx[7:0]};
                  endcase
                end else begin
                  case (modrm_rm)
                    3'd0: next_ax = unary_result;
                    3'd1: next_cx = unary_result;
                    3'd2: next_dx = unary_result;
                    3'd3: next_bx = unary_result;
                    3'd4: next_sp = unary_result;
                    3'd5: next_bp = unary_result;
                    3'd6: next_si = unary_result;
                    default: next_di = unary_result;
                  endcase
                end
              end
            end
            if (modrm_register == 3'd4 || modrm_register == 3'd5) begin
              next_ax = mul_product[15:0];
              if (!execute_width8)
                next_dx = mul_product[31:16];
              next_flags = mul_flags;
            end
            if (modrm_register >= 3'd6) begin
              divide_start = 1'b1;
              next_state = S_DIVIDE_INIT;
            end
          end
          8'hfe: begin
            execute_mem_we = modrm_memory;
            execute_mem_width = SZ_BYTE;
            execute_mem_wdata = rm_incdec_result;
            next_flags = alu_flag_value;
            if (!modrm_memory) begin
              case (modrm_rm)
                3'd0: next_ax = {ax[15:8], rm_incdec_result[7:0]};
                3'd1: next_cx = {cx[15:8], rm_incdec_result[7:0]};
                3'd2: next_dx = {dx[15:8], rm_incdec_result[7:0]};
                3'd3: next_bx = {bx[15:8], rm_incdec_result[7:0]};
                3'd4: next_ax = {rm_incdec_result[7:0], ax[7:0]};
                3'd5: next_cx = {rm_incdec_result[7:0], cx[7:0]};
                3'd6: next_dx = {rm_incdec_result[7:0], dx[7:0]};
                default: next_bx = {rm_incdec_result[7:0], bx[7:0]};
              endcase
            end
          end
          8'hff: begin
            if (modrm_register == 3'd0 || modrm_register == 3'd1) begin
              execute_mem_we = modrm_memory;
              execute_mem_width = SZ_WORD;
              execute_mem_wdata = rm_incdec_result;
              next_flags = alu_flag_value;
              if (!modrm_memory) begin
                case (modrm_rm)
                  3'd0: next_ax = rm_incdec_result;
                  3'd1: next_cx = rm_incdec_result;
                  3'd2: next_dx = rm_incdec_result;
                  3'd3: next_bx = rm_incdec_result;
                  3'd4: next_sp = rm_incdec_result;
                  3'd5: next_bp = rm_incdec_result;
                  3'd6: next_si = rm_incdec_result;
                  default: next_di = rm_incdec_result;
                endcase
              end
            end else if (modrm_register == 3'd2) begin
              next_sp = sp - 16'd2;
              execute_mem_addr = physical_address(2'b10, sp - 16'd2);
              execute_mem_we = 1'b1;
              execute_mem_width = SZ_WORD;
              execute_mem_wdata = ip;
              next_ip = rm_result_value;
            end else if (modrm_register == 3'd3) begin
              next_far_call_return_ip = ip;
              next_ip = rm_result_value;
              next_cs = source_rdata;
              next_state = S_FAR_CALL_PUSH_CS;
            end else if (modrm_register == 3'd4) begin
              next_ip = rm_result_value;
            end else if (modrm_register == 3'd5) begin
              next_ip = rm_result_value;
              next_cs = source_rdata;
            end else if (modrm_register == 3'd6) begin
              next_sp = sp - 16'd2;
              execute_mem_addr = physical_address(2'b10, sp - 16'd2);
              execute_mem_we = 1'b1;
              execute_mem_width = SZ_WORD;
              execute_mem_wdata = rm_result_value;
            end else begin
              next_trap = 1'b1;
            end
          end
          8'h90: next_ax = ax;
          8'h91, 8'h92, 8'h93, 8'h94, 8'h95, 8'h96, 8'h97: begin
            next_ax = register_word(opcode_r[2:0]);
            case (opcode_r[2:0])
              3'd0: next_cx = ax;
              3'd1: next_dx = ax;
              3'd2: next_bx = ax;
              3'd3: next_sp = ax;
              3'd4: next_bp = ax;
              3'd5: next_si = ax;
              default: next_di = ax;
            endcase
          end
          8'h98:
            next_ax = {{8{al}}, al};
          8'h99:
            next_dx = ax[15] ? 16'hffff : 16'h0000;
          8'h9b: next_ax = ax;
          8'h9c: begin
            next_sp = sp - 16'd2;
            execute_mem_addr = physical_address(2'b10, sp - 16'd2);
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = flags;
          end
          8'h9d: begin
            next_flags = mem_rdata;
            next_sp = sp + 16'd2;
            execute_mem_addr = physical_address(2'b10, sp);
          end
          8'h9e:
            next_flags = {flags[15:8], ah};
          8'h9f:
            next_ax = {flags[7:0], al};
          8'ha0: begin
            next_ax = {ah, mem_rdata[7:0]};
            execute_mem_addr = physical_address(
              seg_override_present ? seg_override : 2'b11,
              immediate_r
            );
          end
          8'ha1: begin
            next_ax = mem_rdata;
            execute_mem_addr = physical_address(
              seg_override_present ? seg_override : 2'b11,
              immediate_r
            );
          end
          8'ha2: begin
            execute_mem_addr = physical_address(
              seg_override_present ? seg_override : 2'b11,
              immediate_r
            );
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_BYTE;
            execute_mem_wdata = {8'b0, al};
          end
          8'ha3: begin
            execute_mem_addr = physical_address(
              seg_override_present ? seg_override : 2'b11,
              immediate_r
            );
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = ax;
          end
          8'ha4, 8'ha5: begin
            execute_mem_addr = physical_address(2'b00, di);
            execute_mem_we = 1'b1;
            execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
            execute_mem_wdata = source_rdata;
            next_si = si + string_delta;
            next_di = di + string_delta;
            if (rep_none && cx != 16'd0) begin
              next_cx = cx - 16'd1;
              next_state = S_EXECUTE;
              next_rep_none = rep_none;
              next_rep_while_equal = rep_while_equal;
              next_rep_while_not_equal = rep_while_not_equal;
            end
          end
          8'ha6, 8'ha7: begin
            execute_mem_addr = physical_address(2'b00, di);
            next_flags = alu_flag_value;
            next_si = si + string_delta;
            next_di = di + string_delta;
            if (rep_none && cx != 16'd0 &&
                (!rep_while_equal || alu_flag_value[6]) &&
                (!rep_while_not_equal || !alu_flag_value[6])) begin
              next_cx = cx - 16'd1;
              next_state = S_EXECUTE;
              next_rep_none = rep_none;
              next_rep_while_equal = rep_while_equal;
              next_rep_while_not_equal = rep_while_not_equal;
            end
          end
          8'ha8: begin
            next_flags = test_byte_flags;
          end
          8'ha9: begin
            next_flags = test_word_flags;
          end
          8'haa, 8'hab: begin
            execute_mem_addr = physical_address(2'b00, di);
            execute_mem_we = 1'b1;
            execute_mem_width = execute_width8 ? SZ_BYTE : SZ_WORD;
            execute_mem_wdata = ax;
            next_di = di + string_delta;
            if (rep_none && cx != 16'd0) begin
              next_cx = cx - 16'd1;
              next_state = S_EXECUTE;
              next_rep_none = rep_none;
              next_rep_while_equal = rep_while_equal;
              next_rep_while_not_equal = rep_while_not_equal;
            end
          end
          8'hac, 8'had: begin
            next_ax = source_rdata;
            next_si = si + string_delta;
            if (rep_none && cx != 16'd0) begin
              next_cx = cx - 16'd1;
              next_state = S_EXECUTE;
              next_rep_none = rep_none;
              next_rep_while_equal = rep_while_equal;
              next_rep_while_not_equal = rep_while_not_equal;
            end
          end
          8'hae, 8'haf: begin
            execute_mem_addr = physical_address(2'b00, di);
            next_flags = alu_flag_value;
            next_di = di + string_delta;
            if (rep_none && cx != 16'd0 &&
                (!rep_while_equal || alu_flag_value[6]) &&
                (!rep_while_not_equal || !alu_flag_value[6])) begin
              next_cx = cx - 16'd1;
              next_state = S_EXECUTE;
              next_rep_none = rep_none;
              next_rep_while_equal = rep_while_equal;
              next_rep_while_not_equal = rep_while_not_equal;
            end
          end
          8'he0, 8'he1, 8'he2: begin
            next_cx = cx - 16'd1;
            if (opcode_r == 8'he0)
              next_ip = (cx - 16'd1 != 0 && !flags[6])
                ? ip + {{8{immediate_r[7]}}, immediate_r[7:0]} : ip;
            else if (opcode_r == 8'he1)
              next_ip = (cx - 16'd1 != 0 && flags[6])
                ? ip + {{8{immediate_r[7]}}, immediate_r[7:0]} : ip;
            else
              next_ip = cx - 16'd1 != 0
                ? ip + {{8{immediate_r[7]}}, immediate_r[7:0]} : ip;
          end
          8'he3:
            next_ip = cx == 0 ? ip + {{8{immediate_r[7]}}, immediate_r[7:0]} : ip;
          8'he4: begin
            execute_io_addr = {12'b0, immediate_r[7:0]};
            next_ax = {ah, io_rdata[7:0]};
          end
          8'he5: begin
            execute_io_addr = {12'b0, immediate_r[7:0]};
            next_ax = io_rdata;
          end
          8'he6: begin
            execute_io_addr = {12'b0, immediate_r[7:0]};
            execute_io_we = 1'b1;
            execute_io_width = SZ_BYTE;
            execute_io_wdata = {8'b0, al};
          end
          8'he7: begin
            execute_io_addr = {12'b0, immediate_r[7:0]};
            execute_io_we = 1'b1;
            execute_io_width = SZ_WORD;
            execute_io_wdata = ax;
          end
          8'he8: begin
            next_sp = sp - 16'd2;
            execute_mem_addr = physical_address(2'b10, sp - 16'd2);
            execute_mem_we = 1'b1;
            execute_mem_width = SZ_WORD;
            execute_mem_wdata = ip;
            next_ip = ip + immediate_r;
          end
          8'he9:
            next_ip = ip + immediate_r;
          8'hea: begin
            next_ip = immediate_r;
            next_cs = far_jmp_segment;
          end
          8'heb:
            next_ip = ip + {{8{immediate_r[7]}}, immediate_r[7:0]};
          8'hec:
            next_ax = {ah, io_rdata[7:0]};
          8'hed:
            next_ax = io_rdata;
          8'hee: begin
            execute_io_we = 1'b1;
            execute_io_width = SZ_BYTE;
            execute_io_wdata = {8'b0, al};
          end
          8'hef: begin
            execute_io_we = 1'b1;
            execute_io_width = SZ_WORD;
            execute_io_wdata = ax;
          end
          8'hf5, 8'hf8, 8'hf9, 8'hfa, 8'hfb, 8'hfc, 8'hfd:
            next_flags = simple_flags;
          8'hf4: begin next_halted = 1'b1; next_state = S_HALTED; end
          default: begin next_trap = 1'b1; end
                endcase
              end
            end else if (modrm_register == 3'd4 || modrm_register == 3'd5) begin
              next_ax = mul_product[15:0];
              if (!execute_width8)
                next_dx = mul_product[31:16];
              next_flags = mul_flags;
            end else begin
              divide_start = 1'b1;
              next_state = S_DIVIDE;
            end
          end

  assign mem_addr =
    state == S_EXECUTE ? execute_mem_addr :
    state == S_INT_PUSH_FLAGS ? physical_address(2'b10, sp - 16'd2) :
    state == S_INT_PUSH_CS ? physical_address(2'b10, sp - 16'd2) :
    state == S_INT_PUSH_IP ? physical_address(2'b10, sp - 16'd2) :
    state == S_INT_READ_TARGET ?
      {{10{1'b0}}, interrupt_vector_r, 2'b00} :
    state == S_IRET_POP_IP ? physical_address(2'b10, sp) :
    state == S_IRET_POP_CS ? physical_address(2'b10, sp) :
    state == S_IRET_POP_FLAGS ? physical_address(2'b10, sp) :
    state == S_FAR_CALL_PUSH_CS ? physical_address(2'b10, sp - 16'd2) :
    state == S_FAR_CALL_PUSH_IP ? physical_address(2'b10, sp - 16'd2) :
    state == S_DOS_PRINT ? physical_address(2'b11, dx) :
    fetch_address;
  assign mem_wdata =
    state == S_INT_PUSH_FLAGS ? flags :
    state == S_INT_PUSH_CS ? cs :
    state == S_INT_PUSH_IP ? ip :
    state == S_FAR_CALL_PUSH_CS ? cs :
    state == S_FAR_CALL_PUSH_IP ? far_call_return_ip :
    execute_mem_wdata;
  assign mem_we =
    (state == S_EXECUTE && execute_mem_we) ||
    state == S_INT_PUSH_FLAGS || state == S_INT_PUSH_CS ||
    state == S_INT_PUSH_IP || state == S_FAR_CALL_PUSH_CS ||
    state == S_FAR_CALL_PUSH_IP;
  assign mem_width = state == S_EXECUTE ? execute_mem_width : SZ_WORD;
  assign io_addr = execute_io_addr;
  assign io_we = state == S_EXECUTE && execute_io_we;
  assign io_width = execute_io_width;
  assign console_write = console_write_r || (state == S_DOS_PRINT && mem_rdata[7:0] != 8'h24);
  assign console_char = state == S_DOS_PRINT ? mem_rdata[7:0] : console_char_r;
  assign interrupt_ack = state == S_INT_PUSH_FLAGS ||
                         state == S_INT_PUSH_CS ||
                         state == S_INT_PUSH_IP ||
                         state == S_INT_READ_TARGET;
  assign halted = halted_r;
  assign trap = trap_r;
  assign instructions_retired = retired;
  assign dbg_ax = ax;
  assign dbg_cx = cx;
  assign dbg_dx = dx;
  assign dbg_bx = bx;
  assign dbg_sp = sp;
  assign dbg_bp = bp;
  assign dbg_si = si;
  assign dbg_di = di;
  assign dbg_es = es;
  assign dbg_cs = cs;
  assign dbg_ss = ss;
  assign dbg_ds = ds;
  assign dbg_ip = ip;
  assign dbg_flags = flags;

  always @(posedge clk) begin
    if (rst) begin
      state <= S_FETCH;
      interrupt_vector_r <= 8'b0;
      opcode_r <= 8'b0;
      modrm_r <= 8'b0;
      displacement_r <= 16'b0;
      immediate_r <= 16'b0;
      far_r <= 16'b0;
      seg_override_present <= 1'b0;
      seg_override <= 2'b0;
      rep_none <= 1'b0;
      rep_while_equal <= 1'b0;
      rep_while_not_equal <= 1'b0;
      ax <= 16'b0;
      cx <= 16'b0;
      dx <= 16'b0;
      bx <= 16'b0;
      sp <= init_sp;
      bp <= 16'b0;
      si <= 16'b0;
      di <= 16'b0;
      es <= init_es;
      cs <= init_cs;
      ss <= init_ss;
      ds <= init_ds;
      ip <= init_ip;
      flags <= 16'h0200;
      halted_r <= 1'b0;
      trap_r <= 1'b0;
      retired <= 64'b0;
    end else begin
      case (state)
        S_FETCH: begin
          if (nmi || (irq && flags[9])) begin
            interrupt_vector_r <= nmi ? 8'd2 : interrupt_vector;
            state <= S_INT_PUSH_FLAGS;
          end else if (fetch_prefix) begin
            ip <= ip + 16'd1;
            if (fetch_segment_override) begin
              seg_override_present <= 1'b1;
              seg_override <= fetch_segment_number;
            end
            if (fetch_repeat) begin
              rep_none <= 1'b1;
              rep_while_equal <= fetch_repeat_equal;
              rep_while_not_equal <= ~fetch_repeat_equal;
            end
          end else begin
            opcode_r <= fetch_byte;
            ip <= ip + 16'd1;
            if (fetch_opcode_has_modrm)
              state <= S_MODRM;
            else if (fetch_byte == 8'hea || fetch_byte == 8'h9a)
              state <= S_IMM_LOW;
            else if (fetch_opcode_imm_size != 2'b00)
              state <= S_IMM_LOW;
            else
              state <= S_EXECUTE;
          end
        end
        S_MODRM: begin
          modrm_r <= fetch_byte;
          ip <= ip + 16'd1;
          if (fetch_byte[7:6] == 2'b01)
            state <= S_DISP8;
          else if (fetch_byte[7:6] == 2'b10)
            state <= S_DISP16_LOW;
          else if (fetch_byte[7:6] == 2'b00 && fetch_byte[2:0] == 3'b110)
            state <= S_DISP16_LOW;
          else if (modrm_fetch_needs_immediate)
            state <= S_IMM_LOW;
          else
            state <= S_EXECUTE;
        end
        S_DISP8: begin
          displacement_r <= {{8{fetch_byte[7]}}, fetch_byte};
          ip <= ip + 16'd1;
          if (execute_imm_size != 2'b00)
            state <= S_IMM_LOW;
          else
            state <= S_EXECUTE;
        end
        S_DISP16_LOW: begin
          displacement_r <= {8'b0, fetch_byte};
          ip <= ip + 16'd1;
          state <= S_DISP16_HIGH;
        end
        S_DISP16_HIGH: begin
          displacement_r <= {fetch_byte, displacement_r[7:0]};
          ip <= ip + 16'd1;
          if (execute_imm_size != 2'b00)
            state <= S_IMM_LOW;
          else
            state <= S_EXECUTE;
        end
        S_IMM_LOW: begin
          immediate_r <= {8'b0, fetch_byte};
          ip <= ip + 16'd1;
          if (execute_imm_size == 2'b01 && opcode_r != 8'hea && opcode_r != 8'h9a)
            state <= S_EXECUTE;
          else
            state <= S_IMM_HIGH;
        end
        S_IMM_HIGH: begin
          immediate_r <= {fetch_byte, immediate_r[7:0]};
          ip <= ip + 16'd1;
          if (opcode_r == 8'hea || opcode_r == 8'h9a)
            state <= S_FAR_LOW;
          else
            state <= S_EXECUTE;
        end
        S_FAR_LOW: begin
          far_r <= {8'b0, fetch_byte};
          ip <= ip + 16'd1;
          state <= S_FAR_HIGH;
        end
        S_FAR_HIGH: begin
          far_r <= {fetch_byte, far_r[7:0]};
          ip <= ip + 16'd1;
          state <= S_EXECUTE;
        end
        S_EXECUTE: begin
          state <= next_state;
          interrupt_vector_r <= interrupt_vector_input;
          far_call_return_ip <= next_far_call_return_ip;
          ax <= next_ax;
          cx <= next_cx;
          dx <= next_dx;
          bx <= next_bx;
          sp <= next_sp;
          bp <= next_bp;
          si <= next_si;
          di <= next_di;
          es <= next_es;
          cs <= next_cs;
          ss <= next_ss;
          ds <= next_ds;
          ip <= next_ip;
          flags <= next_flags;
          halted_r <= next_halted;
          trap_r <= next_trap;
          retired <= next_retired;
          seg_override_present <= next_seg_override_present;
          seg_override <= next_seg_override;
          rep_none <= next_rep_none;
          rep_while_equal <= next_rep_while_equal;
          rep_while_not_equal <= next_rep_while_not_equal;
          if (divide_start) begin
            divide_aam <= opcode_r == 8'hd4;
            if (opcode_r == 8'hd4)
              divide_bits_left <= 6'd8;
            else if (execute_width8)
              divide_bits_left <= 6'd16;
            else
              divide_bits_left <= 6'd32;
            divide_remainder <= 32'b0;
            divide_quotient <= 32'b0;
            divide_byte <= opcode_r == 8'hd4 || execute_width8;
            divide_signed <= divide_will_be_signed;
            if (opcode_r == 8'hd4 || execute_width8) begin
              divide_dividend <= opcode_r == 8'hd4
                ? {24'b0, al}
                : divide_will_be_signed && ax[15]
                ? 32'b0 - {16'b0, ax}
                : {16'b0, ax};
              divide_dividend_negative <= divide_will_be_signed && ax[15];
              divide_divisor_negative <= divide_will_be_signed &&
                (opcode_r == 8'hd4 ? immediate_r[7] : rm_result_value[7]);
              if (opcode_r == 8'hd4)
                divide_divisor <= {24'b0, immediate_r[7:0]};
              else if (divide_will_be_signed && rm_result_value[7])
                divide_divisor <= 32'b0 -
                  {{24{rm_result_value[7]}}, rm_result_value[7:0]};
              else
                divide_divisor <= {24'b0, rm_result_value[7:0]};
            end else begin
              divide_dividend <= divide_will_be_signed && dx[15]
                ? 32'b0 - {dx, ax}
                : {dx, ax};
              divide_dividend_negative <= divide_will_be_signed && dx[15];
              divide_divisor_negative <= divide_will_be_signed && rm_result_value[15];
              divide_divisor <= divide_will_be_signed && rm_result_value[15]
                ? 32'b0 - {{16{rm_result_value[15]}}, rm_result_value}
                : {16'b0, rm_result_value};
            end
          end
        end
        S_INT_PUSH_FLAGS: begin
          sp <= sp - 16'd2;
          state <= S_INT_PUSH_CS;
        end
        S_INT_PUSH_CS: begin
          sp <= sp - 16'd2;
          state <= S_INT_PUSH_IP;
        end
        S_INT_PUSH_IP: begin
          sp <= sp - 16'd2;
          state <= S_INT_READ_TARGET;
        end
        S_INT_READ_TARGET: begin
          ip <= mem_rdata;
          cs <= source_rdata;
          flags[9] <= 1'b0;
          flags[8] <= 1'b0;
          state <= S_FETCH;
        end
        S_IRET_POP_IP: begin
          ip <= mem_rdata;
          sp <= sp + 16'd2;
          state <= S_IRET_POP_CS;
        end
        S_IRET_POP_CS: begin
          cs <= mem_rdata;
          sp <= sp + 16'd2;
          state <= S_IRET_POP_FLAGS;
        end
        S_IRET_POP_FLAGS: begin
          flags <= mem_rdata;
          sp <= sp + 16'd2;
          state <= S_FETCH;
        end
        S_FAR_CALL_PUSH_CS: begin
          sp <= sp - 16'd2;
          state <= S_FAR_CALL_PUSH_IP;
        end
        S_FAR_CALL_PUSH_IP: begin
          sp <= sp - 16'd2;
          state <= S_FETCH;
        end
        S_DOS_PRINT: begin
          if (mem_rdata[7:0] == 8'h24)
            state <= S_FETCH;
          else
            dx <= dx + 16'd1;
        end
        S_DIVIDE: begin
          if (divide_bits_left == 6'd0) begin
            if (divide_divisor == 32'b0 ||
                (!divide_aam && divide_byte &&
                 divide_quotient[15:8] != 8'b0) ||
                (!divide_byte && divide_quotient[31:16] != 16'b0) ||
                (divide_signed && divide_byte &&
                 divide_quotient[15:7] != 9'b0 &&
                 divide_quotient[15:7] != 9'h1ff) ||
                (divide_signed && !divide_byte &&
                 divide_quotient[31:15] != 17'b0 &&
                 divide_quotient[31:15] != 17'h1ffff)) begin
              trap_r <= 1'b1;
            end else if (divide_aam) begin
              ax <= {divide_quotient[7:0], divide_remainder[7:0]};
              flags[7] <= divide_remainder[7];
              flags[6] <= divide_remainder[7:0] == 8'b0;
              flags[2] <= parity_even(divide_remainder[7:0]);
            end else if (divide_byte) begin
              if (divide_signed && divide_dividend_negative != divide_divisor_negative)
                ax <= {divide_remainder[7:0], 8'b0 - divide_quotient[7:0]};
              else
                ax <= {divide_remainder[7:0], divide_quotient[7:0]};
            end else begin
              if (divide_signed && divide_dividend_negative != divide_divisor_negative)
                ax <= 16'b0 - divide_quotient[15:0];
              else
                ax <= divide_quotient[15:0];
              if (divide_signed && divide_dividend_negative)
                dx <= 16'b0 - divide_remainder[15:0];
              else
                dx <= divide_remainder[15:0];
            end
            state <= S_FETCH;
          end else begin
            divide_bits_left <= divide_bits_left - 6'd1;
            divide_dividend <= divide_dividend << 1;
            if ({divide_remainder[30:0], divide_dividend_msb} >= divide_divisor) begin
              divide_remainder <= {divide_remainder[30:0], divide_dividend_msb} -
                                 divide_divisor;
              divide_quotient <= {divide_quotient[30:0], 1'b1};
            end else begin
              divide_remainder <= {divide_remainder[30:0], divide_dividend_msb};
              divide_quotient <= {divide_quotient[30:0], 1'b0};
            end
          end
        end
        S_DIVIDE_INIT: state <= S_DIVIDE;
        S_HALTED: state <= S_HALTED;
        default: state <= S_FETCH;
      endcase
    end
  end


endmodule
