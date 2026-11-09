ENTRY(_start)
SECTIONS
{
  . = 0x0;
  .text : { *(.text*) }
  . = ALIGN(8);
  .data : { *(.data*) *(.rodata*) *(.bss*) *(COMMON) }
}
