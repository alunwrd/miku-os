OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start32)

PHYS_BOOT   = 0x100000;
PHYS_KERNEL = 0x200000;
KBASE       = 0xFFFFFFFF80000000;
KOFFSET     = KBASE;

PHDRS {
    boot    PT_LOAD FLAGS(0x5);
    text    PT_LOAD FLAGS(0x5);
    rodata  PT_LOAD FLAGS(0x4);
    data    PT_LOAD FLAGS(0x6);
}

SECTIONS {
    . = PHYS_BOOT;

    .multiboot2_header : AT(PHYS_BOOT) {
        KEEP(*(.multiboot2_header))
    } :boot

    .boot_text : {
        KEEP(*(.boot_text))
    } :boot

    .boot_data : {
        KEEP(*(.boot_data))
    } :boot

    .boot_bss (NOLOAD) : {
        KEEP(*(.boot_bss))
    } :boot

    . = KBASE + PHYS_KERNEL;

    .text : AT(PHYS_KERNEL) {
        *(.text .text.*)
        . = ALIGN(0x1000);
        _ap_trampoline_link_start = .;
        KEEP(*(.ap_trampoline))
        _ap_trampoline_link_end = .;
    } :text

    .rodata ALIGN(0x1000) : AT(ADDR(.rodata) - KOFFSET) {
        *(.rodata .rodata.*)
    } :rodata

    .data ALIGN(0x1000) : AT(ADDR(.data) - KOFFSET) {
        *(.data .data.*)
        *(.got .got.*)
    } :data

    .bss ALIGN(0x1000) : AT(ADDR(.bss) - KOFFSET) {
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(0x1000);
        _stack_bottom = .;
        . += 0x80000;
        _stack_top = .;
    } :data

    . = ALIGN(0x1000);
    _kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
