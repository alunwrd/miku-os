// miku: the system interface crate
//
// Declarations for the shared libraries' exports and the stub from `_start` to
// `_start_main`. The build script generates link stubs that `ld-miku` resolves.

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod heap;
pub mod time;

// Enables `Vec`, `String`, and `Box` over the process heap in core_miku.so
#[global_allocator]
static ALLOC: heap::MikuAlloc = heap::MikuAlloc;

pub type CmpFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;

#[link(name = "core_miku")]
extern "C" {
    // io
    pub fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64;
    pub fn miku_read(fd: u64, buf: *mut u8, len: usize) -> i64;

    // TCP and UDP sockets; fd works with miku_read/write/close
    pub fn miku_socket() -> i64;
    pub fn miku_socket_udp() -> i64;
    pub fn miku_connect(fd: i64, ip: *const u8, port: u16) -> i64;
    pub fn miku_send(fd: i64, buf: *const u8, len: usize) -> i64;
    pub fn miku_recv(fd: i64, buf: *mut u8, len: usize) -> i64;
    pub fn miku_bind(fd: i64, port: u16) -> i64;
    pub fn miku_listen(fd: i64, backlog: i64) -> i64;
    pub fn miku_accept(fd: i64, ip: *mut u8, port: *mut u16) -> i64;
    pub fn miku_sendto(fd: i64, buf: *const u8, len: usize, ip: *const u8, port: u16) -> i64;
    pub fn miku_recvfrom(fd: i64, buf: *mut u8, len: usize, src_ip: *mut u8, src_port: *mut u16) -> i64;
    /// An unconnected local (AF_UNIX) stream socket
    pub fn miku_socket_unix() -> i64;
    /// Give a local socket a name. The name goes away when the socket closes
    pub fn miku_bind_unix(fd: i64, path: *const u8) -> i64;
    /// Connects to a named local socket, blocking until accepted
    pub fn miku_connect_unix(fd: i64, path: *const u8) -> i64;
    /// Two connected local sockets, no name and no rendezvous
    pub fn miku_socketpair(fds: *mut u64) -> i64;
    /// Raw ICMP socket. Unprivileged, since the kernel writes the IP header
    pub fn miku_socket_icmp() -> i64;
    /// Hop limit of this socket's packets
    pub fn miku_set_ttl(fd: i64, ttl: u8) -> i64;

    // stdio
    pub fn miku_print(s: *const u8);
    pub fn miku_println(s: *const u8);
    pub fn miku_puts(s: *const u8) -> i32;
    pub fn miku_eprint(s: *const u8);
    pub fn miku_eprintln(s: *const u8);
    pub fn miku_putchar(c: i32) -> i32;
    pub fn miku_getchar() -> i32;
    pub fn miku_readline(buf: *mut u8, max_len: usize) -> i32;
    pub fn miku_getline() -> *mut u8;

    // fmt
    pub fn miku_printf(fmt: *const u8, ...) -> i32;
    pub fn miku_snprintf(buf: *mut u8, max: usize, fmt: *const u8, ...) -> i32;
    pub fn miku_dprintf(fd: u64, fmt: *const u8, ...) -> i32;
    pub fn miku_fprintf(fd: u64, fmt: *const u8, ...) -> i32;

    // num
    pub fn miku_itoa(val: i64, buf: *mut u8);
    pub fn miku_utoa(val: u64, buf: *mut u8);
    pub fn miku_atoi(s: *const u8) -> i64;
    pub fn miku_print_int(val: i64);
    pub fn miku_print_hex(val: u64);

    // string
    pub fn miku_strlen(s: *const u8) -> usize;
    pub fn miku_strcmp(a: *const u8, b: *const u8) -> i32;
    pub fn miku_strncmp(a: *const u8, b: *const u8, n: usize) -> i32;
    pub fn miku_strcpy(dst: *mut u8, src: *const u8) -> *mut u8;
    pub fn miku_strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn miku_strcat(dst: *mut u8, src: *const u8) -> *mut u8;
    pub fn miku_strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn miku_strchr(s: *const u8, c: i32) -> *const u8;
    pub fn miku_strrchr(s: *const u8, c: i32) -> *const u8;
    pub fn miku_strstr(h: *const u8, n: *const u8) -> *const u8;
    pub fn miku_strdup(s: *const u8) -> *mut u8;
    pub fn miku_strndup(s: *const u8, n: usize) -> *mut u8;
    pub fn miku_strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize;
    pub fn miku_strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize;
    pub fn miku_strtok(s: *mut u8, delim: *const u8) -> *mut u8;
    pub fn miku_strtok_r(s: *mut u8, delim: *const u8, saveptr: *mut *mut u8) -> *mut u8;
    pub fn miku_strnlen(s: *const u8, maxlen: usize) -> usize;
    pub fn miku_strcasecmp(a: *const u8, b: *const u8) -> i32;
    pub fn miku_strncasecmp(a: *const u8, b: *const u8, n: usize) -> i32;
    pub fn miku_strsep(stringp: *mut *mut u8, delim: *const u8) -> *mut u8;
    pub fn miku_strpbrk(s: *const u8, accept: *const u8) -> *const u8;
    pub fn miku_strspn(s: *const u8, accept: *const u8) -> usize;
    pub fn miku_strcspn(s: *const u8, reject: *const u8) -> usize;

    // ctype
    pub fn miku_isdigit(c: i32) -> i32;
    pub fn miku_isalpha(c: i32) -> i32;
    pub fn miku_isalnum(c: i32) -> i32;
    pub fn miku_isspace(c: i32) -> i32;
    pub fn miku_isupper(c: i32) -> i32;
    pub fn miku_islower(c: i32) -> i32;
    pub fn miku_isprint(c: i32) -> i32;
    pub fn miku_ispunct(c: i32) -> i32;
    pub fn miku_iscntrl(c: i32) -> i32;
    pub fn miku_isxdigit(c: i32) -> i32;
    pub fn miku_toupper(c: i32) -> i32;
    pub fn miku_tolower(c: i32) -> i32;
    pub fn miku_isgraph(c: i32) -> i32;
    pub fn miku_isblank(c: i32) -> i32;
    pub fn miku_isascii(c: i32) -> i32;
    pub fn miku_toascii(c: i32) -> i32;

    // convert
    pub fn miku_strtol(s: *const u8, endptr: *mut *const u8, base: i32) -> i64;
    pub fn miku_strtoul(s: *const u8, endptr: *mut *const u8, base: i32) -> u64;

    // mem
    pub fn miku_memset(dst: *mut u8, val: i32, n: usize) -> *mut u8;
    pub fn miku_memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn miku_memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn miku_memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    pub fn miku_bzero(dst: *mut u8, n: usize);
    pub fn miku_memchr(s: *const u8, c: i32, n: usize) -> *const u8;
    pub fn miku_memrchr(s: *const u8, c: i32, n: usize) -> *const u8;
    pub fn miku_memmem(h: *const u8, hlen: usize, n: *const u8, nlen: usize) -> *const u8;

    // heap
    pub fn miku_malloc(size: usize) -> *mut u8;
    pub fn miku_free(ptr: *mut u8);
    pub fn miku_realloc(ptr: *mut u8, new_size: usize) -> *mut u8;
    pub fn miku_calloc(count: usize, size: usize) -> *mut u8;
    pub fn miku_memalign(align: usize, size: usize) -> *mut u8;

    // proc
    pub fn miku_exit(code: i64) -> !;
    pub fn miku_getpid() -> u64;
    pub fn miku_getcwd(buf: *mut u8, size: usize) -> *mut u8;
    pub fn miku_brk(addr: u64) -> u64;
    pub fn miku_mmap(addr: u64, len: usize, prot: u64) -> *mut u8;
    pub fn miku_munmap(addr: *mut u8, len: usize) -> i64;
    pub fn miku_mprotect(addr: u64, len: usize, prot: u64) -> i64;
    pub fn miku_mmap_file(addr: u64, len: usize, prot: u64, flags: u64, fd: i64, offset: u64) -> *mut u8;
    pub fn miku_msync(addr: *mut u8, len: usize) -> i64;
    pub fn miku_set_tls(addr: u64) -> i64;
    pub fn miku_get_tls() -> u64;
    pub fn miku_map_lib(name: *const u8, name_len: usize) -> i64;

    // Threads and futex (`lib/libmiku/thread.rs`)
    pub fn miku_thread_spawn(
        entry: extern "C" fn(*mut u8) -> i32,
        arg: *mut u8,
        stack: *mut u8,
        stack_size: usize,
    ) -> i64;
    pub fn miku_thread_exit(code: i32) -> !;
    pub fn miku_gettid() -> i64;
    pub fn miku_sched_yield() -> i64;
    pub fn miku_futex_wait(addr: *const u32, expected: u32, timeout_ns: u64) -> i64;
    pub fn miku_futex_wake(addr: *const u32, count: u32) -> i64;

    // file
    pub fn miku_open(path: *const u8, len: usize, flags: u32, mode: u32) -> i64;
    pub fn miku_open_cstr(path: *const u8) -> i64;
    pub fn miku_open_rw(path: *const u8) -> i64;
    pub fn miku_create(path: *const u8, mode: u32) -> i64;
    pub fn miku_close(fd: i64) -> i64;
    pub fn miku_seek(fd: i64, offset: u64) -> i64;
    pub fn miku_lseek(fd: i64, offset: i64, whence: u64) -> i64;
    pub fn miku_fsize(fd: i64) -> i64;
    pub fn miku_read_file(path: *const u8, out_size: *mut usize) -> *mut u8;
    pub fn miku_read_fd(fd: i64, buf: *mut u8, len: usize) -> i64;
    pub fn miku_write_fd(fd: i64, buf: *const u8, len: usize) -> i64;

    // filesystem
    pub fn miku_stat(path: *const u8, st: *mut MikuStat) -> i64;
    pub fn miku_fstat(fd: i64, st: *mut MikuStat) -> i64;
    /// `stat` without following a symlink
    pub fn miku_lstat(path: *const u8, st: *mut MikuStat) -> i64;
    /// Changes descriptor flags: FD_CLOEXEC and `O_NONBLOCK`
    pub fn miku_fcntl(fd: i64, cmd: u64, arg: i64) -> i64;
    pub fn miku_set_nonblocking(fd: i64, on: bool) -> i64;
    pub fn miku_mkdir(path: *const u8, mode: u32) -> i64;
    pub fn miku_rmdir(path: *const u8) -> i64;
    pub fn miku_unlink(path: *const u8) -> i64;
    pub fn miku_readdir(path: *const u8, entries: *mut MikuDirent, max: usize) -> i64;
    /// Entries of an open directory from `offset`. Prefer over `miku_readdir`,
    /// which has no cursor
    pub fn miku_getdents(fd: i64, entries: *mut MikuDirent, max: usize, offset: usize) -> i64;
    pub fn miku_rename(old: *const u8, new: *const u8) -> i64;
    pub fn miku_link(old: *const u8, new: *const u8) -> i64;
    pub fn miku_symlink(target: *const u8, linkpath: *const u8) -> i64;
    pub fn miku_readlink(path: *const u8, buf: *mut u8, buf_len: usize) -> i64;
    pub fn miku_chmod(path: *const u8, mode: u32) -> i64;
    pub fn miku_chown(path: *const u8, uid: u32, gid: u32) -> i64;
    /// File times (`utimensat`, 40): `path`, or the file on `fd` when `path` is
    /// null. `times` is two (sec, nsec) pairs, atime then mtime; null sets both
    /// to now. `flags` takes AT_SYMLINK_NOFOLLOW
    pub fn miku_utimensat(path: *const u8, fd: i32, times: *const i64, flags: i32) -> i64;
    /// Flushes everything written to `fd` to the medium
    pub fn miku_fsync(fd: i64) -> i64;
    /// Reserves disk space
    pub fn miku_fallocate(fd: i64, offset: u64, len: u64) -> i64;
    // Identity and capabilities
    /// Writes a line to the kernel log
    pub fn miku_klog(buf: *const u8, len: usize) -> i64;
    /// TCGETS / TCSETS / TIOCGWINSZ on fds 0..2
    pub fn miku_ioctl(fd: u64, request: u64, argp: u64) -> i64;
    pub fn miku_umask(mask: u64) -> u64;
    pub fn miku_getuid() -> u64;
    pub fn miku_getgid() -> u64;
    pub fn miku_geteuid() -> u64;
    pub fn miku_getegid() -> u64;
    pub fn miku_setuid(uid: u64) -> i64;
    pub fn miku_setgid(gid: u64) -> i64;
    pub fn miku_seteuid(uid: u64) -> i64;
    pub fn miku_setegid(gid: u64) -> i64;
    pub fn miku_setresuid(r: u64, e: u64, s: u64) -> i64;
    pub fn miku_setresgid(r: u64, e: u64, s: u64) -> i64;
    pub fn miku_setgroups(list: *const u16, count: u64) -> i64;
    pub fn miku_getgroups(list: *mut u16, max: u64) -> i64;
    pub fn miku_capget(buf: *mut u64) -> i64;
    pub fn miku_capset(permitted: u64, effective: u64, bounding: u64) -> i64;
    pub fn miku_session(op: u64, arg: u64, ptr: u64, len: u64) -> i64;
    pub fn miku_tty(op: u64, arg: u64) -> i64;
    pub fn miku_setpgid(pid: u64, pgid: u64) -> i64;
    pub fn miku_try_wait(pid: u64, status: *mut i64) -> i64;
    pub fn miku_wait4(pid: u64, status: *mut i64, options: u64) -> i64;
    pub fn miku_getpgid(pid: u64) -> i64;
    pub fn miku_sigdisp(sig: u64, disp: u64) -> i64;
    pub fn miku_procinfo(buf: *mut u8, max: u64) -> i64;
    pub fn miku_proc_path(pid: u64, buf: *mut u8, len: u64) -> i64;
    pub fn miku_kcmd(text: *const u8, len: usize) -> i64;
    pub fn miku_statfs(path: *const u8, buf: *mut u8) -> i64;
    pub fn miku_killpg(pgid: u64, sig: u64) -> i64;
    pub fn miku_power(action: u64) -> i64;
    // Peer credentials and descriptor passing over a local socket
    pub fn miku_peercred(fd: u64, out: *mut u8) -> i64;
    pub fn miku_sendfd(sockfd: u64, fd: u64) -> i64;
    pub fn miku_recvfd(sockfd: u64) -> i64;
    pub fn miku_dup(fd: i64) -> i64;
    pub fn miku_dup2(old_fd: i64, new_fd: i64) -> i64;
    pub fn miku_pipe(fds: *mut i64) -> i64;
    /// Waits until one of `fds` is ready or `timeout_ms` passes. Returns the
    /// count with non-zero `revents`, 0 on timeout, or -errno. Negative waits
    /// forever; zero returns at once
    pub fn miku_poll(fds: *mut MikuPollFd, nfds: u64, timeout_ms: i64) -> i64;
    pub fn miku_chdir(path: *const u8) -> i64;
    pub fn miku_access(path: *const u8) -> bool;
    pub fn miku_isdir(path: *const u8) -> bool;
    pub fn miku_isfile(path: *const u8) -> bool;
    pub fn miku_issymlink(path: *const u8) -> bool;
    pub fn miku_filesize(path: *const u8) -> i64;
    pub fn miku_ftruncate(fd: i64, length: u64) -> i64;
    pub fn miku_pread(fd: i64, buf: *mut u8, len: usize, offset: i64) -> i64;
    pub fn miku_pwrite(fd: i64, buf: *const u8, len: usize, offset: i64) -> i64;
    pub fn miku_write_file_cstr(path: *const u8, data: *const u8) -> i64;

    // time
    pub fn miku_sleep(ticks: u64);
    pub fn miku_sleep_ms(ms: u64);
    /// Full clock (`lib/libmiku/time.rs`)
    pub fn miku_clock_gettime(clock: u64, out: *mut MikuTimespec) -> i64;
    pub fn miku_uptime() -> u64;
    pub fn miku_uptime_ms() -> u64;
    pub fn miku_yield();
    /// Seconds since the Unix epoch, or a negative errno if unset
    pub fn miku_realtime() -> i64;
    /// Root only
    pub fn miku_set_realtime(unix_secs: i64) -> i64;

    // math
    pub fn miku_abs(x: i64) -> i64;
    pub fn miku_min(a: i64, b: i64) -> i64;
    pub fn miku_max(a: i64, b: i64) -> i64;
    pub fn miku_clamp(val: i64, lo: i64, hi: i64) -> i64;
    pub fn miku_swap(a: *mut u64, b: *mut u64);
    pub fn miku_umin(a: u64, b: u64) -> u64;
    pub fn miku_umax(a: u64, b: u64) -> u64;

    // random
    pub fn miku_srand(seed: u64);
    pub fn miku_rand() -> u64;
    pub fn miku_rand_range(lo: u64, hi: u64) -> u64;
    pub fn miku_rand_u32() -> u32;
    pub fn miku_rand_bytes(buf: *mut u8, len: usize);
    pub fn miku_rand_shuffle(data: *mut u8, count: usize, elem_size: usize);
    /// Kernel entropy. The others in this group are a xorshift seeded from
    /// uptime and pid
    pub fn miku_getrandom(buf: *mut u8, len: usize) -> i64;

    // panic
    pub fn miku_assert_fail(expr: *const u8, file: *const u8, line: i32);
    pub fn miku_panic(msg: *const u8) -> !;
    pub fn miku_abort() -> !;

    // bitops
    pub fn miku_popcount32(x: u32) -> u32;
    pub fn miku_popcount64(x: u64) -> u64;
    pub fn miku_clz32(x: u32) -> u32;
    pub fn miku_clz64(x: u64) -> u64;
    pub fn miku_ctz32(x: u32) -> u32;
    pub fn miku_ctz64(x: u64) -> u64;
    pub fn miku_fls32(x: u32) -> u32;
    pub fn miku_fls64(x: u64) -> u64;
    pub fn miku_ffs32(x: u32) -> u32;
    pub fn miku_ffs64(x: u64) -> u64;
    pub fn miku_bswap16(x: u16) -> u16;
    pub fn miku_bswap32(x: u32) -> u32;
    pub fn miku_bswap64(x: u64) -> u64;
    pub fn miku_rotl32(x: u32, n: u32) -> u32;
    pub fn miku_rotr32(x: u32, n: u32) -> u32;
    pub fn miku_rotl64(x: u64, n: u64) -> u64;
    pub fn miku_rotr64(x: u64, n: u64) -> u64;
    pub fn miku_is_power_of_two(x: u64) -> bool;
    pub fn miku_next_power_of_two(x: u64) -> u64;
    pub fn miku_log2(x: u64) -> u64;
    pub fn miku_bit_extract(val: u64, start: u32, len: u32) -> u64;
    pub fn miku_bit_insert(val: u64, bits: u64, start: u32, len: u32) -> u64;
    pub fn miku_align_up(val: u64, align: u64) -> u64;
    pub fn miku_align_down(val: u64, align: u64) -> u64;
    pub fn miku_is_aligned(val: u64, align: u64) -> bool;

    // hash
    pub fn miku_fnv1a_32(data: *const u8, len: usize) -> u32;
    pub fn miku_fnv1a_64(data: *const u8, len: usize) -> u64;
    pub fn miku_djb2(data: *const u8, len: usize) -> u64;
    pub fn miku_djb2_str(s: *const u8) -> u64;
    pub fn miku_crc32(data: *const u8, len: usize) -> u32;
    pub fn miku_crc32_update(prev: u32, data: *const u8, len: usize) -> u32;
    pub fn miku_siphash(data: *const u8, len: usize, k0: u64, k1: u64) -> u64;
    pub fn miku_hash_bytes(data: *const u8, len: usize) -> u64;
    pub fn miku_hash_str(s: *const u8) -> u64;
    pub fn miku_hash_u64(val: u64) -> u64;
    pub fn miku_hash_combine(seed: u64, value: u64) -> u64;
    pub fn miku_hash_u32(val: u32) -> u32;
    pub fn miku_adler32(data: *const u8, len: usize) -> u32;
    pub fn miku_adler32_update(prev: u32, data: *const u8, len: usize) -> u32;
    pub fn miku_murmurhash3_fmix64(k: u64) -> u64;
    pub fn miku_murmurhash3(data: *const u8, len: usize, seed: u64) -> u64;

    // base64
    pub fn miku_base64_encode_len(input_len: usize) -> usize;
    pub fn miku_base64_decode_len(input_len: usize) -> usize;
    pub fn miku_base64_encode(input: *const u8, len: usize, out: *mut u8, out_max: usize) -> i32;
    pub fn miku_base64_decode(input: *const u8, len: usize, out: *mut u8, out_max: usize) -> i32;
    pub fn miku_base64_encode_alloc(input: *const u8, len: usize) -> *mut u8;
    pub fn miku_base64_decode_alloc(input: *const u8, len: usize, out_len: *mut usize) -> *mut u8;

    // utf8
    pub fn miku_utf8_encode(codepoint: u32, out: *mut u8) -> usize;
    pub fn miku_utf8_decode(data: *const u8, len: usize, bytes_consumed: *mut usize) -> u32;
    pub fn miku_utf8_len(s: *const u8, byte_len: usize) -> usize;
    pub fn miku_utf8_strlen(s: *const u8) -> usize;
    pub fn miku_utf8_valid(s: *const u8, len: usize) -> bool;
    pub fn miku_utf8_offset(s: *const u8, byte_len: usize, n: usize) -> usize;
    pub fn miku_utf8_is_boundary(s: *const u8, len: usize, pos: usize) -> bool;

    // path
    pub fn miku_basename(path: *const u8) -> *mut u8;
    pub fn miku_dirname(path: *const u8) -> *mut u8;
    pub fn miku_path_ext(path: *const u8) -> *mut u8;
    pub fn miku_path_stem(path: *const u8) -> *mut u8;
    pub fn miku_path_join(a: *const u8, b: *const u8) -> *mut u8;
    pub fn miku_path_normalize(path: *const u8) -> *mut u8;
    pub fn miku_path_is_absolute(path: *const u8) -> bool;
    pub fn miku_path_depth(path: *const u8) -> usize;

    // sort
    pub fn miku_qsort(
        base: *mut u8,
        count: usize,
        size: usize,
        cmp: unsafe extern "C" fn(*const u8, *const u8) -> i32,
    );
    pub fn miku_bsearch(
        key: *const u8,
        base: *const u8,
        count: usize,
        size: usize,
        cmp: unsafe extern "C" fn(*const u8, *const u8) -> i32,
    ) -> *const u8;
    pub fn miku_reverse(base: *mut u8, count: usize, size: usize);
    pub fn miku_is_sorted(
        base: *const u8,
        count: usize,
        size: usize,
        cmp: unsafe extern "C" fn(*const u8, *const u8) -> i32,
    ) -> bool;
    pub fn miku_cmp_i64(a: *const u8, b: *const u8) -> i32;
    pub fn miku_cmp_u64(a: *const u8, b: *const u8) -> i32;

    // vec
    pub fn miku_vec_new(elem_size: usize) -> MikuVec;
    pub fn miku_vec_with_capacity(elem_size: usize, cap: usize) -> MikuVec;
    pub fn miku_vec_free(v: *mut MikuVec);
    pub fn miku_vec_len(v: *const MikuVec) -> usize;
    pub fn miku_vec_cap(v: *const MikuVec) -> usize;
    pub fn miku_vec_is_empty(v: *const MikuVec) -> bool;
    pub fn miku_vec_get(v: *const MikuVec, index: usize) -> *const u8;
    pub fn miku_vec_get_mut(v: *mut MikuVec, index: usize) -> *mut u8;
    pub fn miku_vec_push(v: *mut MikuVec, elem: *const u8) -> bool;
    pub fn miku_vec_pop(v: *mut MikuVec, out: *mut u8) -> bool;
    pub fn miku_vec_insert(v: *mut MikuVec, index: usize, elem: *const u8) -> bool;
    pub fn miku_vec_remove(v: *mut MikuVec, index: usize) -> bool;
    pub fn miku_vec_swap_remove(v: *mut MikuVec, index: usize) -> bool;
    pub fn miku_vec_clear(v: *mut MikuVec);
    pub fn miku_vec_reserve(v: *mut MikuVec, additional: usize) -> bool;
    pub fn miku_vec_shrink(v: *mut MikuVec) -> bool;
    pub fn miku_vec_data(v: *const MikuVec) -> *const u8;
    pub fn miku_vec_contains(v: *const MikuVec, elem: *const u8) -> bool;
    pub fn miku_vec_push_u64(v: *mut MikuVec, val: u64) -> bool;
    pub fn miku_vec_get_u64(v: *const MikuVec, index: usize) -> u64;

    // hashmap
    pub fn miku_map_new(key_size: usize, val_size: usize) -> MikuMap;
    pub fn miku_map_new_u64() -> MikuMap;
    pub fn miku_map_free(m: *mut MikuMap);
    pub fn miku_map_insert(m: *mut MikuMap, key: *const u8, val: *const u8) -> bool;
    pub fn miku_map_get(m: *const MikuMap, key: *const u8) -> *const u8;
    pub fn miku_map_contains(m: *const MikuMap, key: *const u8) -> bool;
    pub fn miku_map_remove(m: *mut MikuMap, key: *const u8) -> bool;
    pub fn miku_map_len(m: *const MikuMap) -> usize;
    pub fn miku_map_clear(m: *mut MikuMap);
    pub fn miku_map_insert_u64(m: *mut MikuMap, key: u64, val: u64) -> bool;
    pub fn miku_map_get_u64(m: *const MikuMap, key: u64) -> u64;

    // list
    pub fn miku_list_new(elem_size: usize) -> MikuList;
    pub fn miku_list_free(l: *mut MikuList);
    pub fn miku_list_len(l: *const MikuList) -> usize;
    pub fn miku_list_is_empty(l: *const MikuList) -> bool;
    pub fn miku_list_push_front(l: *mut MikuList, elem: *const u8) -> bool;
    pub fn miku_list_push_back(l: *mut MikuList, elem: *const u8) -> bool;
    pub fn miku_list_pop_front(l: *mut MikuList, out: *mut u8) -> bool;
    pub fn miku_list_pop_back(l: *mut MikuList, out: *mut u8) -> bool;
    pub fn miku_list_get(l: *const MikuList, index: usize) -> *const u8;
    pub fn miku_list_set(l: *mut MikuList, index: usize, elem: *const u8) -> bool;
    pub fn miku_list_insert(l: *mut MikuList, index: usize, elem: *const u8) -> bool;
    pub fn miku_list_remove(l: *mut MikuList, index: usize) -> bool;
    pub fn miku_list_clear(l: *mut MikuList);
    pub fn miku_list_contains(l: *const MikuList, elem: *const u8) -> bool;
    pub fn miku_list_push_back_u64(l: *mut MikuList, val: u64) -> bool;
    pub fn miku_list_get_u64(l: *const MikuList, index: usize) -> u64;

    // ringbuf
    pub fn miku_ring_new(capacity: usize) -> MikuRingBuf;
    pub fn miku_ring_free(r: *mut MikuRingBuf);
    pub fn miku_ring_len(r: *const MikuRingBuf) -> usize;
    pub fn miku_ring_available(r: *const MikuRingBuf) -> usize;
    pub fn miku_ring_is_empty(r: *const MikuRingBuf) -> bool;
    pub fn miku_ring_is_full(r: *const MikuRingBuf) -> bool;
    pub fn miku_ring_write(r: *mut MikuRingBuf, data: *const u8, len: usize) -> usize;
    pub fn miku_ring_read(r: *mut MikuRingBuf, out: *mut u8, len: usize) -> usize;
    pub fn miku_ring_peek(r: *const MikuRingBuf, out: *mut u8, len: usize) -> usize;
    pub fn miku_ring_push_byte(r: *mut MikuRingBuf, byte: u8) -> bool;
    pub fn miku_ring_pop_byte(r: *mut MikuRingBuf) -> i32;
    pub fn miku_ring_skip(r: *mut MikuRingBuf, n: usize) -> usize;
    pub fn miku_ring_clear(r: *mut MikuRingBuf);

    // endian
    pub fn miku_htobe16(x: u16) -> u16;
    pub fn miku_htobe32(x: u32) -> u32;
    pub fn miku_htobe64(x: u64) -> u64;
    pub fn miku_be16toh(x: u16) -> u16;
    pub fn miku_be32toh(x: u32) -> u32;
    pub fn miku_be64toh(x: u64) -> u64;
    pub fn miku_htole16(x: u16) -> u16;
    pub fn miku_htole32(x: u32) -> u32;
    pub fn miku_htole64(x: u64) -> u64;
    pub fn miku_le16toh(x: u16) -> u16;
    pub fn miku_le32toh(x: u32) -> u32;
    pub fn miku_le64toh(x: u64) -> u64;
    pub fn miku_read_u16_be(ptr: *const u8) -> u16;
    pub fn miku_read_u32_be(ptr: *const u8) -> u32;
    pub fn miku_read_u64_be(ptr: *const u8) -> u64;
    pub fn miku_read_u16_le(ptr: *const u8) -> u16;
    pub fn miku_read_u32_le(ptr: *const u8) -> u32;
    pub fn miku_read_u64_le(ptr: *const u8) -> u64;
    pub fn miku_write_u16_be(ptr: *mut u8, val: u16);
    pub fn miku_write_u32_be(ptr: *mut u8, val: u32);
    pub fn miku_write_u16_le(ptr: *mut u8, val: u16);
    pub fn miku_write_u32_le(ptr: *mut u8, val: u32);

    // arena
    pub fn miku_arena_new() -> MikuArena;
    pub fn miku_arena_with_block_size(block_size: usize) -> MikuArena;
    pub fn miku_arena_alloc(arena: *mut MikuArena, size: usize) -> *mut u8;
    pub fn miku_arena_calloc(arena: *mut MikuArena, size: usize) -> *mut u8;
    pub fn miku_arena_strdup(arena: *mut MikuArena, s: *const u8) -> *mut u8;
    pub fn miku_arena_reset(arena: *mut MikuArena);
    pub fn miku_arena_free(arena: *mut MikuArena);
    pub fn miku_arena_used(arena: *const MikuArena) -> usize;

    // bitset
    pub fn miku_bitset_new(nbits: usize) -> MikuBitset;
    pub fn miku_bitset_free(bs: *mut MikuBitset);
    pub fn miku_bitset_set(bs: *mut MikuBitset, bit: usize) -> bool;
    pub fn miku_bitset_clear(bs: *mut MikuBitset, bit: usize);
    pub fn miku_bitset_test(bs: *const MikuBitset, bit: usize) -> bool;
    pub fn miku_bitset_toggle(bs: *mut MikuBitset, bit: usize) -> bool;
    pub fn miku_bitset_count(bs: *const MikuBitset) -> usize;
    pub fn miku_bitset_clear_all(bs: *mut MikuBitset);
    pub fn miku_bitset_set_all(bs: *mut MikuBitset, nbits: usize);
    pub fn miku_bitset_or(dst: *mut MikuBitset, src: *const MikuBitset);
    pub fn miku_bitset_and(dst: *mut MikuBitset, src: *const MikuBitset);
    pub fn miku_bitset_xor(dst: *mut MikuBitset, src: *const MikuBitset);
    pub fn miku_bitset_ffs(bs: *const MikuBitset) -> i64;
    pub fn miku_bitset_is_empty(bs: *const MikuBitset) -> bool;
    pub fn miku_bitset_capacity(bs: *const MikuBitset) -> usize;

    // heap_queue (priority queue)
    pub fn miku_pq_new(
        elem_size: usize,
        cmp: unsafe extern "C" fn(*const u8, *const u8) -> i32,
    ) -> MikuHeapQueue;
    pub fn miku_pq_free(q: *mut MikuHeapQueue);
    pub fn miku_pq_push(q: *mut MikuHeapQueue, elem: *const u8) -> bool;
    pub fn miku_pq_peek(q: *const MikuHeapQueue) -> *const u8;
    pub fn miku_pq_pop(q: *mut MikuHeapQueue, out: *mut u8) -> bool;
    pub fn miku_pq_len(q: *const MikuHeapQueue) -> usize;
    pub fn miku_pq_is_empty(q: *const MikuHeapQueue) -> bool;
    pub fn miku_pq_clear(q: *mut MikuHeapQueue);
    pub fn miku_pq_cmp_i64(a: *const u8, b: *const u8) -> i32;
    pub fn miku_pq_new_i64() -> MikuHeapQueue;
    pub fn miku_pq_push_i64(q: *mut MikuHeapQueue, val: i64) -> bool;
    pub fn miku_pq_pop_i64(q: *mut MikuHeapQueue) -> i64;

    // glob
    pub fn miku_glob_match(pattern: *const u8, text: *const u8) -> bool;
    pub fn miku_glob_match_nocase(pattern: *const u8, text: *const u8) -> bool;
    pub fn miku_glob_has_magic(pattern: *const u8) -> bool;
    pub fn miku_glob_escape(s: *const u8) -> *mut u8;

    // channel
    pub fn miku_chan_new(elem_size: usize, capacity: usize) -> MikuChannel;
    pub fn miku_chan_free(ch: *mut MikuChannel);
    pub fn miku_chan_send(ch: *mut MikuChannel, data: *const u8) -> bool;
    pub fn miku_chan_recv(ch: *mut MikuChannel, out: *mut u8) -> bool;
    pub fn miku_chan_len(ch: *const MikuChannel) -> usize;
    pub fn miku_chan_is_empty(ch: *const MikuChannel) -> bool;
    pub fn miku_chan_is_full(ch: *const MikuChannel) -> bool;
    pub fn miku_chan_available(ch: *const MikuChannel) -> usize;
    pub fn miku_chan_new_u64(capacity: usize) -> MikuChannel;
    pub fn miku_chan_send_u64(ch: *mut MikuChannel, val: u64) -> bool;
    pub fn miku_chan_recv_u64(ch: *mut MikuChannel) -> u64;

    // slab
    pub fn miku_slab_new(obj_size: usize, capacity: usize) -> MikuSlab;
    pub fn miku_slab_free(s: *mut MikuSlab);
    pub fn miku_slab_alloc(s: *mut MikuSlab) -> *mut u8;
    pub fn miku_slab_dealloc(s: *mut MikuSlab, ptr: *mut u8);
    pub fn miku_slab_in_use(s: *const MikuSlab) -> usize;
    pub fn miku_slab_available(s: *const MikuSlab) -> usize;
    pub fn miku_slab_capacity(s: *const MikuSlab) -> usize;
    pub fn miku_slab_slot_size(s: *const MikuSlab) -> usize;
    pub fn miku_slab_is_full(s: *const MikuSlab) -> bool;
    pub fn miku_slab_is_empty(s: *const MikuSlab) -> bool;

    // format (string builder)
    pub fn miku_sb_new() -> MikuStringBuilder;
    pub fn miku_sb_with_capacity(cap: usize) -> MikuStringBuilder;
    pub fn miku_sb_free(sb: *mut MikuStringBuilder);
    pub fn miku_sb_append(sb: *mut MikuStringBuilder, s: *const u8) -> bool;
    pub fn miku_sb_append_bytes(sb: *mut MikuStringBuilder, data: *const u8, len: usize) -> bool;
    pub fn miku_sb_append_char(sb: *mut MikuStringBuilder, c: u8) -> bool;
    pub fn miku_sb_append_int(sb: *mut MikuStringBuilder, val: i64) -> bool;
    pub fn miku_sb_append_uint(sb: *mut MikuStringBuilder, val: u64) -> bool;
    pub fn miku_sb_repeat(sb: *mut MikuStringBuilder, c: u8, n: usize) -> bool;
    pub fn miku_sb_finish(sb: *mut MikuStringBuilder) -> *mut u8;
    pub fn miku_sb_len(sb: *const MikuStringBuilder) -> usize;
    pub fn miku_sb_clear(sb: *mut MikuStringBuilder);
    pub fn miku_sb_data(sb: *const MikuStringBuilder) -> *const u8;
    pub fn miku_str_join(strs: *const *const u8, count: usize, sep: *const u8) -> *mut u8;
    pub fn miku_str_repeat(s: *const u8, n: usize) -> *mut u8;

    // regex
    pub fn miku_regex_match(pattern: *const u8, text: *const u8) -> bool;
    pub fn miku_regex_match_full(pattern: *const u8, text: *const u8) -> bool;
    pub fn miku_regex_find(pattern: *const u8, text: *const u8) -> i64;
    pub fn miku_regex_count(pattern: *const u8, text: *const u8) -> usize;

    // hex
    pub fn miku_hex_encode_len(input_len: usize) -> usize;
    pub fn miku_hex_decode_len(input_len: usize) -> usize;
    pub fn miku_hex_encode(input: *const u8, len: usize, out: *mut u8, out_max: usize) -> i32;
    pub fn miku_hex_encode_upper(input: *const u8, len: usize, out: *mut u8, out_max: usize)
        -> i32;
    pub fn miku_hex_decode(input: *const u8, len: usize, out: *mut u8, out_max: usize) -> i32;
    pub fn miku_hex_encode_alloc(input: *const u8, len: usize) -> *mut u8;
    pub fn miku_hex_decode_alloc(input: *const u8, len: usize, out_len: *mut usize) -> *mut u8;
    pub fn miku_hex_u64(val: u64, buf: *mut u8);
    pub fn miku_hex_u32(val: u32, buf: *mut u8);

    // checksum
    pub fn miku_fletcher16(data: *const u8, len: usize) -> u16;
    pub fn miku_fletcher32(data: *const u8, len: usize) -> u32;
    pub fn miku_xor_checksum(data: *const u8, len: usize) -> u8;
    pub fn miku_inet_checksum(data: *const u8, len: usize) -> u16;
    pub fn miku_sum8(data: *const u8, len: usize) -> u8;
    pub fn miku_bsd_checksum(data: *const u8, len: usize) -> u16;

    // getopt
    pub fn miku_getopt_init(g: *mut MikuGetopt, argc: i32, argv: *const *const u8);
    pub fn miku_getopt_next(g: *mut MikuGetopt, optstring: *const u8) -> i32;
    pub fn miku_getopt_optind(g: *const MikuGetopt) -> i32;
    pub fn miku_getopt_optarg(g: *const MikuGetopt) -> *const u8;
    pub fn miku_argv_get(argc: i32, argv: *const *const u8, key: *const u8) -> *const u8;
    pub fn miku_argv_has(argc: i32, argv: *const *const u8, flag: *const u8) -> bool;
    pub fn miku_argv_positional_count(argc: i32, argv: *const *const u8) -> i32;

    // treemap (AVL)
    pub fn miku_tree_new() -> MikuTreeMap;
    pub fn miku_tree_free(t: *mut MikuTreeMap);
    pub fn miku_tree_insert(t: *mut MikuTreeMap, key: i64, val: u64) -> bool;
    pub fn miku_tree_get(t: *const MikuTreeMap, key: i64) -> *const u64;
    pub fn miku_tree_contains(t: *const MikuTreeMap, key: i64) -> bool;
    pub fn miku_tree_remove(t: *mut MikuTreeMap, key: i64) -> bool;
    pub fn miku_tree_len(t: *const MikuTreeMap) -> usize;
    pub fn miku_tree_is_empty(t: *const MikuTreeMap) -> bool;
    pub fn miku_tree_iter(
        t: *const MikuTreeMap,
        cb: extern "C" fn(i64, u64, *mut u8),
        ctx: *mut u8,
    );
    pub fn miku_tree_min(t: *const MikuTreeMap, key: *mut i64, val: *mut u64) -> bool;
    pub fn miku_tree_max(t: *const MikuTreeMap, key: *mut i64, val: *mut u64) -> bool;
    pub fn miku_tree_clear(t: *mut MikuTreeMap);
}

// filesystem structs

#[repr(C)]
#[derive(Default)]
pub struct MikuStat {
    pub size: u64,
    pub mode: u32,
    pub nlinks: u32,
    pub uid: u16,
    pub gid: u16,
    pub kind: u8,
    pub fs_type: u8,
    pub dev_major: u8,
    pub dev_minor: u8,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub inode_id: u64,
    pub blocks: u32,
    pub _reserved: [u8; 4],
}

/// `statfs` result, matching the kernel's `sys_statfs` (adapters/fs.rs)
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct MikuStatfs {
    pub magic: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub max_name_len: u32,
    pub flags: u32,
}

/// A `timespec`, as syscall 70 writes one.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MikuTimespec {
    pub sec: i64,
    pub nsec: i64,
}

pub const CLOCK_REALTIME: u64 = 0;
pub const CLOCK_MONOTONIC: u64 = 1;

/// Directory entry as `getdents`(111) writes it: 272 bytes
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MikuDirent {
    pub name: [u8; 256],
    pub inode_id: u64,
    pub kind: u8,
    pub name_len: u8,
    pub _reserved: [u8; 6],
}

/// `MikuDirent` kinds (`VNodeKind`, `kernel/fs/vfs/types.rs`)
pub const KIND_REGULAR: u8 = 0;
pub const KIND_DIRECTORY: u8 = 1;
pub const KIND_SYMLINK: u8 = 2;

/// Directory names with kinds, without `.` and `..`, following nothing. Empty
/// if the directory will not open
pub fn read_dir(path: &str) -> alloc::vec::Vec<(alloc::string::String, u8)> {
    let mut c = alloc::vec::Vec::with_capacity(path.len() + 1);
    c.extend_from_slice(path.as_bytes());
    c.push(0);
    let mut out = alloc::vec::Vec::new();
    const BATCH: usize = 64;
    // O_READ | O_DIRECTORY
    let fd = unsafe { miku_open(c.as_ptr(), c.len() - 1, 0x0001 | 0x0040, 0) };
    if fd < 0 {
        return out;
    }
    let mut entries: [MikuDirent; BATCH] = unsafe { core::mem::zeroed() };
    let mut offset = 0usize;
    loop {
        let n = unsafe { miku_getdents(fd, entries.as_mut_ptr(), BATCH, offset) };
        if n <= 0 {
            break;
        }
        for e in entries.iter().take(n as usize) {
            let len = (e.name_len as usize).min(e.name.len());
            if let Ok(name) = core::str::from_utf8(&e.name[..len]) {
                if name != "." && name != ".." {
                    out.push((alloc::string::String::from(name), e.kind));
                }
            }
        }
        offset += n as usize;
    }
    unsafe { miku_close(fd) };
    out
}

// struct repr(C) types for data structures

#[repr(C)]
pub struct MikuVec {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
    pub elem_size: usize,
}

#[repr(C)]
pub struct MikuMap {
    pub slots: *mut u8,
    pub cap: usize,
    pub count: usize,
    pub key_size: usize,
    pub val_size: usize,
    pub slot_size: usize,
}

#[repr(C)]
pub struct MikuList {
    pub head: *mut u8,
    pub tail: *mut u8,
    pub len: usize,
    pub elem_size: usize,
}

#[repr(C)]
pub struct MikuRingBuf {
    pub buf: *mut u8,
    pub cap: usize,
    pub head: usize,
    pub tail: usize,
}

#[repr(C)]
pub struct MikuArena {
    pub head: *mut u8,
    pub block_size: usize,
    pub total_alloc: usize,
}

#[repr(C)]
pub struct MikuBitset {
    pub words: *mut u64,
    pub num_words: usize,
}

#[repr(C)]
pub struct MikuHeapQueue {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
    pub elem_size: usize,
    pub cmp: unsafe extern "C" fn(*const u8, *const u8) -> i32,
}

#[repr(C)]
pub struct MikuChannel {
    pub buf: *mut u8,
    pub cap: usize,
    pub elem_size: usize,
    pub head: usize,
    pub tail: usize,
}

#[repr(C)]
pub struct MikuSlab {
    pub pool: *mut u8,
    pub free_head: *mut u8,
    pub slot_size: usize,
    pub capacity: usize,
    pub in_use: usize,
}

#[repr(C)]
pub struct MikuStringBuilder {
    pub buf: *mut u8,
    pub len: usize,
    pub cap: usize,
}

#[repr(C)]
pub struct MikuGetopt {
    pub argv: *const *const u8,
    pub argc: i32,
    pub optind: i32,
    pub optpos: i32,
    pub optarg: *const u8,
    pub optopt: u8,
    pub finished: bool,
}

/// One descriptor to watch, as `poll` takes them
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MikuPollFd {
    /// Negative skips this slot
    pub fd: i32,
    pub events: i16,
    /// Written by the kernel on every call
    pub revents: i16,
}

pub const POLLIN:   i16 = 0x001;
pub const POLLOUT:  i16 = 0x004;
pub const POLLERR:  i16 = 0x008;
/// The far end is gone; reported whether requested or not
pub const POLLHUP:  i16 = 0x010;
/// Nothing is open on that descriptor
pub const POLLNVAL: i16 = 0x020;

#[repr(C)]
pub struct MikuTreeMap {
    pub root: *mut u8,
    pub count: usize,
}

pub fn exit(code: i64) -> ! {
    unsafe { miku_exit(code) }
}

/// Program arguments as strings, borrowed from the initial stack. Non-UTF-8
/// arguments are empty strings, keeping positions.
///
/// # Safety
/// `argv` must be the pointer `_start_main` was given.
pub fn args(argc: i32, argv: *const *const u8) -> alloc::vec::Vec<&'static str> {
    let mut out = alloc::vec::Vec::new();
    if argc <= 0 || argv.is_null() {
        return out;
    }
    for i in 0..argc as usize {
        let p = unsafe { *argv.add(i) };
        if p.is_null() {
            break;
        }
        let mut len = 0usize;
        // Bounded in case of a missing terminator
        while len < 4096 && unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        let bytes = unsafe { core::slice::from_raw_parts(p, len) };
        out.push(core::str::from_utf8(bytes).unwrap_or(""));
    }
    out
}

/// Fills `buf` with kernel entropy. False on failure, with no weaker fallback
pub fn getrandom(buf: &mut [u8]) -> bool {
    if buf.is_empty() { return true; }
    unsafe { miku_getrandom(buf.as_mut_ptr(), buf.len()) == buf.len() as i64 }
}

/// Seconds since the Unix epoch, or None if unset
pub fn realtime() -> Option<u64> {
    let s = unsafe { miku_realtime() };
    if s < 0 { None } else { Some(s as u64) }
}
pub fn print(s: &str) {
    unsafe {
        miku_write(1, s.as_ptr(), s.len());
    }
}
pub fn println(s: &str) {
    print(s);
    print("\n");
}
pub fn eprint(s: &str) {
    unsafe {
        miku_write(2, s.as_ptr(), s.len());
    }
}
pub fn eprintln(s: &str) {
    eprint(s);
    eprint("\n");
}

/// Left-aligned decimal. Returns bytes written, or 0 if it does not fit
pub fn fmt_dec(buf: &mut [u8], v: u64) -> usize {
    let mut tmp = [0u8; 20];
    let mut i = 0;
    let mut n = v;
    if n == 0 {
        tmp[0] = b'0';
        i = 1;
    }
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    if i > buf.len() {
        return 0;
    }
    for j in 0..i {
        buf[j] = tmp[i - 1 - j];
    }
    i
}

/// The same with a sign
pub fn fmt_int(buf: &mut [u8], v: i64) -> usize {
    if v >= 0 {
        return fmt_dec(buf, v as u64);
    }
    if buf.is_empty() {
        return 0;
    }
    buf[0] = b'-';
    let n = fmt_dec(&mut buf[1..], (v as u64).wrapping_neg());
    if n == 0 { 0 } else { n + 1 }
}

fn write_dec_to_stdout(val: i64) {
    let mut buf = [0u8; 24];
    let n = fmt_int(&mut buf, val);
    unsafe {
        miku_write(1, buf.as_ptr(), n);
    }
}

fn write_hex_to_stdout(mut val: u64) {
    let mut buf = [0u8; 18];
    let mut i = buf.len();
    if val == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while val > 0 {
            let d = (val & 0xF) as u8;
            i -= 1;
            buf[i] = if d < 10 { b'0' + d } else { b'a' + (d - 10) };
            val >>= 4;
        }
        i -= 2;
        buf[i] = b'0';
        buf[i + 1] = b'x';
    }
    unsafe {
        miku_write(1, buf.as_ptr().add(i), buf.len() - i);
    }
}

pub fn print_int(val: i64) {
    write_dec_to_stdout(val);
}
pub fn print_hex(val: u64) {
    write_hex_to_stdout(val);
}
pub fn putchar(c: u8) {
    unsafe {
        miku_putchar(c as i32);
    }
}
pub fn getchar() -> Option<u8> {
    let r = unsafe { miku_getchar() };
    if r < 0 {
        None
    } else {
        Some(r as u8)
    }
}
pub fn sleep(ticks: u64) {
    unsafe {
        miku_sleep(ticks);
    }
}
pub fn sleep_ms(ms: u64) {
    unsafe {
        miku_sleep_ms(ms);
    }
}
pub fn uptime() -> u64 {
    unsafe { miku_uptime() }
}
pub fn uptime_ms() -> u64 {
    unsafe { miku_uptime_ms() }
}
pub fn yield_now() {
    unsafe {
        miku_yield();
    }
}
pub fn getpid() -> u64 {
    unsafe { miku_getpid() }
}

/// This thread's id, which for a single-threaded program is its pid.
pub fn gettid() -> i64 {
    unsafe { miku_gettid() }
}

/// Starts a thread on a caller-owned stack, which must outlive it. Returns the
/// thread id or a negative errno.
pub fn thread_spawn(
    entry: extern "C" fn(*mut u8) -> i32,
    arg: *mut u8,
    stack: &mut [u8],
) -> i64 {
    unsafe { miku_thread_spawn(entry, arg, stack.as_mut_ptr(), stack.len()) }
}

/// Sleeps while the word holds `expected`. A wake, timeout, or nothing may end
/// it, so the caller rechecks in a loop. `timeout_ns` 0 means none.
pub fn futex_wait(addr: &core::sync::atomic::AtomicU32, expected: u32, timeout_ns: u64) -> i64 {
    unsafe { miku_futex_wait(addr.as_ptr() as *const u32, expected, timeout_ns) }
}

/// Wakes at most `count` sleepers. Returns how many.
pub fn futex_wake(addr: &core::sync::atomic::AtomicU32, count: u32) -> i64 {
    unsafe { miku_futex_wake(addr.as_ptr() as *const u32, count) }
}
pub fn brk(addr: u64) -> u64 {
    unsafe { miku_brk(addr) }
}
pub fn abs(x: i64) -> i64 {
    unsafe { miku_abs(x) }
}
pub fn min(a: i64, b: i64) -> i64 {
    unsafe { miku_min(a, b) }
}
pub fn max(a: i64, b: i64) -> i64 {
    unsafe { miku_max(a, b) }
}
pub fn clamp(val: i64, lo: i64, hi: i64) -> i64 {
    unsafe { miku_clamp(val, lo, hi) }
}
pub fn umin(a: u64, b: u64) -> u64 {
    unsafe { miku_umin(a, b) }
}
pub fn umax(a: u64, b: u64) -> u64 {
    unsafe { miku_umax(a, b) }
}
pub fn rand() -> u64 {
    unsafe { miku_rand() }
}
pub fn srand(seed: u64) {
    unsafe {
        miku_srand(seed);
    }
}
pub fn rand_range(lo: u64, hi: u64) -> u64 {
    unsafe { miku_rand_range(lo, hi) }
}

pub fn strlen(s: &[u8]) -> usize {
    unsafe { miku_strlen(s.as_ptr()) }
}
pub fn streq(a: &[u8], b: &[u8]) -> bool {
    unsafe { miku_strcmp(a.as_ptr(), b.as_ptr()) == 0 }
}

pub unsafe fn malloc(size: usize) -> *mut u8 {
    miku_malloc(size)
}
pub unsafe fn free(ptr: *mut u8) {
    miku_free(ptr)
}
pub unsafe fn realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    miku_realloc(ptr, new_size)
}
pub unsafe fn calloc(count: usize, size: usize) -> *mut u8 {
    miku_calloc(count, size)
}

pub fn open(path: &str) -> Result<i64, i64> {
    let fd = unsafe { miku_open(path.as_ptr(), path.len(), 0x0001, 0) };
    if fd < 0 {
        Err(fd)
    } else {
        Ok(fd)
    }
}
pub fn close(fd: i64) {
    unsafe {
        miku_close(fd);
    }
}
pub fn fsize(fd: i64) -> i64 {
    unsafe { miku_fsize(fd) }
}
pub fn seek(fd: i64, offset: u64) {
    unsafe {
        miku_seek(fd, offset);
    }
}
pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    unsafe { miku_read(fd as u64, buf.as_mut_ptr(), buf.len()) }
}

pub fn read_file(path: &str) -> Option<(*mut u8, usize)> {
    let mut size: usize = 0;
    let mut p = [0u8; 256];
    let len = path.len().min(255);
    p[..len].copy_from_slice(&path.as_bytes()[..len]);
    p[len] = 0;
    let ptr = unsafe { miku_read_file(p.as_ptr(), &mut size as *mut usize) };
    if ptr.is_null() {
        return None;
    }
    if size == 0 {
        // Frees the buffer of an empty file
        unsafe { miku_free(ptr) };
        return None;
    }
    Some((ptr, size))
}

pub fn write(fd: u64, data: &[u8]) -> i64 {
    unsafe { miku_write(fd, data.as_ptr(), data.len()) }
}

// bitops safe wrappers
pub fn popcount(x: u64) -> u64 {
    unsafe { miku_popcount64(x) }
}
pub fn clz(x: u64) -> u64 {
    unsafe { miku_clz64(x) }
}
pub fn ctz(x: u64) -> u64 {
    unsafe { miku_ctz64(x) }
}
pub fn log2(x: u64) -> u64 {
    unsafe { miku_log2(x) }
}
pub fn is_power_of_two(x: u64) -> bool {
    unsafe { miku_is_power_of_two(x) }
}
pub fn next_power_of_two(x: u64) -> u64 {
    unsafe { miku_next_power_of_two(x) }
}
pub fn align_up(val: u64, align: u64) -> u64 {
    unsafe { miku_align_up(val, align) }
}
pub fn align_down(val: u64, align: u64) -> u64 {
    unsafe { miku_align_down(val, align) }
}

// hash safe wrappers
pub fn hash_bytes(data: &[u8]) -> u64 {
    unsafe { miku_hash_bytes(data.as_ptr(), data.len()) }
}
pub fn crc32(data: &[u8]) -> u32 {
    unsafe { miku_crc32(data.as_ptr(), data.len()) }
}
pub fn fnv1a(data: &[u8]) -> u64 {
    unsafe { miku_fnv1a_64(data.as_ptr(), data.len()) }
}

// base64 safe wrappers
pub fn base64_encode(data: &[u8], out: &mut [u8]) -> i32 {
    unsafe { miku_base64_encode(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len()) }
}
pub fn base64_decode(data: &[u8], out: &mut [u8]) -> i32 {
    unsafe { miku_base64_decode(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len()) }
}

// utf8 safe wrappers
pub fn utf8_valid(s: &[u8]) -> bool {
    unsafe { miku_utf8_valid(s.as_ptr(), s.len()) }
}
pub fn utf8_len(s: &[u8]) -> usize {
    unsafe { miku_utf8_len(s.as_ptr(), s.len()) }
}

// path safe wrappers
pub fn path_is_absolute(path: &[u8]) -> bool {
    unsafe { miku_path_is_absolute(path.as_ptr()) }
}
pub fn path_depth(path: &[u8]) -> usize {
    unsafe { miku_path_depth(path.as_ptr()) }
}

// lz compression
extern "C" {
    pub fn miku_lz_compress(input: *const u8, len: usize, out_len: *mut usize) -> *mut u8;
    pub fn miku_lz_decompress(
        input: *const u8,
        len: usize,
        out_len: *mut usize,
        max_out: usize,
    ) -> *mut u8;
    pub fn miku_lz_compress_buf(input: *const u8, len: usize, out: *mut u8, out_max: usize) -> i32;
    pub fn miku_lz_decompress_buf(
        input: *const u8,
        len: usize,
        out: *mut u8,
        out_max: usize,
    ) -> i32;
    pub fn miku_lz_compress_bound(len: usize) -> usize;
}

// env
extern "C" {
    pub fn miku_setenv(key: *const u8, val: *const u8) -> bool;
    pub fn miku_getenv(key: *const u8) -> *const u8;
    pub fn miku_unsetenv(key: *const u8) -> bool;
    pub fn miku_hasenv(key: *const u8) -> bool;
    pub fn miku_env_count() -> usize;
    pub fn miku_env_clear();
    pub fn miku_env_iter(cb: extern "C" fn(*const u8, *const u8, *mut u8), ctx: *mut u8);
    pub fn miku_putenv(s: *const u8) -> bool;
    /// Seeds the environment from the kernel envp (3rd argument of
    /// `_start_main`). Call once; returns the count
    pub fn miku_env_init(envp: *const *const u8) -> usize;
    /// Environment as `execve` wants it: a NULL-terminated array of
    /// "KEY=VALUE". Valid until the next `setenv`/`unsetenv`/`env_clear`
    pub fn miku_environ() -> *mut *mut u8;
}

// process control
extern "C" {
    /// 0 in the child, child pid in the parent, negative errno on failure
    pub fn miku_fork() -> i64;
    /// Blocks until a child exits. pid 0 is any child; status may be null.
    /// Returns the reaped pid or a negative errno
    pub fn miku_waitpid(pid: u64, status: *mut i64) -> i64;
    pub fn miku_kill(pid: u64, sig: u64) -> i64;
    /// Replaces the process image with the kernel's default environment. argv
    /// is an array of C strings, argv[0] the name
    pub fn miku_exec(path: *const u8, argv: *const *const u8, argc: usize) -> i64;
    /// exec with explicit environment ("KEY=value" C strings)
    pub fn miku_execve(
        path: *const u8,
        argv: *const *const u8,
        argc: usize,
        envp: *const *const u8,
        envc: usize,
    ) -> i64;
}

// signal
extern "C" {
    pub fn miku_signal(sig: u32, handler: Option<extern "C" fn(u32)>)
        -> Option<extern "C" fn(u32)>;
    pub fn miku_signal_dispatch(sig: u32) -> bool;
    pub fn miku_signal_has_handler(sig: u32) -> bool;
    pub fn miku_signal_reset_all();
    pub fn miku_signal_block(sig: u32);
    pub fn miku_signal_unblock(sig: u32);
    pub fn miku_signal_is_blocked(sig: u32) -> bool;
    pub fn miku_signal_get_mask() -> u32;
    pub fn miku_signal_set_mask(mask: u32) -> u32;
    pub fn miku_sigaction(sig: u32, act: *const MikuSigaction, oldact: *mut MikuSigaction) -> i32;
    pub fn miku_sigaction_dispatch(sig: u32) -> bool;
    pub fn miku_signal_raise(sig: u32) -> i32;
    pub fn miku_signal_pending() -> u32;

    // errno
    pub fn miku_strerror(code: i64) -> *const u8;
    pub fn miku_perror(prefix: *const u8);
    pub fn miku_perror_code(prefix: *const u8, code: i64);

    // env (new)
    pub fn miku_getenv_r(key: *const u8, buf: *mut u8, buf_size: usize) -> i32;
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuSigaction {
    pub handler: Option<extern "C" fn(u32)>,
    pub flags: u32,
    pub mask: u32,
}

// json
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuJsonToken {
    pub ttype: u8,
    pub start: u32,
    pub end: u32,
    pub size: u32,
    pub parent: i32,
}

#[repr(C)]
pub struct MikuJsonParser {
    pub pos: usize,
    pub next_tok: usize,
    pub super_stack: [i32; 32],
    pub depth: usize,
}

#[repr(C)]
pub struct MikuJsonWriter {
    pub buf: *mut u8,
    pub cap: usize,
    pub len: usize,
    pub depth: usize,
    pub container_stack: [u8; 32],
    pub needs_comma: [bool; 32],
    pub error: bool,
}

extern "C" {
    pub fn miku_json_init(p: *mut MikuJsonParser);
    pub fn miku_json_parse(
        p: *mut MikuJsonParser,
        data: *const u8,
        len: usize,
        tokens: *mut MikuJsonToken,
        max_tokens: usize,
    ) -> i32;
    pub fn miku_json_type(tokens: *const MikuJsonToken, index: usize) -> u8;
    pub fn miku_json_value(
        data: *const u8,
        tokens: *const MikuJsonToken,
        index: usize,
        out_len: *mut usize,
    ) -> *const u8;
    pub fn miku_json_eq(
        data: *const u8,
        tokens: *const MikuJsonToken,
        index: usize,
        s: *const u8,
    ) -> bool;
    pub fn miku_json_size(tokens: *const MikuJsonToken, index: usize) -> u32;
    pub fn miku_json_find(
        data: *const u8,
        tokens: *const MikuJsonToken,
        num_tokens: usize,
        obj_index: usize,
        key: *const u8,
    ) -> i32;
    pub fn miku_json_token_count(p: *const MikuJsonParser) -> usize;
}

// ringbuf2 (byte ring)
#[repr(C)]
pub struct MikuByteRing {
    pub data: *mut u8,
    pub cap: usize,
    pub head: usize,
    pub tail: usize,
}

extern "C" {
    pub fn miku_bring_new(min_cap: usize) -> MikuByteRing;
    pub fn miku_bring_free(r: *mut MikuByteRing);
    pub fn miku_bring_len(r: *const MikuByteRing) -> usize;
    pub fn miku_bring_avail(r: *const MikuByteRing) -> usize;
    pub fn miku_bring_is_empty(r: *const MikuByteRing) -> bool;
    pub fn miku_bring_write(r: *mut MikuByteRing, data: *const u8, len: usize) -> usize;
    pub fn miku_bring_read(r: *mut MikuByteRing, out: *mut u8, len: usize) -> usize;
    pub fn miku_bring_peek(r: *const MikuByteRing, out: *mut u8, len: usize) -> usize;
    pub fn miku_bring_skip(r: *mut MikuByteRing, len: usize) -> usize;
    pub fn miku_bring_put(r: *mut MikuByteRing, byte: u8) -> bool;
    pub fn miku_bring_get(r: *mut MikuByteRing, out: *mut u8) -> bool;
    pub fn miku_bring_find(r: *const MikuByteRing, byte: u8) -> i32;
    pub fn miku_bring_readline(r: *mut MikuByteRing, out: *mut u8, max_len: usize) -> usize;
    pub fn miku_bring_clear(r: *mut MikuByteRing);
    pub fn miku_bring_capacity(r: *const MikuByteRing) -> usize;
}

// sha256
#[repr(C)]
pub struct MikuSha256Ctx {
    pub state: [u32; 8],
    pub buf: [u8; 64],
    pub buf_len: usize,
    pub total_len: u64,
}

extern "C" {
    pub fn miku_sha256_init(ctx: *mut MikuSha256Ctx);
    pub fn miku_sha256_update(ctx: *mut MikuSha256Ctx, data: *const u8, len: usize);
    pub fn miku_sha256_finish(ctx: *mut MikuSha256Ctx, out: *mut u8);
    pub fn miku_sha256(data: *const u8, len: usize, out: *mut u8);
    pub fn miku_sha256_eq(a: *const u8, b: *const u8) -> bool;
    pub fn miku_sha256_hex(hash: *const u8, out: *mut u8);
}

// uuid
#[repr(C)]
pub struct MikuUuid {
    pub bytes: [u8; 16],
}

extern "C" {
    pub fn miku_uuid_gen() -> MikuUuid;
    pub fn miku_uuid_format(uuid: *const MikuUuid, buf: *mut u8) -> *mut u8;
    pub fn miku_uuid_parse(s: *const u8, uuid: *mut MikuUuid) -> bool;
    pub fn miku_uuid_eq(a: *const MikuUuid, b: *const MikuUuid) -> bool;
    pub fn miku_uuid_is_nil(uuid: *const MikuUuid) -> bool;
    pub fn miku_uuid_nil() -> MikuUuid;
}

// strbuf
#[repr(C)]
pub struct MikuStr {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
}

extern "C" {
    pub fn miku_str_new() -> MikuStr;
    pub fn miku_str_from(s: *const u8) -> MikuStr;
    pub fn miku_str_free(s: *mut MikuStr);
    pub fn miku_str_cstr(s: *const MikuStr) -> *const u8;
    pub fn miku_str_len(s: *const MikuStr) -> usize;
    pub fn miku_str_empty(s: *const MikuStr) -> bool;
    pub fn miku_str_push(s: *mut MikuStr, text: *const u8) -> bool;
    pub fn miku_str_push_char(s: *mut MikuStr, c: u8) -> bool;
    pub fn miku_str_push_int(s: *mut MikuStr, val: i64) -> bool;
    pub fn miku_str_clear(s: *mut MikuStr);
    pub fn miku_str_eq(s: *const MikuStr, other: *const u8) -> bool;
    pub fn miku_str_starts_with(s: *const MikuStr, prefix: *const u8) -> bool;
    pub fn miku_str_ends_with(s: *const MikuStr, suffix: *const u8) -> bool;
    pub fn miku_str_find(s: *const MikuStr, needle: *const u8) -> i32;
    pub fn miku_str_contains(s: *const MikuStr, needle: *const u8) -> bool;
    pub fn miku_str_at(s: *const MikuStr, index: usize) -> u8;
    pub fn miku_str_trim(s: *mut MikuStr);
    pub fn miku_str_to_upper(s: *mut MikuStr);
    pub fn miku_str_to_lower(s: *mut MikuStr);
    pub fn miku_str_substr(s: *const MikuStr, start: usize, len: usize) -> MikuStr;
    pub fn miku_str_clone(s: *const MikuStr) -> MikuStr;
}

// pool
#[repr(C)]
pub struct MikuPool {
    pub data: *mut u8,
    pub generations: *mut u32,
    pub free_list: *mut u32,
    pub obj_size: usize,
    pub capacity: usize,
    pub free_count: usize,
    pub active_count: usize,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct PoolHandle(pub u64);

extern "C" {
    pub fn miku_pool_new(obj_size: usize, capacity: usize) -> MikuPool;
    pub fn miku_pool_free(p: *mut MikuPool);
    pub fn miku_pool_alloc(p: *mut MikuPool) -> PoolHandle;
    pub fn miku_pool_release(p: *mut MikuPool, handle: PoolHandle) -> bool;
    pub fn miku_pool_get(p: *const MikuPool, handle: PoolHandle) -> *mut u8;
    pub fn miku_pool_valid(p: *const MikuPool, handle: PoolHandle) -> bool;
    pub fn miku_pool_active(p: *const MikuPool) -> usize;
    pub fn miku_pool_capacity(p: *const MikuPool) -> usize;
    pub fn miku_pool_available(p: *const MikuPool) -> usize;
}

// event
extern "C" {
    pub fn miku_event_on(
        event_id: u32,
        handler: extern "C" fn(u32, *mut u8, *mut u8),
        ctx: *mut u8,
    ) -> i32;
    pub fn miku_event_off(index: i32);
    pub fn miku_event_emit(event_id: u32, data: *mut u8) -> u32;
    pub fn miku_event_has_listeners(event_id: u32) -> bool;
    pub fn miku_event_count(event_id: u32) -> u32;
    pub fn miku_event_clear(event_id: u32);
    pub fn miku_event_clear_all();
}

// log
extern "C" {
    pub fn miku_log_set_level(level: u8);
    pub fn miku_log_get_level() -> u8;
    pub fn miku_log(level: u8, tag: *const u8, msg: *const u8);
    pub fn miku_log_error(tag: *const u8, msg: *const u8);
    pub fn miku_log_warn(tag: *const u8, msg: *const u8);
    pub fn miku_log_info(tag: *const u8, msg: *const u8);
    pub fn miku_log_debug(tag: *const u8, msg: *const u8);
}

// test framework
extern "C" {
    pub fn miku_test_reset();
    pub fn miku_test_suite(name: *const u8);
    pub fn miku_test_summary() -> i32;
}

// bufio
#[repr(C)]
pub struct MikuBufReader {
    pub fd: i64,
    pub buf: *mut u8,
    pub cap: usize,
    pub pos: usize,
    pub filled: usize,
}

#[repr(C)]
pub struct MikuBufWriter {
    pub fd: i64,
    pub buf: *mut u8,
    pub cap: usize,
    pub pos: usize,
    pub mode: u32,
}

extern "C" {
    pub fn miku_bufreader_new(fd: i64) -> MikuBufReader;
    pub fn miku_bufreader_free(r: *mut MikuBufReader);
    pub fn miku_bufreader_read(r: *mut MikuBufReader, dst: *mut u8, len: usize) -> i64;
    pub fn miku_bufreader_getc(r: *mut MikuBufReader) -> i32;
    pub fn miku_bufreader_readline(r: *mut MikuBufReader, dst: *mut u8, max: usize) -> i64;
    pub fn miku_bufreader_peek(r: *mut MikuBufReader) -> i32;
    pub fn miku_bufreader_buffered(r: *const MikuBufReader) -> usize;
    pub fn miku_bufwriter_new(fd: i64) -> MikuBufWriter;
    pub fn miku_bufwriter_free(w: *mut MikuBufWriter);
    pub fn miku_bufwriter_set_mode(w: *mut MikuBufWriter, mode: u32);
    pub fn miku_bufwriter_flush(w: *mut MikuBufWriter) -> i64;
    pub fn miku_bufwriter_write(w: *mut MikuBufWriter, src: *const u8, len: usize) -> i64;
    pub fn miku_bufwriter_putc(w: *mut MikuBufWriter, c: u8) -> i32;
    pub fn miku_bufwriter_puts(w: *mut MikuBufWriter, s: *const u8) -> i64;
    pub fn miku_bufwriter_pending(w: *const MikuBufWriter) -> usize;
}

// Directory handle, matching libmiku's `MikuDir` by value. Prefer `read_dir`
#[repr(C)]
pub struct MikuDir {
    pub path: [u8; 256],
    pub path_len: usize,
    _entries: [MikuDirent; 8],
    pub total: usize,
    pub cursor: usize,
    pub offset: usize,
    pub fd: i64,
    pub done: bool,
}

extern "C" {
    pub fn miku_dir_open(path: *const u8, path_len: usize) -> MikuDir;
    pub fn miku_dir_close(d: *mut MikuDir);
    pub fn miku_dir_next(d: *mut MikuDir, ent: *mut MikuDirent) -> bool;
    pub fn miku_dir_is_open(d: *const MikuDir) -> bool;
    pub fn miku_dir_count(path: *const u8, path_len: usize) -> i64;
    pub fn miku_dir_walk(
        path: *const u8,
        path_len: usize,
        cb: extern "C" fn(*const u8, usize, *const MikuDirent, usize, *mut u8) -> i32,
        ctx: *mut u8,
    ) -> i32;
    pub fn miku_is_directory(path: *const u8) -> bool;
    pub fn miku_mkdir_p(path: *const u8, mode: u32) -> i64;
}

// datetime
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuDateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub weekday: u8,
    pub yearday: u16,
}

extern "C" {
    pub fn miku_dt_from_timestamp(ts: i64) -> MikuDateTime;
    pub fn miku_dt_to_timestamp(dt: *const MikuDateTime) -> i64;
    pub fn miku_dt_now() -> MikuDateTime;
    pub fn miku_dt_format(dt: *const MikuDateTime, buf: *mut u8, len: usize) -> usize;
    pub fn miku_dt_format_date(dt: *const MikuDateTime, buf: *mut u8, len: usize) -> usize;
    pub fn miku_dt_format_time(dt: *const MikuDateTime, buf: *mut u8, len: usize) -> usize;
    pub fn miku_dt_format_iso(dt: *const MikuDateTime, buf: *mut u8, len: usize) -> usize;
    pub fn miku_dt_weekday_name(day: u8) -> *const u8;
    pub fn miku_dt_month_name(month: u8) -> *const u8;
    pub fn miku_dt_diff_secs(a: *const MikuDateTime, b: *const MikuDateTime) -> i64;
    pub fn miku_dt_add_secs(dt: *const MikuDateTime, secs: i64) -> MikuDateTime;
    pub fn miku_dt_add_days(dt: *const MikuDateTime, days: i32) -> MikuDateTime;
}

// trie
#[repr(C)]
pub struct MikuTrie {
    pub nodes: *mut u8,
    pub count: usize,
    pub cap: usize,
}

extern "C" {
    pub fn miku_trie_new() -> MikuTrie;
    pub fn miku_trie_free(t: *mut MikuTrie);
    pub fn miku_trie_insert(t: *mut MikuTrie, key: *const u8, len: usize, value: u64) -> bool;
    pub fn miku_trie_search(t: *const MikuTrie, key: *const u8, len: usize) -> bool;
    pub fn miku_trie_get(t: *const MikuTrie, key: *const u8, len: usize) -> u64;
    pub fn miku_trie_has_prefix(t: *const MikuTrie, prefix: *const u8, len: usize) -> bool;
    pub fn miku_trie_remove(t: *mut MikuTrie, key: *const u8, len: usize) -> bool;
    pub fn miku_trie_prefix_collect(
        t: *const MikuTrie,
        prefix: *const u8,
        prefix_len: usize,
        buf: *mut u8,
        buf_len: usize,
        max: usize,
    ) -> usize;
    pub fn miku_trie_node_count(t: *const MikuTrie) -> usize;
}

// args
#[repr(C)]
#[derive(Copy, Clone)]
pub struct ArgSlice {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
pub struct MikuArgs {
    _pad: [u8; 8192],
}

extern "C" {
    pub fn miku_args_new() -> MikuArgs;
    pub fn miku_args_flag(a: *mut MikuArgs, short: u8, long: *const u8, help: *const u8);
    pub fn miku_args_option(a: *mut MikuArgs, short: u8, long: *const u8, help: *const u8);
    pub fn miku_args_int_option(a: *mut MikuArgs, short: u8, long: *const u8, help: *const u8);
    pub fn miku_args_parse(a: *mut MikuArgs, argv: *const ArgSlice, argc: usize) -> bool;
    pub fn miku_args_has(a: *const MikuArgs, long: *const u8) -> bool;
    pub fn miku_args_get(a: *const MikuArgs, long: *const u8) -> *const u8;
    pub fn miku_args_get_int(a: *const MikuArgs, long: *const u8) -> i64;
    pub fn miku_args_positional_count(a: *const MikuArgs) -> usize;
    pub fn miku_args_positional(a: *const MikuArgs, idx: usize, out_len: *mut usize) -> *const u8;
    pub fn miku_args_has_error(a: *const MikuArgs) -> bool;
    pub fn miku_args_error(a: *const MikuArgs) -> *const u8;
}

// queue
#[repr(C)]
pub struct MikuQueue {
    pub data: *mut u8,
    pub elem_size: usize,
    pub cap: usize,
    pub head: usize,
    pub tail: usize,
    pub count: usize,
}

extern "C" {
    pub fn miku_queue_new(elem_size: usize, capacity: usize) -> MikuQueue;
    pub fn miku_queue_free(q: *mut MikuQueue);
    pub fn miku_queue_push(q: *mut MikuQueue, elem: *const u8) -> bool;
    pub fn miku_queue_pop(q: *mut MikuQueue, out: *mut u8) -> bool;
    pub fn miku_queue_peek(q: *const MikuQueue, out: *mut u8) -> bool;
    pub fn miku_queue_peek_back(q: *const MikuQueue, out: *mut u8) -> bool;
    pub fn miku_queue_pop_back(q: *mut MikuQueue, out: *mut u8) -> bool;
    pub fn miku_queue_len(q: *const MikuQueue) -> usize;
    pub fn miku_queue_capacity(q: *const MikuQueue) -> usize;
    pub fn miku_queue_is_empty(q: *const MikuQueue) -> bool;
    pub fn miku_queue_is_full(q: *const MikuQueue) -> bool;
    pub fn miku_queue_clear(q: *mut MikuQueue);
    pub fn miku_queue_at(q: *const MikuQueue, idx: usize, out: *mut u8) -> bool;

    // math (extended)
    pub fn miku_gcd(a: u64, b: u64) -> u64;
    pub fn miku_lcm(a: u64, b: u64) -> u64;
    pub fn miku_pow(base: i64, exp: u32) -> i64;
    pub fn miku_upow(base: u64, exp: u32) -> u64;
    pub fn miku_isqrt(n: u64) -> u64;
    pub fn miku_icbrt(n: u64) -> u64;
    pub fn miku_ilog2(n: u64) -> u32;
    pub fn miku_ilog10(n: u64) -> u32;
    pub fn miku_sign(x: i64) -> i32;
    pub fn miku_map(val: i64, in_lo: i64, in_hi: i64, out_lo: i64, out_hi: i64) -> i64;
    pub fn miku_lerp(a: i64, b: i64, t_permille: u32) -> i64;
    pub fn miku_sadd(a: i64, b: i64) -> i64;
    pub fn miku_ssub(a: i64, b: i64) -> i64;
    pub fn miku_smul(a: i64, b: i64) -> i64;
    pub fn miku_usadd(a: u64, b: u64) -> u64;
    pub fn miku_ussub(a: u64, b: u64) -> u64;
    pub fn miku_div_ceil(a: u64, b: u64) -> u64;
    pub fn miku_modpow(base: u64, exp: u64, modulus: u64) -> u64;
    pub fn miku_is_prime(n: u64) -> i32;
    pub fn miku_fib(n: u32) -> u64;
    pub fn miku_factorial(n: u32) -> u64;
    pub fn miku_binomial(n: u64, k: u64) -> u64;

    // endian (extended)
    pub fn miku_write_u64_be(ptr: *mut u8, val: u64);
    pub fn miku_write_u64_le(ptr: *mut u8, val: u64);

    // strbuf (extended)
    pub fn miku_str_insert(s: *mut MikuStr, pos: usize, text: *const u8) -> bool;
    pub fn miku_str_remove(s: *mut MikuStr, start: usize, count: usize) -> bool;
    pub fn miku_str_replace(s: *mut MikuStr, needle: *const u8, replacement: *const u8) -> bool;
    pub fn miku_str_replace_all(s: *mut MikuStr, needle: *const u8, replacement: *const u8) -> i32;
    pub fn miku_str_reverse(s: *mut MikuStr);
    pub fn miku_str_count(s: *const MikuStr, needle: *const u8) -> i32;
    pub fn miku_str_repeat_new(text: *const u8, n: u32) -> MikuStr;
    pub fn miku_str_split(
        s: *const MikuStr,
        delim: u8,
        cb: extern "C" fn(*const u8, usize, usize),
        user_data: usize,
    ) -> i32;

    // glob (extended)
    pub fn miku_glob_filter(
        pattern: *const u8,
        strings: *const *const u8,
        count: usize,
        out: *mut *const u8,
    ) -> usize;

    // sort (extended)
    pub fn miku_lower_bound(
        key: *const u8,
        base: *const u8,
        count: usize,
        size: usize,
        cmp: CmpFn,
    ) -> usize;
    pub fn miku_upper_bound(
        key: *const u8,
        base: *const u8,
        count: usize,
        size: usize,
        cmp: CmpFn,
    ) -> usize;
    pub fn miku_unique(base: *mut u8, count: usize, size: usize, cmp: CmpFn) -> usize;
    pub fn miku_nth_element(base: *mut u8, count: usize, size: usize, nth: usize, cmp: CmpFn);
    pub fn miku_cmp_i32(a: *const u8, b: *const u8) -> i32;
    pub fn miku_cmp_u32(a: *const u8, b: *const u8) -> i32;

    // list (extended)
    pub fn miku_list_index_of(l: *const MikuList, elem: *const u8) -> i32;
    pub fn miku_list_reverse(l: *mut MikuList);
    pub fn miku_list_remove_if(l: *mut MikuList, pred: extern "C" fn(*const u8) -> bool) -> usize;
    pub fn miku_list_front(l: *const MikuList) -> *const u8;
    pub fn miku_list_back(l: *const MikuList) -> *const u8;

    // path (extended)
    pub fn miku_path_has_ext(path: *const u8, ext: *const u8) -> bool;
    pub fn miku_path_common(a: *const u8, b: *const u8) -> *mut u8;
    pub fn miku_path_is_relative(path: *const u8) -> bool;
    pub fn miku_path_parent(path: *const u8) -> *mut u8;

    // json (extended)
    pub fn miku_json_array_get(
        tokens: *const MikuJsonToken,
        num_tokens: usize,
        arr_index: usize,
        elem_index: usize,
    ) -> i32;
    pub fn miku_json_int(data: *const u8, tokens: *const MikuJsonToken, index: usize) -> i64;
    pub fn miku_json_u64(data: *const u8, tokens: *const MikuJsonToken, index: usize) -> u64;
    pub fn miku_json_bool(data: *const u8, tokens: *const MikuJsonToken, index: usize) -> i32;
    pub fn miku_json_is_null(tokens: *const MikuJsonToken, index: usize) -> bool;
    pub fn miku_json_strcpy(
        data: *const u8,
        tokens: *const MikuJsonToken,
        index: usize,
        buf: *mut u8,
        buf_size: usize,
    ) -> i32;
    pub fn miku_json_unescape(
        data: *const u8,
        tokens: *const MikuJsonToken,
        index: usize,
        buf: *mut u8,
        buf_size: usize,
    ) -> i32;
    pub fn miku_json_get_path(
        data: *const u8,
        tokens: *const MikuJsonToken,
        num_tokens: usize,
        root: usize,
        path: *const u8,
    ) -> i32;
    pub fn miku_json_object_iter(
        tokens: *const MikuJsonToken,
        num_tokens: usize,
        obj_index: usize,
        cb: unsafe extern "C" fn(usize, usize, *mut u8),
        user: *mut u8,
    ) -> i32;
    pub fn miku_json_array_iter(
        tokens: *const MikuJsonToken,
        num_tokens: usize,
        arr_index: usize,
        cb: unsafe extern "C" fn(usize, usize, *mut u8),
        user: *mut u8,
    ) -> i32;
    pub fn miku_json_strdup(data: *const u8, tokens: *const MikuJsonToken, index: usize)
        -> *mut u8;
    pub fn miku_json_validate(data: *const u8, len: usize) -> i32;
    pub fn miku_json_parent(tokens: *const MikuJsonToken, index: usize) -> i32;
    pub fn miku_json_is_string(tokens: *const MikuJsonToken, index: usize) -> bool;
    pub fn miku_json_is_number(tokens: *const MikuJsonToken, index: usize) -> bool;
    pub fn miku_json_is_bool(tokens: *const MikuJsonToken, index: usize) -> bool;
    pub fn miku_json_is_object(tokens: *const MikuJsonToken, index: usize) -> bool;
    pub fn miku_json_is_array(tokens: *const MikuJsonToken, index: usize) -> bool;

    // json writer
    pub fn miku_json_writer_init(w: *mut MikuJsonWriter, buf: *mut u8, cap: usize);
    pub fn miku_json_write_object_begin(w: *mut MikuJsonWriter);
    pub fn miku_json_write_object_end(w: *mut MikuJsonWriter);
    pub fn miku_json_write_array_begin(w: *mut MikuJsonWriter);
    pub fn miku_json_write_array_end(w: *mut MikuJsonWriter);
    pub fn miku_json_write_key(w: *mut MikuJsonWriter, key: *const u8);
    pub fn miku_json_write_str(w: *mut MikuJsonWriter, val: *const u8);
    pub fn miku_json_write_strn(w: *mut MikuJsonWriter, val: *const u8, len: usize);
    pub fn miku_json_write_int(w: *mut MikuJsonWriter, val: i64);
    pub fn miku_json_write_u64(w: *mut MikuJsonWriter, val: u64);
    pub fn miku_json_write_bool(w: *mut MikuJsonWriter, val: bool);
    pub fn miku_json_write_null(w: *mut MikuJsonWriter);
    pub fn miku_json_write_raw(w: *mut MikuJsonWriter, raw: *const u8, len: usize);
    pub fn miku_json_write_finish(w: *mut MikuJsonWriter) -> i32;
    pub fn miku_json_write_error(w: *const MikuJsonWriter) -> bool;
    pub fn miku_json_write_kv_str(w: *mut MikuJsonWriter, key: *const u8, val: *const u8);
    pub fn miku_json_write_kv_int(w: *mut MikuJsonWriter, key: *const u8, val: i64);
    pub fn miku_json_write_kv_bool(w: *mut MikuJsonWriter, key: *const u8, val: bool);
    pub fn miku_json_write_kv_null(w: *mut MikuJsonWriter, key: *const u8);

    // datetime (extended)
    pub fn miku_dt_valid(dt: *const MikuDateTime) -> bool;
    pub fn miku_dt_is_leap_year(year: i32) -> bool;
    pub fn miku_dt_days_in_month(month: u8, year: i32) -> u8;
    pub fn miku_dt_days_in_year(year: i32) -> u16;
    pub fn miku_dt_weekday_short(day: u8) -> *const u8;
    pub fn miku_dt_month_short(month: u8) -> *const u8;
    pub fn miku_dt_cmp(a: *const MikuDateTime, b: *const MikuDateTime) -> i32;
    pub fn miku_dt_format_rfc2822(dt: *const MikuDateTime, buf: *mut u8, buf_len: usize) -> usize;

    // sort (remaining)
    pub fn miku_cmp_str(a: *const u8, b: *const u8) -> i32;
    pub fn miku_insertion_sort(base: *mut u8, count: usize, size: usize, cmp: CmpFn);

    // log (remaining)
    pub fn miku_log_show_time(show: bool);
    pub fn miku_log_trace(tag: *const u8, msg: *const u8);
    pub fn miku_log_int(level: u8, tag: *const u8, msg: *const u8, val: i64);
    pub fn miku_log_hex(level: u8, tag: *const u8, msg: *const u8, val: u64);
    pub fn miku_log_ptr(level: u8, tag: *const u8, msg: *const u8, ptr: *const u8);
    pub fn miku_log_int2(level: u8, tag: *const u8, msg: *const u8, a: i64, b: i64);

    // list (remaining)
    pub fn miku_list_iter(
        l: *const MikuList,
        cb: extern "C" fn(*const u8, usize, *mut u8),
        user_data: *mut u8,
    );

    // strbuf (remaining)
    pub fn miku_str_push_bytes(s: *mut MikuStr, data: *const u8, len: usize) -> bool;
    pub fn miku_str_with_capacity(cap: usize) -> MikuStr;

    // convert
    pub fn miku_strtod(s: *const u8, endptr: *mut *const u8) -> i64;

    // timer/stopwatch
    pub fn miku_sw_start() -> MikuStopwatch;
    pub fn miku_sw_elapsed_ms(sw: *const MikuStopwatch) -> u64;
    pub fn miku_sw_elapsed_sec(sw: *const MikuStopwatch) -> u64;
    pub fn miku_sw_pause(sw: *mut MikuStopwatch);
    pub fn miku_sw_resume(sw: *mut MikuStopwatch);
    pub fn miku_sw_reset(sw: *mut MikuStopwatch);
    pub fn miku_sw_running(sw: *const MikuStopwatch) -> bool;
    pub fn miku_timer_once(duration_ms: u64) -> MikuTimer;
    pub fn miku_timer_repeat(interval_ms: u64) -> MikuTimer;
    pub fn miku_timer_check(t: *mut MikuTimer) -> bool;
    pub fn miku_timer_remaining(t: *const MikuTimer) -> u64;
    pub fn miku_timer_reset(t: *mut MikuTimer);
    pub fn miku_timer_expired(t: *const MikuTimer) -> bool;
    pub fn miku_delay_ms(ms: u64);
    pub fn miku_delay_sleep(ms: u64);

    // test framework
    pub fn miku_test(name: *const u8, condition: bool) -> bool;
    pub fn miku_test_eq(name: *const u8, actual: i64, expected: i64) -> bool;
    pub fn miku_test_streq(name: *const u8, actual: *const u8, expected: *const u8) -> bool;
    pub fn miku_test_not_null(name: *const u8, ptr: *const u8) -> bool;
    pub fn miku_test_null(name: *const u8, ptr: *const u8) -> bool;
    pub fn miku_test_passed() -> u32;
    pub fn miku_test_failed() -> u32;
    pub fn miku_test_total() -> u32;

    // hashmap (remaining)
    pub fn miku_map_iter(
        m: *const MikuMap,
        cb: extern "C" fn(*const u8, *const u8, *mut u8),
        user_data: *mut u8,
    );

    // bufio (remaining)
    pub fn miku_bufreader_with_capacity(fd: i64, cap: usize) -> MikuBufReader;
    pub fn miku_bufwriter_with_capacity(fd: i64, cap: usize) -> MikuBufWriter;

    // pool (remaining)
    pub fn miku_pool_iter(
        p: *const MikuPool,
        cb: extern "C" fn(u64, *mut u8, *mut u8),
        ctx: *mut u8,
    );

    // csv
    pub fn miku_csv_new() -> MikuCsv;
    pub fn miku_csv_with_delimiter(delim: u8) -> MikuCsv;
    pub fn miku_csv_parse(csv: *mut MikuCsv, data: *const u8, data_len: usize) -> i32;
    pub fn miku_csv_rows(csv: *const MikuCsv) -> usize;
    pub fn miku_csv_cols(csv: *const MikuCsv, row: usize) -> usize;
    pub fn miku_csv_field(
        csv: *const MikuCsv,
        data: *const u8,
        row: usize,
        col: usize,
        out_len: *mut usize,
    ) -> *const u8;
    pub fn miku_csv_field_eq(
        csv: *const MikuCsv,
        data: *const u8,
        row: usize,
        col: usize,
        s: *const u8,
    ) -> bool;
    pub fn miku_csv_field_int(
        csv: *const MikuCsv,
        data: *const u8,
        row: usize,
        col: usize,
        default: i64,
    ) -> i64;

    // ini
    pub fn miku_ini_new() -> MikuIni;
    pub fn miku_ini_parse(ini: *mut MikuIni, data: *const u8, data_len: usize) -> i32;
    pub fn miku_ini_get(ini: *const MikuIni, section: *const u8, key: *const u8) -> *const u8;
    pub fn miku_ini_get_int(
        ini: *const MikuIni,
        section: *const u8,
        key: *const u8,
        default: i64,
    ) -> i64;
    pub fn miku_ini_get_bool(
        ini: *const MikuIni,
        section: *const u8,
        key: *const u8,
        default: bool,
    ) -> bool;
    pub fn miku_ini_has_section(ini: *const MikuIni, section: *const u8) -> bool;
    pub fn miku_ini_has_key(ini: *const MikuIni, section: *const u8, key: *const u8) -> bool;
    pub fn miku_ini_count(ini: *const MikuIni) -> usize;
    pub fn miku_ini_iter_section(
        ini: *const MikuIni,
        section: *const u8,
        cb: extern "C" fn(*const u8, *const u8, *mut u8),
        ctx: *mut u8,
    );

    // Mutex
    pub fn miku_mutex_init(m: *mut MikuMutex);
    pub fn miku_mutex_lock(m: *mut MikuMutex);
    pub fn miku_mutex_unlock(m: *mut MikuMutex);
    pub fn miku_mutex_trylock(m: *mut MikuMutex) -> bool;
    pub fn miku_mutex_is_locked(m: *const MikuMutex) -> bool;

    // Atomics
    pub fn miku_atomic_init(a: *mut MikuAtomic, val: i64);
    pub fn miku_atomic_load(a: *const MikuAtomic) -> i64;
    pub fn miku_atomic_store(a: *mut MikuAtomic, val: i64);
    pub fn miku_atomic_add(a: *mut MikuAtomic, val: i64) -> i64;
    pub fn miku_atomic_sub(a: *mut MikuAtomic, val: i64) -> i64;
    pub fn miku_atomic_cas(a: *mut MikuAtomic, expected: i64, desired: i64) -> bool;
    pub fn miku_atomic_swap(a: *mut MikuAtomic, val: i64) -> i64;

    // Once
    pub fn miku_once_init(o: *mut MikuOnce);
    pub fn miku_once_call(o: *mut MikuOnce, f: extern "C" fn());
    pub fn miku_once_done(o: *const MikuOnce) -> bool;

    // convert (extended)
    pub fn miku_itoa_base(val: i64, buf: *mut u8, base: i32) -> *mut u8;
    pub fn miku_utoa_base(val: u64, buf: *mut u8, base: i32) -> *mut u8;

    // errno (extended)
    pub fn miku_is_error(val: i64) -> bool;
    pub fn miku_to_errno(val: i64) -> i64;
    pub fn miku_errno_name(code: i64) -> *const u8;
    pub fn miku_set_errno(code: i64);
    pub fn miku_get_errno() -> i64;

    // regex (extended)
    pub fn miku_regex_find_span(
        pattern: *const u8,
        text: *const u8,
        out_start: *mut usize,
        out_len: *mut usize,
    ) -> bool;
    pub fn miku_regex_replace(
        pattern: *const u8,
        text: *const u8,
        replacement: *const u8,
    ) -> *mut u8;
    pub fn miku_regex_replace_all(
        pattern: *const u8,
        text: *const u8,
        replacement: *const u8,
    ) -> *mut u8;
    pub fn miku_regex_split(
        pattern: *const u8,
        text: *const u8,
        out_starts: *mut *const u8,
        out_lens: *mut usize,
        max_parts: usize,
    ) -> usize;
    pub fn miku_regex_find_all(
        pattern: *const u8,
        text: *const u8,
        out_starts: *mut usize,
        out_lens: *mut usize,
        max_matches: usize,
    ) -> usize;

    // panic (extended)
    pub fn miku_assert_eq(a: i64, b: i64, file: *const u8, line: i32);
    pub fn miku_assert_not_null(ptr: *const u8, name: *const u8, file: *const u8, line: i32);
    pub fn miku_unreachable(file: *const u8, line: i32) -> !;
    pub fn miku_todo(msg: *const u8) -> !;

    // base64 (extended)
    pub fn miku_base64_is_valid(input: *const u8, len: usize) -> bool;

    // uuid (extended)
    pub fn miku_uuid_version(uuid: *const MikuUuid) -> u8;
    pub fn miku_uuid_variant(uuid: *const MikuUuid) -> u8;
    pub fn miku_uuid_cmp(a: *const MikuUuid, b: *const MikuUuid) -> i32;

    // sha256 (extended)
    pub fn miku_sha256_hmac(
        key: *const u8,
        key_len: usize,
        data: *const u8,
        data_len: usize,
        out: *mut u8,
    );

    // random (extended)
    pub fn miku_rand_bool() -> bool;
    pub fn miku_rand_uniform(bound: u64) -> u64;
    pub fn miku_rand_i64(lo: i64, hi: i64) -> i64;
    pub fn miku_rand_frac_million() -> u64;
    pub fn miku_rand_dice(sides: u32) -> u32;
    pub fn miku_rand_sample(n: usize, k: usize, out: *mut usize) -> usize;
    pub fn miku_rand_weighted(weights: *const u64, n: usize) -> usize;
    pub fn miku_rand_perm(n: usize, out: *mut usize);

    // checksum (extended)
    pub fn miku_crc16(data: *const u8, len: usize) -> u16;
    pub fn miku_crc16_update(prev: u16, data: *const u8, len: usize) -> u16;
    pub fn miku_luhn_check(data: *const u8, len: usize) -> bool;
    pub fn miku_luhn_digit(data: *const u8, len: usize) -> u8;
    pub fn miku_parity8(byte: u8) -> u8;
    pub fn miku_parity(data: *const u8, len: usize) -> u8;
    pub fn miku_sysv_checksum(data: *const u8, len: usize) -> u16;
    pub fn miku_crc32_combine(crc1: u32, crc2: u32, len2: usize) -> u32;

    // csv (extended)
    pub fn miku_csv_field_u64(
        csv: *const u8,
        data: *const u8,
        row: usize,
        col: usize,
        default: u64,
    ) -> u64;
    pub fn miku_csv_field_empty(csv: *const u8, row: usize, col: usize) -> bool;
    pub fn miku_csv_find_col(csv: *const u8, data: *const u8, name: *const u8) -> i32;
    pub fn miku_csv_writer_new(delim: u8) -> MikuCsvWriter;
    pub fn miku_csv_writer_init(buf: *mut u8, cap: usize, delim: u8) -> MikuCsvWriter;
    pub fn miku_csv_write_field(w: *mut MikuCsvWriter, data: *const u8, len: usize);
    pub fn miku_csv_write_cstr(w: *mut MikuCsvWriter, s: *const u8);
    pub fn miku_csv_write_int(w: *mut MikuCsvWriter, val: i64);
    pub fn miku_csv_write_row_end(w: *mut MikuCsvWriter);
    pub fn miku_csv_writer_len(w: *const MikuCsvWriter) -> usize;
    pub fn miku_csv_writer_data(w: *const MikuCsvWriter) -> *const u8;
    pub fn miku_csv_writer_error(w: *const MikuCsvWriter) -> bool;
    pub fn miku_csv_writer_free(w: *mut MikuCsvWriter);
    pub fn miku_csv_writer_reset(w: *mut MikuCsvWriter);

    // lz (extended)
    pub fn miku_rle_compress(input: *const u8, ilen: usize, out: *mut u8, omax: usize) -> i32;
    pub fn miku_rle_decompress(input: *const u8, ilen: usize, out: *mut u8, omax: usize) -> i32;
    pub fn miku_rle_compress_bound(ilen: usize) -> usize;
    pub fn miku_delta_encode(input: *const u8, len: usize, out: *mut u8);
    pub fn miku_delta_decode(input: *const u8, len: usize, out: *mut u8);

    // event (extended)
    pub fn miku_event_once(
        event_id: u32,
        handler: extern "C" fn(u32, *mut u8, *mut u8),
        ctx: *mut u8,
    ) -> i32;
    pub fn miku_event_post(event_id: u32, data: *mut u8) -> i32;
    pub fn miku_event_flush() -> u32;
    pub fn miku_event_pending() -> usize;
    pub fn miku_event_queue_clear();
}

#[repr(C)]
pub struct MikuCsvWriter {
    pub buf: *mut u8,
    pub cap: usize,
    pub len: usize,
    pub col: usize,
    pub delim: u8,
    pub error: bool,
}

/// Mirror of `libmiku`'s `MikuMutex`: a futex `u32`, with 0 free, 1 held, 2
/// held with waiters
#[repr(C, align(4))]
pub struct MikuMutex {
    /// `AtomicU32`: the same layout, allowing `&MikuMutex` while the library
    /// writes it
    pub state: core::sync::atomic::AtomicU32,
}

impl MikuMutex {
    pub const fn new() -> Self {
        MikuMutex { state: core::sync::atomic::AtomicU32::new(0) }
    }

    /// Takes the lock, sleeping if held.
    pub fn lock(&self) {
        unsafe { miku_mutex_lock(self as *const _ as *mut MikuMutex) }
    }

    /// Give it back, waking one sleeper if there is one.
    pub fn unlock(&self) {
        unsafe { miku_mutex_unlock(self as *const _ as *mut MikuMutex) }
    }

    /// Take it if it is free, without waiting.
    pub fn try_lock(&self) -> bool {
        unsafe { miku_mutex_trylock(self as *const _ as *mut MikuMutex) }
    }
}


#[repr(C)]
pub struct MikuAtomic {
    pub val: i64,
}

#[repr(C)]
pub struct MikuOnce {
    pub done: u8,
    pub running: u8,
}

// timer structs
#[repr(C)]
pub struct MikuStopwatch {
    pub start_ms: u64,
    pub paused_elapsed: u64,
    pub running: bool,
}

#[repr(C)]
pub struct MikuTimer {
    pub deadline_ms: u64,
    pub duration_ms: u64,
    pub repeat: bool,
    pub expired: bool,
}

// csv structs
#[repr(C)]
#[derive(Copy, Clone)]
pub struct CsvField {
    pub start: u32,
    pub len: u16,
    pub quoted: bool,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct CsvRow {
    pub fields: [CsvField; 64],
    pub nfields: usize,
}

#[repr(C)]
pub struct MikuCsv {
    pub rows: [CsvRow; 256],
    pub nrows: usize,
    pub delimiter: u8,
}

// ini struct (opaque-ish)
#[repr(C)]
#[derive(Copy, Clone)]
pub struct IniEntry {
    pub section: [u8; 32],
    pub key: [u8; 64],
    pub val: [u8; 128],
    pub used: bool,
}

#[repr(C)]
pub struct MikuIni {
    pub entries: [IniEntry; 128],
    pub count: usize,
}

// libc compatibility layer //

// libc FILE struct
#[repr(C)]
pub struct LibcFILE {
    pub fd: i64,
    pub flags: u32,
    pub error: i32,
    pub eof: i32,
    pub ungetc_buf: i32,
    pub read_buf: *mut u8,
    pub read_cap: usize,
    pub read_pos: usize,
    pub read_filled: usize,
    pub write_buf: *mut u8,
    pub write_cap: usize,
    pub write_pos: usize,
    pub buf_mode: u32,
}

// libc DIR struct
#[repr(C)]
pub struct LibcDIR {
    pub path: [u8; 256],
    pub entries: [MikuDirent; 128],
    pub count: usize,
    pub pos: usize,
}

// libc timespec
#[repr(C)]
pub struct LibcTimespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

extern "C" {
    // string.h //
    #[link_name = "strlen"]
    pub fn libc_strlen(s: *const u8) -> usize;
    pub fn strnlen(s: *const u8, maxlen: usize) -> usize;
    pub fn strcmp(s1: *const u8, s2: *const u8) -> i32;
    pub fn strncmp(s1: *const u8, s2: *const u8, n: usize) -> i32;
    pub fn strcasecmp(s1: *const u8, s2: *const u8) -> i32;
    pub fn strncasecmp(s1: *const u8, s2: *const u8, n: usize) -> i32;
    pub fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8;
    pub fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize;
    pub fn strcat(dst: *mut u8, src: *const u8) -> *mut u8;
    pub fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize;
    pub fn strchr(s: *const u8, c: i32) -> *const u8;
    pub fn strrchr(s: *const u8, c: i32) -> *const u8;
    pub fn strstr(haystack: *const u8, needle: *const u8) -> *const u8;
    pub fn strpbrk(s: *const u8, accept: *const u8) -> *const u8;
    pub fn strspn(s: *const u8, accept: *const u8) -> usize;
    pub fn strcspn(s: *const u8, reject: *const u8) -> usize;
    pub fn strdup(s: *const u8) -> *mut u8;
    pub fn strndup(s: *const u8, n: usize) -> *mut u8;
    pub fn strtok(s: *mut u8, delim: *const u8) -> *mut u8;
    pub fn strtok_r(s: *mut u8, delim: *const u8, saveptr: *mut *mut u8) -> *mut u8;
    pub fn strsep(stringp: *mut *mut u8, delim: *const u8) -> *mut u8;

    // string.h (memory) //
    pub fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8;
    pub fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    pub fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    pub fn memchr(s: *const u8, c: i32, n: usize) -> *const u8;
    pub fn memmem(h: *const u8, hlen: usize, n: *const u8, nlen: usize) -> *const u8;
    pub fn bzero(dst: *mut u8, n: usize);

    // stdlib.h //
    #[link_name = "malloc"]
    pub fn libc_malloc(size: usize) -> *mut u8;
    #[link_name = "free"]
    pub fn libc_free(ptr: *mut u8);
    #[link_name = "realloc"]
    pub fn libc_realloc(ptr: *mut u8, size: usize) -> *mut u8;
    #[link_name = "calloc"]
    pub fn libc_calloc(count: usize, size: usize) -> *mut u8;
    pub fn aligned_alloc(align: usize, size: usize) -> *mut u8;
    pub fn atoi(s: *const u8) -> i32;
    pub fn atol(s: *const u8) -> i64;
    pub fn strtol(s: *const u8, endptr: *mut *const u8, base: i32) -> i64;
    pub fn strtoul(s: *const u8, endptr: *mut *const u8, base: i32) -> u64;
    #[link_name = "abs"]
    pub fn libc_abs(x: i32) -> i32;
    pub fn labs(x: i64) -> i64;
    #[link_name = "exit"]
    pub fn libc_exit(code: i32) -> !;
    pub fn abort() -> !;
    pub fn getenv(key: *const u8) -> *const u8;
    pub fn setenv(key: *const u8, val: *const u8, overwrite: i32) -> i32;
    pub fn unsetenv(key: *const u8) -> i32;
    pub fn putenv(s: *const u8) -> i32;
    #[link_name = "srand"]
    pub fn libc_srand(seed: u32);
    #[link_name = "rand"]
    pub fn libc_rand() -> i32;
    pub fn qsort(
        base: *mut u8,
        nmemb: usize,
        size: usize,
        cmp: extern "C" fn(*const u8, *const u8) -> i32,
    );
    pub fn bsearch(
        key: *const u8,
        base: *const u8,
        nmemb: usize,
        size: usize,
        cmp: extern "C" fn(*const u8, *const u8) -> i32,
    ) -> *const u8;

    // stdio.h //
    pub fn printf(fmt: *const u8, ...) -> i32;
    pub fn snprintf(buf: *mut u8, max: usize, fmt: *const u8, ...) -> i32;
    pub fn sprintf(buf: *mut u8, fmt: *const u8, ...) -> i32;
    pub fn dprintf(fd: i32, fmt: *const u8, ...) -> i32;
    pub fn fopen(path: *const u8, mode: *const u8) -> *mut LibcFILE;
    pub fn fclose(f: *mut LibcFILE) -> i32;
    pub fn fread(ptr: *mut u8, size: usize, nmemb: usize, f: *mut LibcFILE) -> usize;
    pub fn fwrite(ptr: *const u8, size: usize, nmemb: usize, f: *mut LibcFILE) -> usize;
    pub fn fgetc(f: *mut LibcFILE) -> i32;
    pub fn fputc(c: i32, f: *mut LibcFILE) -> i32;
    pub fn fgets(buf: *mut u8, size: i32, f: *mut LibcFILE) -> *mut u8;
    pub fn fputs(s: *const u8, f: *mut LibcFILE) -> i32;
    pub fn puts(s: *const u8) -> i32;
    #[link_name = "putchar"]
    pub fn libc_putchar(c: i32) -> i32;
    #[link_name = "getchar"]
    pub fn libc_getchar() -> i32;
    pub fn fseek(f: *mut LibcFILE, offset: i64, whence: i32) -> i32;
    pub fn ftell(f: *mut LibcFILE) -> i64;
    pub fn rewind(f: *mut LibcFILE);
    pub fn feof(f: *mut LibcFILE) -> i32;
    pub fn ferror(f: *mut LibcFILE) -> i32;
    pub fn clearerr(f: *mut LibcFILE);
    pub fn fflush(f: *mut LibcFILE) -> i32;
    pub fn fileno(f: *mut LibcFILE) -> i32;
    pub fn fdopen(fd: i32, mode: *const u8) -> *mut LibcFILE;
    pub fn ungetc(c: i32, f: *mut LibcFILE) -> i32;
    pub fn setvbuf(f: *mut LibcFILE, buf: *mut u8, mode: i32, size: usize) -> i32;

    // ctype.h //
    pub fn isdigit(c: i32) -> i32;
    pub fn isalpha(c: i32) -> i32;
    pub fn isalnum(c: i32) -> i32;
    pub fn isspace(c: i32) -> i32;
    pub fn isupper(c: i32) -> i32;
    pub fn islower(c: i32) -> i32;
    pub fn isprint(c: i32) -> i32;
    pub fn ispunct(c: i32) -> i32;
    pub fn iscntrl(c: i32) -> i32;
    pub fn isxdigit(c: i32) -> i32;
    pub fn toupper(c: i32) -> i32;
    pub fn tolower(c: i32) -> i32;

    // unistd.h //
    #[link_name = "read"]
    pub fn libc_read(fd: i32, buf: *mut u8, count: usize) -> i64;
    #[link_name = "write"]
    pub fn libc_write(fd: i32, buf: *const u8, count: usize) -> i64;
    #[link_name = "close"]
    pub fn libc_close(fd: i32) -> i32;
    pub fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    #[link_name = "open"]
    pub fn libc_open(path: *const u8, flags: i32, mode: u32) -> i32;
    #[link_name = "dup"]
    pub fn libc_dup(fd: i32) -> i32;
    #[link_name = "dup2"]
    pub fn libc_dup2(old: i32, new: i32) -> i32;
    #[link_name = "pipe"]
    pub fn libc_pipe(fds: *mut i32) -> i32;
    #[link_name = "unlink"]
    pub fn libc_unlink(path: *const u8) -> i32;
    #[link_name = "rmdir"]
    pub fn libc_rmdir(path: *const u8) -> i32;
    #[link_name = "mkdir"]
    pub fn libc_mkdir(path: *const u8, mode: u32) -> i32;
    #[link_name = "link"]
    pub fn libc_link(old: *const u8, new: *const u8) -> i32;
    #[link_name = "symlink"]
    pub fn libc_symlink(target: *const u8, linkpath: *const u8) -> i32;
    #[link_name = "readlink"]
    pub fn libc_readlink(path: *const u8, buf: *mut u8, bufsiz: usize) -> i64;
    #[link_name = "rename"]
    pub fn libc_rename(old: *const u8, new: *const u8) -> i32;
    pub fn getcwd(buf: *mut u8, size: usize) -> *mut u8;
    pub fn chdir(path: *const u8) -> i32;
    #[link_name = "getpid"]
    pub fn libc_getpid() -> i32;
    #[link_name = "access"]
    pub fn libc_access(path: *const u8, mode: i32) -> i32;
    #[link_name = "sleep"]
    pub fn libc_sleep(seconds: u32) -> u32;
    pub fn usleep(usec: u32) -> i32;
    #[link_name = "ftruncate"]
    pub fn libc_ftruncate(fd: i32, length: i64) -> i32;
    pub fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> i64;
    pub fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> i64;
    pub fn sched_yield() -> i32;

    // sys/stat.h //
    pub fn stat_path(path: *const u8, st: *mut MikuStat) -> i32;
    // `fstat` is C's, in Linux's 144 bytes; `miku_fstat` is MikuOS's own
    #[link_name = "chmod"]
    pub fn libc_chmod(path: *const u8, mode: u32) -> i32;
    #[link_name = "chown"]
    pub fn libc_chown(path: *const u8, uid: u32, gid: u32) -> i32;

    // sys/mman.h //
    #[link_name = "mmap"]
    pub fn libc_mmap(
        addr: *mut u8,
        length: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut u8;
    #[link_name = "munmap"]
    pub fn libc_munmap(addr: *mut u8, length: usize) -> i32;
    #[link_name = "mprotect"]
    pub fn libc_mprotect(addr: *mut u8, length: usize, prot: i32) -> i32;
    pub fn sbrk(increment: i64) -> *mut u8;

    // signal.h //
    pub fn raise(sig: i32) -> i32;

    // time.h //
    pub fn nanosleep(req: *const LibcTimespec, rem: *mut LibcTimespec) -> i32;
    pub fn clock_gettime(clockid: i32, tp: *mut LibcTimespec) -> i32;

    // dirent.h //
    pub fn opendir(path: *const u8) -> *mut LibcDIR;
    pub fn readdir(d: *mut LibcDIR) -> *const MikuDirent;
    pub fn closedir(d: *mut LibcDIR) -> i32;
    pub fn rewinddir(d: *mut LibcDIR);

    // misc //
    pub fn remove(path: *const u8) -> i32;
    pub fn strerror(errnum: i64) -> *const u8;
    pub fn perror(s: *const u8);
    pub fn __errno_location() -> *mut i32;
}

#[macro_export]
macro_rules! cstr {
    ($s:expr) => {
        concat!($s, "\0").as_ptr()
    };
}

// SysV initial stack: [rsp]=argc, [rsp+8]=argv[0], ..., argv NULL, envp[0],
// ..., envp NULL, auxv. Passes argc/argv/envp to _start_main in rdi/rsi/rdx, so
// any of these signatures work:
//   fn _start_main() -> !
//   fn _start_main(argc: i32, argv: *const *const u8) -> !
//   fn _start_main(argc: i32, argv: *const *const u8, envp: *const *const u8) -> !
// envp goes to miku_env_init() first, since the environment is a table in
// `sys_miku.so`.
//
// ld-miku reads auxv and passes it to libmiku (`lib/libmiku/auxv.rs`).
//
// argc/argv/envp stay in the callee-saved rbx/r12/r13 across the call
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, [rsp]",
    "lea r12, [rsp + 8]",
    "lea r13, [r12 + rbx*8 + 8]",
    "and rsp, -16",
    "mov rdi, r13",
    "call miku_env_init",
    "mov rdi, rbx",
    "mov rsi, r12",
    "mov rdx, r13",
    "call _start_main",
    "ud2",
);

/// Safe wrappers for identity syscalls 47-55 and 83-91
pub mod cred {
    /// Largest id. 0xFFFF is chown's sentinel
    pub const ID_MAX: u16 = 0xFFFE;

    /// Leave this field unchanged, for the three-argument setters
    pub const KEEP: u64 = 0xFFFF;

    /// Maximum supplementary groups, as the kernel's MAX_GROUPS
    pub const MAX_GROUPS: usize = 16;

    // Capability numbers, as in kernel/process/cred.rs
    pub const CAP_SETUID: u8 = 0;
    pub const CAP_SETGID: u8 = 1;
    pub const CAP_KILL: u8 = 2;
    pub const CAP_DAC_OVERRIDE: u8 = 3;
    pub const CAP_DAC_READ: u8 = 4;
    pub const CAP_FOWNER: u8 = 5;
    pub const CAP_CHOWN: u8 = 6;
    pub const CAP_SYS_TIME: u8 = 7;
    pub const CAP_SYS_ADMIN: u8 = 8;
    pub const CAP_SYS_BOOT: u8 = 9;
    pub const CAP_NET_RAW: u8 = 10;
    pub const CAP_NET_BIND: u8 = 11;
    pub const CAP_MKNOD: u8 = 12;
    pub const CAP_MAY_PRIV: u8 = 13;
    pub const CAP_AUDIT: u8 = 14;
    pub const CAP_GPU: u8 = 15;

    #[inline]
    pub const fn cap_bit(cap: u8) -> u64 { 1u64 << cap }

    pub fn getuid() -> u16 { unsafe { super::miku_getuid() as u16 } }
    pub fn getgid() -> u16 { unsafe { super::miku_getgid() as u16 } }
    pub fn geteuid() -> u16 { unsafe { super::miku_geteuid() as u16 } }
    pub fn getegid() -> u16 { unsafe { super::miku_getegid() as u16 } }
    pub fn umask(mask: u16) -> u16 { unsafe { super::miku_umask(mask as u64) as u16 } }

    fn ok(r: i64) -> Result<(), i64> {
        if r < 0 { Err(r) } else { Ok(()) }
    }

    pub fn setuid(uid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setuid(uid as u64) }) }
    pub fn setgid(gid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setgid(gid as u64) }) }
    pub fn seteuid(uid: u16) -> Result<(), i64> { ok(unsafe { super::miku_seteuid(uid as u64) }) }
    pub fn setegid(gid: u16) -> Result<(), i64> { ok(unsafe { super::miku_setegid(gid as u64) }) }

    /// Sets all three user ids at once. `None` leaves a field. Preferred when
    /// dropping privilege for good
    pub fn setresuid(r: Option<u16>, e: Option<u16>, s: Option<u16>) -> Result<(), i64> {
        let f = |v: Option<u16>| v.map(|x| x as u64).unwrap_or(KEEP);
        ok(unsafe { super::miku_setresuid(f(r), f(e), f(s)) })
    }

    pub fn setresgid(r: Option<u16>, e: Option<u16>, s: Option<u16>) -> Result<(), i64> {
        let f = |v: Option<u16>| v.map(|x| x as u64).unwrap_or(KEEP);
        ok(unsafe { super::miku_setresgid(f(r), f(e), f(s)) })
    }

    pub fn setgroups(list: &[u16]) -> Result<(), i64> {
        ok(unsafe { super::miku_setgroups(list.as_ptr(), list.len() as u64) })
    }

    /// Writes supplementary groups to `out`. Returns the count or a negative
    /// errno
    pub fn getgroups(out: &mut [u16]) -> Result<usize, i64> {
        let r = unsafe { super::miku_getgroups(out.as_mut_ptr(), out.len() as u64) };
        if r < 0 { Err(r) } else { Ok(r as usize) }
    }

    /// (permitted, effective, bounding)
    pub fn capget() -> Result<(u64, u64, u64), i64> {
        let mut buf = [0u64; 3];
        let r = unsafe { super::miku_capget(buf.as_mut_ptr()) };
        if r < 0 { Err(r) } else { Ok((buf[0], buf[1], buf[2])) }
    }

    /// Narrows capabilities; each set must be a subset of what is held. The
    /// bounding set survives fork and exec
    pub fn capset(permitted: u64, effective: u64, bounding: u64) -> Result<(), i64> {
        ok(unsafe { super::miku_capset(permitted, effective, bounding) })
    }

    /// True when this process holds `cap` effectively
    pub fn has_cap(cap: u8) -> bool {
        matches!(capget(), Ok((_, eff, _)) if eff & cap_bit(cap) != 0)
    }

    /// Drops every capability not in `keep` and lowers the bounding set to
    /// match
    pub fn keep_only_caps(keep: u64) -> Result<(), i64> {
        let (perm, _, bound) = capget()?;
        let want = perm & keep;
        capset(want, want, bound & keep)
    }

    /// Peer of a connected local socket at connect time, written by the kernel
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PeerCred {
        pub pid: u64,
        /// Process start time. Compare with the pid, since pids are recycled
        pub start_tick: u64,
        pub uid: u16,
        pub gid: u16,
        pub euid: u16,
        pub egid: u16,
    }

    pub fn peercred(sockfd: i64) -> Result<PeerCred, i64> {
        let mut raw = [0u8; 40];
        let r = unsafe { super::miku_peercred(sockfd as u64, raw.as_mut_ptr()) };
        if r < 0 {
            return Err(r);
        }
        let u64_at = |o: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&raw[o..o + 8]);
            u64::from_le_bytes(b)
        };
        let u32_at = |o: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&raw[o..o + 4]);
            u32::from_le_bytes(b)
        };
        Ok(PeerCred {
            pid: u64_at(0),
            start_tick: u64_at(8),
            uid: u32_at(16) as u16,
            gid: u32_at(20) as u16,
            euid: u32_at(24) as u16,
            egid: u32_at(28) as u16,
        })
    }

    /// Hand one open descriptor to the peer of a local socket
    pub fn sendfd(sockfd: i64, fd: i64) -> Result<(), i64> {
        ok(unsafe { super::miku_sendfd(sockfd as u64, fd as u64) })
    }

    /// Takes the oldest descriptor sent. `Err(-11)` (EAGAIN) when none; never
    /// blocks
    pub fn recvfd(sockfd: i64) -> Result<i64, i64> {
        let r = unsafe { super::miku_recvfd(sockfd as u64) };
        if r < 0 { Err(r) } else { Ok(r) }
    }
}

/// Login sessions: identity, seat, and leader process. Opening and closing need
/// `CAP_SETUID`, so only `mikudod` does either. Listing needs nothing

/// Terminals: a character grid plus a line discipline. The kernel routes output
/// by controlling terminal, and the drawing program reads the grid over
/// `may_console`.
///
/// ```ignore
/// let tty = tty::open(80, 24)?;     // this process is the master
/// // may_console.bind(tty)         // and draws it
/// if proc::fork()? == 0 {
///     tty::attach(tty)?;            // the child's terminal, inherited by
///     proc::exec("/bin/msh", &[])?; //   everything it starts in turn
/// }
/// ```
///
/// Only the master and its descendants may join a terminal
pub mod tty {
    const OPEN:   u64 = 0;
    const ATTACH: u64 = 1;
    const CLOSE:  u64 = 2;
    const SELF:   u64 = 3;
    const SETFG:  u64 = 4;
    const GETFG:  u64 = 5;
    const CLEAR:  u64 = 6;
    const OF_PID: u64 = 7;

    /// The console
    pub const SYSTEM: u32 = 0x0100;

    /// Creates a terminal with `open("/dev/ptmx")`. Returns
    /// `(master_fd, tty_id)`. When the last copy of the descriptor closes, the
    /// terminal hangs up and its reader gets SIGHUP. The id names it to
    /// `may_console` and `attach`. Opened close-on-exec, so a child cannot hold
    /// it open
    pub fn open_pty() -> Result<(i64, u32), i64> {
        const O_RDWR: u32 = 0x0001 | 0x0002;
        const O_CLOEXEC: u32 = 0x0800;
        const TIOCGPTN: u64 = 0x8004_5430;
        let path = b"/dev/ptmx\0";
        let fd = unsafe { super::miku_open(path.as_ptr(), 9, O_RDWR | O_CLOEXEC, 0) };
        if fd < 0 { return Err(fd); }
        let mut id: u32 = 0;
        let r = unsafe { super::miku_ioctl(fd as u64, TIOCGPTN, &mut id as *mut u32 as u64) };
        if r < 0 {
            super::close(fd);
            return Err(r);
        }
        Ok((fd, id))
    }

    /// Joins a terminal; inherited across fork and kept across exec
    pub fn attach(id: u32) -> Result<(), i64> {
        let r = unsafe { super::miku_tty(ATTACH, id as u64) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    // A terminal hangs up when its master descriptor closes

    /// Which terminal this process is on. `0` is no terminal at all
    pub fn current() -> u32 {
        let r = unsafe { super::miku_tty(SELF, 0) };
        if r < 0 { 0 } else { r as u32 }
    }

    /// Gives this terminal's keys to a process group, or `0` to take them back
    /// (`tcsetpgrp`). Both must be on this terminal; use `proc::setpgid` first
    pub fn foreground(pgid: u64) -> Result<(), i64> {
        let r = unsafe { super::miku_tty(SETFG, pgid) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Blanks this terminal and homes the pen, atomically, unlike
    /// `\x1b[2J\x1b[H`
    pub fn clear() -> Result<(), i64> {
        let r = unsafe { super::miku_tty(CLEAR, 0) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Raw id of another process's terminal; 0 for none. Yours, a child's, or
    /// with admin rights anyone's. `mikudod` uses it with a `peercred` pid
    pub fn of_pid(pid: u64) -> Result<u32, i64> {
        let r = unsafe { super::miku_tty(OF_PID, pid) };
        if r < 0 { Err(r) } else { Ok(r as u32) }
    }

    /// Process group holding this terminal's keys (`tcgetpgrp`)
    pub fn foreground_of() -> Result<u64, i64> {
        let r = unsafe { super::miku_tty(GETFG, 0) };
        if r < 0 { Err(r) } else { Ok(r as u64) }
    }

    // Terminal modes

    const TCGETS: u64 = 0x5401;
    const TCSETS: u64 = 0x5402;

    /// The wire form, which `kernel/syscall/adapters/io.rs` defines
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Wire {
        icanon: u8,
        echo: u8,
        isig: u8,
        vmin: u8,
        vtime: u8,
        tostop: u8,
        c_cc: [u8; NCCS],
        _pad: [u8; 1],
    }

    /// Control character count and slots, mirroring `io::tty::ldisc`. Zero
    /// disables a character
    pub const NCCS: usize = 7;
    pub const VINTR: usize = 0;
    pub const VQUIT: usize = 1;
    pub const VERASE: usize = 2;
    pub const VKILL: usize = 3;
    pub const VEOF: usize = 4;
    pub const VWERASE: usize = 5;
    pub const VSUSP: usize = 6;

    /// Line discipline settings: `icanon` line or keystroke reads, `echo`
    /// echoes input, `isig` Ctrl-C as a signal or byte
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub struct Termios {
        pub icanon: bool,
        pub echo: bool,
        pub isig: bool,
        /// Control characters, indexed by `VINTR` and neighbors
        pub c_cc: [u8; NCCS],
        /// Raw-mode read minimum and timeout in tenths of a second; ignored in
        /// canonical mode
        pub vmin: u8,
        pub vtime: u8,
        /// POSIX `TOSTOP`: stops a background job writing to this terminal. Off
        /// by default
        pub tostop: bool,
    }

    /// Terminal settings, or `None` when stdin is not a terminal
    pub fn termios() -> Option<Termios> {
        let mut w = Wire::default();
        let r = unsafe { super::miku_ioctl(0, TCGETS, &mut w as *mut _ as u64) };
        if r < 0 { return None; }
        Some(Termios {
            icanon: w.icanon != 0,
            echo: w.echo != 0,
            isig: w.isig != 0,
            c_cc: w.c_cc,
            vmin: w.vmin,
            vtime: w.vtime,
            tostop: w.tostop != 0,
        })
    }

    pub fn set_termios(t: Termios) -> Result<(), i64> {
        let w = Wire {
            icanon: t.icanon as u8,
            echo: t.echo as u8,
            isig: t.isig as u8,
            vmin: t.vmin,
            vtime: t.vtime,
            tostop: t.tostop as u8,
            c_cc: t.c_cc,
            _pad: [0; 1],
        };
        let r = unsafe { super::miku_ioctl(0, TCSETS, &w as *const _ as u64) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Holds a changed terminal and restores it in `Drop`. Not run on `exit()`
    /// or panic; `tty::set_foreground` and `tty::hangup` reset the discipline
    /// for the next program
    pub struct Saved(Option<Termios>);

    impl Saved {
        /// Applies `f` to a copy of the settings, keeping the original. Does
        /// nothing when stdin is not a terminal
        pub fn change(f: impl FnOnce(&mut Termios)) -> Self {
            let Some(was) = termios() else { return Saved(None) };
            let mut want = was;
            f(&mut want);
            if want != was {
                let _ = set_termios(want);
            }
            Saved(Some(was))
        }

        /// True when there was a terminal to change
        pub fn is_tty(&self) -> bool { self.0.is_some() }

        /// Restores now, for a caller about to `exit`
        pub fn restore(&mut self) {
            if let Some(was) = self.0.take() {
                let _ = set_termios(was);
            }
        }
    }

    impl Drop for Saved {
        fn drop(&mut self) { self.restore(); }
    }
}

/// Process groups and signals. A shell puts each command in its own group and
/// gives it the terminal, so one Ctrl-C ends the job
pub mod proc {
    /// Leave this signal at whatever the system does about it
    pub const SIG_DEFAULT: u64 = 0;
    /// Ignores the signal, unlike no handler, where a fatal default kills
    pub const SIG_IGNORE:  u64 = 1;
    /// Deliver it to this process's handler
    pub const SIG_CATCH:   u64 = 2;

    /// Puts a process in a group. `pid` 0 is the caller, `pgid` 0 makes a new
    /// group led by the target. Called on both sides of a fork to close the
    /// window
    pub fn setpgid(pid: u64, pgid: u64) -> Result<(), i64> {
        let r = unsafe { super::miku_setpgid(pid, pgid) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Reaps an exited child without blocking (`WNOHANG`). `Some(pid)`, or
    /// `None` when nothing is waiting
    pub fn try_wait_any() -> Option<u64> {
        let mut status: i64 = 0;
        let r = unsafe { super::miku_try_wait(u64::MAX, &mut status as *mut i64) };
        if r > 0 { Some(r as u64) } else { None }
    }

    // Decoding a POSIX wait status

    /// Did it exit of its own accord?
    pub fn exited(status: i64) -> bool { (status & 0x7f) == 0 }

    /// Exit code; only when `exited`
    pub fn exit_code(status: i64) -> i64 { (status >> 8) & 0xff }

    /// Killed by a signal
    pub fn signalled(status: i64) -> bool {
        let low = status & 0x7f;
        low != 0 && low != 0x7f
    }

    /// Killing signal; only when `signalled`
    pub fn term_sig(status: i64) -> i64 { status & 0x7f }

    /// Stopped, not ended. Only reported with `WUNTRACED`; the child is alive
    /// and not reaped
    pub fn stopped(status: i64) -> bool { (status & 0xff) == 0x7f }

    /// Stopping signal; only when `stopped`
    pub fn stop_sig(status: i64) -> i64 { (status >> 8) & 0xff }

    /// Shell-style status: the code, or 128+signal
    pub fn wait_status_code(status: i64) -> i64 {
        if signalled(status) { 128 + term_sig(status) } else { exit_code(status) }
    }

    /// Waits for one child, optionally reporting a stop
    pub fn wait_job(pid: u64, untraced: bool) -> Option<(u64, i64)> {
        let mut status: i64 = 0;
        let opts = if untraced { 2 } else { 0 };
        let r = unsafe { super::miku_wait4(pid, &mut status as *mut i64, opts) };
        if r <= 0 { None } else { Some((r as u64, status)) }
    }

    /// Which job a process is in. `pid` 0 is the caller
    pub fn getpgid(pid: u64) -> u64 {
        let r = unsafe { super::miku_getpgid(pid) };
        if r < 0 { 0 } else { r as u64 }
    }

    /// Continues a stopped process
    pub const SIGCONT: u64 = 18;
    /// Suspends; sent by Ctrl-Z through the terminal
    pub const SIGTSTP: u64 = 20;
    /// Suspend, and this one cannot be caught or ignored
    pub const SIGSTOP: u64 = 19;

    /// Signals every process in a group, each permission-checked
    pub fn killpg(pgid: u64, sig: u64) -> Result<(), i64> {
        let r = unsafe { super::miku_killpg(pgid, sig) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// One process, as `procinfo` reports it
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ProcInfo {
        pub pid: u64,
        pub ppid: u64,
        pub pgid: u64,
        /// CPU ticks since start
        pub cpu_time: u64,
        pub uid: u16,
        /// R running, S sleeping, B blocked, D dead, Z zombie
        pub state: u8,
        pub priority: u8,
        /// Tenths of a percent over the last window
        pub cpu_pct_x10: u32,
        /// Terminal as a raw `TtyId`; 0 for none
        pub tty: u32,
        pub name: [u8; 24],
        _pad: [u8; 4],
    }

    impl ProcInfo {
        pub fn name(&self) -> &str {
            let end = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
            core::str::from_utf8(&self.name[..end]).unwrap_or("?")
        }
    }

    /// Every process, up to `out.len()`: pids, names, parents, jobs, CPU.
    /// Nothing about memory
    pub fn procinfo(out: &mut [ProcInfo]) -> usize {
        if out.is_empty() { return 0; }
        let n = unsafe {
            super::miku_procinfo(out.as_mut_ptr() as *mut u8, out.len() as u64)
        };
        if n < 0 { 0 } else { (n as usize).min(out.len()) }
    }

    /// Path `exec` was given, asked per pid. `None` for kernel threads
    pub fn path_of(pid: u64) -> Option<alloc::string::String> {
        let mut buf = [0u8; 256];
        let n = unsafe {
            super::miku_proc_path(pid, buf.as_mut_ptr(), buf.len() as u64)
        };
        if n <= 0 { return None; }
        let n = (n as usize).min(buf.len());
        core::str::from_utf8(&buf[..n]).ok().map(alloc::string::String::from)
    }

    /// Sets a signal's disposition, for ignoring one outright
    pub fn sigdisp(sig: u32, disp: u64) -> Result<(), i64> {
        let r = unsafe { super::miku_sigdisp(sig as u64, disp) };
        if r < 0 { Err(r) } else { Ok(()) }
    }
}

pub mod session {
    use super::cred::MAX_GROUPS;

    /// Open a session, having already proved the password
    pub const OPEN: u64 = 0;
    /// End one
    pub const CLOSE: u64 = 1;
    /// Copy out the open sessions
    pub const LIST: u64 = 2;
    /// Which session a process is in, or 0 for none
    pub const CURRENT: u64 = 3;

    /// The in-kernel shell gets the identity; nothing starts
    pub const CONSOLE: u8 = 0;
    /// Starts the /etc/passwd session program as the user, owning the seat
    pub const GRAPHICAL: u8 = 1;

    /// The only seat
    pub const SEAT0: u8 = 0;

    /// `SessionRequest` in kernel/syscall/adapters/process.rs, field for field
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Request {
        pub uid: u16,
        pub gid: u16,
        pub kind: u8,
        pub seat: u8,
        pub ngroups: u8,
        pub _pad: u8,
        pub groups: [u16; MAX_GROUPS],
    }

    /// One row of `list`. Must match `SessionInfo` in the kernel
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Info {
        pub id: u32,
        pub uid: u16,
        pub gid: u16,
        pub kind: u8,
        pub seat: u8,
        pub _pad: [u8; 2],
        pub leader: u64,
        pub opened_tick: u64,
    }

    /// Publishes a session for a user whose password was already checked.
    /// `leader` is the session process, 0 for console. Returns the id. Performs
    /// no authentication; requires `CAP_SETUID`
    pub fn open(uid: u16, gid: u16, groups: &[u16], kind: u8, seat: u8, leader: u64)
        -> Result<u32, i64>
    {
        let n = groups.len().min(MAX_GROUPS);
        let mut req = Request {
            uid, gid, kind, seat, ngroups: n as u8, _pad: 0,
            groups: [0; MAX_GROUPS],
        };
        req.groups[..n].copy_from_slice(&groups[..n]);
        let r = unsafe {
            super::miku_session(OPEN, leader,
                                &req as *const Request as u64,
                                core::mem::size_of::<Request>() as u64)
        };
        if r < 0 { Err(r) } else { Ok(r as u32) }
    }

    /// End a session. `0` means the caller's own
    pub fn close(id: u32) -> Result<(), i64> {
        let r = unsafe { super::miku_session(CLOSE, id as u64, 0, 0) };
        if r < 0 { Err(r) } else { Ok(()) }
    }

    /// Fill `out` with the open sessions, oldest first. Answers how many
    pub fn list(out: &mut [Info]) -> usize {
        let r = unsafe {
            super::miku_session(LIST, 0, out.as_mut_ptr() as u64, out.len() as u64)
        };
        if r < 0 { 0 } else { r as usize }
    }

    /// The caller's own session, or 0 if it leads none
    pub fn current() -> u32 {
        current_of(0)
    }

    /// Session of another process, or 0. Found by the kernel through its parent
    /// chain
    pub fn current_of(pid: u64) -> u32 {
        let r = unsafe { super::miku_session(CURRENT, pid, 0, 0) };
        if r < 0 { 0 } else { r as u32 }
    }
}


/// Power control, under `CAP_SYS_BOOT`, which sessions hold and `libmay::spawn`
/// drops for launched programs. mikuD stops services and syncs first. Nothing
/// returns on success
pub mod power {
    pub const OFF: u64 = 0;
    pub const REBOOT: u64 = 1;

    /// Stop the machine, or stop it and start it again
    pub fn request(action: u64) -> Result<(), i64> {
        let r = unsafe { super::miku_power(action) };
        if r < 0 { Err(r) } else { Ok(()) }
    }
}

/// Writes a line to the kernel log, adding the terminator if missing
pub fn klog(line: &str) {
    if line.ends_with('\n') {
        unsafe { miku_klog(line.as_ptr(), line.len()) };
        return;
    }
    // One call with the terminator, so log lines do not interleave
    const INLINE: usize = 256;
    if line.len() < INLINE {
        let mut buf = [0u8; INLINE];
        buf[..line.len()].copy_from_slice(line.as_bytes());
        buf[line.len()] = b'\n';
        unsafe { miku_klog(buf.as_ptr(), line.len() + 1) };
        return;
    }
    // Longer than the buffer: one allocation
    let mut owned = alloc::string::String::with_capacity(line.len() + 1);
    owned.push_str(line);
    owned.push('\n');
    unsafe { miku_klog(owned.as_ptr(), owned.len()) };
}

/// Reads a whole file as text, or None if unreadable or not UTF-8. The size is
/// a hint, since /proc files report zero
pub fn read_file_string(path: &str) -> Option<alloc::string::String> {
    let (ptr, len) = read_file(path)?;
    let bytes = unsafe { core::slice::from_raw_parts(ptr, len) };
    let out = core::str::from_utf8(bytes).ok().map(alloc::string::String::from);
    unsafe { miku_free(ptr) };
    out
}

/// Replaces a file's contents, truncating. True on success
pub fn write_file(path: &str, data: &[u8]) -> bool {
    const O_WRITE: u32 = 0x0002;
    const O_CREATE: u32 = 0x0008;
    const O_TRUNC: u32 = 0x0020;
    let mut p = alloc::vec::Vec::with_capacity(path.len() + 1);
    p.extend_from_slice(path.as_bytes());
    p.push(0);
    let fd = unsafe { miku_open(p.as_ptr(), path.len(), O_WRITE | O_CREATE | O_TRUNC, 0o600) };
    if fd < 0 {
        return false;
    }
    let mut off = 0usize;
    let mut ok = true;
    while off < data.len() {
        let n = unsafe { miku_write(fd as u64, data[off..].as_ptr(), data.len() - off) };
        if n <= 0 {
            ok = false;
            break;
        }
        off += n as usize;
    }
    unsafe { miku_close(fd) };
    ok
}
