/* The C half of stdapps/src/stdc.rs: each function exercises one part of
   libc_miku.so from C, the way a C library would call it, and reports back
   in a shape the Rust side can check */

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
#include <wctype.h>
#include <dirent.h>
#include <poll.h>
#include <sys/auxv.h>

/* sizeof, by name, for the Rust side to hold against libc's */
size_t stdc_sizeof(const char *name) {
    if (!strcmp(name, "stat")) return sizeof(struct stat);
    if (!strcmp(name, "dirent")) return sizeof(struct dirent);
    if (!strcmp(name, "tm")) return sizeof(struct tm);
    if (!strcmp(name, "timespec")) return sizeof(struct timespec);
    if (!strcmp(name, "timeval")) return sizeof(struct timeval);
    if (!strcmp(name, "pollfd")) return sizeof(struct pollfd);
    if (!strcmp(name, "flock")) return sizeof(struct flock);
    if (!strcmp(name, "pthread_mutex_t")) return sizeof(pthread_mutex_t);
    if (!strcmp(name, "pthread_cond_t")) return sizeof(pthread_cond_t);
    if (!strcmp(name, "pthread_rwlock_t")) return sizeof(pthread_rwlock_t);
    if (!strcmp(name, "pthread_attr_t")) return sizeof(pthread_attr_t);
    return 0;
}

/* The constants the generator took from the Rust module, read back */
long stdc_const(const char *name) {
    if (!strcmp(name, "O_CREAT")) return O_CREAT;
    if (!strcmp(name, "O_TRUNC")) return O_TRUNC;
    if (!strcmp(name, "EAGAIN")) return EAGAIN;
    if (!strcmp(name, "ENOENT")) return ENOENT;
    if (!strcmp(name, "S_IFDIR")) return S_IFDIR;
    if (!strcmp(name, "MAP_SHARED")) return MAP_SHARED;
    return -1;
}

/* snprintf with more arguments than registers, every width and precision
   form, and floating point */
int stdc_format(char *out, size_t n, int which) {
    switch (which) {
    case 0: return snprintf(out, n, "%d|%5d|%-5d|%05d|%+d|% d|%x|%#X|%o|%#o|%u|%c|%s|%.3s|%%",
                            42, 42, 42, 42, 42, 42, 255, 255, 8, 8, 4000000000u, 'M', "miku", "abcdef");
    case 1: return snprintf(out, n, "%lld %ld %hd %hhu %zu %" PRId64 " %" PRIx64,
                            -9000000000LL, -1L, (short)-2, (unsigned char)300, (size_t)7, (int64_t)-5, (uint64_t)0xdeadbeef);
    case 2: return snprintf(out, n, "%f|%.2f|%10.3f|%-10.1f|%e|%.3E|%g|%g|%g|%.10g|%a",
                            3.14159, 2.675, -1.5, 0.25, 12345.678, 0.000123, 100000.0, 1e-5, 0.1, 1.0 / 3.0, 1.0);
    case 3: return snprintf(out, n, "%*d|%-*d|%.*f|%.*s|%p|%s",
                            6, 1, 6, 2, 3, 2.0, 2, "xyz", (void *)0, (char *)0);
    case 4: return snprintf(out, n, "%g %g %g %f %e", 1.0 / 0.0, -1.0 / 0.0, 0.0 / 0.0, -0.0, 0.0);
    case 5: {
        /* Truncation: the return is what would have been written */
        char small[5];
        int r = snprintf(small, sizeof small, "%s", "truncated");
        return snprintf(out, n, "%d:%s", r, small);
    }
    case 6: {
        char *p = 0;
        int r = asprintf(&p, "%s-%d-%.1f", "asprintf", 7, 7.25);
        if (r < 0) return r;
        r = snprintf(out, n, "%s", p);
        free(p);
        return r;
    }
    }
    return -1;
}

double stdc_strtod(const char *s, int *used) {
    char *end;
    errno = 0;
    double v = strtod(s, &end);
    *used = (int)(end - s);
    return v;
}

int stdc_errno_after_strtod(const char *s) {
    errno = 0;
    (void)strtod(s, 0);
    return errno;
}

/* Four threads, a counter, a mutex; and a condition variable handing a
   value from the main thread to a waiter */
static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static long counter;
static int ready;
static pthread_key_t key;
static int dtor_ran;

static void key_dtor(void *v) { (void)v; __atomic_add_fetch(&dtor_ran, 1, __ATOMIC_SEQ_CST); }

static void *adder(void *arg) {
    pthread_setspecific(key, arg);
    for (int i = 0; i < 10000; i++) {
        pthread_mutex_lock(&mu);
        counter++;
        pthread_mutex_unlock(&mu);
    }
    return (void *)(long)(pthread_getspecific(key) == arg);
}

static void *waiter(void *arg) {
    (void)arg;
    pthread_mutex_lock(&mu);
    while (!ready)
        pthread_cond_wait(&cv, &mu);
    long v = counter;
    pthread_mutex_unlock(&mu);
    return (void *)v;
}

long stdc_threads(void) {
    pthread_key_create(&key, key_dtor);
    pthread_t t[4], w;
    for (long i = 0; i < 4; i++)
        if (pthread_create(&t[i], 0, adder, (void *)(i + 1)) != 0) return -1;
    if (pthread_create(&w, 0, waiter, 0) != 0) return -2;
    long own = 0;
    for (int i = 0; i < 4; i++) {
        void *r;
        pthread_join(t[i], &r);
        own += (long)r;
    }
    pthread_mutex_lock(&mu);
    ready = 1;
    pthread_cond_signal(&cv);
    pthread_mutex_unlock(&mu);
    void *seen;
    pthread_join(w, &seen);
    /* 40000 increments, four threads that each saw their own key value,
       four destructors, and the waiter saw the final count */
    if (own != 4) return -3;
    if (__atomic_load_n(&dtor_ran, __ATOMIC_SEQ_CST) != 4) return -4;
    if ((long)seen != counter) return -5;
    return counter;
}

/* A recursive mutex, taken twice by one thread; a timed wait that times out */
int stdc_sync_edges(void) {
    pthread_mutexattr_t a;
    pthread_mutex_t m;
    pthread_mutexattr_init(&a);
    pthread_mutexattr_settype(&a, PTHREAD_MUTEX_RECURSIVE);
    pthread_mutex_init(&m, &a);
    if (pthread_mutex_lock(&m) || pthread_mutex_lock(&m)) return 1;
    if (pthread_mutex_unlock(&m) || pthread_mutex_unlock(&m)) return 2;
    if (pthread_mutex_trylock(&m)) return 3;
    pthread_mutex_unlock(&m);

    pthread_mutex_t n = PTHREAD_MUTEX_INITIALIZER;
    pthread_cond_t c = PTHREAD_COND_INITIALIZER;
    struct timespec until;
    clock_gettime(CLOCK_REALTIME, &until);
    until.tv_nsec += 50 * 1000000;
    if (until.tv_nsec >= 1000000000) { until.tv_sec++; until.tv_nsec -= 1000000000; }
    pthread_mutex_lock(&n);
    int r = pthread_cond_timedwait(&c, &n, &until);
    pthread_mutex_unlock(&n);
    if (r != ETIMEDOUT) return 4;
    return 0;
}

/* 2000-02-29 12:34:56 UTC, both ways through the calendar */
int stdc_time(char *out, size_t n) {
    struct tm tm = {0};
    tm.tm_year = 100; tm.tm_mon = 1; tm.tm_mday = 29;
    tm.tm_hour = 12; tm.tm_min = 34; tm.tm_sec = 56;
    time_t t = timegm(&tm);
    struct tm back;
    gmtime_r(&t, &back);
    char s[64];
    strftime(s, sizeof s, "%Y-%m-%d %H:%M:%S %a %j", &back);
    time_t now = time(0);
    return snprintf(out, n, "%lld %s %d", (long long)t, s, now > 1700000000);
}

/* A file through stdio, left open for exit to flush */
int stdc_stdio(const char *path) {
    FILE *f = fopen(path, "w");
    if (!f) return -1;
    fprintf(f, "%s %d %.1f\n", "line", 1, 1.5);
    fclose(f);
    f = fopen(path, "r");
    if (!f) return -2;
    char buf[64] = {0};
    if (!fgets(buf, sizeof buf, f)) return -3;
    fclose(f);
    unlink(path);
    return strcmp(buf, "line 1 1.5\n") == 0 ? 0 : -4;
}

int stdc_wide(void) {
    return iswalpha(0x3bb) && iswupper(0x39b) && towlower(0x39b) == 0x3bb
        && iswspace(0x3000) && !iswalpha('1') && iswdigit('7') && towupper('a') == 'A';
}

/* The auxiliary vector through getauxval: the page size, and a vDSO with an
   ELF header where AT_SYSINFO_EHDR says. 0 for none of it wrong */
int stdc_auxv(void) {
    const unsigned char *vdso = (const unsigned char *)getauxval(AT_SYSINFO_EHDR);
    if (getauxval(AT_PAGESZ) != 4096) return -1;
    if (!vdso || vdso[0] != 0x7f || vdso[1] != 'E') return -2;
    if (getauxval(12345) != 0) return -3;
    return 0;
}

double stdc_math(void) {
    return pow(2.0, 10.0) + sqrt(16.0) + floor(-0.5) + fmod(7.0, 3.0);
}

/* Record locks between two processes: the parent holds byte 0 for writing;
   a child cannot take it (EAGAIN), is told by F_GETLK who can, and can take
   byte 1. After the parent unlocks, a second child can take byte 0. The
   children's answers come back as exit codes */
static int lock_range(int fd, short type, off_t start, off_t len, int cmd) {
    struct flock fl = {0};
    fl.l_type = type;
    fl.l_whence = SEEK_SET;
    fl.l_start = start;
    fl.l_len = len;
    return fcntl(fd, cmd, &fl);
}

#include <sys/wait.h>

int stdc_locks(const char *path) {
    int fd = open(path, O_RDWR | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) return 1;
    if (write(fd, "xx", 2) != 2) return 2;
    if (lock_range(fd, F_WRLCK, 0, 1, F_SETLK) != 0) return 3;
    pid_t me = getpid();

    pid_t c = fork();
    if (c == 0) {
        int f = open(path, O_RDWR);
        if (f < 0) _exit(10);
        if (lock_range(f, F_WRLCK, 0, 1, F_SETLK) != -1 || errno != EAGAIN) _exit(11);
        struct flock q = {0};
        q.l_type = F_WRLCK; q.l_whence = SEEK_SET; q.l_start = 0; q.l_len = 1;
        if (fcntl(f, F_GETLK, &q) != 0 || q.l_type != F_WRLCK || q.l_pid != me) _exit(12);
        if (lock_range(f, F_WRLCK, 1, 1, F_SETLK) != 0) _exit(13);
        _exit(0);
    }
    int st = 0;
    if (c < 0 || waitpid(c, &st, 0) != c) return 4;
    if (WEXITSTATUS(st) != 0) return 100 + WEXITSTATUS(st);

    if (lock_range(fd, F_UNLCK, 0, 0, F_SETLK) != 0) return 5;
    c = fork();
    if (c == 0) {
        int f = open(path, O_RDWR);
        _exit(f >= 0 && lock_range(f, F_WRLCK, 0, 0, F_SETLK) == 0 ? 0 : 20);
    }
    if (c < 0 || waitpid(c, &st, 0) != c) return 6;
    close(fd);
    unlink(path);
    return WEXITSTATUS(st) == 0 ? 0 : 100 + WEXITSTATUS(st);
}
