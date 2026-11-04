/* alloca: the compiler's own, not a library function: stack memory that is
   gone when the calling function returns. glibc spells it this way, and
   <stdlib.h> includes it the way glibc's does under _GNU_SOURCE and
   _DEFAULT_SOURCE, which is where QuickJS expects to find it */
#ifndef _ALLOCA_H
#define _ALLOCA_H

#define alloca(size) __builtin_alloca(size)

#endif
