// setjmp and longjmp for C
//
// FreeType's sfnt driver validates a font's character maps and unwinds by
// `longjmp` on a broken table (`ft_setjmp` in ttcmap.c). Assembly, since
// `setjmp` returns twice.
//
// `jmp_buf` (lib/sdk/c/include/setjmp.h) is eight words: the x86_64 SysV
// callee-saved registers rbx, rbp, r12 to r15, then the stack pointer as
// `setjmp`'s caller has it after return, then the return address. `longjmp`
// restores them and jumps, with its value as `setjmp`'s result, or 1 for 0, per
// C. The signal mask is not kept. musl's (src/setjmp/x86_64), in Intel syntax.

core::arch::global_asm!(
    ".global setjmp",
    ".type setjmp, @function",
    "setjmp:",
    "mov [rdi], rbx",
    "mov [rdi + 8], rbp",
    "mov [rdi + 16], r12",
    "mov [rdi + 24], r13",
    "mov [rdi + 32], r14",
    "mov [rdi + 40], r15",
    "lea rdx, [rsp + 8]",
    "mov [rdi + 48], rdx",
    "mov rdx, [rsp]",
    "mov [rdi + 56], rdx",
    "xor eax, eax",
    "ret",
    ".size setjmp, . - setjmp",
);

core::arch::global_asm!(
    ".global longjmp",
    ".type longjmp, @function",
    "longjmp:",
    // eax = esi, or 1 when esi is 0: the carry of `1 > esi` added in
    "xor eax, eax",
    "cmp esi, 1",
    "adc eax, esi",
    "mov rbx, [rdi]",
    "mov rbp, [rdi + 8]",
    "mov r12, [rdi + 16]",
    "mov r13, [rdi + 24]",
    "mov r14, [rdi + 32]",
    "mov r15, [rdi + 40]",
    "mov rsp, [rdi + 48]",
    "jmp qword ptr [rdi + 56]",
    ".size longjmp, . - longjmp",
);
