#ifndef _STDLIB_H
#define _STDLIB_H

#include <_miku.h>
#include <stddef.h>
#include <bits/stdlib.h>
#include <alloca.h>

#define RAND_MAX 0x7fffffff
#define MB_CUR_MAX 4

typedef struct { int quot, rem; } div_t;
typedef struct { long quot, rem; } ldiv_t;

__MIKU_BEGIN
int posix_openpt(int);
int grantpt(int);
int unlockpt(int);
char *ptsname(int);
int ptsname_r(int, char *, size_t);
void *malloc(size_t);
void *calloc(size_t, size_t);
void *realloc(void *, size_t);
void free(void *);
void *aligned_alloc(size_t, size_t);
int posix_memalign(void **, size_t, size_t);

__miku_noreturn void abort(void);
__miku_noreturn void exit(int);
__miku_noreturn void _Exit(int);
int atexit(void (*)(void));

char *getenv(const char *);
char *realpath(const char *, char *);
int setenv(const char *, const char *, int);
int unsetenv(const char *);
int putenv(char *);

int atoi(const char *);
long atol(const char *);
long long atoll(const char *);
double atof(const char *);
long strtol(const char *, char **, int);
unsigned long strtoul(const char *, char **, int);
long long strtoll(const char *, char **, int);
unsigned long long strtoull(const char *, char **, int);
double strtod(const char *, char **);
float strtof(const char *, char **);

int abs(int);
long labs(long);
long long llabs(long long);
div_t div(int, int);
ldiv_t ldiv(long, long);

void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
void *bsearch(const void *, const void *, size_t, size_t, int (*)(const void *, const void *));

int rand(void);
void srand(unsigned);
__MIKU_END

#endif
