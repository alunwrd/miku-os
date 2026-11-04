/* Layouts: lib/rust-libc/libc/src/unix/miku/mod.rs, pinned by lib/c/check/layout.c */
#ifndef _SYS_TYPES_H
#define _SYS_TYPES_H

#include <stddef.h>
#include <stdint.h>

typedef long ssize_t;
typedef int64_t off_t;
typedef int64_t time_t;
typedef int64_t clock_t;
typedef int64_t suseconds_t;
typedef uint64_t dev_t;
typedef uint64_t ino_t;
typedef uint32_t mode_t;
typedef uint64_t nlink_t;
typedef int64_t blksize_t;
typedef int64_t blkcnt_t;
typedef int pid_t;
typedef uint32_t uid_t;
typedef uint32_t gid_t;
typedef uint32_t id_t;
typedef uint32_t useconds_t;
typedef int clockid_t;
typedef off_t off64_t;

/* POSIX puts the thread types here rather than only in <pthread.h>; libraries
   that hold a lock in a public struct (aws-lc's CRYPTO_MUTEX) rely on it.
   Sizes are the Rust libc module's (pthread.h) */
typedef unsigned long pthread_t;
typedef uint32_t pthread_key_t;
typedef int pthread_once_t;
typedef struct { uint64_t __w[5]; } pthread_mutex_t;
typedef struct { uint64_t __w[4]; } pthread_mutexattr_t;
typedef struct { uint64_t __w[6]; } pthread_cond_t;
typedef struct { uint64_t __w[4]; } pthread_condattr_t;
typedef struct { uint64_t __w[7]; } pthread_rwlock_t;
typedef struct { uint64_t __w[4]; } pthread_rwlockattr_t;
typedef struct { uint64_t __w[7]; } pthread_attr_t;

#endif
