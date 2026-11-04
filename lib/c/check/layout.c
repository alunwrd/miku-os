/* The C headers' structs, pinned to lib/rust-libc/libc/src/unix/miku/mod.rs.
 *
 * Compiled with -fsyntax-only by lib/c/miku-cc whenever a header changes, so
 * a drift is a build error. The numbers are the Rust side's; `stdc` on the
 * machine checks the same sizes against `core::mem::size_of` at run time.
 * That every function a header declares exists is checked by the builder
 * before any C compiles (builder/src/stdport/cheaders.rs) */

#include <stddef.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/uio.h>
#include <sys/resource.h>
#include <time.h>
#include <dirent.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <setjmp.h>

#define SIZE(t, n) _Static_assert(sizeof(t) == (n), "sizeof " #t)
#define AT(t, f, n) _Static_assert(offsetof(t, f) == (n), "offsetof " #t "." #f)

SIZE(struct stat, 144);
AT(struct stat, st_dev, 0);
AT(struct stat, st_ino, 8);
AT(struct stat, st_nlink, 16);
AT(struct stat, st_mode, 24);
AT(struct stat, st_uid, 28);
AT(struct stat, st_gid, 32);
AT(struct stat, st_rdev, 40);
AT(struct stat, st_size, 48);
AT(struct stat, st_blksize, 56);
AT(struct stat, st_blocks, 64);
AT(struct stat, st_atime, 72);
AT(struct stat, st_mtime, 88);
AT(struct stat, st_ctime, 104);

SIZE(struct dirent, 272);
AT(struct dirent, d_ino, 256);
AT(struct dirent, d_type, 264);

SIZE(struct tm, 36);
SIZE(struct timespec, 16);
SIZE(struct timeval, 16);
SIZE(struct iovec, 16);
SIZE(struct rlimit, 16);
SIZE(struct pollfd, 8);
SIZE(struct flock, 32);
AT(struct flock, l_start, 8);
AT(struct flock, l_pid, 24);
SIZE(sigset_t, 128);
SIZE(siginfo_t, 128);
AT(siginfo_t, si_code, 8);
AT(siginfo_t, si_addr, 16);
SIZE(struct sigaction, 152);
AT(struct sigaction, sa_mask, 8);
AT(struct sigaction, sa_flags, 136);
AT(struct sigaction, sa_restorer, 144);

SIZE(pthread_t, 8);
SIZE(pthread_key_t, 4);
SIZE(pthread_mutex_t, 40);
SIZE(pthread_mutexattr_t, 32);
SIZE(pthread_cond_t, 48);
SIZE(pthread_condattr_t, 32);
SIZE(pthread_rwlock_t, 56);
SIZE(pthread_rwlockattr_t, 32);
SIZE(pthread_attr_t, 56);

/* No Rust side to pin it to: lib/libmiku/c/setjmp.rs's assembly is what
 * keeps eight words, and this is that number */
SIZE(jmp_buf, 64);
