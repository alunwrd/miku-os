#ifndef _SYS_TIME_H
#define _SYS_TIME_H

#include <_miku.h>
#include <sys/types.h>
#include <time.h>

struct timeval { time_t tv_sec; suseconds_t tv_usec; };
struct timezone { int tz_minuteswest, tz_dsttime; };

__MIKU_BEGIN
int gettimeofday(struct timeval *, void *);
int utimes(const char *, const struct timeval[2]);
__MIKU_END

#endif
