/* select, over poll(76): MikuOS has no select of its own. fd_set is 1024
   bits, as everywhere */
#ifndef _SYS_SELECT_H
#define _SYS_SELECT_H

#include <_miku.h>
#include <sys/types.h>
#include <sys/time.h>

#define FD_SETSIZE 1024
typedef struct { uint64_t fds_bits[FD_SETSIZE / 64]; } fd_set;

#define FD_ZERO(s)   __builtin_memset((s), 0, sizeof(fd_set))
#define FD_SET(d, s) ((s)->fds_bits[(d) / 64] |= (1ULL << ((d) % 64)))
#define FD_CLR(d, s) ((s)->fds_bits[(d) / 64] &= ~(1ULL << ((d) % 64)))
#define FD_ISSET(d, s) (((s)->fds_bits[(d) / 64] >> ((d) % 64)) & 1)

__MIKU_BEGIN
int select(int, fd_set *, fd_set *, fd_set *, struct timeval *);
__MIKU_END

#endif
