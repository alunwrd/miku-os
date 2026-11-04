#ifndef _STRING_H
#define _STRING_H

#include <_miku.h>
#include <stddef.h>

__MIKU_BEGIN
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);
void *memchr(const void *, int, size_t);
void *memmem(const void *, size_t, const void *, size_t);
size_t strlen(const char *);
size_t strnlen(const char *, size_t);
int strcmp(const char *, const char *);
int strncmp(const char *, const char *, size_t);
char *strcpy(char *, const char *);
char *strncpy(char *, const char *, size_t);
char *strcat(char *, const char *);
char *strncat(char *, const char *, size_t);
size_t strlcpy(char *, const char *, size_t);
size_t strlcat(char *, const char *, size_t);
char *strchr(const char *, int);
char *strrchr(const char *, int);
char *strstr(const char *, const char *);
char *strpbrk(const char *, const char *);
size_t strspn(const char *, const char *);
size_t strcspn(const char *, const char *);
char *strtok(char *, const char *);
char *strtok_r(char *, const char *, char **);
char *strsep(char **, const char *);
char *strdup(const char *);
char *strndup(const char *, size_t);
char *strerror(int);
int strerror_r(int, char *, size_t);
int strcoll(const char *, const char *);
__MIKU_END

#endif
