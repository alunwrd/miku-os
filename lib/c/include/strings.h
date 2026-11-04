#ifndef _STRINGS_H
#define _STRINGS_H

#include <_miku.h>
#include <stddef.h>

__MIKU_BEGIN
int strcasecmp(const char *, const char *);
int strncasecmp(const char *, const char *, size_t);
void bzero(void *, size_t);
__MIKU_END

#endif
