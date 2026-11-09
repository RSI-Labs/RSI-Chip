#!/bin/sh
# Build the RV64G user-level ISS conformance images with LLVM 20.
set -eu
cd "$(dirname "$0")"

LLVM=${LLVM:-/opt/homebrew/opt/llvm@20/bin}
LLD=${LLD:-/opt/homebrew/Cellar/lld@20/20.1.8/bin/ld.lld}
CFLAGS="--target=riscv64-unknown-elf -march=rv64g -mabi=lp64d -O2 \
 -ffreestanding -fno-builtin -fno-stack-protector -fno-pic \
 -mno-relax -msmall-data-limit=0 -nostdlib"

for src in src/*.S; do
    name=$(basename "$src" .S)
    $LLVM/clang $CFLAGS -c "$src" -o "$name.o"
    $LLD -T link.ld -o "$name.elf" "$name.o"
    $LLVM/llvm-objcopy -O binary "$name.elf" "$name.bin"
    rm -f "$name.o"
    echo "built $name.bin"
done
