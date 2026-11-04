/* A terminal and both its ends (the master from /dev/ptmx, the slave
   /dev/ptsN), and making the slave a process's terminal. lib/libmiku/c/pty.rs */
#ifndef _PTY_H
#define _PTY_H

#include <_miku.h>
#include <termios.h>
#include <sys/ioctl.h>

__MIKU_BEGIN
int openpty(int *, int *, char *, const struct termios *, const struct winsize *);
int login_tty(int);
__MIKU_END

#endif
