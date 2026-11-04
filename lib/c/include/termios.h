/* Linux's struct termios, translated by libc_miku onto the kernel's own
   (lib/libmiku/c/termios.rs): ICANON, ECHO, ISIG, TOSTOP, the control
   characters, VMIN and VTIME act; speeds, parity and output processing are
   accepted and not reported back: there is no serial line under a pty */
#ifndef _TERMIOS_H
#define _TERMIOS_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/termios.h>

typedef unsigned char cc_t;
typedef unsigned int speed_t;
typedef unsigned int tcflag_t;

struct termios {
    tcflag_t c_iflag;
    tcflag_t c_oflag;
    tcflag_t c_cflag;
    tcflag_t c_lflag;
    cc_t c_line;
    cc_t c_cc[NCCS];
    speed_t c_ispeed;
    speed_t c_ospeed;
};

__MIKU_BEGIN
int tcgetattr(int, struct termios *);
int tcsetattr(int, int, const struct termios *);
void cfmakeraw(struct termios *);
speed_t cfgetispeed(const struct termios *);
speed_t cfgetospeed(const struct termios *);
int cfsetispeed(struct termios *, speed_t);
int cfsetospeed(struct termios *, speed_t);
int cfsetspeed(struct termios *, speed_t);
int tcdrain(int);
int tcflush(int, int);
int tcflow(int, int);
int tcsendbreak(int, int);
__MIKU_END

#endif
