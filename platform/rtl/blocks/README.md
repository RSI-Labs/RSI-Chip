# Reference blocks

Seven synthesizable SystemVerilog blocks used as end-to-end EDA regressions.
Every block is exercised by `crates/eda-tests/tests/eda_blocks.rs` through:

1. RTL simulation against a golden expected trace,
2. RTL-derived LEC against the standard-cell-mapped netlist,
3. the full physical flow (placement, routing, DRC, SDF export),
4. mapped gate-level simulation compared bit-for-bit with the RTL trace,
5. an SDF-annotated run for the 8-bit counter.

| Block | File | Key behavior |
|---|---|---|
| 8-bit counter | `counter8.sv` | up/down, enable, synchronous load, `carry` and `zero` flags |
| 32-bit shift register | `shift_register32.sv` | load, enable, left/right, arithmetic right, serial in/out |
| 8-bit ALU | `alu8.sv` | add, sub, and, or, xor, not, shift left/right, `zero`/`negative`/`carry`/`overflow` |
| UART transmitter | `uart_tx.sv` | 8 data bits, 1 start bit, 1 stop bit, LSB first, `busy`/`done` |
| SPI master | `spi_master.sv` | mode 0 (CPOL=0, CPHA=0), 8 bits, MSB first, MISO sampled on rising SCLK |
| PWM | `pwm.sv` | free-running counter, duty/period compare |
| LFSR | `lfsr8.sv` | maximal-length 8-bit Galois LFSR (period 255), loadable seed |

Synthesize one block with:

```sh
cargo run -p eda-cli -- synth platform/rtl/blocks/counter8.sv --top counter8
```

Run LEC for one block with:

```sh
cargo run -p eda-cli -- lec platform/rtl/blocks/counter8.sv --top counter8
```

Run the whole block regression with:

```sh
cargo test --release -p eda-tests --test eda_blocks
```
