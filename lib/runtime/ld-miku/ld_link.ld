ENTRY(_ldstart)

SECTIONS {
    . = 0x7F0000000000;

    .text : {
        *(.text._ldstart)
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
    }

    /* The build-id, in its own section so lld puts a PT_NOTE over it: that
     * makes it visible to the kernel's note walk, and so to a crash report
     * inside the linker itself */
    .note ALIGN(4) : {
        *(.note.gnu.build-id)
    }

    . = ALIGN(4096);

    .data.rel.ro : { *(.data.rel.ro .data.rel.ro.*) }
    .dynamic     : { *(.dynamic) }

    . = ALIGN(4096);

    .data : { *(.data .data.*) }

    . = ALIGN(4096);

    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }

    .rela.dyn : { *(.rela.dyn) }
    .rela.plt : { *(.rela.plt) }

    /DISCARD/ : {
        *(.comment*) *(.eh_frame*) *(.gnu.version*)
    }
}
