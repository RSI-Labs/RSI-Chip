// Free-running PWM generator. duty_period is the period in clock cycles.
module pwm #(
    parameter integer WIDTH = 8
) (
    input  wire             clk,
    input  wire             rst,
    input  wire             en,
    input  wire [WIDTH-1:0] duty,
    input  wire [WIDTH-1:0] period,
    output reg              pwm_out
);
    reg [WIDTH-1:0] counter;

    always @(posedge clk) begin
        if (rst)
            counter <= {WIDTH{1'b0}};
        else if (en) begin
            if (counter >= period - 1'b1)
                counter <= {WIDTH{1'b0}};
            else
                counter <= counter + 1'b1;
        end
    end

    always @* begin
        pwm_out = (counter < duty);
    end
endmodule
