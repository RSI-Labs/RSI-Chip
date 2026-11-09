// RV64D double-precision coverage for the user-level ISS.

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

.macro CHECK_FD freg, value, id
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
    li      t0, 0x3ff8000000000000   // 1.5
    fmv.d.x ft0, t0
    li      t0, 0x4004000000000000   // 2.5
    fmv.d.x ft1, t0

    fadd.d  ft2, ft0, ft1
    CHECK_FD ft2, 0x4010000000000000, 1
    fsub.d  ft2, ft1, ft0
    CHECK_FD ft2, 0x3ff0000000000000, 2
    fmul.d  ft2, ft0, ft1
    CHECK_FD ft2, 0x400e000000000000, 3
    li      t0, 0x4008000000000000   // 3.0
    fmv.d.x ft3, t0
    fdiv.d  ft2, ft3, ft0
    CHECK_FD ft2, 0x4000000000000000, 4
    li      t0, 0x4010000000000000   // 4.0
    fmv.d.x ft3, t0
    fsqrt.d ft2, ft3
    CHECK_FD ft2, 0x4000000000000000, 5

    fmin.d  ft2, ft0, ft1
    CHECK_FD ft2, 0x3ff8000000000000, 6
    fmax.d  ft2, ft0, ft1
    CHECK_FD ft2, 0x4004000000000000, 7

    li      t0, 0xbff8000000000000   // -1.5
    fmv.d.x ft4, t0
    fsgnj.d ft2, ft0, ft4
    CHECK_FD ft2, 0xbff8000000000000, 8
    fsgnjn.d ft2, ft0, ft4
    CHECK_FD ft2, 0x3ff8000000000000, 9
    fsgnjx.d ft2, ft0, ft4
    CHECK_FD ft2, 0xbff8000000000000, 10

    feq.d   t2, ft0, ft0
    CHECK_EQ t2, 1, 11
    flt.d   t2, ft0, ft1
    CHECK_EQ t2, 1, 12
    fle.d   t2, ft1, ft0
    CHECK_EQ t2, 0, 13
    fle.d   t2, ft0, ft0
    CHECK_EQ t2, 1, 14

    fcvt.w.d t2, ft4              // -1.5 -> -1
    CHECK_EQ t2, -1, 15
    fcvt.wu.d t2, ft1             // 2.5 -> 2
    CHECK_EQ t2, 2, 16
    li      t0, 0xc010000000000000 // -4.0
    fmv.d.x ft4, t0
    fcvt.l.d t2, ft4
    CHECK_EQ t2, -4, 17
    fcvt.lu.d t2, ft1
    CHECK_EQ t2, 2, 18

    li      t0, -7
    fcvt.d.w ft4, t0
    CHECK_FD ft4, 0xc01c000000000000, 19
    li      t0, 7
    fcvt.d.wu ft4, t0
    CHECK_FD ft4, 0x401c000000000000, 20
    fcvt.d.l ft4, t0
    CHECK_FD ft4, 0x401c000000000000, 21
    fcvt.d.lu ft4, t0
    CHECK_FD ft4, 0x401c000000000000, 22

    // Cross-precision conversion
    fcvt.s.d ft4, ft1             // 2.5 -> 2.5f
    CHECK_FD ft4, 0xffffffff40200000, 23
    li      t0, 0x3fc00000        // 1.5f
    fmv.w.x ft4, t0
    fcvt.d.s ft4, ft4
    CHECK_FD ft4, 0x3ff8000000000000, 24

    // Bit moves and FSD/FLD roundtrip
    fmv.x.d t2, ft0
    CHECK_EQ t2, 0x3ff8000000000000, 25
    li      t0, 0x401c000000000000
    fmv.d.x ft4, t0
    CHECK_FD ft4, 0x401c000000000000, 26
    la      t5, scratch
    fsd     ft1, 0(t5)
    fld     ft4, 0(t5)
    CHECK_FD ft4, 0x4004000000000000, 27

    // FCLASS
    li      t0, 0x3ff0000000000000 // +1.0
    fmv.d.x ft4, t0
    fclass.d t2, ft4
    CHECK_EQ t2, 0x40, 28
    li      t0, 0xbff0000000000000 // -1.0
    fmv.d.x ft4, t0
    fclass.d t2, ft4
    CHECK_EQ t2, 0x2, 29
    li      t0, 0x7ff0000000000000 // +inf
    fmv.d.x ft4, t0
    fclass.d t2, ft4
    CHECK_EQ t2, 0x80, 30
    li      t0, 0x7ff8000000000000 // quiet NaN
    fmv.d.x ft4, t0
    fclass.d t2, ft4
    CHECK_EQ t2, 0x200, 31
    li      t0, 0x8000000000000000 // -0.0
    fmv.d.x ft4, t0
    fclass.d t2, ft4
    CHECK_EQ t2, 0x8, 32

    // Fused multiply-add family
    li      t0, 0x3ff0000000000000 // 1.0
    fmv.d.x ft3, t0
    fmadd.d  ft2, ft0, ft1, ft3
    CHECK_FD ft2, 0x4013000000000000, 33
    fmsub.d  ft2, ft0, ft1, ft3
    CHECK_FD ft2, 0x4006000000000000, 34
    fnmsub.d ft2, ft0, ft1, ft3
    CHECK_FD ft2, 0xc006000000000000, 35
    fnmadd.d ft2, ft0, ft1, ft3
    CHECK_FD ft2, 0xc013000000000000, 36

    // Conversion saturation and invalid flag
    csrw    fflags, zero
    li      t0, 0x7ff8000000000000   // quiet NaN
    fmv.d.x ft4, t0
    fcvt.l.d t2, ft4
    CHECK_EQ t2, 0x7fffffffffffffff, 38
    csrr    t2, fflags
    andi    t2, t2, 16
    CHECK_EQ t2, 16, 39

    csrw    fflags, zero
    li      t0, 0xbff8000000000000   // -1.5
    fmv.d.x ft4, t0
    fcvt.l.d t2, ft4
    CHECK_EQ t2, -1, 40
    csrr    t2, fflags
    andi    t2, t2, 1
    CHECK_EQ t2, 1, 41

    // Divide-by-zero flag (DZ = bit 3)
    csrw    fflags, zero
    li      t0, 0x3ff0000000000000 // 1.0
    fmv.d.x ft0, t0
    li      t0, 0
    fmv.d.x ft1, t0               // 0.0
    fdiv.d  ft2, ft0, ft1
    csrr    t2, fflags
    andi    t2, t2, 8
    CHECK_EQ t2, 8, 42
    csrw    fflags, zero

    li      t0, 0x10000004
    li      t1, 0
    sw      t1, 0(t0)
1:  j       1b

.section .data
.align 3
scratch:
    .dword 0
