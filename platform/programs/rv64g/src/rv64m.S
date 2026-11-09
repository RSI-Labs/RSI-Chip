// RV64M multiply/divide coverage for the user-level ISS.

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
    li      t0, 6
    li      t1, 7
    mul     t2, t0, t1
    CHECK_EQ t2, 42, 1
    li      t0, -6
    mul     t2, t0, t1
    CHECK_EQ t2, -42, 2

    // Signed high half: 2^62 * 4 = 2^64 -> high word 1.
    li      t0, 0x4000000000000000
    li      t1, 4
    mulh    t2, t0, t1
    CHECK_EQ t2, 1, 3
    // (-2^63) * 2 = -2^64 -> high word -1.
    li      t0, 0x8000000000000000
    li      t1, 2
    mulh    t2, t0, t1
    CHECK_EQ t2, -1, 30

    // Signed x unsigned high half: -1 * 2 = 0xfffffffffffffffe
    li      t0, -1
    li      t1, 2
    mulhsu  t2, t0, t1
    CHECK_EQ t2, -1, 4
    mulhu   t2, t0, t1
    CHECK_EQ t2, 1, 5

    li      t0, 100
    li      t1, 7
    div     t2, t0, t1
    CHECK_EQ t2, 14, 6
    li      t0, -100
    div     t2, t0, t1
    CHECK_EQ t2, -14, 7
    li      t0, -1
    li      t1, 2
    divu    t2, t0, t1
    CHECK_EQ t2, 0x7fffffffffffffff, 8

    li      t0, 100
    li      t1, 7
    rem     t2, t0, t1
    CHECK_EQ t2, 2, 9
    li      t0, -100
    rem     t2, t0, t1
    CHECK_EQ t2, -2, 10
    li      t0, -1
    li      t1, 2
    remu    t2, t0, t1
    CHECK_EQ t2, 1, 11

    // Division by zero and signed overflow
    li      t0, 100
    li      t1, 0
    div     t2, t0, t1
    CHECK_EQ t2, -1, 12
    divu    t2, t0, t1
    CHECK_EQ t2, 0xffffffffffffffff, 13
    rem     t2, t0, t1
    CHECK_EQ t2, 100, 14
    remu    t2, t0, t1
    CHECK_EQ t2, 100, 15

    li      t0, 0x8000000000000000
    li      t1, -1
    div     t2, t0, t1
    CHECK_EQ t2, 0x8000000000000000, 16
    rem     t2, t0, t1
    CHECK_EQ t2, 0, 17

    // 32-bit multiply/divide variants
    li      t0, 6
    li      t1, 7
    mulw    t2, t0, t1
    CHECK_EQ t2, 42, 18
    li      t0, 0x7fffffff
    li      t1, 2
    mulw    t2, t0, t1
    CHECK_EQ t2, 0xfffffffffffffffe, 19

    li      t0, 100
    li      t1, 7
    divw    t2, t0, t1
    CHECK_EQ t2, 14, 20
    li      t0, -100
    divw    t2, t0, t1
    CHECK_EQ t2, -14, 21
    li      t0, -1
    li      t1, 2
    divuw   t2, t0, t1
    CHECK_EQ t2, 0x7fffffff, 22
    li      t0, -100
    li      t1, 7
    remw    t2, t0, t1
    CHECK_EQ t2, -2, 23
    li      t0, 100
    remuw   t2, t0, t1
    CHECK_EQ t2, 2, 24

    li      t0, 100
    li      t1, 0
    divw    t2, t0, t1
    CHECK_EQ t2, -1, 25
    divuw   t2, t0, t1
    CHECK_EQ t2, 0xffffffffffffffff, 26

    li      t0, 0xffffffff80000000
    li      t1, -1
    divw    t2, t0, t1
    CHECK_EQ t2, 0xffffffff80000000, 27
    remw    t2, t0, t1
    CHECK_EQ t2, 0, 28

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b
