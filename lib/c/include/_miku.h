/* Shared by every MikuOS C header: the linkage and attribute spellings.
   Not for programs to include (dev/c-architecture.md) */
#ifndef _MIKU_H
#define _MIKU_H

#ifdef __cplusplus
#define __MIKU_BEGIN extern "C" {
#define __MIKU_END }
#else
#define __MIKU_BEGIN
#define __MIKU_END
#endif

#define __miku_noreturn __attribute__((__noreturn__))
#define __miku_printf(f, a) __attribute__((__format__(__printf__, f, a)))

#endif
