#ifndef _FCNTL_H
#define _FCNTL_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/fcntl.h>

/* Accepted and meaningless: there is no large-file split on MikuOS */
#define O_LARGEFILE 0

struct flock {
    short l_type;
    short l_whence;
    off_t l_start;
    off_t l_len;
    pid_t l_pid;
};

__MIKU_BEGIN
int open(const char *, int, ...);
int creat(const char *, mode_t);
int fcntl(int, int, ...);
int faccessat(int, const char *, int, int);
int posix_fadvise(int, off_t, off_t, int);
int posix_fallocate(int, off_t, off_t);
__MIKU_END

#endif
