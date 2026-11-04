/* One locale, "C" */
#ifndef _LOCALE_H
#define _LOCALE_H

#include <_miku.h>

#define LC_ALL 0
#define LC_COLLATE 1
#define LC_CTYPE 2
#define LC_MONETARY 3
#define LC_NUMERIC 4
#define LC_TIME 5

__MIKU_BEGIN
char *setlocale(int, const char *);
__MIKU_END

#endif
