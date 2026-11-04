/* No run-time loading: C in a MikuOS program is linked in. dlopen answers
   NULL with a dlerror that says so; dlsym(RTLD_DEFAULT, ...) answers NULL,
   which is what "that symbol is not here" means */
#ifndef _DLFCN_H
#define _DLFCN_H

#include <_miku.h>

#define RTLD_LAZY 1
#define RTLD_NOW 2
#define RTLD_GLOBAL 0x100
#define RTLD_LOCAL 0
#define RTLD_DEFAULT ((void *)0)

__MIKU_BEGIN
void *dlopen(const char *, int);
void *dlsym(void *, const char *);
char *dlerror(void);
int dlclose(void *);
__MIKU_END

#endif
