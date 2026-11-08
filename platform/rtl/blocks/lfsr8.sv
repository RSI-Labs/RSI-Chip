// 8-bit maximal-length Galois LFSR (x^8 + x^6 + x^5 + x^4 + 1).
module lfsr8 (
    input  wire       clk,
    input  wire       rst,
    input  wire       en,
    input  wire       load,
    input  wire [7:0] seed,
    output reg  [7:0] state,
    output wire       bit_out
);
    assign bit_out = state[7];
    wire feedback = state[7] ^ state[5] ^ state[4] ^ state[3];

    always @(posedge clk) begin
        if (rst)
            state <= 8'h01;
        else if (load)
            state <= seed;
        else if (en)
            state <= {state[6:0], feedback};
    end
endmodule
