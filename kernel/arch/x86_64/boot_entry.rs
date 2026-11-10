use core::arch::global_asm;

/// PML4 entries of direct map the entry stub fills, 512 GiB each
pub const DIRECT_MAP_SLOTS: u64 = 2;
/// Physical memory reachable through the direct map before `mm::vmm` can add
/// to it
pub const DIRECT_MAP_SPAN: u64 = DIRECT_MAP_SLOTS << 39;

/// True if physical [base, base+len) is inside the entry stub's direct map
pub fn direct_mapped(base: u64, len: u64) -> bool {
    base.checked_add(len).is_some_and(|end| end <= DIRECT_MAP_SPAN)
}

// The page tables are part of the image. A bootloader keeps its own data clear
// of the image and nothing else: under UEFI GRUB puts the multiboot2
// information at 0x2000
global_asm!(r#"
.set HHDM_SLOTS, {slots}
.set BOOT_TABLE_PAGES, 3 + HHDM_SLOTS

.section .multiboot2_header, "a"
.align 8
_mb2_hdr_start:
    .long  0xe85250d6
    .long  0
    .long  (_mb2_hdr_end - _mb2_hdr_start)
    .long  -(0xe85250d6 + (_mb2_hdr_end - _mb2_hdr_start))
    .align 8
    .short 5
    .short 1
    .long  20
    .long  0
    .long  0
    .long  0

    # Information request, optional (flags=1): tag 9, the ELF section
    # headers. GRUB provides them for an ELF kernel anyway, but asking is
    # what makes it contractual - without them the kernel cannot find its
    # own .symtab, and a panic prints bare addresses
    .align 8
    .short 1
    .short 1
    .long  12
    .long  9

    .align 8
    .short 0
    .short 0
    .long  8
_mb2_hdr_end:

.section .boot_data, "a"
.align 16
_boot_gdt64:
    .quad 0x0000000000000000
    .quad 0x00AF9A000000FFFF
    .quad 0x00CF92000000FFFF
_boot_gdt64_ptr:
    .short (_boot_gdt64_ptr - _boot_gdt64 - 1)
    .long  _boot_gdt64

.section .boot_bss, "aw", @nobits
.balign 4096
_boot_pml4:
    .skip 4096
_boot_pdpt_low:
    .skip 4096
_boot_pdpt_kernel:
    .skip 4096
_boot_pdpt_hhdm:
    .skip 4096 * HHDM_SLOTS

.section .boot_text, "ax"
.code32
.global _start32
_start32:
    cli
    cld

    # ebx is the multiboot2 information and is not touched below
    movl  $_boot_pml4, %edi
    xorl  %eax, %eax
    movl  $(BOOT_TABLE_PAGES * 1024), %ecx
    rep   stosl

    movl  $(_boot_pdpt_low + 3),    _boot_pml4
    movl  $(_boot_pdpt_kernel + 3), _boot_pml4 + 511 * 8

    # Identity, 0 .. 4 GiB
    movl  $0x00000083, _boot_pdpt_low
    movl  $0x40000083, _boot_pdpt_low + 8
    movl  $0x80000083, _boot_pdpt_low + 16
    movl  $0xC0000083, _boot_pdpt_low + 24

    # Direct map: PML4[256 + n] is table n
    movl  $(_boot_pml4 + 256 * 8), %edi
    movl  $(_boot_pdpt_hhdm + 3), %eax
    movl  $HHDM_SLOTS, %ecx
.Lhhdm_slot:
    movl  %eax, (%edi)
    addl  $4096, %eax
    addl  $8, %edi
    decl  %ecx
    jnz   .Lhhdm_slot

    # One entry a gigabyte across all of them
    xorl  %ecx, %ecx
    movl  $_boot_pdpt_hhdm, %edi
.Lfill_hhdm:
    movl  %ecx, %eax
    andl  $3,   %eax
    shll  $30,  %eax
    orl   $0x83,%eax
    movl  %eax, (%edi)

    movl  %ecx, %eax
    shrl  $2,   %eax
    movl  %eax, 4(%edi)

    addl  $8,   %edi
    incl  %ecx
    cmpl  $(512 * HHDM_SLOTS), %ecx
    jb    .Lfill_hhdm

    # The image: the top 2 GiB onto physical 0
    movl  $0x00000083, _boot_pdpt_kernel + 510 * 8

    movl  %cr4, %eax
    orl   $0x20, %eax
    movl  %eax, %cr4

    movl  $_boot_pml4, %eax
    movl  %eax, %cr3

    movl  $0xC0000080, %ecx
    rdmsr
    orl   $0x100, %eax
    wrmsr

    movl  %cr0, %eax
    orl   $0x80000000, %eax
    movl  %eax, %cr0

    lgdt  _boot_gdt64_ptr

    ljmp  $0x08, $_start64_compat

.code64
_start64_compat:
    movw  $0x10, %ax
    movw  %ax,   %ds
    movw  %ax,   %es
    movw  %ax,   %ss
    xorw  %ax,   %ax
    movw  %ax,   %fs
    movw  %ax,   %gs

    movabsq $_start64_high, %rax
    jmpq   *%rax

.section .text
.code64
.global _start
_start:
_start64_high:
    movabsq $_stack_top, %rsp
    # The upper half of a register is undefined after 32-bit code, and a UEFI
    # bootloader ran 64-bit code before it
    movl    %ebx, %edi
    callq   kernel_main_grub
.Lhlt:
    hlt
    jmp .Lhlt
"#, slots = const DIRECT_MAP_SLOTS, options(att_syntax));
