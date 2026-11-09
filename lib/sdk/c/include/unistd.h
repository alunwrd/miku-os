#ifndef _UNISTD_H
#define _UNISTD_H

#include <_miku.h>
#include <stddef.h>
#include <sys/types.h>
#include <bits/unistd.h>

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

__MIKU_BEGIN
ssize_t read(int, void *, size_t);
ssize_t write(int, const void *, size_t);
ssize_t pread(int, void *, size_t, off_t);
ssize_t pwrite(int, const void *, size_t, off_t);
off_t lseek(int, off_t, int);
int close(int);
int dup(int);
int dup2(int, int);
int dup3(int, int, int);
int pipe(int[2]);
int pipe2(int[2], int);
int fsync(int);
int fdatasync(int);
void sync(void);
int ftruncate(int, off_t);
int truncate(const char *, off_t);
int unlink(const char *);
int rmdir(const char *);
int link(const char *, const char *);
int symlink(const char *, const char *);
ssize_t readlink(const char *, char *, size_t);
int access(const char *, int);
int chdir(const char *);
char *getcwd(char *, size_t);
int chown(const char *, uid_t, gid_t);
int fchown(int, uid_t, gid_t);
int isatty(int);
pid_t getpid(void);
pid_t getppid(void);
uid_t getuid(void);
uid_t geteuid(void);
gid_t getgid(void);
gid_t getegid(void);
pid_t fork(void);
int execve(const char *, char *const[], char *const[]);
int execvpe(const char *, char *const[], char *const[]);
int fexecve(int, char *const[], char *const[]);
__miku_noreturn void _exit(int);
unsigned sleep(unsigned);
int usleep(useconds_t);
long sysconf(int);
void *sbrk(intptr_t);
int getpagesize(void);
__MIKU_END

#endif
