#![allow(dead_code)]

pub type CmpFn = unsafe extern "C" fn(*const u8, *const u8) -> i32;

#[link(name = "miku")]
extern "C" {
    // io
    pub fn miku_write(fd: u64, buf: *const u8, len: usize) -> i64;
    pub fn miku_read(fd: u64, buf: *mut u8, len: usize) -> i64;

    // net (TCP client sockets); fd is also usable with miku_read/write/close
    pub fn miku_socket() -> i64;
    pub fn miku_connect(fd: i64, ip: *const u8, port: u16) -> i64;
    pub fn miku_send(fd: i64, buf: *const u8, len: usize) -> i64;
    pub fn miku_recv(fd: i64, buf: *mut u8, len: usize) -> i64;

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
    pub fn miku_mkdir(path: *const u8, mode: u32) -> i64;
    pub fn miku_rmdir(path: *const u8) -> i64;
    pub fn miku_unlink(path: *const u8) -> i64;
    pub fn miku_readdir(path: *const u8, entries: *mut MikuDirent, max: usize) -> i64;
    pub fn miku_rename(old: *const u8, new: *const u8) -> i64;
    pub fn miku_link(old: *const u8, new: *const u8) -> i64;
    pub fn miku_symlink(target: *const u8, linkpath: *const u8) -> i64;
    pub fn miku_readlink(path: *const u8, buf: *mut u8, buf_len: usize) -> i64;
    pub fn miku_chmod(path: *const u8, mode: u32) -> i64;
    pub fn miku_chown(path: *const u8, uid: u32, gid: u32) -> i64;
    pub fn miku_dup(fd: i64) -> i64;
    pub fn miku_dup2(old_fd: i64, new_fd: i64) -> i64;
    pub fn miku_pipe(fds: *mut i64) -> i64;
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
    pub fn miku_uptime() -> u64;
    pub fn miku_uptime_ms() -> u64;
    pub fn miku_yield();

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

#[repr(C)]
pub struct MikuDirent {
    pub name: [u8; 64],
    pub inode_id: u16,
    pub kind: u8,
    pub name_len: u8,
    pub _reserved: u32,
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

#[repr(C)]
pub struct MikuTreeMap {
    pub root: *mut u8,
    pub count: usize,
}

pub fn exit(code: i64) -> ! {
    unsafe { miku_exit(code) }
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

fn write_dec_to_stdout(mut val: i64) {
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let neg = val < 0;
    let mut n = if neg {
        val.wrapping_neg() as u64
    } else {
        val as u64
    };
    if n == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        if neg {
            i -= 1;
            buf[i] = b'-';
        }
    }
    unsafe {
        miku_write(1, buf.as_ptr().add(i), buf.len() - i);
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
    if ptr.is_null() || size == 0 {
        None
    } else {
        Some((ptr, size))
    }
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

// dir
#[repr(C)]
pub struct MikuDir {
    pub path: [u8; 256],
    pub path_len: usize,
    _entries: [u8; 1152], // DIR_BATCH * sizeof(MikuDirent)
    pub total: usize,
    pub cursor: usize,
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

    // sync - mutex
    pub fn miku_mutex_init(m: *mut MikuMutex);
    pub fn miku_mutex_lock(m: *mut MikuMutex);
    pub fn miku_mutex_unlock(m: *mut MikuMutex);
    pub fn miku_mutex_trylock(m: *mut MikuMutex) -> bool;
    pub fn miku_mutex_is_locked(m: *const MikuMutex) -> bool;

    // sync - atomic
    pub fn miku_atomic_init(a: *mut MikuAtomic, val: i64);
    pub fn miku_atomic_load(a: *const MikuAtomic) -> i64;
    pub fn miku_atomic_store(a: *mut MikuAtomic, val: i64);
    pub fn miku_atomic_add(a: *mut MikuAtomic, val: i64) -> i64;
    pub fn miku_atomic_sub(a: *mut MikuAtomic, val: i64) -> i64;
    pub fn miku_atomic_cas(a: *mut MikuAtomic, expected: i64, desired: i64) -> bool;
    pub fn miku_atomic_swap(a: *mut MikuAtomic, val: i64) -> i64;

    // sync - once
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

#[repr(C)]
pub struct MikuMutex {
    pub locked: u8, // AtomicBool is 1 byte
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
    pub fn fstat(fd: i32, st: *mut MikuStat) -> i32;
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
    pub fn sigaction(sig: i32, act: *const MikuSigaction, oldact: *mut MikuSigaction) -> i32;

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
    pub fn __errno_location() -> *mut i64;
}

#[macro_export]
macro_rules! cstr {
    ($s:expr) => {
        concat!($s, "\0").as_ptr()
    };
}

// SysV-style initial stack layout: [rsp]=argc, [rsp+8]=argv[0], ...
// Pass argc/argv to _start_main via rdi/rsi so 'extern "C" fn
// _start_main(argc: i32, argv: *const *const u8)'' works, while
// argless _start_main signatures (the original convention) keep
// compiling; extra register args are harmless for a callee that
// doesn't read them
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rdi, [rsp]",
    "lea rsi, [rsp + 8]",
    "and rsp, -16",
    "call _start_main",
    "ud2",
);
