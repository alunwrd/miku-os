/* A non-local return (lib/runtime/libmiku/libc/setjmp.rs)
 *
 * Eight words: rbx, rbp, r12-r15, the stack pointer and the return address.
 * The signal mask is not saved, and there is no sigsetjmp. */
#ifndef _SETJMP_H
#define _SETJMP_H

typedef long jmp_buf[8];

int setjmp(jmp_buf env);
_Noreturn void longjmp(jmp_buf env, int val);

#endif
