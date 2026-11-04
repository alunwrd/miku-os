/* No time zones: MikuOS has no zone database, so local time is UTC */
#ifndef _TIME_H
#define _TIME_H

#include <_miku.h>
#include <stddef.h>
#include <sys/types.h>
#include <bits/time.h>

#define CLOCKS_PER_SEC 1000000L
#define TIMER_ABSTIME 1

struct timespec { time_t tv_sec; long tv_nsec; };

struct tm {
    int tm_sec, tm_min, tm_hour, tm_mday, tm_mon, tm_year, tm_wday, tm_yday, tm_isdst;
};

__MIKU_BEGIN
time_t time(time_t *);
clock_t clock(void);
int clock_gettime(clockid_t, struct timespec *);
int clock_getres(clockid_t, struct timespec *);
int nanosleep(const struct timespec *, struct timespec *);
struct tm *gmtime_r(const time_t *, struct tm *);
struct tm *localtime_r(const time_t *, struct tm *);
struct tm *gmtime(const time_t *);
struct tm *localtime(const time_t *);
time_t mktime(struct tm *);
time_t timegm(struct tm *);
double difftime(time_t, time_t);
size_t strftime(char *, size_t, const char *, const struct tm *);
__MIKU_END

#endif
