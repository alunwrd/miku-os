# MikuOS ABI v0.2.4-rc

Application Binary Interface for MikuOS userspace.

---

## 1. Overview

MikuOS is an x86_64 OS. Userspace programs run in Ring 3 and communicate with the kernel via `syscall` (entries 0..59; socket syscalls occupy 56..59). The standard library **libmiku** links dynamically through **ld-miku**.

```
+----------------------------------+
|        Program (ELF)             |
|  _start -> _start_main -> code  |
+----------------------------------+
|    libmiku.so  (956 functions)  |
|  63 modules: string/ mem/ heap/ |
|  io/ fmt/ file/ libc/ json/...  |
+----------------------------------+
|     ld-miku.so  (linker)        |
|  loads .so, PLT, relocations    |
+----------------------------------+
|     MikuOS Kernel               |
|  syscall nr=0..59, mikuD init   |
+----------------------------------+
```

---

## 2. Environment

### 2.1 Requirements

```bash
# Rust nightly + rust-src
rustup toolchain install nightly
rustup component add rust-src --toolchain nightly

# GCC (for stub generation)
You need to install GCC for your OS

# e2tools (copying to ext4)
You need to install e2tools for your OS
```

### 2.2 Kernel environment

New hardware subsystems initialized before userspace starts:

| Subsystem | Details |
|:--|:--|
| **ACPI** | RSDP/RSDT/XSDT parser, MADT enumeration, LAPIC and IOAPIC discovery |
| **APIC** | Local APIC + I/O APIC driver replaces PIC8259 |
| **SMP** | AP trampoline, per-CPU state (percpu), SIPI sequence for multi-core bring-up |
| **PS/2** | Keyboard controller initialization |
| **USB** | USB legacy handoff (EHCI/xHCI BIOS release) |
| **Splash** | Boot splash via framebuffer before shell starts |
| **NVIDIA** | GSP-era GPU driver: GTX 1650/1660 full pipeline + generic host-side bring-up for any Turing/Ampere/Ada GPU |
| **fwload** | On-demand firmware loader from `/lib/firmware` (Linux `request_firmware` model) |
| **netd** | mikuD service: auto-DHCP at MultiUser target, runs as a background thread, does not block boot |

### 2.3 SDK Structure

```
src/lib/userspace/
├── Cargo.toml              crate configuration
├── build.rs                auto-generates stub libmiku.so
├── build.sh                build + deploy script
├── x86_64-miku-app.json    target spec
└── src/
    ├── miku.rs             SDK: extern bindings + safe wrappers
    ├── hello.rs            example program
    └── test_full.rs        test suite
```

### 2.4 libmiku Structure

63 modules, 956 exported functions. Modules:

| Category | Modules |
|---|---|
| **Core** | lib, sys, proc, io, mem, num, string, heap, file, time, fmt |
| **Data structures** | vec, list, hashmap, treemap, trie, queue, ringbuf, ringbuf2, heap_queue, bitset, channel |
| **Strings** | strbuf, ctype, utf8, format, regex, glob |
| **I/O** | bufio, stdio, dir, path |
| **Encoding** | base64, hex, json, csv, ini, lz |
| **Crypto / hash** | sha256, checksum, hash, uuid |
| **System** | signal, env, errno, args, getopt |
| **Concurrency** | sync, event, timer |
| **Time** | datetime |
| **Memory** | arena, slab, pool |
| **Math / RNG / sort** | math, random, convert, endian, bitops, sort |
| **Diagnostics** | log, test, panic |
| **libc compat** | libc (fopen/fclose/fread/fwrite/fprintf/fgets/fputs etc., 151 functions) |

> The old `util` module has been split: `math` owns `miku_abs` / `miku_min` / `miku_max` / `miku_clamp` / `miku_swap` / `miku_isqrt` / `miku_div_ceil` / `miku_is_prime`; `random` owns `miku_srand` / `miku_rand` / `miku_rand_range` / `miku_rand_bytes`; `panic` owns `miku_assert_fail` / `miku_panic` / `miku_assert_eq` / `miku_assert_not_null`. Symbol names are unchanged, so existing binaries keep working.

---

## 3. Syscall ABI

### 3.1 Calling Convention

```
Instruction:  syscall
Number:       rax
Arguments:    rdi, rsi, rdx, r10
Return:       rax (negative = errno)
Clobbered:    rcx, r11
```

### 3.2 Syscall table

| Nr | Name | rdi | rsi | rdx | r10 | Return |
|---|---|---|---|---|---|---|
| 0 | exit | code | | | | never |
| 1 | write | fd | buf | len | | bytes / -errno |
| 2 | read | fd | buf | len | | bytes / -errno |
| 3 | mmap | addr | len | prot | flags | addr / -errno |
| 4 | munmap | addr | len | | | 0 / -errno |
| 5 | mprotect | addr | len | prot | | 0 / -errno |
| 6 | brk | addr | | | | new_brk |
| 7 | getpid | | | | | pid |
| 8 | getcwd | buf | size | | | ptr / -errno |
| 9 | set_tls | addr | | | | 0 |
| 10 | get_tls | | | | | addr |
| 11 | open | path | len | | | fd / -errno |
| 12 | close | fd | | | | 0 / -errno |
| 13 | seek | fd | offset | whence | | new_offset / -errno |
| 14 | fsize | fd | | | | size / -errno |
| 15 | map_lib | name | len | | | base / -errno |
| 16 | sleep | ticks | | | | 0 |
| 17 | uptime | | | | | ticks |
| 18 | stat | path | path_len | buf | | 0 / -errno |
| 19 | fstat | fd | buf | | | 0 / -errno |
| 20 | mkdir | path | path_len | mode | | 0 / -errno |
| 21 | rmdir | path | path_len | | | 0 / -errno |
| 22 | unlink | path | path_len | | | 0 / -errno |
| 23 | readdir | path | path_len | buf | max_entries | entries / -errno |
| 24 | rename | old_path | old_len | new_path | new_len | 0 / -errno |
| 25 | link | target | target_len | link_path | link_len | 0 / -errno |
| 26 | chmod | path | path_len | mode | | 0 / -errno |
| 27 | chown | path | path_len | uid | gid | 0 / -errno |
| 28 | dup | fd | | | | new_fd / -errno |
| 29 | dup2 | old_fd | new_fd | | | new_fd / -errno |
| 30 | truncate | fd | size | | | 0 / -errno |
| 31 | write_file | fd | buf | len | | bytes / -errno |
| 32 | symlink | target | target_len | link_path | link_len | 0 / -errno |
| 33 | readlink | path | path_len | buf | buf_len | len / -errno |
| 34 | pipe | fds_ptr | | | | 0 / -errno |
| 35 | chdir | path | path_len | | | 0 / -errno |
| 36 | statfs | path | path_len | buf | | 0 / -errno |
| 37 | fallocate | fd | offset | len | | 0 / -errno |
| 38 | getxattr | ino | name | name_len | buf | len / -errno (ext-only) |
| 39 | setxattr | ino | name | value | (name_len<<16)\|value_len | 0 / -errno (ext-only) |
| 40 | utimensat | ino | atime | mtime | | 0 / -errno (ext-only) |
| 41 | fsync | fd | | | | 0 / -errno |
| 42 | punch_hole | fd | offset | len | | 0 / -errno |
| 43 | fork | | | | | child_pid / 0 / -errno |
| 44 | wait4 | pid | status_ptr | options | | pid / -errno |
| 45 | kill | pid | sig | | | 0 / -errno |
| 46 | exec | path | path_len | argv | argc | never (success) / -errno |
| 47 | umask | mask | | | | previous_mask |
| 48 | getuid | | | | | uid |
| 49 | getgid | | | | | gid |
| 50 | geteuid | | | | | euid |
| 51 | getegid | | | | | egid |
| 52 | setuid | uid | | | | 0 / -EPERM |
| 53 | setgid | gid | | | | 0 / -EPERM |
| 54 | seteuid | euid | | | | 0 / -EPERM |
| 55 | setegid | egid | | | | 0 / -EPERM |
| 56 | socket | domain | type | protocol | | fd / -errno |
| 57 | connect | fd | sockaddr* | addrlen | | 0 / -errno |
| 58 | send | fd | buf | len | flags | n / -errno |
| 59 | recv | fd | buf | len | flags | n (0=EOF) / -errno |

Socket fds are returned in a dedicated range (`SOCK_FD_BASE = 4096`..) and are
also usable with `read`/`write`/`close` (which route to recv/send/close by fd
range). Phase 1 is AF_INET / SOCK_STREAM (TCP client) with blocking semantics;
`sockaddr` is the 16-byte `sockaddr_in` (family LE u16, port BE u16, addr[4]).

The kernel socket layer (`net/socket.rs`) maintains a per-process table of up to
64 sockets. A blocking recv/connect times out after 7500 ticks (~30 s) so a dead
peer cannot wedge a process forever. Sockets are freed automatically when the
owning process exits (`close_all_for_pid`). The `sys_close` handler routes any
fd >= `SOCK_FD_BASE` directly to the socket layer and validates pid ownership
before releasing it.

### 3.3 Constants

```
PROT_READ  = 1
PROT_WRITE = 2
PROT_EXEC  = 4

O_READ      = 0x0001
O_WRITE     = 0x0002
O_APPEND    = 0x0004
O_CREATE    = 0x0008
O_EXCLUSIVE = 0x0010
O_TRUNC     = 0x0020
O_DIRECTORY = 0x0040
O_NOFOLLOW  = 0x0080
O_DIRECT    = 0x0100
O_SYNC      = 0x0200
O_NONBLOCK  = 0x0400
O_CLOEXEC   = 0x0800     // closed by exec(); preserved otherwise
O_NOATIME   = 0x1000

EPERM        = -1
ENOENT       = -2     (file not found)
ESRCH        = -3     (no such process)
EIO          = -5     (I/O error)
EBADF        = -9     (bad file descriptor)
EAGAIN       = -11    (try again / resource temporarily unavailable)
ENOMEM       = -12    (out of memory)
EACCES       = -13    (permission denied)
EFAULT       = -14    (bad address)
EBUSY        = -16    (device or resource busy)
EEXIST       = -17    (file exists)
ENOTDIR      = -20    (not a directory)
EISDIR       = -21    (is a directory)
EINVAL       = -22    (invalid argument)
EMFILE       = -24    (too many open files)
ECHILD       = -10    (no child processes)
ENOSPC       = -28    (no space left)
EPIPE        = -32    (broken pipe)
ENAMETOOLONG = -36    (file name too long)
ENOSYS       = -38    (syscall does not exist)
ENOTEMPTY    = -39    (directory not empty)
EPROTONOSUPPORT = -93  (protocol not supported)
EAFNOSUPPORT    = -97  (address family not supported)
ECONNRESET      = -104 (connection reset by peer)
EISCONN         = -106 (socket is already connected)
ENOTCONN        = -107 (socket is not connected)
ECONNREFUSED    = -111 (connection refused)

SIGKILL = 9
SIGTERM = 15
SIGCHLD = 17

Timer: Local APIC timer at 250 Hz (1 tick ~= 4 ms). PIT is used only for
       LAPIC calibration; runtime ticks come from LAPIC. See
       `apic::TIMER_HZ_DEFAULT` in src/apic.rs.
```

### 3.4 File Descriptors

| fd | Purpose |
|---|---|
| 0 | stdin (keyboard) |
| 1 | stdout (screen) |
| 2 | stderr (screen) |
| 3+ | open files |

---

## 4. ELF Format

### 4.1 Binary requirements

- Format: ELF64, ET_EXEC
- `.interp` points to `/lib/ld-miku.so`
- `NEEDED: libmiku.so`
- Entry point: `_start`
- No PIE (fixed addresses)
- No red zone (`-mno-red-zone`)

### 4.2 Loading sequence

1. Kernel reads ELF, maps segments
2. Loads `ld-miku.so` from `.interp`
3. `ld-miku` loads `libmiku.so` from the kernel via `map_lib`
4. `ld-miku` resolves PLT/GOT
5. Jumps to `_start` in the program

### 4.3 Address space layout

```
0x0000_0000_0040_0000 .. 0x0000_0000_0080_0000  PIE program (code + data, ASLR-shifted)
0x0000_0000_4100_0000                            TLS area
0x0000_0000_6000_0000_0000                       brk arena base
0x0000_0001_0000_0000 .. 0x0000_7F00_0000_0000  mmap / libmiku / heap
0x0000_7F00_0000_0000                            ld-miku interpreter load base
0x0000_7FFF_FFEF_0000 .. 0x0000_7FFF_FFFF_0000  user stack (1 MiB, 256 pages)
```

Exact constants live in `src/elf_loader.rs` (`PIE_BASE`, `TLS_VIRT`,
`INTERP_BASE`, `USER_STACK_TOP`, `STACK_PAGES`) and `src/mmap.rs`
(`MMAP_BASE`, `MMAP_LIMIT`, `BRK_BASE`).

---

## 5. libmiku API

### 5.1 Module `io`: input / output

```c
long miku_write(unsigned long fd, const char *buf, unsigned long len);
long miku_read(unsigned long fd, void *buf, unsigned long len);
void miku_print(const char *s);                    // no newline
void miku_println(const char *s);                  // with newline
int  miku_puts(const char *s);                     // = println
int  miku_putchar(int c);                          // single byte
int  miku_getchar(void);                           // -1 on EOF
int  miku_readline(char *buf, unsigned long max);  // reads until \n
char *miku_getline(void);                          // malloc, caller must free
```

### 5.2 Module `string`: strings

```c
// Basic
unsigned long miku_strlen(const char *s);
int  miku_strcmp(const char *a, const char *b);
int  miku_strncmp(const char *a, const char *b, unsigned long n);
char *miku_strcpy(char *dst, const char *src);
char *miku_strncpy(char *dst, const char *src, unsigned long n);
char *miku_strcat(char *dst, const char *src);
char *miku_strncat(char *dst, const char *src, unsigned long n);
const char *miku_strchr(const char *s, int c);
const char *miku_strrchr(const char *s, int c);
const char *miku_strstr(const char *haystack, const char *needle);
char *miku_strdup(const char *s);                  // malloc, caller must free

// Classification
int miku_isdigit(int c);    // '0'..'9'
int miku_isalpha(int c);    // a-z, A-Z
int miku_isalnum(int c);    // letter or digit
int miku_isspace(int c);    // space / tab / \n
int miku_toupper(int c);    // 'a' -> 'A'
int miku_tolower(int c);    // 'A' -> 'a'

// Tokenization
char *miku_strtok(char *s, const char *delim);
const char *miku_strpbrk(const char *s, const char *accept);
unsigned long miku_strspn(const char *s, const char *accept);
unsigned long miku_strcspn(const char *s, const char *reject);

// Numeric parsing
long miku_strtol(const char *s, const char **endptr, int base);
unsigned long miku_strtoul(const char *s, const char **endptr, int base);

// BSD-safe
unsigned long miku_strlcpy(char *dst, const char *src, unsigned long size);
unsigned long miku_strlcat(char *dst, const char *src, unsigned long size);

// Extended
char *miku_strndup(const char *s, unsigned long n);      // malloc, caller must free
unsigned long miku_strnlen(const char *s, unsigned long maxlen);
int  miku_strcasecmp(const char *a, const char *b);
int  miku_strncasecmp(const char *a, const char *b, unsigned long n);
char *miku_strsep(char **stringp, const char *delim);    // BSD-style tokenization
char *miku_strtok_r(char *s, const char *delim, char **saveptr); // thread-safe strtok
```

### 5.3 Module `num`: numbers

```c
void miku_itoa(long val, char *buf);           // int -> string (buf >= 21 bytes)
void miku_utoa(unsigned long val, char *buf);  // uint -> string
long miku_atoi(const char *s);                 // string -> int
void miku_print_int(long val);                 // print decimal
void miku_print_hex(unsigned long val);        // print 0x...
```

### 5.4 Module `mem`: memory

```c
void *miku_memset(void *dst, int val, unsigned long n);
void *miku_memcpy(void *dst, const void *src, unsigned long n);
void *miku_memmove(void *dst, const void *src, unsigned long n);  // overlap-safe
int   miku_memcmp(const void *a, const void *b, unsigned long n);
void  miku_bzero(void *dst, unsigned long n);
const void *miku_memchr(const void *s, int c, unsigned long n);
const void *miku_memrchr(const void *s, int c, unsigned long n);  // reverse search
const void *miku_memmem(const void *haystack, unsigned long hlen,
                        const void *needle, unsigned long nlen);
```

### 5.5 Module `heap`: dynamic memory

```c
void *miku_malloc(unsigned long size);
void  miku_free(void *ptr);
void *miku_realloc(void *ptr, unsigned long new_size);
void *miku_calloc(unsigned long count, unsigned long size);
```

Implementation: mmap-based slab (128 KB) for allocations under 32 KB. Dedicated `mmap` + `munmap` per allocation for 32 KB and above.

### 5.6 Module `fmt`: formatted output

```c
int miku_printf(const char *fmt, ...);
int miku_snprintf(char *buf, unsigned long max, const char *fmt, ...);
```

| Format | C type | Width | Description |
|---|---|---|---|
| `%s` | `const char *` | 64-bit | C string |
| `%d` | `int` | 32-bit | Signed integer |
| `%u` | `unsigned int` | 32-bit | Unsigned integer |
| `%x` | `unsigned int` | 32-bit | Hex lowercase |
| `%c` | `int` | 32-bit | Character |
| `%p` | `void *` | 64-bit | Pointer, 0x + 16 digits |
| `%%` | | | Literal percent sign |

Limitations: up to 5 arguments. `%d/%x/%u` are 32-bit. For 64-bit values use `miku_print_int` / `miku_print_hex`.

Implementation: `global_asm!` trampoline saves `rsi`/`rdx`/`rcx`/`r8`/`r9` onto the stack and passes them as an array to the Rust `_impl`. No XMM registers used, no SSE alignment issues.

### 5.7 Module `file`: file I/O

```c
long miku_open(const char *path, unsigned long path_len, int flags, int mode);
long miku_open_cstr(const char *path);                    // computes len internally, O_READ
long miku_close(long fd);
long miku_seek(long fd, unsigned long offset);
long miku_fsize(long fd);
void *miku_read_file(const char *path, unsigned long *out_size);  // malloc
```

Flags: O_READ=0, O_WRITE=1, O_CREATE=2, O_TRUNC=4, O_APPEND=8.

### 5.8 Module `time`: time

```c
void miku_sleep(unsigned long ticks);      // ~4 ms per tick (250 Hz LAPIC)
void miku_sleep_ms(unsigned long ms);
unsigned long miku_uptime(void);           // ticks since boot
unsigned long miku_uptime_ms(void);
```

### 5.9 Module `proc`: process

```c
void miku_exit(long code);                  // noreturn
unsigned long miku_getpid(void);
char *miku_getcwd(char *buf, unsigned long size);
unsigned long miku_brk(unsigned long addr); // 0 = query current break
void *miku_mmap(unsigned long addr, unsigned long len, unsigned long prot);
long  miku_munmap(void *addr, unsigned long len);
long  miku_mprotect(unsigned long addr, unsigned long len, unsigned long prot);
long  miku_set_tls(unsigned long addr);
unsigned long miku_get_tls(void);
long  miku_map_lib(const char *name, unsigned long name_len);
```

### 5.10 Modules `math`, `random`, `panic`

The former `util` module was split in three. All symbols kept their original names.

#### `math`: arithmetic helpers (saturating / overflow-safe)

```c
long  miku_abs(long x);                            // saturating_abs, safe on INT64_MIN
long  miku_min(long a, long b);
long  miku_max(long a, long b);
long  miku_clamp(long val, long lo, long hi);
void  miku_swap(unsigned long *a, unsigned long *b);
unsigned long miku_isqrt(unsigned long n);         // bit-length-seeded Newton, safe on UINT64_MAX
unsigned long miku_div_ceil(unsigned long a, unsigned long b);
int   miku_is_prime(unsigned long n);              // trial division, uses miku_isqrt
```

#### `random`: PRNG

```c
void  miku_srand(unsigned long seed);                              // xorshift64
unsigned long miku_rand(void);
unsigned long miku_rand_range(unsigned long lo, unsigned long hi); // [lo, hi)
unsigned int  miku_rand_u32(void);
void  miku_rand_bytes(unsigned char *buf, unsigned long len);
int   miku_rand_bool(void);
unsigned long miku_rand_uniform(unsigned long bound);              // [0, bound) bias-free
long  miku_rand_i64(long lo, long hi);
unsigned int  miku_rand_dice(unsigned int sides);
unsigned long miku_rand_sample(unsigned long n, unsigned long k, unsigned long *out);
unsigned long miku_rand_weighted(const unsigned long *weights, unsigned long n);
void  miku_rand_perm(unsigned long n, unsigned long *out);
void  miku_rand_shuffle(unsigned char *data, unsigned long count, unsigned long elem_size);
```

Note: `random` is a userspace PRNG (xorshift). The kernel TLS / ECDH paths use RDRAND-backed CSPRNG internally; that is not exposed via libmiku.

#### `panic`: assertions and aborts

```c
void miku_assert_fail(const char *expr, const char *file, int line);  // noreturn
void miku_panic(const char *msg);                                     // noreturn
void miku_assert_eq(long a, long b, const char *file, int line);      // noreturn on mismatch
void miku_assert_not_null(const void *ptr, const char *name,
                          const char *file, int line);                // noreturn on NULL
```

### 5.11 Module `libc`: POSIX libc compatibility

151 functions providing C standard library compatibility:

```c
// stdio
FILE *fopen(const char *path, const char *mode);   // modes: "r","w","a","r+","w+","a+"
int   fclose(FILE *f);
unsigned long fread(void *buf, unsigned long size, unsigned long count, FILE *f);
unsigned long fwrite(const void *buf, unsigned long size, unsigned long count, FILE *f);
int   fputc(int c, FILE *f);
int   fgetc(FILE *f);
int   fputs(const char *s, FILE *f);
char *fgets(char *buf, int size, FILE *f);
int   fprintf(FILE *f, const char *fmt, ...);
int   fseek(FILE *f, long offset, int whence);
long  ftell(FILE *f);
void  rewind(FILE *f);
int   fflush(FILE *f);
int   feof(FILE *f);
int   ferror(FILE *f);

// stdlib
int   atoi(const char *s);
long  atol(const char *s);
long  strtol(const char *s, char **endptr, int base);
void *malloc(unsigned long size);
void  free(void *ptr);
void *realloc(void *ptr, unsigned long size);
void *calloc(unsigned long count, unsigned long size);
void  exit(int status);
void  abort(void);
int   abs(int x);
int   rand(void);
void  srand(unsigned int seed);

// string.h
void *memcpy(void *dst, const void *src, unsigned long n);
void *memset(void *dst, int c, unsigned long n);
void *memmove(void *dst, const void *src, unsigned long n);
int   memcmp(const void *a, const void *b, unsigned long n);
unsigned long strlen(const char *s);
int   strcmp(const char *a, const char *b);
char *strcpy(char *dst, const char *src);
char *strcat(char *dst, const char *src);
char *strchr(const char *s, int c);
char *strstr(const char *haystack, const char *needle);
char *strdup(const char *s);
// ... and more
```

---

## 6. Programming in Rust

### 6.1 Minimal Program

```rust
#![no_std]
#![no_main]

#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello MikuOS!");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

### 6.2 Required Elements

| Element | Purpose |
|---|---|
| `#![no_std]` | No std (using libmiku instead) |
| `#![no_main]` | Entry point is not `main` |
| `mod miku` | SDK bindings |
| `fn _start_main() -> !` | Entry point (never returns) |
| `#[panic_handler]` | Panic handler |

The entry point is `_start_main`, not `_start`, because `miku.rs` contains an asm trampoline `_start` that moves `[rsp]` → `rdi` (argc), `lea rsi, [rsp+8]` (argv), runs `and rsp, -16` for SSE alignment, and then calls `_start_main`. Programs that don't care about arguments declare `_start_main() -> !`; programs that do declare `_start_main(argc: i32, argv: *const *const u8) -> !`.

### 6.3 Safe wrappers (miku.rs)

```rust
miku::print("text");
miku::println("text");
miku::print_int(-42);
miku::print_hex(0xFF);
miku::putchar(b'A');
miku::exit(0);
miku::sleep_ms(1000);
miku::getpid();
miku::uptime_ms();
miku::srand(miku::uptime());
miku::rand_range(1, 100);
miku::abs(-5);
miku::min(a, b);
miku::max(a, b);
miku::clamp(val, 0, 100);
```

### 6.4 Unsafe operations

```rust
unsafe {
    // printf via C ABI
    miku::miku_printf(cstr!("num=%d\n"), 42u64);

    // malloc / free
    let p = miku::malloc(256);
    miku::free(p);

    // files
    let fd = miku::miku_open_cstr(cstr!("/myfile"));
}

// Safe file wrapper
match miku::open("/myfile") {
    Ok(fd) => { /* ... */ miku::close(fd); }
    Err(_) => { /* not found */ }
}
```

### 6.5 The cstr! Macro

```rust
cstr!("hello")  // -> "hello\0".as_ptr()
```

Required for C strings passed to `miku_printf`, `miku_open_cstr`, etc.

### 6.6 Registering a binary

In `Cargo.toml`:

```toml
[[bin]]
name = "my_app"
path = "src/my_app.rs"
```

---

## 7. Programming in C

### 7.1 Minimal Program

```c
extern void miku_println(const char *s);
extern void miku_exit(long code) __attribute__((noreturn));

void _start(void) {
    miku_println("Hello from C!");
    miku_exit(0);
}
```

### 7.2 Compilation

```bash
gcc -nostdlib -nostdinc -fno-builtin -fno-stack-protector \
    -fno-pie -no-pie -ffreestanding -mno-red-zone \
    -c app.c -o app.o
```

### 7.3 Linking

```bash
# Generate stub (one time only):
gcc -shared -nostdlib -fPIC -Wl,-soname,libmiku.so -o libmiku.so miku_stub.c

# Link:
ld app.o -o app \
    --dynamic-linker=/lib/ld-miku.so \
    libmiku.so --no-as-needed -e _start
```

### 7.4 ASSERT Macro

```c
#define ASSERT(x) do { \
    if (!(x)) miku_assert_fail(#x, __FILE__, __LINE__); \
} while(0)
```

---

## 8. Build and Deploy

### 8.1 Rust (recommended)

```bash
cd ~/miku-os/src/lib/userspace

# Build everything:
./build.sh

# Single binary:
./build.sh my_app

# Manual build:
cargo +nightly build --release \
    --target x86_64-miku-app.json \
    -Z json-target-spec \
    -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem \
    --bin my_app

e2cp target/x86_64-miku-app/release/my_app ~/miku-os/miku-os/data.img:/
```

### 8.2 C

```bash
gcc [flags] -c app.c -o app.o
ld app.o -o app [link flags]
e2cp app ~/miku-os/miku-os/data.img:/
```

### 8.3 Disk operations

```bash
# Copy binary:
e2cp binary ~/miku-os/miku-os/data.img:/

# List files:
e2ls ~/miku-os/miku-os/data.img:/

# Remove file:
e2rm ~/miku-os/miku-os/data.img:/binary
```

### 8.4 Running

```
miku@os:/ $ ext4mount 3
miku@os:/ $ ls
miku@os:/ $ exec my_app
```

---

## 9. Rebuilding the Kernel

When libmiku or ld-miku changes:

```bash
cd ~/miku-os/libmiku && cargo clean
cd ~/miku-os/builder && cargo run
```

Userspace binaries do **not** need to be rebuilt; dynamic linking handles it.

---

## 10. Examples

### 10.1 Random Guessing Game

```rust
#![no_std]
#![no_main]
#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::srand(miku::uptime());
    let secret = miku::rand_range(1, 101) as i64;
    miku::println("Guess 1-100:");
    loop {
        miku::print("> ");
        let mut buf = [0u8; 16];
        let n = unsafe { miku::miku_readline(buf.as_mut_ptr(), 16) };
        if n <= 0 { break; }
        let guess = unsafe { miku::miku_atoi(buf.as_ptr()) };
        if guess < secret { miku::println("Low!"); }
        else if guess > secret { miku::println("High!"); }
        else { miku::println("Correct!"); break; }
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

### 10.2 File Reader

```rust
#![no_std]
#![no_main]
#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    if let Some((ptr, size)) = miku::read_file("/hello") {
        miku::print("Read ");
        miku::print_int(size as i64);
        miku::println(" bytes");
        let data = unsafe { core::slice::from_raw_parts(ptr, size) };
        miku::write(1, data);
        unsafe { miku::free(ptr); }
    } else {
        miku::println("File not found");
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

### 10.3 Countdown Timer

```rust
#![no_std]
#![no_main]
#[path = "miku.rs"]
mod miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    for i in (1..=5).rev() {
        miku::print_int(i);
        miku::println("...");
        miku::sleep_ms(1000);
    }
    miku::println("Go!");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
```

---

## 11. Debugging

### 11.1 Verifying a Binary

```bash
readelf -l app | grep INTERP        # should show /lib/ld-miku.so
readelf -d app | grep NEEDED        # should show libmiku.so
readelf --dyn-syms app | grep miku  # should list miku_* symbols
```

### 11.2 Troubleshooting table

| Symptom | Cause | Fix |
|---|---|---|
| `page fault addr=0x0 INSTRUCTION_FETCH` | Missing `.interp` or unresolved symbols | Link against `libmiku.so` stub |
| `interp=false` in boot log | `--unresolved-symbols` produced a static binary | Use stub |
| `not found: libmiku_stub.so` | Wrong soname on stub | Add `-Wl,-soname,libmiku.so` |
| GPF `code=0` in libmiku | SSE movaps alignment fault | Set `opt-level = 1` in libmiku `Cargo.toml` |
| GPF on 3rd+ exec | Shared pages freed prematurely | Apply solib fix: copy pages |
| `[swap] slot=0` spam | `is_swap_pte` false positive | Add `slot != 0` check |
| Files disappear | ext4 64-bit feature enabled | Format with `mkfs.ext4 -O ^64bit,^metadata_csum` |
| printf shows garbage for `-99` | 32/64-bit mismatch | `%d` is 32-bit; use `print_int` for 64-bit values |
| ~~VMA table full~~ | _historical_ | VMAs now use `BTreeMap`; no fixed cap |

---

## 12. Limitations

- `printf`: max 5 arguments; `%d`/`%x` are 32-bit only
- Single thread per process (no `clone`, no thread syscalls)
- Cooperative scheduling: preemption from the timer ISR is currently
  disabled; threads must call `miku_sleep`/`yield`/any blocking syscall
  for the scheduler to run another task. See the long-form comment on
  `timer_interrupt_handler` in `src/interrupts.rs`
- No errno variable; errors returned as negative values (libc compat layer provides errno)
- No float support in printf
- Heap slab does not return memory to the kernel when small blocks are freed
- Networking from userspace: AF_INET / SOCK_STREAM (TCP client) via syscalls 56-59
  (`socket`/`connect`/`send`/`recv`). Socket fds start at `SOCK_FD_BASE=4096` and
  route through the generic `read`/`write`/`close`. No UDP, listen/accept, or
  non-blocking mode yet. Max 64 sockets per system; connect/recv block with a 30 s
  hard timeout. The `libmiku/net.rs` module exposes safe Rust wrappers for the
  socket syscalls.
- No `fcntl`, `ioctl`, `poll`, `select`, or `epoll`; `pipe` works but cannot be multiplexed
- NVIDIA GPU driver does not yet expose a userspace API; accessible only via shell commands (nvidia info/debug/falcon/dma-test/gsp etc.)
- Firmware blobs (NVIDIA GSP, etc.) are loaded on-demand from `/lib/firmware` via
  the `fwload` module (Linux-style `request_firmware`). They are freed after use
  and are never pinned for the full uptime.

### 12.1 Hard limits

| Limit | Value | Source |
|---|---|---|
| Max open FDs per process | 128 | `src/vfs/types.rs` `MAX_FDS` |
| Max `exec` argv entries | 64 | `src/elf_loader.rs` `MAX_ARGS` |
| Max single argv string length | 4096 bytes | `src/syscall/user_mem.rs` `MAX_ARG_BYTES` |
| Max path length | 4096 bytes | `src/syscall/user_mem.rs` `MAX_PATH_LEN` |
| Max ELF file size | 64 MiB | `src/elf_loader.rs` `MAX_ELF_SIZE` |
| `sleep(ticks)` upper bound | 100_000 ticks (~400 s) | `src/syscall/process.rs` clamp |
| User stack | 256 pages = 1 MiB | `src/elf_loader.rs` `STACK_PAGES` |

### 12.2 Signal handling

`kill(pid, sig)`:

| sig | Behaviour |
|---|---|
| 0 | Probe: returns 0 if `pid` exists, `-ESRCH` otherwise |
| 9 (SIGKILL) / 15 (SIGTERM) | Hard terminate, send SIGCHLD to parent |
| other | Routed through `signal::send_signal`; default action depends on signal number |

### 12.3 xattr / utimensat are ino-based

`getxattr`, `setxattr` and `utimensat` (syscalls 38-40) currently take an
ext2/ext4 inode number rather than a path, and only operate on the
active ext filesystem (return `-ENOSYS` otherwise). They have no
libmiku wrapper yet. Path-based versions will replace these once the
VFS gains a unified ino-by-path lookup.

### 12.4 `map_lib` syscall

`map_lib` (nr 15) resolves a shared object by name. Lookup order:

1. In-kernel solib cache (includes the preloaded `libmiku.so`).
2. Filesystem search paths configured via `solib::add_search_path`
   (default: `/lib`). Reads through the VFS, parses the ELF, maps
   segments into the caller's address space, returns the load base.

Other shared libraries must be reachable via one of those search paths.

---

## 13. Checklist

### New Rust program

1. Create `src/my_app.rs` with `_start_main`, `panic_handler`, and `mod miku`
2. Add `[[bin]] name = "my_app"` to `Cargo.toml`
3. Run `./build.sh my_app`
4. In MikuOS: `ext4mount 3` then `exec my_app`

### New C program

1. Write `app.c` with `_start` and extern declarations
2. Compile: `gcc ... -c app.c -o app.o`
3. Link: `ld app.o -o app ... libmiku.so ...`
4. Deploy: `e2cp app data.img:/`
5. In MikuOS: `ext4mount 3` then `exec app`
