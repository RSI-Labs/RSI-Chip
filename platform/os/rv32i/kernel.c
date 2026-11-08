// MiniOS-RV32I: a small unikernel for the RV32I full core.
//
// Devices:
//   0x10000000  console TX (byte)
//   0x10000004  shutdown (word; low bits are the exit status)
//   0x10000008  console status (bit 0 = TX ready)
//
// System call ABI (Linux-like):
//   a7=64  write(fd=a0, buf=a1, len=a2) -> a0=len
//   a7=93  exit(code=a0)
//   a7=1000 print_decimal(value=a0) -> a0=0

typedef unsigned int u32;

#define MMIO_UART_TX   (*(volatile unsigned char *)0x10000000u)
#define MMIO_SHUTDOWN  (*(volatile unsigned int  *)0x10000004u)
#define MMIO_UART_STAT (*(volatile unsigned int  *)0x10000008u)

static void putc(char c) {
    while ((MMIO_UART_STAT & 1u) == 0) {
    }
    MMIO_UART_TX = (unsigned char)c;
}

static void puts(const char *s) {
    while (*s) {
        putc(*s++);
    }
}

static const u32 pow10[10] = {
    1000000000u, 100000000u, 10000000u, 1000000u, 100000u,
    10000u, 1000u, 100u, 10u, 1u,
};

static void putu(u32 value) {
    int started = 0;
    for (int i = 0; i < 10; i++) {
        u32 digit = 0;
        while (value >= pow10[i]) {
            value -= pow10[i];
            digit++;
        }
        if (digit != 0 || started || i == 9) {
            putc((char)('0' + digit));
            started = 1;
        }
    }
}

static void shutdown(u32 code) {
    puts("MiniOS: shutdown code ");
    putu(code);
    puts("\n");
    MMIO_SHUTDOWN = code;
    for (;;) {
    }
}

// Called from trap.S with the saved register frame.
__attribute__((used)) u32 trap_handler(u32 *frame, u32 cause, u32 epc) {
    if (cause == 11u) { // environment call from M-mode
        u32 number = frame[17]; // a7
        u32 arg0 = frame[10];   // a0
        if (number == 64u) {    // write
            u32 length = frame[12]; // a2
            volatile unsigned char *buffer =
                (volatile unsigned char *)frame[11]; // a1
            for (u32 i = 0; i < length; i++) {
                putc((char)buffer[i]);
            }
            frame[10] = length;
            return epc + 4;
        }
        if (number == 93u || number == 94u) { // exit / exit_group
            shutdown(arg0);
        }
        if (number == 1000u) {
            putu(arg0);
            frame[10] = 0;
            return epc + 4;
        }
        puts("MiniOS: unknown syscall ");
        putu(number);
        puts("\n");
        frame[10] = (u32)-38; // -ENOSYS
        return epc + 4;
    }

    puts("MiniOS: unexpected trap cause ");
    putu(cause);
    puts(" at epc ");
    putu(epc);
    puts("\n");
    shutdown(0xdead);
    return epc;
}

// Implemented in start.S.
extern void enter_user(u32 entry, u32 stack);

__attribute__((used)) void kmain(void) {
    puts("MiniOS-RV32I: boot\n");
    puts("MiniOS-RV32I: console ready\n");
    puts("MiniOS-RV32I: launching user asm program at 0x1000\n");
    enter_user(0x1000u, 0x00009000u);
    puts("MiniOS: enter_user returned unexpectedly\n");
    shutdown(0xbad0);
}
