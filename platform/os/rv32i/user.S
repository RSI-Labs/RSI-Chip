// User asm program for MiniOS-RV32I.
// Prints a banner, computes 1+2+...+10, prints the result, exits.
.section .usercode, "ax", @progbits
.globl user_main
user_main:
    // write(1, banner, banner_end - banner)
    la      a1, banner
    li      a2, banner_end - banner - 1
    li      a0, 1
    li      a7, 64
    ecall

    // sum = 1 + 2 + ... + 10
    li      t0, 0
    li      t1, 1
1:
    add     t0, t0, t1
    addi    t1, t1, 1
    li      t2, 11
    blt     t1, t2, 1b

    // write(1, "sum(1..10) = ", ...)
    la      a1, sum_label
    li      a2, sum_label_end - sum_label - 1
    li      a0, 1
    li      a7, 64
    ecall

    // print_decimal(sum)
    mv      a0, t0
    li      a7, 1000
    ecall

    // write(1, "\n", 1)
    la      a1, newline
    li      a2, 1
    li      a0, 1
    li      a7, 64
    ecall

    // exit(0)
    li      a0, 0
    li      a7, 93
    ecall
1:  j       1b

.section .userdata, "a", @progbits
banner:
    .asciz "user-asm: hello from the RV32I user program\n"
banner_end:
sum_label:
    .asciz "user-asm: sum(1..10) = "
sum_label_end:
newline:
    .byte 10
