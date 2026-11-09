// Exit through ECALL with mtvec == 0 to exercise the RTL trap path.
.section .text
.globl _start
_start:
    li      a0, 7
    li      a7, 93
    ecall
1:  j       1b
