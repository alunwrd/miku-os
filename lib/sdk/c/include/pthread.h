/* POSIX threads, over MikuOS's own: thread_create(106) and futex(110).
   Implemented in lib/runtime/libmiku/libc/pthread.rs; the sizes are the Rust libc
   module's, so a struct holding one has one layout on both sides. Every
   object is valid all-zero, which is what the static initialisers are */
#ifndef _PTHREAD_H
#define _PTHREAD_H

#include <_miku.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>
#include <sched.h>

/* The types are <sys/types.h>'s, as POSIX puts them: code that never
   includes this header still names them (aws-lc's CRYPTO_MUTEX is one) */
#include <sys/types.h>

#define PTHREAD_MUTEX_INITIALIZER { { 0 } }
#define PTHREAD_COND_INITIALIZER { { 0 } }
#define PTHREAD_RWLOCK_INITIALIZER { { 0 } }
#define PTHREAD_ONCE_INIT 0

#define PTHREAD_MUTEX_NORMAL 0
#define PTHREAD_MUTEX_RECURSIVE 1
#define PTHREAD_MUTEX_ERRORCHECK 2
#define PTHREAD_MUTEX_DEFAULT PTHREAD_MUTEX_NORMAL
#define PTHREAD_CREATE_JOINABLE 0
#define PTHREAD_CREATE_DETACHED 1
/* SHARED holds for a mutex in a tmpfs file mapped MAP_SHARED (pthread.rs) */
#define PTHREAD_PROCESS_PRIVATE 0
#define PTHREAD_PROCESS_SHARED 1

__MIKU_BEGIN
int pthread_create(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *);
int pthread_join(pthread_t, void **);
int pthread_detach(pthread_t);
pthread_t pthread_self(void);
int pthread_equal(pthread_t, pthread_t);
__miku_noreturn void pthread_exit(void *);

int pthread_attr_init(pthread_attr_t *);
int pthread_attr_destroy(pthread_attr_t *);
int pthread_attr_setstacksize(pthread_attr_t *, size_t);
int pthread_attr_getstacksize(const pthread_attr_t *, size_t *);
int pthread_attr_setdetachstate(pthread_attr_t *, int);

int pthread_mutex_init(pthread_mutex_t *, const pthread_mutexattr_t *);
int pthread_mutex_destroy(pthread_mutex_t *);
int pthread_mutex_lock(pthread_mutex_t *);
int pthread_mutex_trylock(pthread_mutex_t *);
int pthread_mutex_unlock(pthread_mutex_t *);
int pthread_mutexattr_init(pthread_mutexattr_t *);
int pthread_mutexattr_destroy(pthread_mutexattr_t *);
int pthread_mutexattr_settype(pthread_mutexattr_t *, int);
int pthread_mutexattr_setpshared(pthread_mutexattr_t *, int);
int pthread_mutexattr_getpshared(const pthread_mutexattr_t *, int *);

int pthread_cond_init(pthread_cond_t *, const pthread_condattr_t *);
int pthread_cond_destroy(pthread_cond_t *);
int pthread_cond_wait(pthread_cond_t *, pthread_mutex_t *);
int pthread_cond_timedwait(pthread_cond_t *, pthread_mutex_t *, const struct timespec *);
int pthread_cond_signal(pthread_cond_t *);
int pthread_cond_broadcast(pthread_cond_t *);
int pthread_condattr_init(pthread_condattr_t *);
int pthread_condattr_destroy(pthread_condattr_t *);
/* CLOCK_REALTIME or CLOCK_MONOTONIC, for pthread_cond_timedwait */
int pthread_condattr_setclock(pthread_condattr_t *, clockid_t);
int pthread_condattr_getclock(const pthread_condattr_t *, clockid_t *);

int pthread_rwlock_init(pthread_rwlock_t *, const pthread_rwlockattr_t *);
int pthread_rwlock_destroy(pthread_rwlock_t *);
int pthread_rwlock_rdlock(pthread_rwlock_t *);
int pthread_rwlock_tryrdlock(pthread_rwlock_t *);
int pthread_rwlock_wrlock(pthread_rwlock_t *);
int pthread_rwlock_trywrlock(pthread_rwlock_t *);
int pthread_rwlock_unlock(pthread_rwlock_t *);

int pthread_once(pthread_once_t *, void (*)(void));
int pthread_key_create(pthread_key_t *, void (*)(void *));
int pthread_key_delete(pthread_key_t);
void *pthread_getspecific(pthread_key_t);
int pthread_setspecific(pthread_key_t, const void *);
__MIKU_END

#endif
