/* The vDSO is one page of code, mapped read+execute into every process.
 *
 * The address here is a link-time base only: the kernel places the real
 * mapping wherever it has room, and userspace finds it through
 * AT_SYSINFO_EHDR. Everything inside is PC-relative, so the base it was
 * linked at need not match the base it runs at.
 *
 * The ELF header must be inside the mapping. AT_SYSINFO_EHDR promises an ELF
 * header at its address, which holds only if the header is part of a PT_LOAD
 * (`FILEHDR PHDRS` on the text segment below). Without it the first LOAD
 * starts after the headers and the address points at whatever section landed
 * at offset zero.
 *
 * It must also be one segment. The kernel maps this page by hand rather than
 * through the ELF loader, so a second segment would be dropped. Everything is
 * folded into a single read-execute PT_LOAD with no writable data: the vDSO's
 * variables live in the vvar page the kernel owns.
 */
ENTRY(0)

PHDRS {
    text    PT_LOAD    FILEHDR PHDRS FLAGS(5);   /* R E */
    dynamic PT_DYNAMIC          FLAGS(4);        /* R   */
    note    PT_NOTE             FLAGS(4);        /* R   */
}

SECTIONS {
    . = SIZEOF_HEADERS;

    .hash          : { *(.hash) }            :text
    .gnu.hash      : { *(.gnu.hash) }
    .dynsym        : { *(.dynsym) }
    .dynstr        : { *(.dynstr) }
    .gnu.version   : { *(.gnu.version) }
    .gnu.version_d : { *(.gnu.version_d) }
    .note          : { *(.note .note.*) }    :text :note
    .dynamic       : { *(.dynamic) }         :text :dynamic
    .rodata        : { *(.rodata .rodata.*) } :text
    .text          : { *(.text .text.*) }    :text

    /* Only so the linker has somewhere to put an entry it decides it needs.
     * Anything landing here is a bug in the vDSO source: nothing relocates
     * this image, so a relocation in it is never applied */
    .rela.dyn      : { *(.rela.dyn) }
    .got           : { *(.got) }
    .got.plt       : { *(.got.plt) }

    /* No writable data. This mapping is shared by every process, so a write
     * to a variable here would land in all of them */
    /DISCARD/ : {
        *(.data .data.* .bss .bss.* COMMON)
        *(.comment*) *(.eh_frame*)
    }
}
