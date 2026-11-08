// 8-bit ALU: add, sub, and, or, xor, not, shl, shr with flags.
module alu8 (
    input  wire [2:0] op,
    input  wire [7:0] a,
    input  wire [7:0] b,
    output reg  [7:0] y,
    output wire       zero,
    output wire       negative,
    output wire       carry,
    output wire       overflow
);
    wire [8:0] sum  = {1'b0, a} + {1'b0, b};
    wire [8:0] diff = {1'b0, a} - {1'b0, b};

    always @* begin
        case (op)
            3'd0: y = sum[7:0];
            3'd1: y = diff[7:0];
            3'd2: y = a & b;
            3'd3: y = a | b;
            3'd4: y = a ^ b;
            3'd5: y = ~a;
            3'd6: y = a << b[2:0];
            default: y = a >> b[2:0];
        endcase
    end

    assign zero     = (y == 8'd0);
    assign negative = y[7];
    assign carry    = (op == 3'd0) ? sum[8] : (op == 3'd1) ? diff[8] : 1'b0;
    assign overflow = (op == 3'd0) ? (~(a[7] ^ b[7]) & (a[7] ^ y[7])) :
                      (op == 3'd1) ? ((a[7] ^ b[7]) & (a[7] ^ y[7])) : 1'b0;
endmodule
