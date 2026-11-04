/* Unicode classification, from Rust's `char` tables, not a locale */
#ifndef _WCTYPE_H
#define _WCTYPE_H

#include <_miku.h>
#include <stdint.h>
#include <stddef.h>

typedef uint32_t wint_t;
#define WEOF ((wint_t)-1)

__MIKU_BEGIN
int iswalnum(wint_t); int iswalpha(wint_t); int iswblank(wint_t);
int iswcntrl(wint_t); int iswdigit(wint_t); int iswgraph(wint_t);
int iswlower(wint_t); int iswprint(wint_t); int iswpunct(wint_t);
int iswspace(wint_t); int iswupper(wint_t); int iswxdigit(wint_t);
wint_t towlower(wint_t); wint_t towupper(wint_t);
__MIKU_END

#endif
