// RV64F non-arithmetic subset for the RTL FP milestone.
// Covers FP register moves, sign injection, min/max, compares, and FCLASS.
// Arithmetic (FADD/FSUB/FMUL/FDIV/FSQRT/FMA) is exercised by rv64f.bin on the ISS.

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
    li      t0, 0x3fc00000       // 1.5f
    fmv.w.x f0, t0
    li      t0, 0xbfc00000       // -1.5f
    fmv.w.x f1, t0
    li      t0, 0x40200000       // 2.5f
    fmv.w.x f2, t0

    fmv.x.w t1, f0
    CHECK_EQ t1, 0x3fc00000, 1
    fmv.x.w t1, f1
    CHECK_EQ t1, 0xffffffffbfc00000, 2

    fsgnj.s  f3, f0, f1
    fmv.x.w  t1, f3
    CHECK_EQ t1, 0xffffffffbfc00000, 3
    fsgnjn.s f3, f0, f1
    fmv.x.w  t1, f3
    CHECK_EQ t1, 0x3fc00000, 4
    fsgnjx.s f3, f0, f1
    fmv.x.w  t1, f3
    CHECK_EQ t1, 0xffffffffbfc00000, 5

    fmin.s   f3, f0, f2
    fmv.x.w  t1, f3
    CHECK_EQ t1, 0x3fc00000, 6
    fmax.s   f3, f0, f2
    fmv.x.w  t1, f3
    CHECK_EQ t1, 0x40200000, 7

    feq.s    t1, f0, f0
    CHECK_EQ t1, 1, 8
    flt.s    t1, f0, f2
    CHECK_EQ t1, 1, 9
    fle.s    t1, f2, f0
    CHECK_EQ t1, 0, 10
    fle.s    t1, f0, f0
    CHECK_EQ t1, 1, 11

    fclass.s t1, f0
    CHECK_EQ t1, 0x40, 12
    fclass.s t1, f1
    CHECK_EQ t1, 0x2, 13
    li       t0, 0x7f800000      // +inf
    fmv.w.x  f3, t0
    fclass.s t1, f3
    CHECK_EQ t1, 0x80, 14
    li       t0, 0x7fc00000      // quiet NaN
    fmv.w.x  f3, t0
    fclass.s t1, f3
    CHECK_EQ t1, 0x200, 15
    li       t0, 0x80000000      // -0.0
    fmv.w.x  f3, t0
    fclass.s t1, f3
    CHECK_EQ t1, 0x8, 16

    li       t0, 0x7fc00000      // NaN compares are all false
    fmv.w.x  f4, t0
    feq.s    t1, f4, f4
    CHECK_EQ t1, 0, 17
    flt.s    t1, f4, f0
    CHECK_EQ t1, 0, 18
    fle.s    t1, f4, f0
    CHECK_EQ t1, 0, 19

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b
