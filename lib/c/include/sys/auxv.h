#ifndef _SYS_AUXV_H
#define _SYS_AUXV_H

#include <_miku.h>

/* The auxiliary vector's keys, as the kernel writes them
   (kernel/process/elf.rs): the ELF ABI's own numbers, docs/MikuOS_ABI.md 4.4 */
#define AT_NULL           0
#define AT_PHDR           3
#define AT_PHENT          4
#define AT_PHNUM          5
#define AT_PAGESZ         6
#define AT_BASE           7
#define AT_FLAGS          8
#define AT_ENTRY          9
#define AT_UID           11
#define AT_EUID          12
#define AT_GID           13
#define AT_EGID          14
#define AT_PLATFORM      15
#define AT_HWCAP         16
#define AT_CLKTCK        17
#define AT_SECURE        23
#define AT_RANDOM        25
#define AT_HWCAP2        26
#define AT_EXECFN        31
#define AT_SYSINFO_EHDR  33
#define AT_MINSIGSTKSZ   51

__MIKU_BEGIN
/* The value of a key, or 0 when the vector has none */
unsigned long getauxval(unsigned long);
__MIKU_END

#endif
