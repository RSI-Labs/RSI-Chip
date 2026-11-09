// RV64I instruction coverage for the user-level ISS.
//
// Every check exits with the failing check id; a clean run exits 0.

.macro FAIL id
    li      t0, 0x10000004
    li      a0, \id
    sw      a0, 0(t0)
.endm

.macro CHECK_EQ reg, value, id
    li      t6, \value
    beq     \reg, t6, 9f
    FAIL    \id
9:
.endm

.section .text
.globl _start
_start:
    // 64-bit arithmetic
    li      t0, 0x1000000000000001
    li      t1, 2
    add     t2, t0, t1
    CHECK_EQ t2, 0x1000000000000003, 1
    sub     t2, t0, t1
    CHECK_EQ t2, 0x0fffffffffffffff, 2

    li      t0, 0xf0f0f0f0f0f0f0f0
    li      t1, 0x0ff00ff00ff00ff0
    and     t2, t0, t1
    CHECK_EQ t2, 0x00f000f000f000f0, 3
    or      t2, t0, t1
    CHECK_EQ t2, 0xfff0fff0fff0fff0, 4
    xor     t2, t0, t1
    CHECK_EQ t2, 0xff00ff00ff00ff00, 5

    // 64-bit shifts (6-bit shamt)
    li      t0, 1
    slli    t2, t0, 40
    CHECK_EQ t2, 0x10000000000, 6
    li      t0, 0x8000000000000000
    srli    t2, t0, 40
    CHECK_EQ t2, 0x800000, 7
    srai    t2, t0, 40
    CHECK_EQ t2, 0xffffffffff800000, 8

    li      t0, 1
    li      t1, 63
    sll     t2, t0, t1
    CHECK_EQ t2, 0x8000000000000000, 9
    srl     t2, t2, t1
    CHECK_EQ t2, 1, 10
    li      t0, 0x8000000000000000
    li      t1, 4
    sra     t2, t0, t1
    CHECK_EQ t2, 0xf800000000000000, 11

    // RV64 word operations sign-extend bit 31
    li      t0, 0x00000000ffffffff
    addiw   t2, t0, 1
    CHECK_EQ t2, 0, 12
    li      t0, 0x7fffffff
    addiw   t2, t0, 1
    CHECK_EQ t2, 0xffffffff80000000, 13
    li      t0, 0x00000000ffffffff
    li      t1, 2
    addw    t2, t0, t1
    CHECK_EQ t2, 1, 14
    subw    t2, t0, t1
    CHECK_EQ t2, 0xfffffffffffffffd, 15
    li      t1, 1
    sllw    t2, t0, t1
    CHECK_EQ t2, 0xfffffffffffffffe, 16
    li      t0, 0xffffffff80000000
    li      t1, 4
    srlw    t2, t0, t1
    CHECK_EQ t2, 0x08000000, 17
    sraw    t2, t0, t1
    CHECK_EQ t2, 0xfffffffff8000000, 18
    li      t0, 1
    li      t1, 33
    sllw    t2, t0, t1        // shamt is masked to 5 bits: 33 -> 1
    CHECK_EQ t2, 2, 19

    // Signed/unsigned compares
    li      t0, -1
    li      t1, 1
    slt     t2, t0, t1
    CHECK_EQ t2, 1, 20
    sltu    t2, t0, t1
    CHECK_EQ t2, 0, 21
    slti    t2, t0, 0
    CHECK_EQ t2, 1, 22
    sltiu   t2, t0, 1
    CHECK_EQ t2, 0, 23

    // LUI sign extension
    lui     t0, 0x80000
    CHECK_EQ t0, 0xffffffff80000000, 24

    // JAL link and target
    li      t0, 0
    jal     t1, 1f
    li      t0, 1
1:
    auipc   t2, 0
    sub     t3, t2, t1        // link points at the 4-byte instruction after JAL
    CHECK_EQ t3, 4, 25
    CHECK_EQ t0, 0, 26

    // JALR link and target
    la      t3, 2f
    jalr    t1, 0(t3)
    li      t0, 1
2:
    auipc   t2, 0
    sub     t4, t2, t3
    CHECK_EQ t4, 0, 27
    CHECK_EQ t0, 0, 28

    // Branches: taken and not-taken paths
    li      t0, 5
    li      t1, 5
    beq     t0, t1, 3f
    FAIL    29
3:
    bne     t0, t1, 4f
    j       5f
4:
    FAIL    30
5:
    li      t1, 3
    blt     t1, t0, 6f
    FAIL    31
6:
    bge     t0, t1, 7f
    FAIL    32
7:
    li      t1, -1
    bgeu    t0, t1, 8f        // 5 >= huge unsigned: false
    j       9f
8:
    FAIL    33
9:
    bltu    t0, t1, 10f       // 5 < huge unsigned: true
    FAIL    34
10:
    bgeu    t1, t0, 11f       // 0xffff... >= 5 unsigned: true
    FAIL    35
11:

    // Loads and stores
    la      t0, data
    li      t1, 0x123456789abcdef0
    sd      t1, 0(t0)
    ld      t2, 0(t0)
    CHECK_EQ t2, 0x123456789abcdef0, 36
    li      t1, 0xffffffff80000001
    sw      t1, 8(t0)
    lw      t2, 8(t0)
    CHECK_EQ t2, 0xffffffff80000001, 37
    lwu     t2, 8(t0)
    CHECK_EQ t2, 0x0000000080000001, 38
    li      t1, 0x1234
    sh      t1, 12(t0)
    lh      t2, 12(t0)
    CHECK_EQ t2, 0x1234, 39
    li      t1, 0xffff
    sh      t1, 14(t0)
    lhu     t2, 14(t0)
    CHECK_EQ t2, 0xffff, 40
    lh      t2, 14(t0)
    CHECK_EQ t2, -1, 41
    li      t1, 0x80
    sb      t1, 16(t0)
    lbu     t2, 16(t0)
    CHECK_EQ t2, 0x80, 42
    lb      t2, 16(t0)
    CHECK_EQ t2, -128, 43

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b

.section .data
.align 3
data:
    .dword 0, 0, 0, 0, 0
