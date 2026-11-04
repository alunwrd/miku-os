#ifndef _SYS_RESOURCE_H
#define _SYS_RESOURCE_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/resource.h>

typedef uint64_t rlim_t;
#define RLIM_INFINITY (~(rlim_t)0)
struct rlimit { rlim_t rlim_cur, rlim_max; };

__MIKU_BEGIN
int getrlimit(int, struct rlimit *);
int setrlimit(int, const struct rlimit *);
__MIKU_END

#endif
