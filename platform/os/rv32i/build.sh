#!/bin/sh
# Build MiniOS-RV32I with LLVM 20's RISC-V backend.
set -eu
cd "$(dirname "$0")"
LLVM=${LLVM:-/opt/homebrew/opt/llvm@20/bin}
LLD=${LLD:-/opt/homebrew/Cellar/lld@20/20.1.8/bin/ld.lld}
CFLAGS="--target=riscv32-unknown-elf -march=rv32i -mabi=ilp32 -O2 -ffreestanding -fno-builtin -fno-stack-protector -fno-pic -mno-relax -msmall-data-limit=0 -nostdlib"
$LLVM/clang $CFLAGS -c start.S -o start.o
$LLVM/clang $CFLAGS -c trap.S -o trap.o
$LLVM/clang $CFLAGS -c kernel.c -o kernel.o
$LLVM/clang $CFLAGS -c user.S -o user.o
$LLD -T link.ld -o minios.elf start.o trap.o kernel.o user.o
$LLVM/llvm-objcopy -O binary minios.elf minios.bin
rm -f start.o trap.o kernel.o user.o
echo "built minios.elf and minios.bin"
