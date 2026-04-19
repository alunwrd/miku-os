use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let stub_c = out_dir.join("miku_stub.c");
    let stub_so = out_dir.join("libmiku.so");

    std::fs::write(&stub_c, STUB_SOURCE).unwrap();

    let status = Command::new("gcc")
        .args([
            "-shared",
            "-nostdlib",
            "-fPIC",
            "-Wl,-soname,libmiku.so",
            "-o",
            stub_so.to_str().unwrap(),
            stub_c.to_str().unwrap(),
        ])
        .status()
        .expect("gcc failed");
    assert!(status.success(), "Failed to build libmiku stub");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=dylib=miku");
}

const STUB_SOURCE: &str = r#"
/* libc compat stubs */
void *memcpy(void *d, const void *s, unsigned long n) { return d; }
void *memset(void *d, int v, unsigned long n) { return d; }
void *memmove(void *d, const void *s, unsigned long n) { return d; }
void __stack_chk_fail(void) { for(;;); }

static inline long miku_syscall0(unsigned long nr) {
    long ret;
    __asm__ __volatile__("syscall"
                         : "=a"(ret)
                         : "a"(nr)
                         : "rcx", "r11", "memory");
    return ret;
}

static inline long miku_syscall1(unsigned long nr, unsigned long a1) {
    long ret;
    __asm__ __volatile__("syscall"
                         : "=a"(ret)
                         : "a"(nr), "D"(a1)
                         : "rcx", "r11", "memory");
    return ret;
}

static inline long miku_syscall3(unsigned long nr, unsigned long a1, unsigned long a2, unsigned long a3) {
    long ret;
    __asm__ __volatile__("syscall"
                         : "=a"(ret)
                         : "a"(nr), "D"(a1), "S"(a2), "d"(a3)
                         : "rcx", "r11", "memory");
    return ret;
}

/* sys (syscall primitives are inline, no stubs needed) */

/* errno */
const char *miku_strerror(long code) { return ""; }
const char *miku_errno_name(long code) { return ""; }
void miku_perror(const char *prefix) {}
void miku_perror_code(const char *prefix, long code) {}
int miku_is_error(long val) { return val < 0; }
long miku_to_errno(long val) { return val < 0 ? -val : 0; }
void miku_set_errno(long code) {}
long miku_get_errno(void) { return 0; }

/* sync (internal, no C exports) */

/* mem */
void *miku_memset(void *d, int v, unsigned long n) { return d; }
void *miku_memcpy(void *d, const void *s, unsigned long n) { return d; }
void *miku_memmove(void *d, const void *s, unsigned long n) { return d; }
int miku_memcmp(const void *a, const void *b, unsigned long n) { return 0; }
void miku_bzero(void *d, unsigned long n) {}
const void *miku_memchr(const void *s, int c, unsigned long n) { return 0; }
const void *miku_memrchr(const void *s, int c, unsigned long n) { return 0; }
const void *miku_memmem(const void *h, unsigned long hl, const void *n, unsigned long nl) { return 0; }

/* heap */
void *miku_malloc(unsigned long s) { return 0; }
void miku_free(void *p) {}
void *miku_realloc(void *p, unsigned long s) { return 0; }
void *miku_calloc(unsigned long c, unsigned long s) { return 0; }
void *miku_memalign(unsigned long a, unsigned long s) { return 0; }

/* proc */
void miku_exit(long code) { miku_syscall1(0, (unsigned long)code); for(;;); }
unsigned long miku_getpid(void) { return (unsigned long)miku_syscall0(7); }
char *miku_getcwd(char *b, unsigned long s) { return (char *)miku_syscall3(8, (unsigned long)b, s, 0); }
unsigned long miku_brk(unsigned long a) { return (unsigned long)miku_syscall1(6, a); }
void *miku_mmap(unsigned long a, unsigned long l, unsigned long p) { return (void *)0; }
long miku_munmap(void *a, unsigned long l) { return 0; }
long miku_mprotect(unsigned long a, unsigned long l, unsigned long p) { return 0; }
long miku_set_tls(unsigned long a) { return 0; }
unsigned long miku_get_tls(void) { return 0; }
long miku_map_lib(const char *n, unsigned long l) { return 0; }

/* io */
long miku_write(unsigned long fd, const void *b, unsigned long l) { return 0; }
long miku_read(unsigned long fd, void *b, unsigned long l) { return 0; }

/* stdio */
void miku_print(const char *s) {}
void miku_println(const char *s) {}
int miku_puts(const char *s) { return 0; }
void miku_eprint(const char *s) {}
void miku_eprintln(const char *s) {}
int miku_putchar(int c) { return c; }
int miku_getchar(void) { return -1; }
int miku_readline(char *b, unsigned long m) { return -1; }
char *miku_getline(void) { return 0; }

/* file */
long miku_open(const char *p, unsigned long l, unsigned int f, unsigned int m) { return -1; }
long miku_open_cstr(const char *p) { return -1; }
long miku_open_rw(const char *p) { return -1; }
long miku_create(const char *p, unsigned int m) { return -1; }
long miku_close(long fd) { return 0; }
long miku_seek(long fd, unsigned long o) { return 0; }
long miku_lseek(long fd, long o, unsigned long w) { return 0; }
long miku_fsize(long fd) { return 0; }
void *miku_read_file(const char *p, unsigned long *s) { return 0; }
long miku_read_fd(long fd, void *b, unsigned long l) { return 0; }
long miku_write_fd(long fd, const void *b, unsigned long l) { return 0; }

/* filesystem operations */
typedef struct { unsigned long sz; unsigned int mode; unsigned int nl; unsigned short uid; unsigned short gid; unsigned char kind; unsigned char fst; unsigned char dmj; unsigned char dmn; unsigned long at; unsigned long mt; unsigned long ct; unsigned long ino; unsigned int blk; unsigned char _r[4]; } MikuStat;
typedef struct { char name[64]; unsigned short ino; unsigned char kind; unsigned char nlen; unsigned int _r; } MikuDirent;

long miku_stat(const char *p, MikuStat *s) { return -1; }
long miku_fstat(long fd, MikuStat *s) { return -1; }
long miku_mkdir(const char *p, unsigned int m) { return -1; }
long miku_rmdir(const char *p) { return -1; }
long miku_unlink(const char *p) { return -1; }
long miku_readdir(const char *p, MikuDirent *e, unsigned long n) { return -1; }
long miku_rename(const char *o, const char *n) { return -1; }
long miku_link(const char *o, const char *n) { return -1; }
long miku_symlink(const char *t, const char *l) { return -1; }
long miku_readlink(const char *p, char *b, unsigned long bl) { return -1; }
long miku_chmod(const char *p, unsigned int m) { return -1; }
long miku_chown(const char *p, unsigned int uid, unsigned int gid) { return -1; }
long miku_dup(long fd) { return -1; }
long miku_dup2(long ofd, long nfd) { return -1; }
long miku_pipe(long *fds) { return -1; }
long miku_chdir(const char *p) { return -1; }
int miku_access(const char *p) { return 0; }
int miku_isdir(const char *p) { return 0; }
int miku_isfile(const char *p) { return 0; }
int miku_issymlink(const char *p) { return 0; }
long miku_filesize(const char *p) { return -1; }
long miku_ftruncate(long fd, unsigned long length) { return 0; }
long miku_pread(long fd, void *buf, unsigned long len, long offset) { return 0; }
long miku_pwrite(long fd, const void *buf, unsigned long len, long offset) { return 0; }
long miku_write_file_cstr(const char *path, const char *data) { return 0; }

/* string */
unsigned long miku_strlen(const char *s) { return 0; }
int miku_strcmp(const char *a, const char *b) { return 0; }
int miku_strncmp(const char *a, const char *b, unsigned long n) { return 0; }
char *miku_strcpy(char *d, const char *s) { return d; }
char *miku_strncpy(char *d, const char *s, unsigned long n) { return d; }
char *miku_strcat(char *d, const char *s) { return d; }
char *miku_strncat(char *d, const char *s, unsigned long n) { return d; }
const char *miku_strchr(const char *s, int c) { return 0; }
const char *miku_strrchr(const char *s, int c) { return 0; }
const char *miku_strstr(const char *h, const char *n) { return 0; }
char *miku_strdup(const char *s) { return 0; }
char *miku_strndup(const char *s, unsigned long n) { return 0; }
unsigned long miku_strlcpy(char *d, const char *s, unsigned long n) { return 0; }
unsigned long miku_strlcat(char *d, const char *s, unsigned long n) { return 0; }
char *miku_strtok(char *s, const char *d) { return 0; }
char *miku_strtok_r(char *s, const char *d, char **saveptr) { return 0; }
unsigned long miku_strnlen(const char *s, unsigned long maxlen) { return 0; }
int miku_strcasecmp(const char *a, const char *b) { return 0; }
int miku_strncasecmp(const char *a, const char *b, unsigned long n) { return 0; }
char *miku_strsep(char **stringp, const char *delim) { return 0; }
const char *miku_strpbrk(const char *s, const char *a) { return 0; }
unsigned long miku_strspn(const char *s, const char *a) { return 0; }
unsigned long miku_strcspn(const char *s, const char *r) { return 0; }

/* ctype */
int miku_isdigit(int c) { return 0; }
int miku_isalpha(int c) { return 0; }
int miku_isalnum(int c) { return 0; }
int miku_isspace(int c) { return 0; }
int miku_isupper(int c) { return 0; }
int miku_islower(int c) { return 0; }
int miku_isprint(int c) { return 0; }
int miku_ispunct(int c) { return 0; }
int miku_iscntrl(int c) { return 0; }
int miku_isxdigit(int c) { return 0; }
int miku_toupper(int c) { return c; }
int miku_tolower(int c) { return c; }
int miku_isgraph(int c) { return 0; }
int miku_isblank(int c) { return 0; }
int miku_isascii(int c) { return 0; }
int miku_toascii(int c) { return c & 0x7F; }

/* convert */
long miku_strtol(const char *s, const char **e, int b) { return 0; }
unsigned long miku_strtoul(const char *s, const char **e, int b) { return 0; }
long miku_strtod(const char *s, const char **e) { return 0; }

/* num */
void miku_itoa(long val, char *buf) {}
void miku_utoa(unsigned long val, char *buf) {}
long miku_atoi(const char *s) { return 0; }
void miku_print_int(long v) {}
void miku_print_hex(unsigned long v) {}

/* fmt */
int miku_printf(const char *f, ...) { return 0; }
int miku_snprintf(char *b, unsigned long m, const char *f, ...) { return 0; }
int miku_dprintf(unsigned long fd, const char *f, ...) { return 0; }
int miku_fprintf(unsigned long fd, const char *f, ...) { return 0; }

/* time */
void miku_sleep(unsigned long t) {}
void miku_sleep_ms(unsigned long t) {}
unsigned long miku_uptime(void) { return 0; }
unsigned long miku_uptime_ms(void) { return 0; }
void miku_yield(void) {}

/* math */
long miku_abs(long x) { return x < 0 ? -x : x; }
long miku_min(long a, long b) { return a < b ? a : b; }
long miku_max(long a, long b) { return a > b ? a : b; }
long miku_clamp(long v, long lo, long hi) { return v; }
void miku_swap(unsigned long *a, unsigned long *b) {}
unsigned long miku_umin(unsigned long a, unsigned long b) { return a < b ? a : b; }
unsigned long miku_umax(unsigned long a, unsigned long b) { return a > b ? a : b; }

/* random */
void miku_srand(unsigned long s) {}
unsigned long miku_rand(void) { return 0; }
unsigned long miku_rand_range(unsigned long lo, unsigned long hi) { return lo; }
unsigned int miku_rand_u32(void) { return 0; }
void miku_rand_bytes(void *buf, unsigned long len) {}
void miku_rand_shuffle(void *data, unsigned long count, unsigned long elem_size) {}

/* panic */
void miku_assert_fail(const char *e, const char *f, int l) {}
void miku_panic(const char *m) { for(;;); }
void miku_abort(void) { for(;;); }

/* vec */
typedef struct { void *d; unsigned long l, c, e; } MikuVec;
MikuVec miku_vec_new(unsigned long e) { MikuVec v = {0}; return v; }
MikuVec miku_vec_with_capacity(unsigned long e, unsigned long c) { MikuVec v = {0}; return v; }
void miku_vec_free(MikuVec *v) {}
unsigned long miku_vec_len(const MikuVec *v) { return 0; }
unsigned long miku_vec_cap(const MikuVec *v) { return 0; }
int miku_vec_is_empty(const MikuVec *v) { return 1; }
const void *miku_vec_get(const MikuVec *v, unsigned long i) { return 0; }
void *miku_vec_get_mut(MikuVec *v, unsigned long i) { return 0; }
int miku_vec_push(MikuVec *v, const void *e) { return 0; }
int miku_vec_pop(MikuVec *v, void *o) { return 0; }
int miku_vec_insert(MikuVec *v, unsigned long i, const void *e) { return 0; }
int miku_vec_remove(MikuVec *v, unsigned long i) { return 0; }
int miku_vec_swap_remove(MikuVec *v, unsigned long i) { return 0; }
void miku_vec_clear(MikuVec *v) {}
int miku_vec_reserve(MikuVec *v, unsigned long a) { return 0; }
int miku_vec_shrink(MikuVec *v) { return 0; }
const void *miku_vec_data(const MikuVec *v) { return 0; }
int miku_vec_contains(const MikuVec *v, const void *e) { return 0; }
int miku_vec_push_u64(MikuVec *v, unsigned long val) { return 0; }
unsigned long miku_vec_get_u64(const MikuVec *v, unsigned long i) { return 0; }

/* hashmap */
typedef struct { void *s; unsigned long c, n, k, v, ss; } MikuMap;
MikuMap miku_map_new(unsigned long k, unsigned long v) { MikuMap m = {0}; return m; }
void miku_map_free(MikuMap *m) {}
int miku_map_insert(MikuMap *m, const void *k, const void *v) { return 0; }
const void *miku_map_get(const MikuMap *m, const void *k) { return 0; }
int miku_map_contains(const MikuMap *m, const void *k) { return 0; }
int miku_map_remove(MikuMap *m, const void *k) { return 0; }
unsigned long miku_map_len(const MikuMap *m) { return 0; }
void miku_map_clear(MikuMap *m) {}
void miku_map_iter(const MikuMap *m, void (*cb)(const void*, const void*, void*), void *u) {}
MikuMap miku_map_new_u64(void) { MikuMap m = {0}; return m; }
int miku_map_insert_u64(MikuMap *m, unsigned long k, unsigned long v) { return 0; }
unsigned long miku_map_get_u64(const MikuMap *m, unsigned long k) { return 0; }

/* sort */
void miku_qsort(void *b, unsigned long c, unsigned long s, int (*cmp)(const void*, const void*)) {}
void miku_insertion_sort(void *b, unsigned long c, unsigned long s, int (*cmp)(const void*, const void*)) {}
const void *miku_bsearch(const void *k, const void *b, unsigned long c, unsigned long s, int (*cmp)(const void*, const void*)) { return 0; }
void miku_reverse(void *b, unsigned long c, unsigned long s) {}
int miku_is_sorted(const void *b, unsigned long c, unsigned long s, int (*cmp)(const void*, const void*)) { return 1; }
int miku_cmp_i64(const void *a, const void *b) { return 0; }
int miku_cmp_u64(const void *a, const void *b) { return 0; }
int miku_cmp_str(const void *a, const void *b) { return 0; }

/* bitops */
unsigned int miku_popcount32(unsigned int x) { return 0; }
unsigned long miku_popcount64(unsigned long x) { return 0; }
unsigned int miku_clz32(unsigned int x) { return 32; }
unsigned long miku_clz64(unsigned long x) { return 64; }
unsigned int miku_ctz32(unsigned int x) { return 32; }
unsigned long miku_ctz64(unsigned long x) { return 64; }
unsigned int miku_fls32(unsigned int x) { return 0; }
unsigned long miku_fls64(unsigned long x) { return 0; }
unsigned int miku_ffs32(unsigned int x) { return 0; }
unsigned long miku_ffs64(unsigned long x) { return 0; }
unsigned short miku_bswap16(unsigned short x) { return 0; }
unsigned int miku_bswap32(unsigned int x) { return 0; }
unsigned long miku_bswap64(unsigned long x) { return 0; }
unsigned int miku_rotl32(unsigned int x, unsigned int n) { return 0; }
unsigned int miku_rotr32(unsigned int x, unsigned int n) { return 0; }
unsigned long miku_rotl64(unsigned long x, unsigned long n) { return 0; }
unsigned long miku_rotr64(unsigned long x, unsigned long n) { return 0; }
int miku_is_power_of_two(unsigned long x) { return 0; }
unsigned long miku_next_power_of_two(unsigned long x) { return 0; }
unsigned long miku_log2(unsigned long x) { return 0; }
unsigned long miku_bit_extract(unsigned long v, unsigned int s, unsigned int l) { return 0; }
unsigned long miku_bit_insert(unsigned long v, unsigned long b, unsigned int s, unsigned int l) { return 0; }
unsigned long miku_align_up(unsigned long v, unsigned long a) { return 0; }
unsigned long miku_align_down(unsigned long v, unsigned long a) { return 0; }
int miku_is_aligned(unsigned long v, unsigned long a) { return 0; }

/* endian */
unsigned short miku_htobe16(unsigned short x) { return 0; }
unsigned int miku_htobe32(unsigned int x) { return 0; }
unsigned long miku_htobe64(unsigned long x) { return 0; }
unsigned short miku_be16toh(unsigned short x) { return 0; }
unsigned int miku_be32toh(unsigned int x) { return 0; }
unsigned long miku_be64toh(unsigned long x) { return 0; }
unsigned short miku_htole16(unsigned short x) { return x; }
unsigned int miku_htole32(unsigned int x) { return x; }
unsigned long miku_htole64(unsigned long x) { return x; }
unsigned short miku_le16toh(unsigned short x) { return x; }
unsigned int miku_le32toh(unsigned int x) { return x; }
unsigned long miku_le64toh(unsigned long x) { return x; }
unsigned short miku_read_u16_be(const void *p) { return 0; }
unsigned int miku_read_u32_be(const void *p) { return 0; }
unsigned long miku_read_u64_be(const void *p) { return 0; }
unsigned short miku_read_u16_le(const void *p) { return 0; }
unsigned int miku_read_u32_le(const void *p) { return 0; }
unsigned long miku_read_u64_le(const void *p) { return 0; }
void miku_write_u16_be(void *p, unsigned short v) {}
void miku_write_u32_be(void *p, unsigned int v) {}
void miku_write_u16_le(void *p, unsigned short v) {}
void miku_write_u32_le(void *p, unsigned int v) {}

/* base64 */
unsigned long miku_base64_encode_len(unsigned long l) { return 0; }
unsigned long miku_base64_decode_len(unsigned long l) { return 0; }
int miku_base64_encode(const void *i, unsigned long l, void *o, unsigned long m) { return 0; }
int miku_base64_decode(const void *i, unsigned long l, void *o, unsigned long m) { return 0; }
void *miku_base64_encode_alloc(const void *i, unsigned long l) { return 0; }
void *miku_base64_decode_alloc(const void *i, unsigned long l, unsigned long *ol) { return 0; }

/* utf8 */
unsigned long miku_utf8_encode(unsigned int cp, void *o) { return 0; }
unsigned int miku_utf8_decode(const void *d, unsigned long l, unsigned long *c) { return 0; }
unsigned long miku_utf8_len(const void *s, unsigned long l) { return 0; }
unsigned long miku_utf8_strlen(const void *s) { return 0; }
int miku_utf8_valid(const void *s, unsigned long l) { return 0; }
unsigned long miku_utf8_offset(const void *s, unsigned long l, unsigned long n) { return 0; }
int miku_utf8_is_boundary(const void *s, unsigned long l, unsigned long p) { return 0; }

/* hash */
unsigned int miku_fnv1a_32(const void *d, unsigned long l) { return 0; }
unsigned long miku_fnv1a_64(const void *d, unsigned long l) { return 0; }
unsigned long miku_djb2(const void *d, unsigned long l) { return 0; }
unsigned long miku_djb2_str(const void *s) { return 0; }
unsigned int miku_crc32(const void *d, unsigned long l) { return 0; }
unsigned int miku_crc32_update(unsigned int p, const void *d, unsigned long l) { return 0; }
unsigned long miku_siphash(const void *d, unsigned long l, unsigned long k0, unsigned long k1) { return 0; }
unsigned long miku_hash_bytes(const void *d, unsigned long l) { return 0; }
unsigned long miku_hash_str(const void *s) { return 0; }
unsigned long miku_hash_u64(unsigned long v) { return 0; }
unsigned long miku_hash_combine(unsigned long seed, unsigned long value) { return 0; }
unsigned int miku_hash_u32(unsigned int v) { return 0; }
unsigned int miku_adler32(const void *data, unsigned long len) { return 1; }
unsigned int miku_adler32_update(unsigned int prev, const void *data, unsigned long len) { return prev; }
unsigned long miku_murmurhash3_fmix64(unsigned long k) { return 0; }
unsigned long miku_murmurhash3(const void *data, unsigned long len, unsigned long seed) { return 0; }

/* path */
char *miku_basename(const char *p) { return 0; }
char *miku_dirname(const char *p) { return 0; }
char *miku_path_ext(const char *p) { return 0; }
char *miku_path_stem(const char *p) { return 0; }
char *miku_path_join(const char *a, const char *b) { return 0; }
char *miku_path_normalize(const char *p) { return 0; }
int miku_path_is_absolute(const char *p) { return 0; }
unsigned long miku_path_depth(const char *p) { return 0; }

/* ringbuf */
typedef struct { void *b; unsigned long c, h, t; } MikuRingBuf;
MikuRingBuf miku_ring_new(unsigned long c) { MikuRingBuf r = {0}; return r; }
void miku_ring_free(MikuRingBuf *r) {}
unsigned long miku_ring_len(const MikuRingBuf *r) { return 0; }
unsigned long miku_ring_available(const MikuRingBuf *r) { return 0; }
int miku_ring_is_empty(const MikuRingBuf *r) { return 1; }
int miku_ring_is_full(const MikuRingBuf *r) { return 1; }
unsigned long miku_ring_write(MikuRingBuf *r, const void *d, unsigned long l) { return 0; }
unsigned long miku_ring_read(MikuRingBuf *r, void *o, unsigned long l) { return 0; }
unsigned long miku_ring_peek(const MikuRingBuf *r, void *o, unsigned long l) { return 0; }
int miku_ring_push_byte(MikuRingBuf *r, unsigned char b) { return 0; }
int miku_ring_pop_byte(MikuRingBuf *r) { return -1; }
unsigned long miku_ring_skip(MikuRingBuf *r, unsigned long n) { return 0; }
void miku_ring_clear(MikuRingBuf *r) {}

/* list */
typedef struct { void *h, *t; unsigned long l, e; } MikuList;
MikuList miku_list_new(unsigned long e) { MikuList l = {0}; return l; }
void miku_list_free(MikuList *l) {}
unsigned long miku_list_len(const MikuList *l) { return 0; }
int miku_list_is_empty(const MikuList *l) { return 1; }
int miku_list_push_front(MikuList *l, const void *e) { return 0; }
int miku_list_push_back(MikuList *l, const void *e) { return 0; }
int miku_list_pop_front(MikuList *l, void *o) { return 0; }
int miku_list_pop_back(MikuList *l, void *o) { return 0; }
const void *miku_list_get(const MikuList *l, unsigned long i) { return 0; }
int miku_list_set(MikuList *l, unsigned long i, const void *e) { return 0; }
int miku_list_insert(MikuList *l, unsigned long i, const void *e) { return 0; }
int miku_list_remove(MikuList *l, unsigned long i) { return 0; }
void miku_list_clear(MikuList *l) {}
int miku_list_contains(const MikuList *l, const void *e) { return 0; }
void miku_list_iter(const MikuList *l, void (*cb)(const void*, unsigned long, void*), void *u) {}
int miku_list_push_back_u64(MikuList *l, unsigned long v) { return 0; }
unsigned long miku_list_get_u64(const MikuList *l, unsigned long i) { return 0; }

/* arena */
typedef struct { void *h; unsigned long bs, ta; } MikuArena;
MikuArena miku_arena_new(void) { MikuArena a = {0}; return a; }
MikuArena miku_arena_with_block_size(unsigned long bs) { MikuArena a = {0}; return a; }
void *miku_arena_alloc(MikuArena *a, unsigned long s) { return 0; }
void *miku_arena_calloc(MikuArena *a, unsigned long s) { return 0; }
char *miku_arena_strdup(MikuArena *a, const char *s) { return 0; }
void miku_arena_reset(MikuArena *a) {}
void miku_arena_free(MikuArena *a) {}
unsigned long miku_arena_used(const MikuArena *a) { return 0; }

/* bitset */
typedef struct { unsigned long *w; unsigned long n; } MikuBitset;
MikuBitset miku_bitset_new(unsigned long n) { MikuBitset b = {0}; return b; }
void miku_bitset_free(MikuBitset *b) {}
int miku_bitset_set(MikuBitset *b, unsigned long bit) { return 0; }
void miku_bitset_clear(MikuBitset *b, unsigned long bit) {}
int miku_bitset_test(const MikuBitset *b, unsigned long bit) { return 0; }
int miku_bitset_toggle(MikuBitset *b, unsigned long bit) { return 0; }
unsigned long miku_bitset_count(const MikuBitset *b) { return 0; }
void miku_bitset_clear_all(MikuBitset *b) {}
void miku_bitset_set_all(MikuBitset *b, unsigned long n) {}
void miku_bitset_or(MikuBitset *d, const MikuBitset *s) {}
void miku_bitset_and(MikuBitset *d, const MikuBitset *s) {}
void miku_bitset_xor(MikuBitset *d, const MikuBitset *s) {}
long miku_bitset_ffs(const MikuBitset *b) { return -1; }
int miku_bitset_is_empty(const MikuBitset *b) { return 1; }
unsigned long miku_bitset_capacity(const MikuBitset *b) { return 0; }

/* heap_queue (priority queue) */
typedef struct { void *d; unsigned long l, c, e; int (*cmp)(const void*, const void*); } MikuHeapQueue;
MikuHeapQueue miku_pq_new(unsigned long e, int (*cmp)(const void*, const void*)) { MikuHeapQueue q = {0}; return q; }
void miku_pq_free(MikuHeapQueue *q) {}
int miku_pq_push(MikuHeapQueue *q, const void *e) { return 0; }
const void *miku_pq_peek(const MikuHeapQueue *q) { return 0; }
int miku_pq_pop(MikuHeapQueue *q, void *o) { return 0; }
unsigned long miku_pq_len(const MikuHeapQueue *q) { return 0; }
int miku_pq_is_empty(const MikuHeapQueue *q) { return 1; }
void miku_pq_clear(MikuHeapQueue *q) {}
int miku_pq_cmp_i64(const void *a, const void *b) { return 0; }
MikuHeapQueue miku_pq_new_i64(void) { MikuHeapQueue q = {0}; return q; }
int miku_pq_push_i64(MikuHeapQueue *q, long v) { return 0; }
long miku_pq_pop_i64(MikuHeapQueue *q) { return 0; }

/* glob */
int miku_glob_match(const char *p, const char *t) { return 0; }
int miku_glob_match_nocase(const char *p, const char *t) { return 0; }
int miku_glob_has_magic(const char *p) { return 0; }
char *miku_glob_escape(const char *s) { return 0; }

/* channel */
typedef struct { void *b; unsigned long c, e, h, t; } MikuChannel;
MikuChannel miku_chan_new(unsigned long e, unsigned long c) { MikuChannel ch = {0}; return ch; }
void miku_chan_free(MikuChannel *ch) {}
int miku_chan_send(MikuChannel *ch, const void *d) { return 0; }
int miku_chan_recv(MikuChannel *ch, void *o) { return 0; }
unsigned long miku_chan_len(const MikuChannel *ch) { return 0; }
int miku_chan_is_empty(const MikuChannel *ch) { return 1; }
int miku_chan_is_full(const MikuChannel *ch) { return 1; }
unsigned long miku_chan_available(const MikuChannel *ch) { return 0; }
MikuChannel miku_chan_new_u64(unsigned long c) { MikuChannel ch = {0}; return ch; }
int miku_chan_send_u64(MikuChannel *ch, unsigned long v) { return 0; }
unsigned long miku_chan_recv_u64(MikuChannel *ch) { return 0; }

/* slab */
typedef struct { void *p, *f; unsigned long ss, c, u; } MikuSlab;
MikuSlab miku_slab_new(unsigned long o, unsigned long c) { MikuSlab s = {0}; return s; }
void miku_slab_free(MikuSlab *s) {}
void *miku_slab_alloc(MikuSlab *s) { return 0; }
void miku_slab_dealloc(MikuSlab *s, void *p) {}
unsigned long miku_slab_in_use(const MikuSlab *s) { return 0; }
unsigned long miku_slab_available(const MikuSlab *s) { return 0; }
unsigned long miku_slab_capacity(const MikuSlab *s) { return 0; }
unsigned long miku_slab_slot_size(const MikuSlab *s) { return 0; }
int miku_slab_is_full(const MikuSlab *s) { return 1; }
int miku_slab_is_empty(const MikuSlab *s) { return 1; }

/* format (string builder) */
typedef struct { void *b; unsigned long l, c; } MikuStringBuilder;
MikuStringBuilder miku_sb_new(void) { MikuStringBuilder s = {0}; return s; }
MikuStringBuilder miku_sb_with_capacity(unsigned long c) { MikuStringBuilder s = {0}; return s; }
void miku_sb_free(MikuStringBuilder *s) {}
int miku_sb_append(MikuStringBuilder *s, const char *t) { return 0; }
int miku_sb_append_bytes(MikuStringBuilder *s, const void *d, unsigned long l) { return 0; }
int miku_sb_append_char(MikuStringBuilder *s, unsigned char c) { return 0; }
int miku_sb_append_int(MikuStringBuilder *s, long v) { return 0; }
int miku_sb_append_uint(MikuStringBuilder *s, unsigned long v) { return 0; }
int miku_sb_repeat(MikuStringBuilder *s, unsigned char c, unsigned long n) { return 0; }
char *miku_sb_finish(MikuStringBuilder *s) { return 0; }
unsigned long miku_sb_len(const MikuStringBuilder *s) { return 0; }
void miku_sb_clear(MikuStringBuilder *s) {}
const void *miku_sb_data(const MikuStringBuilder *s) { return 0; }
char *miku_str_join(const char **strs, unsigned long c, const char *sep) { return 0; }
char *miku_str_repeat(const char *s, unsigned long n) { return 0; }

/* regex */
int miku_regex_match(const char *p, const char *t) { return 0; }
int miku_regex_match_full(const char *p, const char *t) { return 0; }
long miku_regex_find(const char *p, const char *t) { return -1; }
unsigned long miku_regex_count(const char *p, const char *t) { return 0; }

/* hex */
unsigned long miku_hex_encode_len(unsigned long l) { return 0; }
unsigned long miku_hex_decode_len(unsigned long l) { return 0; }
int miku_hex_encode(const void *i, unsigned long l, void *o, unsigned long m) { return 0; }
int miku_hex_encode_upper(const void *i, unsigned long l, void *o, unsigned long m) { return 0; }
int miku_hex_decode(const void *i, unsigned long l, void *o, unsigned long m) { return 0; }
void *miku_hex_encode_alloc(const void *i, unsigned long l) { return 0; }
void *miku_hex_decode_alloc(const void *i, unsigned long l, unsigned long *ol) { return 0; }
void miku_hex_u64(unsigned long v, char *b) {}
void miku_hex_u32(unsigned int v, char *b) {}

/* checksum */
unsigned short miku_fletcher16(const void *d, unsigned long l) { return 0; }
unsigned int miku_fletcher32(const void *d, unsigned long l) { return 0; }
unsigned char miku_xor_checksum(const void *d, unsigned long l) { return 0; }
unsigned short miku_inet_checksum(const void *d, unsigned long l) { return 0; }
unsigned char miku_sum8(const void *d, unsigned long l) { return 0; }
unsigned short miku_bsd_checksum(const void *d, unsigned long l) { return 0; }

/* getopt */
typedef struct { const char **av; int ac, oi, op; const char *oa; unsigned char oo; int fin; } MikuGetopt;
void miku_getopt_init(MikuGetopt *g, int ac, const char **av) {}
int miku_getopt_next(MikuGetopt *g, const char *os) { return -1; }
int miku_getopt_optind(const MikuGetopt *g) { return 0; }
const char *miku_getopt_optarg(const MikuGetopt *g) { return 0; }
const char *miku_argv_get(int ac, const char **av, const char *k) { return 0; }
int miku_argv_has(int ac, const char **av, const char *f) { return 0; }
int miku_argv_positional_count(int ac, const char **av) { return 0; }

/* treemap (AVL) */
typedef struct { void *r; unsigned long c; } MikuTreeMap;
MikuTreeMap miku_tree_new(void) { MikuTreeMap t = {0}; return t; }
void miku_tree_free(MikuTreeMap *t) {}
int miku_tree_insert(MikuTreeMap *t, long k, unsigned long v) { return 0; }
const unsigned long *miku_tree_get(const MikuTreeMap *t, long k) { return 0; }
int miku_tree_contains(const MikuTreeMap *t, long k) { return 0; }
int miku_tree_remove(MikuTreeMap *t, long k) { return 0; }
unsigned long miku_tree_len(const MikuTreeMap *t) { return 0; }
int miku_tree_is_empty(const MikuTreeMap *t) { return 1; }
void miku_tree_iter(const MikuTreeMap *t, void (*cb)(long, unsigned long, void*), void *ctx) {}
int miku_tree_min(const MikuTreeMap *t, long *k, unsigned long *v) { return 0; }
int miku_tree_max(const MikuTreeMap *t, long *k, unsigned long *v) { return 0; }
void miku_tree_clear(MikuTreeMap *t) {}

/* lz (compression) */
void *miku_lz_compress(const void *i, unsigned long l, unsigned long *ol) { return 0; }
void *miku_lz_decompress(const void *i, unsigned long l, unsigned long *ol, unsigned long m) { return 0; }
int miku_lz_compress_buf(const void *i, unsigned long l, void *o, unsigned long om) { return -1; }
int miku_lz_decompress_buf(const void *i, unsigned long l, void *o, unsigned long om) { return -1; }
unsigned long miku_lz_compress_bound(unsigned long l) { return 0; }

/* env */
int miku_setenv(const char *k, const char *v) { return 0; }
const char *miku_getenv(const char *k) { return 0; }
int miku_unsetenv(const char *k) { return 0; }
int miku_hasenv(const char *k) { return 0; }
unsigned long miku_env_count(void) { return 0; }
void miku_env_clear(void) {}
void miku_env_iter(void (*cb)(const char*, const char*, void*), void *ctx) {}
int miku_putenv(const char *s) { return 0; }

/* signal */
void *miku_signal(unsigned int sig, void (*handler)(unsigned int)) { return 0; }
int miku_signal_dispatch(unsigned int sig) { return 0; }
int miku_signal_has_handler(unsigned int sig) { return 0; }
void miku_signal_reset_all(void) {}
void miku_signal_block(unsigned int sig) {}
void miku_signal_unblock(unsigned int sig) {}
int miku_signal_is_blocked(unsigned int sig) { return 0; }
unsigned int miku_signal_get_mask(void) { return 0; }
unsigned int miku_signal_set_mask(unsigned int m) { return 0; }
typedef struct { void (*handler)(unsigned int); unsigned int flags; unsigned int mask; } MikuSigaction;
int miku_sigaction(unsigned int sig, const MikuSigaction *act, MikuSigaction *oldact) { return 0; }
int miku_sigaction_dispatch(unsigned int sig) { return 0; }
int miku_signal_raise(unsigned int sig) { return 0; }
unsigned int miku_signal_pending(void) { return 0; }

/* env (new) */
int miku_getenv_r(const char *key, char *buf, unsigned long buf_size) { return -1; }

/* json */
typedef struct { unsigned char tt; unsigned int s, e, sz; int p; } MikuJsonToken;
typedef struct { unsigned long pos, nt; int ss[32]; unsigned long d; } MikuJsonParser;
void miku_json_init(MikuJsonParser *p) {}
int miku_json_parse(MikuJsonParser *p, const char *d, unsigned long l, MikuJsonToken *t, unsigned long mt) { return -1; }
unsigned char miku_json_type(const MikuJsonToken *t, unsigned long i) { return 0; }
const char *miku_json_value(const char *d, const MikuJsonToken *t, unsigned long i, unsigned long *ol) { return 0; }
int miku_json_eq(const char *d, const MikuJsonToken *t, unsigned long i, const char *s) { return 0; }
unsigned int miku_json_size(const MikuJsonToken *t, unsigned long i) { return 0; }
int miku_json_find(const char *d, const MikuJsonToken *t, unsigned long nt, unsigned long oi, const char *k) { return -1; }
unsigned long miku_json_token_count(const MikuJsonParser *p) { return 0; }

/* ringbuf2 (byte ring) */
typedef struct { void *d; unsigned long c, h, t; } MikuByteRing;
MikuByteRing miku_bring_new(unsigned long mc) { MikuByteRing r = {0}; return r; }
void miku_bring_free(MikuByteRing *r) {}
unsigned long miku_bring_len(const MikuByteRing *r) { return 0; }
unsigned long miku_bring_avail(const MikuByteRing *r) { return 0; }
int miku_bring_is_empty(const MikuByteRing *r) { return 1; }
unsigned long miku_bring_write(MikuByteRing *r, const void *d, unsigned long l) { return 0; }
unsigned long miku_bring_read(MikuByteRing *r, void *o, unsigned long l) { return 0; }
unsigned long miku_bring_peek(const MikuByteRing *r, void *o, unsigned long l) { return 0; }
unsigned long miku_bring_skip(MikuByteRing *r, unsigned long l) { return 0; }
int miku_bring_put(MikuByteRing *r, unsigned char b) { return 0; }
int miku_bring_get(MikuByteRing *r, unsigned char *o) { return 0; }
int miku_bring_find(const MikuByteRing *r, unsigned char b) { return -1; }
unsigned long miku_bring_readline(MikuByteRing *r, void *o, unsigned long ml) { return 0; }
void miku_bring_clear(MikuByteRing *r) {}
unsigned long miku_bring_capacity(const MikuByteRing *r) { return 0; }

/* ini */
typedef struct { char _opaque[24960]; } MikuIni;
MikuIni miku_ini_new(void) { MikuIni i; return i; }
int miku_ini_parse(MikuIni *i, const char *d, unsigned long l) { return -1; }
const char *miku_ini_get(const MikuIni *i, const char *s, const char *k) { return 0; }
long miku_ini_get_int(const MikuIni *i, const char *s, const char *k, long d) { return d; }
int miku_ini_get_bool(const MikuIni *i, const char *s, const char *k, int d) { return d; }
int miku_ini_has_section(const MikuIni *i, const char *s) { return 0; }
int miku_ini_has_key(const MikuIni *i, const char *s, const char *k) { return 0; }
unsigned long miku_ini_count(const MikuIni *i) { return 0; }
void miku_ini_iter_section(const MikuIni *i, const char *s, void (*cb)(const char*, const char*, void*), void *ctx) {}

/* csv */
typedef struct { char _opaque[55296]; } MikuCsv;
MikuCsv miku_csv_new(void) { MikuCsv c; return c; }
MikuCsv miku_csv_with_delimiter(unsigned char d) { MikuCsv c; return c; }
int miku_csv_parse(MikuCsv *c, const char *d, unsigned long l) { return -1; }
unsigned long miku_csv_rows(const MikuCsv *c) { return 0; }
unsigned long miku_csv_cols(const MikuCsv *c, unsigned long r) { return 0; }
const char *miku_csv_field(const MikuCsv *c, const char *d, unsigned long r, unsigned long col, unsigned long *ol) { return 0; }
int miku_csv_field_eq(const MikuCsv *c, const char *d, unsigned long r, unsigned long col, const char *s) { return 0; }
long miku_csv_field_int(const MikuCsv *c, const char *d, unsigned long r, unsigned long col, long def) { return def; }

/* event */
int miku_event_on(unsigned int id, void (*handler)(unsigned int, void*, void*), void *ctx) { return -1; }
void miku_event_off(int idx) {}
unsigned int miku_event_emit(unsigned int id, void *data) { return 0; }
int miku_event_has_listeners(unsigned int id) { return 0; }
unsigned int miku_event_count(unsigned int id) { return 0; }
void miku_event_clear(unsigned int id) {}
void miku_event_clear_all(void) {}

/* pool */
typedef struct { void *d; unsigned int *g; unsigned int *fl; unsigned long os, c, fc, ac; } MikuPool;
typedef struct { unsigned long v; } PoolHandle;
MikuPool miku_pool_new(unsigned long os, unsigned long c) { MikuPool p = {0}; return p; }
void miku_pool_free(MikuPool *p) {}
PoolHandle miku_pool_alloc(MikuPool *p) { PoolHandle h = {0xFFFFFFFFFFFFFFFF}; return h; }
int miku_pool_release(MikuPool *p, PoolHandle h) { return 0; }
void *miku_pool_get(const MikuPool *p, PoolHandle h) { return 0; }
int miku_pool_valid(const MikuPool *p, PoolHandle h) { return 0; }
unsigned long miku_pool_active(const MikuPool *p) { return 0; }
unsigned long miku_pool_capacity(const MikuPool *p) { return 0; }
unsigned long miku_pool_available(const MikuPool *p) { return 0; }
void miku_pool_iter(const MikuPool *p, void (*cb)(PoolHandle, void*, void*), void *ctx) {}

/* timer */
typedef struct { unsigned long s, pe; int r; } MikuStopwatch;
typedef struct { unsigned long dl, dur; int rep, exp; } MikuTimer;
MikuStopwatch miku_sw_start(void) { MikuStopwatch s = {0}; return s; }
unsigned long miku_sw_elapsed_ms(const MikuStopwatch *s) { return 0; }
unsigned long miku_sw_elapsed_sec(const MikuStopwatch *s) { return 0; }
void miku_sw_pause(MikuStopwatch *s) {}
void miku_sw_resume(MikuStopwatch *s) {}
void miku_sw_reset(MikuStopwatch *s) {}
int miku_sw_running(const MikuStopwatch *s) { return 0; }
MikuTimer miku_timer_once(unsigned long ms) { MikuTimer t = {0}; return t; }
MikuTimer miku_timer_repeat(unsigned long ms) { MikuTimer t = {0}; return t; }
int miku_timer_check(MikuTimer *t) { return 0; }
unsigned long miku_timer_remaining(const MikuTimer *t) { return 0; }
void miku_timer_reset(MikuTimer *t) {}
int miku_timer_expired(const MikuTimer *t) { return 0; }
void miku_delay_ms(unsigned long ms) {}
void miku_delay_sleep(unsigned long ms) {}

/* log */
void miku_log_set_level(unsigned char l) {}
unsigned char miku_log_get_level(void) { return 2; }
void miku_log_show_time(int s) {}
void miku_log(unsigned char l, const char *t, const char *m) {}
void miku_log_error(const char *t, const char *m) {}
void miku_log_warn(const char *t, const char *m) {}
void miku_log_info(const char *t, const char *m) {}
void miku_log_debug(const char *t, const char *m) {}
void miku_log_trace(const char *t, const char *m) {}
void miku_log_int(unsigned char l, const char *t, const char *m, long v) {}

/* strbuf */
typedef struct { void *d; unsigned long l, c; } MikuStr;
MikuStr miku_str_new(void) { MikuStr s = {0}; return s; }
MikuStr miku_str_from(const char *s) { MikuStr r = {0}; return r; }
MikuStr miku_str_with_capacity(unsigned long c) { MikuStr s = {0}; return s; }
void miku_str_free(MikuStr *s) {}
const char *miku_str_cstr(const MikuStr *s) { return 0; }
unsigned long miku_str_len(const MikuStr *s) { return 0; }
int miku_str_empty(const MikuStr *s) { return 1; }
int miku_str_push(MikuStr *s, const char *t) { return 0; }
int miku_str_push_char(MikuStr *s, unsigned char c) { return 0; }
int miku_str_push_bytes(MikuStr *s, const void *d, unsigned long l) { return 0; }
int miku_str_push_int(MikuStr *s, long v) { return 0; }
void miku_str_clear(MikuStr *s) {}
int miku_str_eq(const MikuStr *s, const char *o) { return 0; }
int miku_str_starts_with(const MikuStr *s, const char *p) { return 0; }
int miku_str_ends_with(const MikuStr *s, const char *p) { return 0; }
int miku_str_find(const MikuStr *s, const char *n) { return -1; }
int miku_str_contains(const MikuStr *s, const char *n) { return 0; }
unsigned char miku_str_at(const MikuStr *s, unsigned long i) { return 0; }
void miku_str_trim(MikuStr *s) {}
void miku_str_to_upper(MikuStr *s) {}
void miku_str_to_lower(MikuStr *s) {}
MikuStr miku_str_substr(const MikuStr *s, unsigned long start, unsigned long len) { MikuStr r = {0}; return r; }
MikuStr miku_str_clone(const MikuStr *s) { MikuStr r = {0}; return r; }

/* sha256 */
typedef struct { unsigned int st[8]; unsigned char buf[64]; unsigned long bl, tl; } MikuSha256;
void miku_sha256_init(MikuSha256 *c) {}
void miku_sha256_update(MikuSha256 *c, const void *d, unsigned long l) {}
void miku_sha256_finish(MikuSha256 *c, void *o) {}
void miku_sha256(const void *d, unsigned long l, void *o) {}
int miku_sha256_eq(const void *a, const void *b) { return 0; }
void miku_sha256_hex(const void *h, char *o) {}

/* uuid */
typedef struct { unsigned char b[16]; } MikuUuid;
MikuUuid miku_uuid_gen(void) { MikuUuid u = {{0}}; return u; }
char *miku_uuid_format(const MikuUuid *u, char *b) { return b; }
int miku_uuid_parse(const char *s, MikuUuid *u) { return 0; }
int miku_uuid_eq(const MikuUuid *a, const MikuUuid *b) { return 0; }
int miku_uuid_is_nil(const MikuUuid *u) { return 1; }
MikuUuid miku_uuid_nil(void) { MikuUuid u = {{0}}; return u; }

/* test framework */
void miku_test_reset(void) {}
int miku_test(const char *n, int c) { return c; }
int miku_test_eq(const char *n, long a, long e) { return a == e; }
int miku_test_streq(const char *n, const char *a, const char *e) { return 0; }
int miku_test_not_null(const char *n, const void *p) { return p != 0; }
int miku_test_null(const char *n, const void *p) { return p == 0; }
void miku_test_suite(const char *n) {}
int miku_test_summary(void) { return 0; }
unsigned int miku_test_passed(void) { return 0; }
unsigned int miku_test_failed(void) { return 0; }
unsigned int miku_test_total(void) { return 0; }

/* bufio */
typedef struct { long fd; void *buf; unsigned long cap, pos, filled; } MikuBufReader;
typedef struct { long fd; void *buf; unsigned long cap, pos; unsigned int mode; } MikuBufWriter;
MikuBufReader miku_bufreader_new(long fd) { MikuBufReader r = {0}; return r; }
MikuBufReader miku_bufreader_with_capacity(long fd, unsigned long cap) { MikuBufReader r = {0}; return r; }
void miku_bufreader_free(MikuBufReader *r) {}
long miku_bufreader_read(MikuBufReader *r, void *dst, unsigned long len) { return 0; }
int miku_bufreader_getc(MikuBufReader *r) { return -1; }
long miku_bufreader_readline(MikuBufReader *r, void *dst, unsigned long max) { return 0; }
int miku_bufreader_peek(MikuBufReader *r) { return -1; }
unsigned long miku_bufreader_buffered(const MikuBufReader *r) { return 0; }
MikuBufWriter miku_bufwriter_new(long fd) { MikuBufWriter w = {0}; return w; }
MikuBufWriter miku_bufwriter_with_capacity(long fd, unsigned long cap) { MikuBufWriter w = {0}; return w; }
void miku_bufwriter_set_mode(MikuBufWriter *w, unsigned int mode) {}
long miku_bufwriter_flush(MikuBufWriter *w) { return 0; }
long miku_bufwriter_write(MikuBufWriter *w, const void *src, unsigned long len) { return 0; }
int miku_bufwriter_putc(MikuBufWriter *w, unsigned char c) { return -1; }
long miku_bufwriter_puts(MikuBufWriter *w, const char *s) { return 0; }
unsigned long miku_bufwriter_pending(const MikuBufWriter *w) { return 0; }
void miku_bufwriter_free(MikuBufWriter *w) {}

/* dir */
typedef struct { char path[256]; unsigned long path_len; char _entries[1152]; unsigned long total, cursor; int done; } MikuDir;
MikuDir miku_dir_open(const char *p, unsigned long l) { MikuDir d = {{0}}; d.done = 1; return d; }
void miku_dir_close(MikuDir *d) {}
int miku_dir_next(MikuDir *d, void *ent) { return 0; }
int miku_dir_is_open(const MikuDir *d) { return 0; }
long miku_dir_count(const char *p, unsigned long l) { return 0; }
int miku_dir_walk(const char *p, unsigned long l, void *cb, void *ctx) { return 0; }
int miku_is_directory(const char *p) { return 0; }
long miku_mkdir_p(const char *p, unsigned int mode) { return 0; }

/* datetime */
typedef struct { int year; unsigned char month, day, hour, minute, second, weekday; unsigned short yearday; } MikuDateTime;
MikuDateTime miku_dt_from_timestamp(long ts) { MikuDateTime dt = {0}; return dt; }
long miku_dt_to_timestamp(const MikuDateTime *dt) { return 0; }
MikuDateTime miku_dt_now(void) { MikuDateTime dt = {0}; return dt; }
unsigned long miku_dt_format(const MikuDateTime *dt, char *buf, unsigned long len) { return 0; }
unsigned long miku_dt_format_date(const MikuDateTime *dt, char *buf, unsigned long len) { return 0; }
unsigned long miku_dt_format_time(const MikuDateTime *dt, char *buf, unsigned long len) { return 0; }
unsigned long miku_dt_format_iso(const MikuDateTime *dt, char *buf, unsigned long len) { return 0; }
const char *miku_dt_weekday_name(unsigned char day) { return ""; }
const char *miku_dt_month_name(unsigned char month) { return ""; }
long miku_dt_diff_secs(const MikuDateTime *a, const MikuDateTime *b) { return 0; }
MikuDateTime miku_dt_add_secs(const MikuDateTime *dt, long secs) { MikuDateTime r = {0}; return r; }
MikuDateTime miku_dt_add_days(const MikuDateTime *dt, int days) { MikuDateTime r = {0}; return r; }

/* trie */
typedef struct { void *nodes; unsigned long count, cap; } MikuTrie;
MikuTrie miku_trie_new(void) { MikuTrie t = {0}; return t; }
void miku_trie_free(MikuTrie *t) {}
int miku_trie_insert(MikuTrie *t, const char *k, unsigned long l, unsigned long v) { return 0; }
int miku_trie_search(const MikuTrie *t, const char *k, unsigned long l) { return 0; }
unsigned long miku_trie_get(const MikuTrie *t, const char *k, unsigned long l) { return 0; }
int miku_trie_has_prefix(const MikuTrie *t, const char *p, unsigned long l) { return 0; }
int miku_trie_remove(MikuTrie *t, const char *k, unsigned long l) { return 0; }
unsigned long miku_trie_prefix_collect(const MikuTrie *t, const char *p, unsigned long pl, char *buf, unsigned long bl, unsigned long max) { return 0; }
unsigned long miku_trie_node_count(const MikuTrie *t) { return 0; }

/* args */
typedef struct { char _pad[8192]; } MikuArgs;
MikuArgs miku_args_new(void) { MikuArgs a = {{0}}; return a; }
void miku_args_flag(MikuArgs *a, unsigned char s, const char *l, const char *h) {}
void miku_args_option(MikuArgs *a, unsigned char s, const char *l, const char *h) {}
void miku_args_int_option(MikuArgs *a, unsigned char s, const char *l, const char *h) {}
int miku_args_parse(MikuArgs *a, const void *argv, unsigned long argc) { return 0; }
int miku_args_has(const MikuArgs *a, const char *l) { return 0; }
const char *miku_args_get(const MikuArgs *a, const char *l) { return 0; }
long miku_args_get_int(const MikuArgs *a, const char *l) { return 0; }
unsigned long miku_args_positional_count(const MikuArgs *a) { return 0; }
const char *miku_args_positional(const MikuArgs *a, unsigned long idx, unsigned long *out_len) { return 0; }
int miku_args_has_error(const MikuArgs *a) { return 0; }
const char *miku_args_error(const MikuArgs *a) { return ""; }

/* queue */
typedef struct { void *data; unsigned long elem_size, cap, head, tail, count; } MikuQueue;
MikuQueue miku_queue_new(unsigned long es, unsigned long cap) { MikuQueue q = {0}; return q; }
void miku_queue_free(MikuQueue *q) {}
int miku_queue_push(MikuQueue *q, const void *e) { return 0; }
int miku_queue_pop(MikuQueue *q, void *e) { return 0; }
int miku_queue_peek(const MikuQueue *q, void *e) { return 0; }
int miku_queue_peek_back(const MikuQueue *q, void *e) { return 0; }
int miku_queue_pop_back(MikuQueue *q, void *e) { return 0; }
unsigned long miku_queue_len(const MikuQueue *q) { return 0; }
unsigned long miku_queue_capacity(const MikuQueue *q) { return 0; }
int miku_queue_is_empty(const MikuQueue *q) { return 1; }
int miku_queue_is_full(const MikuQueue *q) { return 1; }
void miku_queue_clear(MikuQueue *q) {}
int miku_queue_at(const MikuQueue *q, unsigned long idx, void *out) { return 0; }

/* math (extended) */
unsigned long miku_gcd(unsigned long a, unsigned long b) { return 1; }
unsigned long miku_lcm(unsigned long a, unsigned long b) { return 0; }
long miku_pow(long b, unsigned int e) { return 1; }
unsigned long miku_upow(unsigned long b, unsigned int e) { return 1; }
unsigned long miku_isqrt(unsigned long n) { return 0; }
unsigned long miku_icbrt(unsigned long n) { return 0; }
unsigned int miku_ilog2(unsigned long n) { return 0; }
unsigned int miku_ilog10(unsigned long n) { return 0; }
int miku_sign(long x) { return 0; }
long miku_map(long v, long il, long ih, long ol, long oh) { return 0; }
long miku_lerp(long a, long b, unsigned int t) { return a; }
long miku_sadd(long a, long b) { return a; }
long miku_ssub(long a, long b) { return a; }
long miku_smul(long a, long b) { return a; }
unsigned long miku_usadd(unsigned long a, unsigned long b) { return a; }
unsigned long miku_ussub(unsigned long a, unsigned long b) { return a; }
unsigned long miku_div_ceil(unsigned long a, unsigned long b) { return 0; }
unsigned long miku_modpow(unsigned long b, unsigned long e, unsigned long m) { return 0; }
int miku_is_prime(unsigned long n) { return 0; }
unsigned long miku_fib(unsigned int n) { return 0; }
unsigned long miku_factorial(unsigned int n) { return 1; }
unsigned long miku_binomial(unsigned long n, unsigned long k) { return 0; }

/* endian (extended) */
void miku_write_u64_be(void *p, unsigned long v) {}
void miku_write_u64_le(void *p, unsigned long v) {}

/* strbuf (extended) */
int miku_str_insert(void *s, unsigned long pos, const char *t) { return 0; }
int miku_str_remove(void *s, unsigned long start, unsigned long count) { return 0; }
int miku_str_replace(void *s, const char *n, const char *r) { return 0; }
int miku_str_replace_all(void *s, const char *n, const char *r) { return 0; }
void miku_str_reverse(void *s) {}
int miku_str_count(const void *s, const char *n) { return 0; }
void *miku_str_repeat_new(const char *t, unsigned int n);
int miku_str_split(const void *s, unsigned char d, void (*cb)(const void*, unsigned long, unsigned long), unsigned long ud) { return 0; }

/* glob (extended) */
unsigned long miku_glob_filter(const char *p, const char **ss, unsigned long c, const char **o) { return 0; }

/* sort (extended) */
unsigned long miku_lower_bound(const void *k, const void *b, unsigned long c, unsigned long sz, int(*cmp)(const void*,const void*)) { return 0; }
unsigned long miku_upper_bound(const void *k, const void *b, unsigned long c, unsigned long sz, int(*cmp)(const void*,const void*)) { return 0; }
unsigned long miku_unique(void *b, unsigned long c, unsigned long sz, int(*cmp)(const void*,const void*)) { return 0; }
void miku_nth_element(void *b, unsigned long c, unsigned long sz, unsigned long n, int(*cmp)(const void*,const void*)) {}
int miku_cmp_i32(const void *a, const void *b) { return 0; }
int miku_cmp_u32(const void *a, const void *b) { return 0; }

/* list (extended) */
int miku_list_index_of(const void *l, const void *e) { return -1; }
void miku_list_reverse(void *l) {}
unsigned long miku_list_remove_if(void *l, int (*pred)(const void*)) { return 0; }
const void *miku_list_front(const void *l) { return 0; }
const void *miku_list_back(const void *l) { return 0; }

/* path (extended) */
int miku_path_has_ext(const char *p, const char *e) { return 0; }
char *miku_path_common(const char *a, const char *b) { return 0; }
int miku_path_is_relative(const char *p) { return 1; }
char *miku_path_parent(const char *p) { return 0; }

/* json (extended) */
int miku_json_array_get(const void *t, unsigned long nt, unsigned long ai, unsigned long ei) { return -1; }
long miku_json_int(const char *d, const void *t, unsigned long i) { return 0; }
unsigned long miku_json_u64(const char *d, const void *t, unsigned long i) { return 0; }
int miku_json_bool(const char *d, const void *t, unsigned long i) { return -1; }
int miku_json_is_null(const void *t, unsigned long i) { return 0; }
int miku_json_strcpy(const char *d, const void *t, unsigned long i, char *b, unsigned long bs) { return -1; }
int miku_json_unescape(const char *d, const void *t, unsigned long i, char *b, unsigned long bs) { return -1; }
int miku_json_get_path(const char *d, const void *t, unsigned long nt, unsigned long r, const char *p) { return -1; }
int miku_json_object_iter(const void *t, unsigned long nt, unsigned long oi, void (*cb)(unsigned long, unsigned long, void*), void *u) { return -1; }
int miku_json_array_iter(const void *t, unsigned long nt, unsigned long ai, void (*cb)(unsigned long, unsigned long, void*), void *u) { return -1; }
char *miku_json_strdup(const char *d, const void *t, unsigned long i) { return 0; }
int miku_json_validate(const char *d, unsigned long l) { return -1; }
int miku_json_parent(const void *t, unsigned long i) { return -1; }
int miku_json_is_string(const void *t, unsigned long i) { return 0; }
int miku_json_is_number(const void *t, unsigned long i) { return 0; }
int miku_json_is_bool(const void *t, unsigned long i) { return 0; }
int miku_json_is_object(const void *t, unsigned long i) { return 0; }
int miku_json_is_array(const void *t, unsigned long i) { return 0; }

/* json writer */
typedef struct { char *b; unsigned long c, l, d; unsigned char cs[32]; unsigned char nc[32]; unsigned char e; } MikuJsonWriter;
void miku_json_writer_init(MikuJsonWriter *w, char *b, unsigned long c) {}
void miku_json_write_object_begin(MikuJsonWriter *w) {}
void miku_json_write_object_end(MikuJsonWriter *w) {}
void miku_json_write_array_begin(MikuJsonWriter *w) {}
void miku_json_write_array_end(MikuJsonWriter *w) {}
void miku_json_write_key(MikuJsonWriter *w, const char *k) {}
void miku_json_write_str(MikuJsonWriter *w, const char *v) {}
void miku_json_write_strn(MikuJsonWriter *w, const char *v, unsigned long l) {}
void miku_json_write_int(MikuJsonWriter *w, long v) {}
void miku_json_write_u64(MikuJsonWriter *w, unsigned long v) {}
void miku_json_write_bool(MikuJsonWriter *w, int v) {}
void miku_json_write_null(MikuJsonWriter *w) {}
void miku_json_write_raw(MikuJsonWriter *w, const char *r, unsigned long l) {}
int miku_json_write_finish(MikuJsonWriter *w) { return -1; }
int miku_json_write_error(const MikuJsonWriter *w) { return 1; }
void miku_json_write_kv_str(MikuJsonWriter *w, const char *k, const char *v) {}
void miku_json_write_kv_int(MikuJsonWriter *w, const char *k, long v) {}
void miku_json_write_kv_bool(MikuJsonWriter *w, const char *k, int v) {}
void miku_json_write_kv_null(MikuJsonWriter *w, const char *k) {}

/* datetime (extended) */
int miku_dt_valid(const void *dt) { return 0; }
int miku_dt_is_leap_year(int y) { return 0; }
unsigned char miku_dt_days_in_month(unsigned char m, int y) { return 0; }
unsigned short miku_dt_days_in_year(int y) { return 365; }
const char *miku_dt_weekday_short(unsigned char d) { return "???"; }
const char *miku_dt_month_short(unsigned char m) { return "???"; }
int miku_dt_cmp(const void *a, const void *b) { return 0; }
unsigned long miku_dt_format_rfc2822(const void *dt, char *b, unsigned long bl) { return 0; }

/* log (extended) */
void miku_log_hex(unsigned char l, const char *t, const char *m, unsigned long v) {}
void miku_log_ptr(unsigned char l, const char *t, const char *m, const void *p) {}
void miku_log_int2(unsigned char l, const char *t, const char *m, long a, long b) {}

/* sync (C API) */
typedef struct { unsigned char _locked; } MikuMutex;
void miku_mutex_init(MikuMutex *m) {}
void miku_mutex_lock(MikuMutex *m) {}
void miku_mutex_unlock(MikuMutex *m) {}
int miku_mutex_trylock(MikuMutex *m) { return 0; }
int miku_mutex_is_locked(const MikuMutex *m) { return 0; }
typedef struct { long _val; } MikuAtomic;
void miku_atomic_init(MikuAtomic *a, long v) {}
long miku_atomic_load(const MikuAtomic *a) { return 0; }
void miku_atomic_store(MikuAtomic *a, long v) {}
long miku_atomic_add(MikuAtomic *a, long v) { return 0; }
long miku_atomic_sub(MikuAtomic *a, long v) { return 0; }
int miku_atomic_cas(MikuAtomic *a, long exp, long des) { return 0; }
long miku_atomic_swap(MikuAtomic *a, long v) { return 0; }
typedef struct { unsigned char _d, _r; } MikuOnce;
void miku_once_init(MikuOnce *o) {}
void miku_once_call(MikuOnce *o, void (*f)(void)) {}
int miku_once_done(const MikuOnce *o) { return 0; }

/* convert (extended) */
char *miku_itoa_base(long v, char *b, int base) { return b; }
char *miku_utoa_base(unsigned long v, char *b, int base) { return b; }

/* regex (extended) */
int miku_regex_find_span(const char *p, const char *t, unsigned long *os, unsigned long *ol) { return 0; }
char *miku_regex_replace(const char *p, const char *t, const char *r) { return 0; }
char *miku_regex_replace_all(const char *p, const char *t, const char *r) { return 0; }
unsigned long miku_regex_split(const char *p, const char *t, const char **os, unsigned long *ol, unsigned long m) { return 0; }
unsigned long miku_regex_find_all(const char *p, const char *t, unsigned long *os, unsigned long *ol, unsigned long m) { return 0; }

/* panic (extended) */
void miku_assert_eq(long a, long b, const char *f, int l) {}
void miku_assert_not_null(const void *p, const char *n, const char *f, int l) {}
void miku_unreachable(const char *f, int l) { for(;;); }
void miku_todo(const char *m) { for(;;); }

/* base64 (extended) */
int miku_base64_is_valid(const char *input, unsigned long len) { return 1; }

/* uuid (extended) */
unsigned char miku_uuid_version(const void *uuid) { return 0; }
unsigned char miku_uuid_variant(const void *uuid) { return 0; }
int miku_uuid_cmp(const void *a, const void *b) { return 0; }

/* sha256 (extended) */
void miku_sha256_hmac(const void *key, unsigned long kl, const void *data, unsigned long dl, void *out) {}

/* random (extended) */
int miku_rand_bool(void) { return 0; }
unsigned long miku_rand_uniform(unsigned long bound) { return 0; }
long miku_rand_i64(long lo, long hi) { return lo; }
unsigned long miku_rand_frac_million(void) { return 0; }
unsigned int miku_rand_dice(unsigned int sides) { return 1; }
unsigned long miku_rand_sample(unsigned long n, unsigned long k, unsigned long *out) { return 0; }
unsigned long miku_rand_weighted(const unsigned long *weights, unsigned long n) { return 0; }
void miku_rand_perm(unsigned long n, unsigned long *out) {}
/* checksum (extended) */
unsigned short miku_crc16(const void *data, unsigned long len) { return 0; }
unsigned short miku_crc16_update(unsigned short prev, const void *data, unsigned long len) { return 0; }
int miku_luhn_check(const void *data, unsigned long len) { return 0; }
unsigned char miku_luhn_digit(const void *data, unsigned long len) { return 0; }
unsigned char miku_parity8(unsigned char byte) { return 0; }
unsigned char miku_parity(const void *data, unsigned long len) { return 0; }
unsigned short miku_sysv_checksum(const void *data, unsigned long len) { return 0; }
unsigned int miku_crc32_combine(unsigned int crc1, unsigned int crc2, unsigned long len2) { return 0; }

/* csv (extended) */
unsigned long miku_csv_field_u64(const void *csv, const void *data, unsigned long row, unsigned long col, unsigned long def) { return def; }
int miku_csv_field_empty(const void *csv, unsigned long row, unsigned long col) { return 1; }
int miku_csv_find_col(const void *csv, const void *data, const void *name) { return -1; }
typedef struct { void *buf; unsigned long cap; unsigned long len; unsigned long col; unsigned char delim; int error; } MikuCsvWriter;
MikuCsvWriter miku_csv_writer_new(unsigned char delim) { MikuCsvWriter w = {0}; return w; }
MikuCsvWriter miku_csv_writer_init(void *buf, unsigned long cap, unsigned char delim) { MikuCsvWriter w = {0}; return w; }
void miku_csv_write_field(MikuCsvWriter *w, const void *data, unsigned long len) {}
void miku_csv_write_cstr(MikuCsvWriter *w, const void *s) {}
void miku_csv_write_int(MikuCsvWriter *w, long val) {}
void miku_csv_write_row_end(MikuCsvWriter *w) {}
unsigned long miku_csv_writer_len(const MikuCsvWriter *w) { return 0; }
const void *miku_csv_writer_data(const MikuCsvWriter *w) { return 0; }
int miku_csv_writer_error(const MikuCsvWriter *w) { return 0; }
void miku_csv_writer_free(MikuCsvWriter *w) {}
void miku_csv_writer_reset(MikuCsvWriter *w) {}
void miku_csv_foreach_row(const void *csv, const void *data, void (*cb)(unsigned long, const void*, const void*, void*), void *ctx) {}

/* lz (extended) */
int miku_rle_compress(const void *input, unsigned long ilen, void *out, unsigned long omax) { return -1; }
int miku_rle_decompress(const void *input, unsigned long ilen, void *out, unsigned long omax) { return -1; }
unsigned long miku_rle_compress_bound(unsigned long ilen) { return 0; }
void miku_delta_encode(const void *input, unsigned long len, void *out) {}
void miku_delta_decode(const void *input, unsigned long len, void *out) {}

/* event (extended) */
int miku_event_once(unsigned int event_id, void (*handler)(unsigned int, void*, void*), void *ctx) { return -1; }
int miku_event_post(unsigned int event_id, void *data) { return -1; }
unsigned int miku_event_flush(void) { return 0; }
unsigned long miku_event_pending(void) { return 0; }
void miku_event_queue_clear(void) {}

/* ===== libc compatibility layer stubs ===== */

/* errno.h */
long *__errno_location(void) { static long e = 0; return &e; }
const char *strerror(long errnum) { return ""; }
void perror(const char *s) {}

/* string.h */
unsigned long strlen(const char *s) { return 0; }
unsigned long strnlen(const char *s, unsigned long maxlen) { return 0; }
int strcmp(const char *s1, const char *s2) { return 0; }
int strncmp(const char *s1, const char *s2, unsigned long n) { return 0; }
int strcasecmp(const char *s1, const char *s2) { return 0; }
int strncasecmp(const char *s1, const char *s2, unsigned long n) { return 0; }
char *strcpy(char *d, const char *s) { return d; }
char *strncpy(char *d, const char *s, unsigned long n) { return d; }
unsigned long strlcpy(char *d, const char *s, unsigned long n) { return 0; }
char *strcat(char *d, const char *s) { return d; }
char *strncat(char *d, const char *s, unsigned long n) { return d; }
unsigned long strlcat(char *d, const char *s, unsigned long n) { return 0; }
const char *strchr(const char *s, int c) { return 0; }
const char *strrchr(const char *s, int c) { return 0; }
const char *strstr(const char *h, const char *n) { return 0; }
const char *strpbrk(const char *s, const char *a) { return 0; }
unsigned long strspn(const char *s, const char *a) { return 0; }
unsigned long strcspn(const char *s, const char *r) { return 0; }
char *strdup(const char *s) { return 0; }
char *strndup(const char *s, unsigned long n) { return 0; }
char *strtok(char *s, const char *d) { return 0; }
char *strtok_r(char *s, const char *d, char **sp) { return 0; }
char *strsep(char **sp, const char *d) { return 0; }
void bzero(void *d, unsigned long n) {}
const void *memmem(const void *h, unsigned long hl, const void *n, unsigned long nl) { return 0; }

/* stdlib.h */
void *malloc(unsigned long s) { return 0; }
void free(void *p) {}
void *realloc(void *p, unsigned long s) { return 0; }
void *calloc(unsigned long c, unsigned long s) { return 0; }
void *aligned_alloc(unsigned long a, unsigned long s) { return 0; }
int atoi(const char *s) { return 0; }
long atol(const char *s) { return 0; }
long atoll(const char *s) { return 0; }
long strtol(const char *s, const char **e, int b) { return 0; }
unsigned long strtoul(const char *s, const char **e, int b) { return 0; }
long strtoll(const char *s, const char **e, int b) { return 0; }
unsigned long strtoull(const char *s, const char **e, int b) { return 0; }
int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }
long llabs(long x) { return x < 0 ? -x : x; }
void exit(int code) { for(;;); }
void _exit(int code) { for(;;); }
void _Exit(int code) { for(;;); }
void abort(void) { for(;;); }
const char *getenv(const char *k) { return 0; }
int setenv(const char *k, const char *v, int o) { return -1; }
int unsetenv(const char *k) { return -1; }
int putenv(const char *s) { return -1; }
void srand(unsigned int s) {}
int rand(void) { return 0; }
void qsort(void *b, unsigned long n, unsigned long s, int (*cmp)(const void*, const void*)) {}
const void *bsearch(const void *k, const void *b, unsigned long n, unsigned long s, int (*cmp)(const void*, const void*)) { return 0; }
int posix_memalign(void **m, unsigned long a, unsigned long s) { return -1; }

/* stdio.h */
typedef struct { long fd; unsigned int fl; int er; int ef; int ug; void *rb; unsigned long rc; unsigned long rp; unsigned long rf; void *wb; unsigned long wc; unsigned long wp; unsigned int bm; } FILE;
FILE *stdin_ptr = 0;
FILE *stdout_ptr = 0;
FILE *stderr_ptr = 0;
FILE *fopen(const char *p, const char *m) { return 0; }
int fclose(FILE *f) { return 0; }
unsigned long fread(void *p, unsigned long s, unsigned long n, FILE *f) { return 0; }
unsigned long fwrite(const void *p, unsigned long s, unsigned long n, FILE *f) { return 0; }
int fgetc(FILE *f) { return -1; }
int fputc(int c, FILE *f) { return c; }
int getc(FILE *f) { return -1; }
int putc(int c, FILE *f) { return c; }
int putchar(int c) { return c; }
int getchar(void) { return -1; }
char *fgets(char *b, int s, FILE *f) { return 0; }
int fputs(const char *s, FILE *f) { return 0; }
int puts(const char *s) { return 0; }
int fseek(FILE *f, long o, int w) { return -1; }
long ftell(FILE *f) { return -1; }
void rewind(FILE *f) {}
int feof(FILE *f) { return 0; }
int ferror(FILE *f) { return 0; }
void clearerr(FILE *f) {}
int fflush(FILE *f) { return 0; }
int fileno(FILE *f) { return -1; }
FILE *fdopen(int fd, const char *m) { return 0; }
int ungetc(int c, FILE *f) { return c; }
int setvbuf(FILE *f, char *b, int m, unsigned long s) { return 0; }
void setbuf(FILE *f, char *b) {}
int printf(const char *f, ...) { return 0; }
int fprintf(FILE *f, const char *fmt, ...) { return 0; }
int dprintf(int fd, const char *fmt, ...) { return 0; }
int snprintf(char *b, unsigned long m, const char *f, ...) { return 0; }
int sprintf(char *b, const char *f, ...) { return 0; }

/* ctype.h */
int isdigit(int c) { return 0; }
int isalpha(int c) { return 0; }
int isalnum(int c) { return 0; }
int isspace(int c) { return 0; }
int isupper(int c) { return 0; }
int islower(int c) { return 0; }
int isprint(int c) { return 0; }
int ispunct(int c) { return 0; }
int iscntrl(int c) { return 0; }
int isxdigit(int c) { return 0; }
int isgraph(int c) { return 0; }
int isblank(int c) { return 0; }
int isascii(int c) { return 0; }
int toupper(int c) { return c; }
int tolower(int c) { return c; }
int toascii(int c) { return c & 0x7F; }

/* unistd.h */
long read(int fd, void *b, unsigned long c) { return 0; }
long write(int fd, const void *b, unsigned long c) { return 0; }
int close(int fd) { return 0; }
long lseek(int fd, long o, int w) { return 0; }
int open(const char *p, int f, unsigned int m) { return -1; }
int creat(const char *p, unsigned int m) { return -1; }
int dup(int fd) { return -1; }
int dup2(int o, int n) { return -1; }
int pipe(int *fds) { return -1; }
int unlink(const char *p) { return -1; }
int rmdir(const char *p) { return -1; }
int mkdir(const char *p, unsigned int m) { return -1; }
int link(const char *o, const char *n) { return -1; }
int symlink(const char *t, const char *l) { return -1; }
long readlink(const char *p, char *b, unsigned long s) { return -1; }
int rename(const char *o, const char *n) { return -1; }
char *getcwd(char *b, unsigned long s) { return 0; }
int chdir(const char *p) { return -1; }
int getpid(void) { return 0; }
int access(const char *p, int m) { return -1; }
unsigned int sleep(unsigned int s) { return 0; }
int usleep(unsigned int u) { return 0; }
int ftruncate(int fd, long l) { return -1; }
int truncate(const char *p, long l) { return -1; }
long pread(int fd, void *b, unsigned long c, long o) { return 0; }
long pwrite(int fd, const void *b, unsigned long c, long o) { return 0; }
int sched_yield(void) { return 0; }
int remove(const char *p) { return -1; }

/* sys/stat.h */
int stat_path(const char *p, MikuStat *s) { return -1; }
int fstat(int fd, MikuStat *s) { return -1; }
int chmod(const char *p, unsigned int m) { return -1; }
int chown(const char *p, unsigned int u, unsigned int g) { return -1; }

/* sys/mman.h */
void *mmap(void *a, unsigned long l, int p, int f, int fd, long o) { return (void*)-1; }
int munmap(void *a, unsigned long l) { return -1; }
int mprotect(void *a, unsigned long l, int p) { return -1; }
int brk(void *a) { return -1; }
void *sbrk(long i) { return (void*)-1; }

/* signal.h */
int raise(int s) { return -1; }
int sigaction(int s, const void *a, void *o) { return -1; }

/* time.h */
typedef struct { long tv_sec; long tv_nsec; } timespec;
int nanosleep(const timespec *req, timespec *rem) { return 0; }
int clock_gettime(int clockid, timespec *tp) { return 0; }

/* dirent.h */
typedef struct { char path[256]; MikuDirent entries[128]; unsigned long count; unsigned long pos; } DIR;
DIR *opendir(const char *p) { return 0; }
const MikuDirent *readdir(DIR *d) { return 0; }
int closedir(DIR *d) { return -1; }
void rewinddir(DIR *d) {}

"#;
