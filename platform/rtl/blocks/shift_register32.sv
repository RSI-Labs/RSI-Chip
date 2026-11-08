// 32-bit loadable shift register with left/right and arithmetic-right modes.
module shift_register32 (
    input  wire        clk,
    input  wire        rst,
    input  wire        en,
    input  wire        load,
    input  wire        left,
    input  wire        arithmetic,
    input  wire        serial_in,
    input  wire [31:0] load_value,
    output reg  [31:0] q,
    output wire        serial_out
);
    wire fill = (!left && arithmetic) ? q[31] : 1'b0;
    assign serial_out = left ? q[31] : q[0];

    always @(posedge clk) begin
        if (rst)
            q <= 32'd0;
        else if (load)
            q <= load_value;
        else if (en) begin
            if (left)
                q <= {q[30:0], serial_in};
            else
                q <= {fill, q[31:1]};
        end
    end
endmodule
