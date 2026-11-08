ENTRY(_start)
SECTIONS {
  . = 0x00000000;
  .text : {
    *(.text.start)
    *(.text*)
  }
  .rodata : { *(.rodata*) *(.srodata*) }
  .data : { *(.data*) *(.sdata*) }
  .bss : { *(.bss*) *(.sbss*) *(COMMON) }

  . = 0x00001000;
  .user : {
    *(.usercode)
    *(.userdata)
  }
}
