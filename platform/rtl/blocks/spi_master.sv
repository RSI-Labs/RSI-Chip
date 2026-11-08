// SPI master, mode 0 (CPOL=0, CPHA=0), 8-bit transfers, MSB first.
//
// MOSI is presented while SCLK is low, the slave samples it on the rising
// edge, and the master samples MISO on that same rising edge.  The next MOSI
// bit is presented after the falling edge.
module spi_master #(
    parameter integer DIV = 4
) (
    input  wire       clk,
    input  wire       rst,
    input  wire       start,
    input  wire [7:0] tx_data,
    input  wire       miso,
    output reg        sclk,
    output reg        mosi,
    output reg        cs_n,
    output reg        busy,
    output reg        done,
    output reg  [7:0] rx_data
);
    localparam integer IDLE = 0;
    localparam integer XFER = 1;
    localparam integer DONE = 2;

    reg [1:0]  state;
    reg [15:0] div_count;
    reg [2:0]  bit_index;
    reg [7:0]  tx_shift;
    reg [7:0]  rx_shift;
    reg        phase;

    always @(posedge clk) begin
        if (rst) begin
            state     <= IDLE;
            sclk      <= 1'b0;
            mosi      <= 1'b0;
            cs_n      <= 1'b1;
            busy      <= 1'b0;
            done      <= 1'b0;
            rx_data   <= 8'd0;
            div_count <= 16'd0;
            bit_index <= 3'd0;
            tx_shift  <= 8'd0;
            rx_shift  <= 8'd0;
            phase     <= 1'b0;
        end else begin
            done <= 1'b0;
            case (state)
                IDLE: begin
                    cs_n <= 1'b1;
                    sclk <= 1'b0;
                    busy <= 1'b0;
                    if (start) begin
                        // Present the first MOSI bit before the first edge.
                        tx_shift  <= tx_data;
                        mosi      <= tx_data[7];
                        rx_shift  <= 8'd0;
                        bit_index <= 3'd0;
                        div_count <= 16'd0;
                        phase     <= 1'b0;
                        cs_n      <= 1'b0;
                        busy      <= 1'b1;
                        state     <= XFER;
                    end
                end
                XFER: begin
                    if (div_count == DIV - 1) begin
                        div_count <= 16'd0;
                        if (!phase) begin
                            // Rising edge: slave samples MOSI, master samples MISO.
                            sclk     <= 1'b1;
                            rx_shift <= {rx_shift[6:0], miso};
                            phase    <= 1'b1;
                        end else begin
                            // Falling edge: present the next MOSI bit.
                            sclk     <= 1'b0;
                            phase    <= 1'b0;
                            mosi     <= tx_shift[6];
                            tx_shift <= {tx_shift[6:0], 1'b0};
                            if (bit_index == 3'd7)
                                state <= DONE;
                            else
                                bit_index <= bit_index + 3'd1;
                        end
                    end else begin
                        div_count <= div_count + 16'd1;
                    end
                end
                DONE: begin
                    cs_n    <= 1'b1;
                    sclk    <= 1'b0;
                    busy    <= 1'b0;
                    done    <= 1'b1;
                    rx_data <= rx_shift;
                    state   <= IDLE;
                end
                default: state <= IDLE;
            endcase
        end
    end
endmodule
