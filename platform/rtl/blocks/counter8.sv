// 8-bit loadable up/down counter with enable, carry and zero flags.
module counter8 (
    input  wire       clk,
    input  wire       rst,
    input  wire       en,
    input  wire       up,
    input  wire       load,
    input  wire [7:0] load_value,
    output reg  [7:0] count,
    output wire       carry,
    output wire       zero
);
    assign zero  = (count == 8'd0);
    assign carry = up ? (en && (count == 8'hff)) : (en && (count == 8'h00));

    always @(posedge clk) begin
        if (rst)
            count <= 8'd0;
        else if (load)
            count <= load_value;
        else if (en)
            count <= up ? (count + 8'd1) : (count - 8'd1);
    end
endmodule
