/* The compiler's limits.h, and the POSIX ones it does not know */
#ifndef _MIKU_LIMITS_H
#define _MIKU_LIMITS_H

#include_next <limits.h>

#define PATH_MAX 4096
#define NAME_MAX 255
#define PIPE_BUF 4096
#define IOV_MAX 1024
#define SSIZE_MAX LONG_MAX
#define PTHREAD_KEYS_MAX 128
#define PTHREAD_STACK_MIN 16384

#endif
