#ifndef _STDIO_H
#define _STDIO_H

#include <_miku.h>
#include <stddef.h>
#include <stdarg.h>
#include <sys/types.h>

typedef struct _miku_FILE FILE;
typedef int64_t fpos_t;

#define EOF (-1)
#define BUFSIZ 4096
#define FILENAME_MAX 4096
#define FOPEN_MAX 64
#define L_tmpnam 64
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2
#ifndef SEEK_SET
#define SEEK_SET 0
#define SEEK_CUR 1
#define SEEK_END 2
#endif

__MIKU_BEGIN
/* The three standard streams are objects in libc_miku.so; C reaches them
   through a function because the link stubs carry functions, not data */
FILE *__miku_stdfile(int fd);
#define stdin  (__miku_stdfile(0))
#define stdout (__miku_stdfile(1))
#define stderr (__miku_stdfile(2))

FILE *fopen(const char *, const char *);
FILE *fdopen(int, const char *);
int fclose(FILE *);
int fflush(FILE *);
size_t fread(void *, size_t, size_t, FILE *);
size_t fwrite(const void *, size_t, size_t, FILE *);
int fgetc(FILE *);
int getc(FILE *);
int getchar(void);
char *fgets(char *, int, FILE *);
int fputc(int, FILE *);
int putc(int, FILE *);
int putchar(int);
int fputs(const char *, FILE *);
int puts(const char *);
int ungetc(int, FILE *);
int fseek(FILE *, long, int);
long ftell(FILE *);
void rewind(FILE *);
int feof(FILE *);
int ferror(FILE *);
void clearerr(FILE *);
int fileno(FILE *);
void setbuf(FILE *, char *);
int setvbuf(FILE *, char *, int, size_t);
void perror(const char *);
int remove(const char *);
int rename(const char *, const char *);

int printf(const char *, ...) __miku_printf(1, 2);
int fprintf(FILE *, const char *, ...) __miku_printf(2, 3);
int dprintf(int, const char *, ...) __miku_printf(2, 3);
int sprintf(char *, const char *, ...) __miku_printf(2, 3);
int snprintf(char *, size_t, const char *, ...) __miku_printf(3, 4);
int vprintf(const char *, va_list) __miku_printf(1, 0);
int vfprintf(FILE *, const char *, va_list) __miku_printf(2, 0);
int vdprintf(int, const char *, va_list) __miku_printf(2, 0);
int vsprintf(char *, const char *, va_list) __miku_printf(2, 0);
int vsnprintf(char *, size_t, const char *, va_list) __miku_printf(3, 0);
int asprintf(char **, const char *, ...) __miku_printf(2, 3);
int vasprintf(char **, const char *, va_list) __miku_printf(2, 0);
int sscanf(const char *, const char *, ...) __attribute__((format(scanf, 2, 3)));
int vsscanf(const char *, const char *, va_list) __attribute__((format(scanf, 2, 0)));
__MIKU_END

#endif
