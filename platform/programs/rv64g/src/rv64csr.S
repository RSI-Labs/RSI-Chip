// RV64 Zicsr coverage for the user-level ISS.

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
    li      t0, 0x5
    csrw    fflags, t0
    csrr    t1, fflags
    CHECK_EQ t1, 0x5, 1

    li      t0, 0x3
    csrw    frm, t0
    csrr    t1, frm
    CHECK_EQ t1, 0x3, 2
    csrr    t1, fcsr
    CHECK_EQ t1, 0x65, 3

    csrwi   fcsr, 0x1f
    csrr    t1, fflags
    CHECK_EQ t1, 0x1f, 4
    csrr    t1, frm
    CHECK_EQ t1, 0, 5

    li      t0, 0x1234
    csrw    mscratch, t0
    csrr    t1, mscratch
    CHECK_EQ t1, 0x1234, 6

    li      t0, 0xabc
    csrw    mtvec, t0
    csrr    t1, mtvec
    CHECK_EQ t1, 0xabc, 7

    li      t0, 0x10
    csrw    mscratch, t0
    li      t1, 0x3
    csrrs   t2, mscratch, t1
    CHECK_EQ t2, 0x10, 8
    csrr    t3, mscratch
    CHECK_EQ t3, 0x13, 9
    csrrc   t2, mscratch, t1
    CHECK_EQ t2, 0x13, 10
    csrr    t3, mscratch
    CHECK_EQ t3, 0x10, 11

    csrrwi  t2, mscratch, 0x5
    CHECK_EQ t2, 0x10, 12
    csrr    t3, mscratch
    CHECK_EQ t3, 0x5, 13

    li      t0, 8
    csrs    mstatus, t0
    csrr    t1, mstatus
    andi    t1, t1, 8
    CHECK_EQ t1, 8, 14
    csrc    mstatus, t0
    csrr    t1, mstatus
    andi    t1, t1, 8
    CHECK_EQ t1, 0, 15

    csrr    t1, misa
    li      t2, 0x8000000000000100
    and     t1, t1, t2
    CHECK_EQ t1, 0x8000000000000100, 16
    csrr    t1, mhartid
    CHECK_EQ t1, 0, 17
    csrr    t1, mepc
    CHECK_EQ t1, 0, 18
    csrr    t1, mcause
    CHECK_EQ t1, 0, 19

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b
