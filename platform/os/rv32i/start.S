// MiniOS-RV32I boot and privilege-entry stubs.
.section .text.start, "ax", @progbits
.globl _start
_start:
    li      sp, 0x00008000          // kernel stack top
    la      t0, trap_entry
    csrw    mtvec, t0
    call    kmain
1:  j       1b

// void enter_user(u32 entry, u32 stack)
.globl enter_user
enter_user:
    csrw    mepc, a0
    mv      sp, a1
    mret
