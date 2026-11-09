/* `struct stat` is Linux's, 144 bytes. The kernel's own is 64 bytes and
   libc_miku's stat/fstat/lstat translate it (lib/runtime/libmiku/libc/, to_c_stat):
   the file type goes into st_mode, st_dev is which filesystem, st_rdev the
   device a /dev node is, st_blksize 4096, and the nanoseconds are zero */
#ifndef _SYS_STAT_H
#define _SYS_STAT_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/stat.h>

struct stat {
    dev_t st_dev;
    ino_t st_ino;
    nlink_t st_nlink;
    mode_t st_mode;
    uid_t st_uid;
    gid_t st_gid;
    int __pad0;
    dev_t st_rdev;
    off_t st_size;
    blksize_t st_blksize;
    blkcnt_t st_blocks;
    time_t st_atime;
    long st_atime_nsec;
    time_t st_mtime;
    long st_mtime_nsec;
    time_t st_ctime;
    long st_ctime_nsec;
    long __unused[3];
};

#define S_ISREG(m)  (((m) & S_IFMT) == S_IFREG)
#define S_ISDIR(m)  (((m) & S_IFMT) == S_IFDIR)
#define S_ISLNK(m)  (((m) & S_IFMT) == S_IFLNK)
#define S_ISCHR(m)  (((m) & S_IFMT) == S_IFCHR)
#define S_ISBLK(m)  (((m) & S_IFMT) == S_IFBLK)
#define S_ISFIFO(m) (((m) & S_IFMT) == S_IFIFO)
#define S_ISSOCK(m) (((m) & S_IFMT) == S_IFSOCK)

__MIKU_BEGIN
int stat(const char *, struct stat *);
int lstat(const char *, struct stat *);
int fstat(int, struct stat *);
int mkdir(const char *, mode_t);
int chmod(const char *, mode_t);
int fchmod(int, mode_t);
mode_t umask(mode_t);
__MIKU_END

#endif
