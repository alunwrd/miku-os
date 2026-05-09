use core::arch::global_asm;

global_asm!(r#"
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

.section .boot_text, "ax"
.code32
.global _start32
_start32:
    cli
    cld

    movl  $0x1000, %edi
    xorl  %eax, %eax
    movl  $4096, %ecx
    rep   stosl

    movl  $0x2003, 0x1000
    movl  $0x3003, 0x1800
    movl  $0x4003, 0x1FF8

    movl  $0x00000083, 0x2000
    movl  $0,          0x2004
    movl  $0x40000083, 0x2008
    movl  $0,          0x200C
    movl  $0x80000083, 0x2010
    movl  $0,          0x2014
    movl  $0xC0000083, 0x2018
    movl  $0,          0x201C

    xorl  %ecx, %ecx
    movl  $0x3000, %edi

.Lfill_hhdm:
    cmpl  $512, %ecx
    jge   .Lfill_hhdm_done

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
    jmp   .Lfill_hhdm

.Lfill_hhdm_done:
    movl  $0x00000083, 0x4FF0
    movl  $0,          0x4FF4

    movl  %cr4, %eax
    orl   $0x20, %eax
    movl  %eax, %cr4

    movl  $0x1000, %eax
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

    movl  $0x5000, %esp

    movabsq $_start64_high, %rax
    jmpq   *%rax

.section .text
.code64
.global _start
_start:
_start64_high:
    movabsq $_stack_top, %rsp
    movq    %rbx, %rdi
    callq   kernel_main_grub
.Lhlt:
    hlt
    jmp .Lhlt
"#, options(att_syntax));
