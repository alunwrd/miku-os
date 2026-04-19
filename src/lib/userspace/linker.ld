OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS {
    . = 0x400000 + SIZEOF_HEADERS;

    .text : {
        *(.text .text.*)
    }

    .rodata ALIGN(0x1000) : {
        *(.rodata .rodata.*)
    }

    /* RELRO-protected sections - read-only after relocations */
    .got ALIGN(0x1000) : {
        *(.got .got.*)
    }

    .dynamic : {
        *(.dynamic)
    }

    /* Writable data - must be AFTER RELRO sections */
    .data ALIGN(0x1000) : {
        *(.data .data.*)
    }

    .bss ALIGN(0x1000) : {
        *(COMMON)
        *(.bss .bss.*)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment*)
    }
}
