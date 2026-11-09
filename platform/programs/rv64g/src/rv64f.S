// RV64F single-precision coverage for the user-level ISS.
//
// FP results are compared as exact NaN-boxed bit patterns after an FSD, so
// both the arithmetic result and the required RV64 boxing are checked.

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

.macro CHECK_FS freg, value, id
    la      t5, scratch
    fsd     \freg, 0(t5)
    ld      t6, 0(t5)
    li      t4, \value
    beq     t6, t4, 9f
    FAIL    \id
9:
.endm

.section .text
.globl _start
_start:
    li      t0, 0x3fc00000        // 1.5f
    fmv.w.x ft0, t0
    li      t0, 0x40200000        // 2.5f
    fmv.w.x ft1, t0

    fadd.s  ft2, ft0, ft1
    CHECK_FS ft2, 0xffffffff40800000, 1
    fsub.s  ft2, ft1, ft0
    CHECK_FS ft2, 0xffffffff3f800000, 2
    fmul.s  ft2, ft0, ft1
    CHECK_FS ft2, 0xffffffff40700000, 3
    li      t0, 0x40400000        // 3.0f
    fmv.w.x ft3, t0
    fdiv.s  ft2, ft3, ft0
    CHECK_FS ft2, 0xffffffff40000000, 4
    li      t0, 0x40800000        // 4.0f
    fmv.w.x ft3, t0
    fsqrt.s ft2, ft3
    CHECK_FS ft2, 0xffffffff40000000, 5

    fmin.s  ft2, ft0, ft1
    CHECK_FS ft2, 0xffffffff3fc00000, 6
    fmax.s  ft2, ft0, ft1
    CHECK_FS ft2, 0xffffffff40200000, 7

    li      t0, 0xbfc00000        // -1.5f
    fmv.w.x ft4, t0
    fsgnj.s ft2, ft0, ft4
    CHECK_FS ft2, 0xffffffffbfc00000, 8
    fsgnjn.s ft2, ft0, ft4
    CHECK_FS ft2, 0xffffffff3fc00000, 9
    fsgnjx.s ft2, ft0, ft4
    CHECK_FS ft2, 0xffffffffbfc00000, 10

    feq.s   t2, ft0, ft0
    CHECK_EQ t2, 1, 11
    flt.s   t2, ft0, ft1
    CHECK_EQ t2, 1, 12
    fle.s   t2, ft1, ft0
    CHECK_EQ t2, 0, 13
    fle.s   t2, ft0, ft0
    CHECK_EQ t2, 1, 14

    // Conversions from FP to integer
    fcvt.w.s t2, ft4             // -1.5 -> -1
    CHECK_EQ t2, -1, 15
    fcvt.wu.s t2, ft1            // 2.5 -> 2
    CHECK_EQ t2, 2, 16
    li      t0, 0xc0800000       // -4.0f
    fmv.w.x ft4, t0
    fcvt.l.s t2, ft4
    CHECK_EQ t2, -4, 17
    fcvt.lu.s t2, ft1
    CHECK_EQ t2, 2, 18

    // Conversions from integer to FP
    li      t0, -7
    fcvt.s.w ft4, t0
    CHECK_FS ft4, 0xffffffffc0e00000, 19
    li      t0, 7
    fcvt.s.wu ft4, t0
    CHECK_FS ft4, 0xffffffff40e00000, 20
    fcvt.s.l ft4, t0
    CHECK_FS ft4, 0xffffffff40e00000, 21
    fcvt.s.lu ft4, t0
    CHECK_FS ft4, 0xffffffff40e00000, 22

    // Bit moves
    li      t0, 0x3fc00000
    fmv.w.x ft4, t0
    fmv.x.w t2, ft4
    CHECK_EQ t2, 0x3fc00000, 23
    li      t0, 0xbfc00000
    fmv.w.x ft4, t0
    CHECK_FS ft4, 0xffffffffbfc00000, 24

    // FCLASS
    li      t0, 0x3f800000       // +1.0
    fmv.w.x ft4, t0
    fclass.s t2, ft4
    CHECK_EQ t2, 0x40, 25
    li      t0, 0xbf800000       // -1.0
    fmv.w.x ft4, t0
    fclass.s t2, ft4
    CHECK_EQ t2, 0x2, 26
    li      t0, 0x7f800000       // +inf
    fmv.w.x ft4, t0
    fclass.s t2, ft4
    CHECK_EQ t2, 0x80, 27
    li      t0, 0x7fc00000       // quiet NaN
    fmv.w.x ft4, t0
    fclass.s t2, ft4
    CHECK_EQ t2, 0x200, 28
    li      t0, 0x80000000       // -0.0
    fmv.w.x ft4, t0
    fclass.s t2, ft4
    CHECK_EQ t2, 0x8, 29

    // Fused multiply-add family
    li      t0, 0x3f800000       // 1.0f
    fmv.w.x ft3, t0
    fmadd.s  ft2, ft0, ft1, ft3
    CHECK_FS ft2, 0xffffffff40980000, 30
    fmsub.s  ft2, ft0, ft1, ft3
    CHECK_FS ft2, 0xffffffff40300000, 31
    fnmsub.s ft2, ft0, ft1, ft3
    CHECK_FS ft2, 0xffffffffc0300000, 32
    fnmadd.s ft2, ft0, ft1, ft3
    CHECK_FS ft2, 0xffffffffc0980000, 33

    // Conversion saturation and invalid/inexact flags
    csrw    fflags, zero
    li      t0, 0x7fc00000       // quiet NaN
    fmv.w.x ft4, t0
    fcvt.w.s t2, ft4
    CHECK_EQ t2, 0x7fffffff, 35
    csrr    t2, fflags
    andi    t2, t2, 16
    CHECK_EQ t2, 16, 36

    csrw    fflags, zero
    li      t0, 0xbf800000       // -1.0
    fmv.w.x ft4, t0
    fcvt.wu.s t2, ft4
    CHECK_EQ t2, 0, 37
    csrr    t2, fflags
    andi    t2, t2, 16
    CHECK_EQ t2, 16, 38

    csrw    fflags, zero
    li      t0, 0x3fc00000       // 1.5
    fmv.w.x ft4, t0
    fcvt.w.s t2, ft4
    CHECK_EQ t2, 1, 39
    csrr    t2, fflags
    andi    t2, t2, 1
    CHECK_EQ t2, 1, 40

    // Divide-by-zero flag (DZ = bit 3)
    csrw    fflags, zero
    li      t0, 0x3f800000       // 1.0f
    fmv.w.x ft0, t0
    li      t0, 0
    fmv.w.x ft1, t0              // 0.0f
    fdiv.s  ft2, ft0, ft1
    csrr    t2, fflags
    andi    t2, t2, 8
    CHECK_EQ t2, 8, 41
    csrw    fflags, zero

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b

.section .data
.align 3
scratch:
    .dword 0
