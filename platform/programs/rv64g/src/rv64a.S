// RV64A atomic and reservation coverage for the user-level ISS.

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
    la      t0, data

    // LR / SC success
    li      t1, 0x1122334455667788
    sd      t1, 0(t0)
    lr.d    t2, (t0)
    CHECK_EQ t2, 0x1122334455667788, 1
    li      t3, 0x99
    sc.d    t4, t3, (t0)
    CHECK_EQ t4, 0, 2
    ld      t5, 0(t0)
    CHECK_EQ t5, 0x99, 3

    // SC without a reservation fails
    sc.d    t4, t3, (t0)
    CHECK_EQ t4, 1, 4

    // AMOADD.D
    li      t1, 0x100
    sd      t1, 0(t0)
    li      t2, 0x23
    amoadd.d t3, t2, (t0)
    CHECK_EQ t3, 0x100, 5
    ld      t4, 0(t0)
    CHECK_EQ t4, 0x123, 6

    // AMOSWAP.D
    li       t2, 0x23
    amoswap.d t3, t2, (t0)
    CHECK_EQ t3, 0x123, 7
    ld       t4, 0(t0)
    CHECK_EQ t4, 0x23, 8

    // AMOXOR.D
    li       t1, 0xf0
    sd       t1, 0(t0)
    li       t2, 0x0f
    amoxor.d t3, t2, (t0)
    CHECK_EQ t3, 0xf0, 9
    ld       t4, 0(t0)
    CHECK_EQ t4, 0xff, 10

    // AMOAND.D
    li       t2, 0x3c
    amoand.d t3, t2, (t0)
    CHECK_EQ t3, 0xff, 11
    ld       t4, 0(t0)
    CHECK_EQ t4, 0x3c, 12

    // AMOOR.D
    li       t1, 0x30
    sd       t1, 0(t0)
    li       t2, 0x0f
    amoor.d  t3, t2, (t0)
    CHECK_EQ t3, 0x30, 13
    ld       t4, 0(t0)
    CHECK_EQ t4, 0x3f, 14

    // AMOMIN.D / AMOMAX.D
    li       t1, -5
    sd       t1, 0(t0)
    li       t2, 3
    amomin.d t3, t2, (t0)
    CHECK_EQ t3, -5, 15
    ld       t4, 0(t0)
    CHECK_EQ t4, -5, 16
    amomax.d t3, t2, (t0)
    CHECK_EQ t3, -5, 17
    ld       t4, 0(t0)
    CHECK_EQ t4, 3, 18

    // AMOMINU.D / AMOMAXU.D
    li       t1, -1
    sd       t1, 0(t0)
    li       t2, 1
    amominu.d t3, t2, (t0)
    CHECK_EQ t3, -1, 19
    ld       t4, 0(t0)
    CHECK_EQ t4, 1, 20

    li       t1, 1
    sd       t1, 0(t0)
    li       t2, -1
    amomaxu.d t3, t2, (t0)
    CHECK_EQ t3, 1, 21
    ld       t4, 0(t0)
    CHECK_EQ t4, -1, 22

    // 32-bit AMOs sign-extend their old-value result
    li       t1, 0xffffffff80000000
    sd       t1, 0(t0)
    li       t2, 0x10
    amoadd.w t3, t2, (t0)
    CHECK_EQ t3, 0xffffffff80000000, 23
    lw       t4, 0(t0)
    CHECK_EQ t4, 0xffffffff80000010, 24

    li       t1, 0x12345678
    sd       t1, 0(t0)
    li       t2, -1
    amoswap.w t3, t2, (t0)
    CHECK_EQ t3, 0x12345678, 25
    lw       t4, 0(t0)
    CHECK_EQ t4, -1, 26

    li       t1, -1
    sd       t1, 0(t0)
    li       t2, 5
    amomin.w t3, t2, (t0)
    CHECK_EQ t3, -1, 27
    li       t2, 1
    amomaxu.w t3, t2, (t0)
    CHECK_EQ t3, -1, 28

    // LR.W sign-extends the loaded word
    li       t1, 0xffffffff80000000
    sd       t1, 0(t0)
    lr.w     t3, (t0)
    CHECK_EQ t3, 0xffffffff80000000, 29

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b

.section .data
.align 3
data:
    .dword 0
