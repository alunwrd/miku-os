#ifndef _ERRNO_H
#define _ERRNO_H

#include <_miku.h>
#include <bits/errno.h>

__MIKU_BEGIN
/* Per thread: the word at tp+40 in the thread control block */
int *__errno_location(void);
#define errno (*__errno_location())
__MIKU_END

#endif
