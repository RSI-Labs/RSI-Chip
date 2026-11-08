// UART transmitter: 8 data bits, 1 start bit, 1 stop bit, LSB first.
module uart_tx #(
    parameter integer CLKS_PER_BIT = 16
) (
    input  wire       clk,
    input  wire       rst,
    input  wire       start,
    input  wire [7:0] data,
    output reg        tx,
    output reg        busy,
    output reg        done
);
    localparam integer IDLE  = 0;
    localparam integer START = 1;
    localparam integer DATA  = 2;
    localparam integer STOP  = 3;

    reg [1:0]  state;
    reg [15:0] clk_count;
    reg [2:0]  bit_index;
    reg [7:0]  shift;

    always @(posedge clk) begin
        if (rst) begin
            state     <= IDLE;
            tx        <= 1'b1;
            busy      <= 1'b0;
            done      <= 1'b0;
            clk_count <= 16'd0;
            bit_index <= 3'd0;
            shift     <= 8'd0;
        end else begin
            done <= 1'b0;
            case (state)
                IDLE: begin
                    tx   <= 1'b1;
                    busy <= 1'b0;
                    if (start) begin
                        shift     <= data;
                        busy      <= 1'b1;
                        state     <= START;
                        clk_count <= 16'd0;
                    end
                end
                START: begin
                    tx <= 1'b0;
                    if (clk_count == CLKS_PER_BIT - 1) begin
                        clk_count <= 16'd0;
                        bit_index <= 3'd0;
                        state     <= DATA;
                    end else begin
                        clk_count <= clk_count + 16'd1;
                    end
                end
                DATA: begin
                    tx <= shift[0];
                    if (clk_count == CLKS_PER_BIT - 1) begin
                        clk_count <= 16'd0;
                        shift     <= shift >> 1;
                        if (bit_index == 3'd7)
                            state <= STOP;
                        else
                            bit_index <= bit_index + 3'd1;
                    end else begin
                        clk_count <= clk_count + 16'd1;
                    end
                end
                STOP: begin
                    tx <= 1'b1;
                    if (clk_count == CLKS_PER_BIT - 1) begin
                        clk_count <= 16'd0;
                        busy      <= 1'b0;
                        done      <= 1'b1;
                        state     <= IDLE;
                    end else begin
                        clk_count <= clk_count + 16'd1;
                    end
                end
                default: state <= IDLE;
            endcase
        end
    end
endmodule
