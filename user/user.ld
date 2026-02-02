OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
  . = 0x0;

  .text ALIGN(4096) : {
    *(.text.entry)
    *(.text .text.*)
  }

  .rodata ALIGN(4096) : {
    *(.rodata .rodata.*)
  }

  .data ALIGN(4096) : {
    *(.data .data.*)
    *(.sdata .sdata.*)
  }

  .bss ALIGN(4096) : {
    *(.sbss .sbss.*)
    *(.bss .bss.*)
  }

  /DISCARD/ : {
    *(.eh_frame)
  }
}
