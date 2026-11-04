#ifndef _POLL_H
#define _POLL_H

#include <_miku.h>
#include <bits/poll.h>

typedef unsigned long nfds_t;
struct pollfd { int fd; short events; short revents; };

__MIKU_BEGIN
int poll(struct pollfd *, nfds_t, int);
__MIKU_END

#endif
