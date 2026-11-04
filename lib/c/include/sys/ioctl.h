/* ioctl reaches terminals in the kernel; FIONBIO is answered by libc_miku.so
   as the fcntl it is (lib/libmiku/libc.rs) */
#ifndef _SYS_IOCTL_H
#define _SYS_IOCTL_H

#include <_miku.h>
#include <bits/ioctl.h>

/* The size a terminal says it is: rows and columns; the pixel sizes are
   reported as zero */
struct winsize {
    unsigned short ws_row;
    unsigned short ws_col;
    unsigned short ws_xpixel;
    unsigned short ws_ypixel;
};

__MIKU_BEGIN
int ioctl(int, unsigned long, ...);
__MIKU_END

#endif
