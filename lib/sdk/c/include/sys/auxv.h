#ifndef _SYS_AUXV_H
#define _SYS_AUXV_H

#include <_miku.h>

#include <bits/auxv.h>

__MIKU_BEGIN
/* The value of a key, or 0 when the vector has none */
unsigned long getauxval(unsigned long);
__MIKU_END

#endif
