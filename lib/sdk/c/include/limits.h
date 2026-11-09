/* The compiler's limits.h, and the POSIX ones it does not know */
#ifndef _MIKU_LIMITS_H
#define _MIKU_LIMITS_H

#include_next <limits.h>

#include <bits/limits.h>

#define SSIZE_MAX LONG_MAX
#define PTHREAD_KEYS_MAX 128
#define PTHREAD_STACK_MIN 16384

#endif
