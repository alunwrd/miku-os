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

    /* The build-id note. Needs an output section of its own so it lands in
     * a read-only PT_LOAD with a PT_NOTE header over it: the kernel's PT_NOTE
     * walk looks for that header, and without it the id is in the file but
     * invisible to a crash report or a core dump */
    .note ALIGN(4) : {
        *(.note.gnu.build-id)
        *(.note .note.*)
    }

    /* RELRO-protected sections: read-only after relocations */
    .got ALIGN(0x1000) : {
        *(.got .got.*)
    }

    .dynamic : {
        *(.dynamic)
    }

    /* Writable data, after the RELRO sections */
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
