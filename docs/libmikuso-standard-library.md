## libmiku.so (Standard Library)

libmiku is a C-compatible standard library for MikuOS userspace programs. Written in Rust, 63 modules, 956 exported functions covering I/O, data structures, cryptography, parsing, and a full POSIX libc compatibility layer (stdio, stdlib, string.h). Embedded in the kernel via `include_bytes!`, loaded dynamically by `ld-miku`.

### Module Categories

| Category | Modules |
|---|---|
| **Data Structures** | vec, list, hashmap, treemap, trie, queue, ringbuf, ringbuf2, heap_queue, bitset, channel |
| **Strings** | string, strbuf, ctype, utf8, format, regex, glob |
| **I/O** | io, bufio, stdio, file, dir, path |
| **Numbers / Math** | num, math, random, convert, endian, bitops |
| **Encoding** | base64, hex, json, csv, ini, lz |
| **Crypto / Hash** | sha256, checksum, hash, uuid |
| **System** | sys, proc, signal, env, errno, args, getopt |
| **Concurrency** | sync, channel, event, timer |
| **Time** | time, datetime |
| **Memory** | mem, heap, arena, pool, slab |
| **Logging / Testing** | log, test, panic |
| **Sorting** | sort |
| **libc compat** | libc (fopen/fclose/fread/fwrite/fprintf/fgets/fputs etc., 151 functions) |

> The previous `util` module was split into `math` (abs/min/max/clamp/isqrt/div_ceil/is_prime), `random` (srand/rand/rand_range/rand_bytes), and `panic` (assert_fail/panic/assert_eq/assert_not_null). Calls like `miku_abs` / `miku_rand_range` remain ABI-compatible.

### Core Function Reference

| Module | Key Functions |
|---|---|
| **io** | write, read, print, println, puts, putchar, getchar, readline, getline |
| **string** | strlen, strcmp, strncmp, strcpy, strncpy, strcat, strncat, strchr, strrchr, strstr, strdup, toupper, tolower, isdigit, isalpha, isalnum, isspace, strtok, strpbrk, strspn, strcspn, strtol, strtoul, strlcpy, strlcat |
| **num** | itoa, utoa, atoi, print_int, print_hex |
| **math** | abs, min, max, clamp, isqrt, div_ceil, is_prime |
| **random** | srand, rand, rand_range, rand_bytes |
| **mem** | memset, memcpy, memmove, memcmp, bzero, memchr, memrchr, memmem |
| **heap** | malloc, free, realloc, calloc |
| **fmt** | printf, snprintf (supports %s %d %u %x %c %p %%) |
| **file** | open, open_cstr, close, seek, fsize, read_file |
| **time** | sleep (~4 ms/tick), sleep_ms, uptime, uptime_ms |
| **proc** | exit, getpid, getcwd, brk, mmap, munmap, mprotect, set_tls, get_tls, map_lib |
| **panic** | assert_fail, panic, assert_eq, assert_not_null |
| **libc** | fopen, fclose, fread, fwrite, fprintf, fgets, fputs, ... (151 fns) |

### Heap Implementation

mmap-based slab allocator. Allocations under 32 KB are carved from a 128 KB slab. Allocations of 32 KB or larger get a dedicated mmap region, returned to the kernel via `munmap` on free.

### printf Implementation

`global_asm!` trampolines save `rsi`/`rdx`/`rcx`/`r8`/`r9` to a stack array, then call Rust `_impl` functions. No XMM registers used, no SSE alignment issues. `%d/%x/%u` are 32-bit (read as `i32`/`u32`).

---