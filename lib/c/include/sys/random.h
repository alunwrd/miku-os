#ifndef _SYS_RANDOM_H
#define _SYS_RANDOM_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/random.h>

__MIKU_BEGIN
ssize_t getrandom(void *, size_t, unsigned int);
__MIKU_END

#endif
