#ifndef _SIGNAL_H
#define _SIGNAL_H

#include <_miku.h>
#include <sys/types.h>
#include <bits/signal.h>

typedef int sig_atomic_t;
typedef struct { uint64_t __bits[16]; } sigset_t;
typedef void (*sighandler_t)(int);

#define SIG_DFL ((sighandler_t)0)
#define SIG_IGN ((sighandler_t)1)
#define SIG_ERR ((sighandler_t)-1)

/* Linux's shapes, which is what libc_miku.so takes and translates onto the
   kernel's disposition and sys_miku's handler table (lib/runtime/libmiku/libc/,
   `sigaction`). A siginfo_t is 128 bytes; the fields a handler reads are
   the ones MikuOS fills in (dev/signal-architecture.md) */
typedef struct {
    int si_signo;
    int si_errno;
    int si_code;
    int __pad0;
    union {
        struct { pid_t __pid; uid_t __uid; } __kill;
        void *__addr;
        int __pad[28];
    } __u;
} siginfo_t;
#define si_pid  __u.__kill.__pid
#define si_uid  __u.__kill.__uid
#define si_addr __u.__addr

struct sigaction {
    union {
        sighandler_t __handler;
        void (*__sigaction)(int, siginfo_t *, void *);
    } __sa;
    sigset_t sa_mask;
    int sa_flags;
    void (*sa_restorer)(void);
};
#define sa_handler   __sa.__handler
#define sa_sigaction __sa.__sigaction

__MIKU_BEGIN
int raise(int);
int kill(pid_t, int);
int sigaction(int, const struct sigaction *, struct sigaction *);
sighandler_t signal(int, sighandler_t);
int sigprocmask(int, const sigset_t *, sigset_t *);
int pthread_sigmask(int, const sigset_t *, sigset_t *);
int sigsuspend(const sigset_t *);
int sigwait(const sigset_t *, int *);
int sigemptyset(sigset_t *);
int sigfillset(sigset_t *);
int sigaddset(sigset_t *, int);
int sigdelset(sigset_t *, int);
int sigismember(const sigset_t *, int);
__MIKU_END

#endif
