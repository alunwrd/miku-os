/* Wide characters: the types, and the classification wctype.h has. No
   multibyte conversion: libc_miku has no locale to convert through */
#ifndef _WCHAR_H
#define _WCHAR_H

#include <_miku.h>
#include <stddef.h>
#include <wctype.h>

typedef struct { unsigned __opaque; } mbstate_t;

#define WCHAR_MIN (-2147483647 - 1)
#define WCHAR_MAX 2147483647

#endif
