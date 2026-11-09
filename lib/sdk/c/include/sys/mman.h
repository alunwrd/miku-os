#ifndef _SYS_MMAN_H
#define _SYS_MMAN_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/mman.h>

#define MAP_FAILED ((void *)-1)
#define MS_ASYNC 1
#define MS_SYNC 4
#define MS_INVALIDATE 2

__MIKU_BEGIN
void *mmap(void *, size_t, int, int, int, off_t);
int munmap(void *, size_t);
int mprotect(void *, size_t, int);
int msync(void *, size_t, int);
int madvise(void *, size_t, int);
__MIKU_END

#endif
