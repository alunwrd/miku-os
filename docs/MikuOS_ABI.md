# MikuOS ABI v1.0.2

The system call interface of the MikuOS kernel: how a ring-3 program enters the
kernel, what every system call number from 0 to 123 (124 numbers, all
assigned) takes and answers, the structures
and constants that cross the boundary, and what the kernel puts in front of a
new program (its stack, auxiliary vector, thread block and vDSO). It is written
from the code in `kernel/syscall/` and its adapters, and every entry names the
wrappers that `libmiku` (the `miku_*` functions in `core_miku.so` and its
siblings) and `libc_miku.so` export for it. How to write and build a program on
top of these is in [wiki/sdk.md](wiki/sdk.md); processes, signals and threads
as a subsystem are in [wiki/processes.md](wiki/processes.md).

> **v0.2.11 changed the permission model.** Syscalls 47-55 gained userspace
> bindings, 83-91 are new, and `exec` does not honour setuid or setgid mode
> bits. See [§4.9](#49-identity-and-capabilities-47-55-86-91) and
> [MikuOS_Security.md](MikuOS_Security.md). Everything added after v0.2.10 is
> listed in [§8](#8-changes-since-v0210).

## Contents

1. [Overview](#1-overview)
2. [Conventions](#2-conventions)
   - [2.1 Entering the kernel](#21-entering-the-kernel)
   - [2.2 Return values and errno](#22-return-values-and-errno)
   - [2.3 User pointers](#23-user-pointers)
   - [2.4 Paths and the path word](#24-paths-and-the-path-word)
   - [2.5 Descriptors](#25-descriptors)
   - [2.6 Blocking, signals and EINTR](#26-blocking-signals-and-eintr)
   - [2.7 Capabilities](#27-capabilities)
   - [2.8 Limits](#28-limits)
3. [Index of all system calls](#3-index-of-all-system-calls)
4. [System call reference](#4-system-call-reference)
   - [4.1 Process lifetime](#41-process-lifetime)
   - [4.2 Memory](#42-memory)
   - [4.3 Reading, writing and waiting](#43-reading-writing-and-waiting)
   - [4.4 Descriptors and open files](#44-descriptors-and-open-files)
   - [4.5 The file namespace](#45-the-file-namespace)
   - [4.6 Extended attributes (38, 39, 81, 82)](#46-extended-attributes-38-39-81-82)
   - [4.7 Watching files: inotify (116-118)](#47-watching-files-inotify-116-118)
   - [4.8 Time and entropy](#48-time-and-entropy)
   - [4.9 Identity and capabilities (47-55, 86-91)](#49-identity-and-capabilities-47-55-86-91)
   - [4.10 Sessions and power (92, 93)](#410-sessions-and-power-92-93)
   - [4.11 Signals (68, 69, 100, 115)](#411-signals-68-69-100-115)
   - [4.12 Threads and the futex (106-110)](#412-threads-and-the-futex-106-110)
   - [4.13 Terminals and jobs (99, 101, 102)](#413-terminals-and-jobs-99-101-102)
   - [4.14 Sockets](#414-sockets)
   - [4.15 Display pools (72-74)](#415-display-pools-72-74)
   - [4.16 The GPU (94-98, 121)](#416-the-gpu-94-98-121)
   - [4.17 The administrative channel (104)](#417-the-administrative-channel-104)
5. [Structures](#5-structures)
6. [Constants, ioctls and fcntl commands](#6-constants-ioctls-and-fcntl-commands)
7. [Starting a program](#7-starting-a-program)
   - [7.1 Binary requirements](#71-binary-requirements)
   - [7.2 Loading sequence](#72-loading-sequence)
   - [7.3 Address space layout](#73-address-space-layout)
   - [7.4 Initial stack](#74-initial-stack)
   - [7.5 Auxiliary vector](#75-auxiliary-vector)
   - [7.6 The thread control block](#76-the-thread-control-block)
   - [7.7 The vDSO](#77-the-vdso)
   - [7.8 Shared image pages](#78-shared-image-pages)
   - [7.9 When a process dies](#79-when-a-process-dies)
8. [Changes since v0.2.10](#8-changes-since-v0210)
9. [Programs and libraries](#9-programs-and-libraries)
10. [Debugging](#10-debugging)
11. [Limitations](#11-limitations)
12. [Where the code is](#12-where-the-code-is)

---

## 1. Overview

Every number and layout in this document is defined once, in
[`lib/abi`](../lib/abi/src/lib.rs): the kernel, the libraries, `std`, `libc` and
the C headers all take it from there, so they cannot disagree. A compile-time
assertion there pins each structure's size.

MikuOS is an x86-64 kernel. Programs run in ring 3 and reach the kernel with
the `syscall` instruction. The numbers are MikuOS's own, not Linux's: a call
with Linux's name usually has Linux's semantics, but the number, and often the
argument shape, differ (four argument registers at most, paths passed as a
pointer and a length). Programs normally do not make the instruction
themselves; they call the shared libraries:

```
+-----------------------------------------------+
|  program (ELF, PIE or ET_EXEC)                |
|  _start -> _start_main / main                 |
+-----------------------------------------------+
|  libc_miku.so   POSIX names, errno, pthreads  |
|  core_miku.so  sys_miku.so  fs_miku.so  ...   |
|  (the miku libraries, miku_* functions)       |
|  Rust std: lib/sdk/rust-std over rust-libc        |
+-----------------------------------------------+
|  ld-miku.so    loads DT_NEEDED, relocates     |
+-----------------------------------------------+
|  vDSO page     clock_gettime, getcpu          |
+-----------------------------------------------+
|  MikuOS kernel: syscall 0..123                |
+-----------------------------------------------+
```

Areas of the number space, with the section that documents each:

| Numbers | Area | Section |
|---|---|---|
| 0, 7, 43-46, 67, 103, 105 | process lifetime | [4.1](#41-process-lifetime) |
| 3-6, 9, 10, 15, 60, 61 | memory, TLS, shared objects | [4.2](#42-memory) |
| 1, 2, 31, 71, 75, 76, 123 | read, write, ioctl, log, poll | [4.3](#43-reading-writing-and-waiting) |
| 11-14, 28-30, 34, 37, 41, 42, 113, 120 | descriptors | [4.4](#44-descriptors-and-open-files) |
| 8, 18-27, 32, 33, 35, 36, 40, 111, 112, 119 | file namespace | [4.5](#45-the-file-namespace) |
| 38, 39, 81, 82 | extended attributes | [4.6](#46-extended-attributes-38-39-81-82) |
| 116-118 | inotify | [4.7](#47-watching-files-inotify-116-118) |
| 16, 17, 70, 78, 79, 122 | time and entropy | [4.8](#48-time-and-entropy) |
| 47-55, 86-91 | identity and capabilities | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 92, 93 | sessions, power | [4.10](#410-sessions-and-power-92-93) |
| 68, 69, 100, 115 | signals | [4.11](#411-signals-68-69-100-115) |
| 106-110 | threads, futex | [4.12](#412-threads-and-the-futex-106-110) |
| 99, 101, 102 | terminals, job control | [4.13](#413-terminals-and-jobs-99-101-102) |
| 56-59, 62-66, 77, 80, 83-85, 114 | sockets | [4.14](#414-sockets) |
| 72-74 | display server pools | [4.15](#415-display-pools-72-74) |
| 94-98, 121 | GPU | [4.16](#416-the-gpu-94-98-121) |
| 104 | kernel commands | [4.17](#417-the-administrative-channel-104) |

---

## 2. Conventions

### 2.1 Entering the kernel

```
Instruction:  syscall
Number:       rax
Arguments:    rdi, rsi, rdx, r10      (at most four)
Return:       rax
Clobbered:    rcx, r11                 (syscall itself overwrites them)
Preserved:    everything else, including rdi, rsi, rdx, r8, r9, r10
```

This is Linux's rule, and the dynamic loader, the vDSO and `lib/sdk/miku` are
written to it. The entry stub (`syscall_handler` in `kernel/syscall/mod.rs`)
saves `rcx`, `r11`, `rbp`, `rbx`, `r12`-`r15`, `r10`, `r9`, `r8`, `rdi`, `rsi`
and `rdx` on the task's kernel stack and restores them before `sysretq`.
Exceptions:

- **`exec`** (46, 67): on success the new image starts at its entry point with
  `rdi`, `rsi` and `rdx` zero and `rsp` at the new stack ([§7.4](#74-initial-stack)).
- **`sigreturn`** (69) is diverted before anything is saved and leaves through
  `iretq`, restoring every register from the signal frame, `rcx` and `r11`
  included ([§4.11](#411-signals-68-69-100-115)).
- A **signal handler** may run between the call and its return: the kernel
  rewrites the return address at the end of the call, and the handler's
  `sigreturn` puts back the call's result and every register.

What the CPU does on entry: `SFMASK` clears **IF**, **AC** and **DF**. IF off
means the handler starts with interrupts disabled; AC off keeps SMAP enforced
however the program left the flag; DF off means a program that set the
direction flag (`std`) cannot make the kernel's `rep movs` run backwards. The
kernel's own naked entry points from ring 3 also execute `cld` (see
[wiki/processes.md](wiki/processes.md#syscall-entry)).

More than four arguments travel in a block of `u64`s in user memory, named by
one pointer: `mmap_file` (60), `execve` (67), `gpu_submit` (95),
`gpu_program` (98) and `thread_create` (106). `setxattr` (39) packs two
lengths into one register instead.

Every call returns through `dispatch` in `kernel/syscall/mod.rs`, which does
two things after the handler: a task whose process was killed while it was in
the call stops there (`sched::stop_if_dead`), and pending signals are
delivered (`signal::deliver_pending`). That tail is the only point at which a
signal sent by another process reaches a handler.

An unknown number logs `[syscall] unknown nr=N` on the serial line and
returns `-ENOSYS`.

### 2.2 Return values and errno

A call returns a non-negative value on success and a **negative errno** on
failure, in `rax`, read as a signed 64-bit integer. The kernel never writes a
global `errno`. The `miku_*` wrappers in `libmiku` pass the negative value
through unchanged; the POSIX functions in `libc_miku.so` return `-1` and store
the positive code in the calling thread's `errno`, which lives in the thread
control block at `%fs:0 + 40` ([§7.6](#76-the-thread-control-block)).

Two calls return values that could otherwise collide with the error range and
are shaped to avoid it: a GPU fence keeps bit 63 clear ([§4.16](#416-the-gpu-94-98-121)),
and `brk` (6) never fails with an errno at all (it returns the current break).

The errno values the kernel uses (`kernel/syscall/errno.rs`), all Linux's
numbers:

| errno | value | errno | value | errno | value |
|---|---|---|---|---|---|
| `EPERM` | 1 | `ENOTTY` | 25 | `ENOPROTOOPT` | 92 |
| `ENOENT` | 2 | `EFBIG` | 27 | `EPROTONOSUPPORT` | 93 |
| `ESRCH` | 3 | `ENOSPC` | 28 | `EOPNOTSUPP` | 95 |
| `EINTR` | 4 | `EROFS` | 30 | `EAFNOSUPPORT` | 97 |
| `EIO` | 5 | `EMLINK` | 31 | `EADDRINUSE` | 98 |
| `ENXIO` | 6 | `EPIPE` | 32 | `ECONNRESET` | 104 |
| `ENOEXEC` | 8 | `ERANGE` | 34 | `EISCONN` | 106 |
| `EBADF` | 9 | `ENAMETOOLONG` | 36 | `ENOTCONN` | 107 |
| `ECHILD` | 10 | `ENOSYS` | 38 | `ETIMEDOUT` | 110 |
| `EAGAIN` | 11 | `ENOTEMPTY` | 39 | `ECONNREFUSED` | 111 |
| `ENOMEM` | 12 | `ELOOP` | 40 | `EALREADY` | 114 |
| `EACCES` | 13 | | | `EINPROGRESS` | 115 |
| `EFAULT` | 14 | | | `ESTALE` | 116 |
| `EBUSY` | 16 | | | `EUCLEAN` | 117 |
| `EEXIST` | 17 | | | `EDQUOT` | 122 |
| `EXDEV` | 18 | | | | |
| `ENODEV` | 19 | | | | |
| `ENOTDIR` | 20 | | | | |
| `EISDIR` | 21 | | | | |
| `EINVAL` | 22 | | | | |
| `EMFILE` | 24 | | | | |

Filesystem calls translate a VFS error with one table (`errno::vfs_err`). The
mapping a caller can rely on:

| VFS error | errno | VFS error | errno |
|---|---|---|---|
| NotFound | `ENOENT` | NotSupported | **`ENOSYS`** |
| PermissionDenied | `EACCES` | TooManyLinks | `EMLINK` |
| AlreadyExists | `EEXIST` | OutOfMemory | `ENOMEM` |
| NotDirectory | `ENOTDIR` | Loop, TooManySymlinks | `ELOOP` |
| IsDirectory | `EISDIR` | FileTooLarge | `EFBIG` |
| NotEmpty | `ENOTEMPTY` | Corrupted | `EUCLEAN` |
| InvalidPath, InvalidArgument | `EINVAL` | XattrTooLarge | `ERANGE` |
| NoSpace | `ENOSPC` | NotMounted | `ENODEV` |
| ReadOnly | `EROFS` | NoTerminal | `ENXIO` |
| BadFd | `EBADF` | Deferred, WouldBlock | `EAGAIN` |
| TooManyOpenFiles | `EMFILE` | CrossDevice | `EXDEV` |
| NameTooLong | `ENAMETOOLONG` | Busy | `EBUSY` |
| BrokenPipe | `EPIPE` | Stale | `ESTALE` |
| IoError | `EIO` | Interrupted | `EINTR` |

Anything else becomes `EINVAL`. Note that an operation a filesystem does not
implement answers `ENOSYS`, not `EOPNOTSUPP`.

Socket calls translate with their own table (`net::socket::sock_errno`), the
same one `SO_ERROR` reports through: BadFd `EBADF`, NotConnected `ENOTCONN`,
AlreadyConnected `EISCONN`, Busy `EBUSY`, ConnRefused `ECONNREFUSED`,
BrokenPipe `EPIPE`, ConnReset `ECONNRESET`, AddrInUse `EADDRINUSE`,
WouldBlock `EAGAIN`, Invalid `EINVAL`, NotSupported `EOPNOTSUPP`, TableFull
`EMFILE`, InProgress `EINPROGRESS`, Already `EALREADY`, TimedOut `ETIMEDOUT`.

### 2.3 User pointers

Every pointer a program passes is checked before the kernel touches it
(`kernel/syscall/usercopy.rs`):

- the whole range `[ptr, ptr+len)` must lie in the canonical user half
  (`ptr <= USER_MAX`, no wrap-around), and a null pointer or a zero length is
  refused;
- every page in the range must be **present and user-accessible**; a page of a
  lazily populated mapping (the stack, an anonymous `mmap`, a file mapping) is
  faulted in first, so an untouched buffer is not an `EFAULT`;
- a range the kernel will **write** must be writable or copy-on-write; COW
  pages are broken eagerly before the write;
- the access happens inside a SMAP window (`UserAccess`) that the check hands
  back and that closes when it is dropped.

A failed check is `-EFAULT`. A call whose length is zero usually returns 0
before any pointer check (`read`, `write`, `send`, `recv`, `getrandom`, `klog`,
`proc_path`, `procinfo`, `getdents`).

**Strings are copied, not borrowed.** A path is copied into kernel memory
before it is parsed, so a program cannot change it between the check and the
use. A path must be valid UTF-8 (`-EINVAL` otherwise), at most 4096 bytes
(`-EINVAL` above that, and for a zero length without a descriptor base);
trailing NUL bytes inside the given length are ignored, so passing
`strlen(p) + 1` works as well as `strlen(p)`.

**Ids are range-checked, never truncated.** A user or group id above `0xFFFE`
(`ID_MAX`) is `-EINVAL` everywhere it crosses the boundary; `0xFFFF` is the
"leave unchanged" value where a call has one.

### 2.4 Paths and the path word

A path argument is two words: `ptr` and a **path word**. The low 32 bits of
the word are the length of the string. The high 32 bits say where a relative
path starts:

| high half | relative paths start at |
|---|---|
| 0 | the calling process's working directory |
| `n` (n >= 1) | the directory open on descriptor `n - 1` |

```c
uint64_t word = strlen(path) | ((uint64_t)(dirfd + 1) << 32);   /* dirfd == AT_FDCWD: high half 0 */
```

An absolute path ignores the base, as POSIX says. A base descriptor that is
not open is `-EBADF`; one that is not a directory is `-ENOTDIR`.

**An empty path with a base names the descriptor's own file** (Linux's
`AT_EMPTY_PATH`), and that file need not be a directory. This is how
`fchmod`, `fchown`, `futimens`, `fstatvfs` and `F_GETPATH` are built. An empty
path without a base is `-EINVAL`.

The calls that read a path word: `open` (11), `stat` (18), `mkdir` (20),
`rmdir` (21), `unlink` (22), `readdir` (23), `rename` (24) and `link` (25)
(each of their two paths has its own word), `chmod` (26), `chown` (27),
`symlink` (32, the link's path; the target is stored as text), `readlink` (33),
`chdir` (35), `statfs` (36), `utimensat` (40), `lstat` (112),
`inotify_add_watch` (117) and `realpath` (119). There are therefore no
separate `*at` numbers: `openat`, `fstatat`, `mkdirat`, `unlinkat`, `renameat`,
`linkat`, `symlinkat`, `readlinkat`, `fchmodat`, `fchownat` and `utimensat` in
`libc_miku` are the plain calls with a base (`lib/runtime/libmiku/libc/at.rs`);
`fchdir(fd)` is `chdir` of the empty path from `fd`.

`exec` (46), `execve` (67), the xattr calls and `map_lib` (15) do **not** read
a path word: their path is relative to the working directory, or not a path at
all.

The kernel keeps no name for an open file, only the vnode, so a directory
renamed while a descriptor is open on it stays a valid base; `realpath` (119)
on the descriptor answers its current name.

### 2.5 Descriptors

Two ranges, routed by number:

| Range | What | Scope | Limit |
|---|---|---|---|
| 0 .. 511 | VFS descriptors: files, directories, pipes, device nodes, pty masters and slaves, inotify instances | per process (thread group) | `MAX_OPEN_FILES` = 512 |
| 4096 .. 4351 | sockets: TCP, UDP, ICMP, AF_UNIX, display-server connections | one table for the whole machine | `MAX_SOCKETS` = 256, at most 160 per account, at most 4 display connections per process |

`read`, `write`, `close`, `poll`, `dup` and `fcntl` work on both ranges and
route by number; `dup2` works on the VFS range only.

**Descriptors 0, 1 and 2.** A real descriptor installed at 0, 1 or 2 (a
redirect, a pipe from a shell, a pty slave put there with `dup2` or
`login_tty`) is used like any other. When nothing is installed there, the
syscall layer routes 0 to the caller's controlling terminal's input and 1 and
2 to its output; such an unredirected 0-2 is not a table entry, so `close` on
it succeeds and does nothing, and `sendfd` of it fails with `-EBADF`. The
routing rule is one function, `classify` in `kernel/syscall/adapters/io.rs`,
used by `read`, `write`, `ioctl` and `poll` alike.

Descriptors belong to the process: every thread sees the same table, `fork`
copies it, `exec` closes those marked close-on-exec (only after the new image
has loaded, so a failed `exec` leaves them all open), and exit closes all of
them. Sockets are owned by the thread group too; a socket call from another
process is `-EBADF`.

### 2.6 Blocking, signals and EINTR

A signal sent by another process is delivered on the way out of a system
call, never in the middle of one, and a blocking call is woken by it only
where the call checks for it. The calls that end early with **`-EINTR`** when
a signal the process will act on is pending:

| Call | When |
|---|---|
| `wait4` (44) | while it would block |
| `write` (1) to a full pipe | before any byte was written (otherwise the short count) |
| `read` (2) of an inotify descriptor | while no event is queued |
| `fcntl` (113) `F_SETLKW` | while waiting for a conflicting lock |
| `ioctl` (71) `TCSETS` from a background job | when the job is stopped for it (`SIGTTOU`) |

Everything else that blocks (a pipe read, a terminal read, `poll`, `futex`,
`recv`, `accept`, `connect`, `sleep`, `sleep_ns`) runs to its own end - data,
timeout or hangup - and the handler runs when it returns. A fatal signal ends
the process wherever it is: the task stops at the end of the call and never
returns to ring 3.

There is **no `SA_RESTART`**. `libc_miku`'s `sigaction` accepts the flag and
does not honour it; a call that returned `-EINTR` is not restarted, and every
caller of `wait4` in `std` retries by itself. Every long kernel wait also has
a backstop (25 ticks, 100 ms, for pipes, sockets, `wait4` and terminals; 50 ms
for `poll`), so a lost wake-up costs a delay and never a hang.

### 2.7 Capabilities

Privilege is sixteen capabilities, not uid 0. Root is the credential that
holds all of them; each privileged operation asks for the one it needs
(`kernel/process/cred.rs`). The ones a system call checks:

| Bit | Name | Checked by |
|---|---|---|
| 0 | `CAP_SETUID` (`setuid`) | `setuid`/`seteuid`/`setresuid` to an id not already held; `session` open and close |
| 1 | `CAP_SETGID` (`setgid`) | `setgid`/`setegid`/`setresgid` to an id not held; `setgroups` |
| 2 | `CAP_KILL` (`kill`) | `kill` of a process whose uid is not the caller's euid |
| 3 | `CAP_DAC_OVERRIDE` (`dac_override`) | every VFS permission check (read, write, search) |
| 4 | `CAP_DAC_READ` (`dac_read`) | VFS read and search checks only |
| 5 | `CAP_FOWNER` (`fowner`) | acting as a file's owner (`chmod`, times, xattrs) |
| 6 | `CAP_CHOWN` (`chown`) | `chown` |
| 7 | `CAP_SYS_TIME` (`sys_time`) | `clock_settime` |
| 8 | `CAP_SYS_ADMIN` (`sys_admin`) | `tty(TTY_OF_PID)` and attaching to any terminal; privileged `kcmd` commands |
| 9 | `CAP_SYS_BOOT` (`sys_boot`) | `power` |
| 10 | `CAP_NET_RAW` (`net_raw`) | `open` of `/dev/net0` (raw frames); the ICMP socket does not need it |
| 11 | `CAP_NET_BIND` (`net_bind`) | `bind` to a TCP/UDP port below 1024 |
| 12 | `CAP_MKNOD` (`mknod`) | defined, checked nowhere: no system call creates device nodes |
| 13 | `CAP_MAY_PRIV` (`may_priv`) | binding the display server's privileged globals (Mayland protocol, not a syscall) |
| 14 | `CAP_AUDIT` (`audit`) | defined, checked nowhere in the kernel |
| 15 | `CAP_GPU` (`gpu`) | `gpu_program` |

The rules for gaining, keeping and dropping them are in
[§4.9](#49-identity-and-capabilities-47-55-86-91); the policy that decides who
gets them is in [MikuOS_Security.md](MikuOS_Security.md).

### 2.8 Limits

| Limit | Value | Where |
|---|---|---|
| Path length | 4096 bytes | `usercopy::MAX_PATH_LEN` |
| Symlinks followed in one lookup | 8 | `vfs::types::MAX_SYMLINK_DEPTH` |
| Open VFS descriptors per process | 512 | `vfs::types::MAX_OPEN_FILES` |
| Sockets on the machine / per account / display connections per process | 256 / 160 / 4 | `net/socket.rs` |
| Bytes moved by one `read` or `write` | 1 MiB (larger requests are clamped) | `io.rs` `MAX_IO_LEN` |
| Bytes moved by one `send`/`recv`/`sendto`/`recvfrom` | 16 MiB (`-EINVAL` above) | `net.rs` `MAX_IO_LEN` |
| `exec` argv / envp entries (each) | 256 | `usercopy::MAX_ARGV_ENTRIES` |
| `exec` single argument | 32 KiB | `usercopy::MAX_ARG_BYTES` |
| `exec` argv + envp string bytes | 128 KiB | `elf_loader::ARG_MAX_STRING_BYTES` |
| ELF file size read by the loader | 64 MiB | `elf_loader::MAX_ELF_SIZE` |
| `poll` descriptors per call | 256 | `io.rs` `MAX_POLL_FDS` |
| Directory entries per `getdents`/`readdir` call | 120 | `fs.rs` `DIRENTS_PER_CALL` |
| `getrandom` per call | 1024 bytes | `GETRANDOM_MAX` |
| `kcmd` command line | 512 bytes | `admin.rs` `MAX_CMD` |
| Supplementary groups | 16 | `vfs::types::MAX_GROUPS` |
| `sleep` (16) | 100 000 ticks (400 s), clamped | `sys_sleep` |
| `sleep_ns` (122) | one day, clamped | `sys_sleep_ns` |
| User stack | 8 MiB mapping, 256 KiB populated at exec | `elf_loader::STACK_SIZE` |
| Display pool size | 64 MiB; 64 pools on the machine, 16 per process | `mayland/server/shm.rs` |
| Signals | 1..31 | `signal::NSIG` = 32 |

There are no per-process resource limits in the kernel. `libc_miku`'s
`getrlimit` answers from the library: `RLIMIT_NOFILE` is the descriptor table
size and every other resource is `RLIM_INFINITY`.

---
## 3. Index of all system calls

"pw" means a path word ([§2.4](#24-paths-and-the-path-word)). "-" in the
wrapper columns means no library wraps the call. libmiku wrappers are in
`lib/runtime/libmiku/` (exported from `core_miku.so` unless noted), libc wrappers in
`lib/runtime/libmiku/libc/` and `lib/runtime/libmiku/libc/` (exported from `libc_miku.so`).

| Nr | Name | rdi | rsi | rdx | r10 | Success | libmiku | libc | Cap | § |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | exit | code | | | | does not return | `miku_exit` | `exit`, `_exit`, `_Exit` | | [4.1](#41-process-lifetime) |
| 1 | write | fd | buf | len | | bytes | `miku_write`, `miku_write_fd` | `write`, `writev`, `pwrite` | | [4.3](#43-reading-writing-and-waiting) |
| 2 | read | fd | buf | len | | bytes, 0 = EOF | `miku_read` | `read`, `readv`, `pread` | | [4.3](#43-reading-writing-and-waiting) |
| 3 | mmap | addr | len | prot | flags | address | `miku_mmap` | `mmap` (anonymous) | | [4.2](#42-memory) |
| 4 | munmap | addr | len | | | 0 | `miku_munmap` | `munmap` | | [4.2](#42-memory) |
| 5 | mprotect | addr | len | prot | | 0 | `miku_mprotect` | `mprotect` | | [4.2](#42-memory) |
| 6 | brk | addr (0 = ask) | | | | current break | `miku_brk` | `brk`, `sbrk` | | [4.2](#42-memory) |
| 7 | getpid | | | | | pid | `miku_getpid` | `getpid` | | [4.1](#41-process-lifetime) |
| 8 | getcwd | buf | size | | | buf | `miku_getcwd` | `getcwd` | | [4.5](#45-the-file-namespace) |
| 9 | set_tls | addr | | | | 0 | `miku_set_tls` | - | | [4.2](#42-memory) |
| 10 | get_tls | | | | | FS base | `miku_get_tls` | - | | [4.2](#42-memory) |
| 11 | open | path | pw | flags | mode | fd | `miku_open` | `open`, `openat`, `creat` | | [4.4](#44-descriptors-and-open-files) |
| 12 | close | fd | | | | 0 | `miku_close` | `close` | | [4.4](#44-descriptors-and-open-files) |
| 13 | seek | fd | offset | whence | | new offset | `miku_seek`, `miku_lseek` | `lseek` | | [4.4](#44-descriptors-and-open-files) |
| 14 | fsize | fd | | | | size | `miku_fsize` | - | | [4.4](#44-descriptors-and-open-files) |
| 15 | map_lib | name | len | | | load base | `miku_map_lib` | - (ld-miku) | | [4.2](#42-memory) |
| 16 | sleep | ticks | | | | 0 | `miku_sleep`, `miku_yield` | `sched_yield` (as `sleep(0)`) | | [4.8](#48-time-and-entropy) |
| 17 | uptime | | | | | ticks | `miku_uptime`, `miku_uptime_ms` | - | | [4.8](#48-time-and-entropy) |
| 18 | stat | path | pw | stat* | | 0 | `miku_stat` | `stat`, `fstatat`, `access` | | [4.5](#45-the-file-namespace) |
| 19 | fstat | fd | stat* | | | 0 | `miku_fstat` | `fstat` | | [4.5](#45-the-file-namespace) |
| 20 | mkdir | path | pw | mode | | 0 | `miku_mkdir` | `mkdir`, `mkdirat` | | [4.5](#45-the-file-namespace) |
| 21 | rmdir | path | pw | | | 0 | `miku_rmdir` | `rmdir`, `unlinkat` | | [4.5](#45-the-file-namespace) |
| 22 | unlink | path | pw | | | 0 | `miku_unlink` | `unlink`, `unlinkat`, `remove` | | [4.5](#45-the-file-namespace) |
| 23 | readdir | path | pw | dirent* | max | count | `miku_readdir` | - | | [4.5](#45-the-file-namespace) |
| 24 | rename | old | old pw | new | new pw | 0 | `miku_rename` | `rename`, `renameat` | | [4.5](#45-the-file-namespace) |
| 25 | link | old | old pw | new | new pw | 0 | `miku_link` | `link`, `linkat` | | [4.5](#45-the-file-namespace) |
| 26 | chmod | path | pw | mode | | 0 | `miku_chmod`, `miku_fchmod` | `chmod`, `fchmod`, `fchmodat` | | [4.5](#45-the-file-namespace) |
| 27 | chown | path | pw | uid | gid | 0 | `miku_chown`, `miku_fchown` | `chown`, `fchown`, `fchownat` | (`chown`) | [4.5](#45-the-file-namespace) |
| 28 | dup | fd | | | | new fd | `miku_dup` | `dup`, `fcntl(F_DUPFD)` | | [4.4](#44-descriptors-and-open-files) |
| 29 | dup2 | old | new | | | new | `miku_dup2` | `dup2`, `dup3` | | [4.4](#44-descriptors-and-open-files) |
| 30 | truncate | fd | length | | | 0 | `miku_ftruncate` | `ftruncate`, `truncate` | | [4.4](#44-descriptors-and-open-files) |
| 31 | write_file | fd | buf | len | | bytes | - | - | | [4.3](#43-reading-writing-and-waiting) |
| 32 | symlink | target | len | link | pw | 0 | `miku_symlink` | `symlink`, `symlinkat` | | [4.5](#45-the-file-namespace) |
| 33 | readlink | path | pw | buf | size | length | `miku_readlink` | `readlink`, `readlinkat` | | [4.5](#45-the-file-namespace) |
| 34 | pipe | fds* (2 x u64) | | | | 0 | `miku_pipe` | `pipe` | | [4.4](#44-descriptors-and-open-files) |
| 35 | chdir | path | pw | | | 0 | `miku_chdir` | `chdir`, `fchdir` | | [4.5](#45-the-file-namespace) |
| 36 | statfs | path | pw | statfs* | | 0 | `miku_statfs` | `statfs`, `fstatfs`, `statvfs`, `fstatvfs` | | [4.5](#45-the-file-namespace) |
| 37 | fallocate | fd | offset | len | | 0 | `miku_fallocate` | `posix_fallocate` | | [4.4](#44-descriptors-and-open-files) |
| 38 | getxattr | ino | name | name_len | buf | length | - | - | | [4.6](#46-extended-attributes-38-39-81-82) |
| 39 | setxattr | ino | name | value | sizes | 0 | - | - | | [4.6](#46-extended-attributes-38-39-81-82) |
| 40 | utimensat | path | pw | times* | flags | 0 | `miku_utimensat` | `utimensat`, `futimens`, `utimes`, `lutimes` | | [4.5](#45-the-file-namespace) |
| 41 | fsync | fd | | | | 0 | `miku_fsync` | `fsync`, `fdatasync`, `syncfs` | | [4.4](#44-descriptors-and-open-files) |
| 42 | punch_hole | fd | offset | len | | 0 | - | - | | [4.4](#44-descriptors-and-open-files) |
| 43 | fork | | | | | child pid / 0 | `miku_fork` | `fork` | | [4.1](#41-process-lifetime) |
| 44 | wait4 | pid | status* | options | | pid / 0 | `miku_wait4`, `miku_waitpid`, `miku_try_wait` | `waitpid`, `waitid` | | [4.1](#41-process-lifetime) |
| 45 | kill | pid | sig | | | 0 | `miku_kill`, `miku_killpg` | `kill`, `killpg`, `raise` | (`kill`) | [4.1](#41-process-lifetime) |
| 46 | exec | path | len | argv | argc | does not return | `miku_exec` | - | | [4.1](#41-process-lifetime) |
| 47 | umask | mask | | | | old mask | `miku_umask` | `umask` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 48 | getuid | | | | | uid | `miku_getuid` | `getuid` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 49 | getgid | | | | | gid | `miku_getgid` | `getgid` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 50 | geteuid | | | | | euid | `miku_geteuid` | `geteuid` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 51 | getegid | | | | | egid | `miku_getegid` | `getegid` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 52 | setuid | uid | | | | 0 | `miku_setuid` | - | (`setuid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 53 | setgid | gid | | | | 0 | `miku_setgid` | - | (`setgid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 54 | seteuid | euid | | | | 0 | `miku_seteuid` | - | (`setuid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 55 | setegid | egid | | | | 0 | `miku_setegid` | - | (`setgid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 56 | socket | domain | type | protocol | | fd | `miku_socket`, `_udp`, `_icmp`, `_unix` (`net_miku.so`) | `socket` | | [4.14](#414-sockets) |
| 57 | connect | fd | sockaddr* | addrlen | | 0 | `miku_connect`, `miku_connect_unix` | `connect` | | [4.14](#414-sockets) |
| 58 | send | fd | buf | len | flags (ignored) | bytes | `miku_send` | `send`, `sendmsg` | | [4.14](#414-sockets) |
| 59 | recv | fd | buf | len | flags (ignored) | bytes | `miku_recv` | `recv`, `recvmsg` | | [4.14](#414-sockets) |
| 60 | mmap_file | args* (6 x u64) | | | | address | `miku_mmap_file` | `mmap` (file) | | [4.2](#42-memory) |
| 61 | msync | addr | len | | | 0 | `miku_msync` | `msync` | | [4.2](#42-memory) |
| 62 | bind | fd | sockaddr* | addrlen | | 0 | `miku_bind`, `miku_bind_unix` | `bind` | (`net_bind`) | [4.14](#414-sockets) |
| 63 | listen | fd | backlog (ignored) | | | 0 | `miku_listen` | `listen` | | [4.14](#414-sockets) |
| 64 | accept | fd | sockaddr* | addrlen (ignored) | | fd | `miku_accept` | `accept`, `accept4` | | [4.14](#414-sockets) |
| 65 | sendto | fd | buf | len | sockaddr* | bytes | `miku_sendto` | `sendto` | | [4.14](#414-sockets) |
| 66 | recvfrom | fd | buf | len | sockaddr* | bytes | `miku_recvfrom` | `recvfrom` | | [4.14](#414-sockets) |
| 67 | execve | args* (6 x u64) | | | | does not return | `miku_execve` | `execve`, `execvpe`, `fexecve` | | [4.1](#41-process-lifetime) |
| 68 | sigentry | entry | | | | 0 | `miku_signal_init` (`sys_miku.so`) | (implicit) | | [4.11](#411-signals-68-69-100-115) |
| 69 | sigreturn | frame* | | | | does not return | signal entry stub | (implicit) | | [4.11](#411-signals-68-69-100-115) |
| 70 | clock_gettime | clock | timespec* | | | 0 | `miku_clock_gettime`, `miku_realtime` | `clock_gettime`, `gettimeofday`, `time` | | [4.8](#48-time-and-entropy) |
| 71 | ioctl | fd | request | arg | | 0 | `miku_ioctl` | `ioctl`, `tcgetattr`, `tcsetattr`, `isatty` | | [4.3](#43-reading-writing-and-waiting) |
| 72 | may_pool | size | out* (2 x u64) | | | 0 | libmay `Pool::new` | - | | [4.15](#415-display-pools-72-74) |
| 73 | may_unpool | handle | | | | 0 | libmay `Pool::free` | - | | [4.15](#415-display-pools-72-74) |
| 74 | may_map | handle | out* (u64) | | | 0 | libmay `Pool::adopt` | - | | [4.15](#415-display-pools-72-74) |
| 75 | klog | buf | len | | | bytes | `miku_klog` | - | | [4.3](#43-reading-writing-and-waiting) |
| 76 | poll | pollfd* | nfds | timeout_ms | | ready count | `miku_poll` | `poll`, `select` | | [4.3](#43-reading-writing-and-waiting) |
| 77 | socketpair | fds* (2 x u64) | | | | 0 | `miku_socketpair` | `socketpair` | | [4.14](#414-sockets) |
| 78 | getrandom | buf | len | | | bytes | `miku_getrandom` | `getrandom` | | [4.8](#48-time-and-entropy) |
| 79 | clock_settime | clock | timespec* | | | 0 | `miku_set_realtime` | `clock_settime` | `sys_time` | [4.8](#48-time-and-entropy) |
| 80 | setsockopt | fd | level | optname | value | 0 | `miku_set_ttl` | `setsockopt` | | [4.14](#414-sockets) |
| 81 | listxattr | ino | buf | len | | length | - | - | | [4.6](#46-extended-attributes-38-39-81-82) |
| 82 | removexattr | ino | name | name_len | | 0 | - | - | | [4.6](#46-extended-attributes-38-39-81-82) |
| 83 | peercred | fd | ucred* (40 B) | | | 0 | `miku_peercred` | - | | [4.14](#414-sockets) |
| 84 | sendfd | sockfd | fd | | | 0 | `miku_sendfd` | - | | [4.14](#414-sockets) |
| 85 | recvfd | sockfd | | | | fd | `miku_recvfd` | - | | [4.14](#414-sockets) |
| 86 | setresuid | ruid | euid | suid | | 0 | `miku_setresuid` | - | (`setuid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 87 | setresgid | rgid | egid | sgid | | 0 | `miku_setresgid` | - | (`setgid`) | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 88 | setgroups | list* (u16) | count | | | 0 | `miku_setgroups` | `setgroups`, `initgroups` | `setgid` | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 89 | getgroups | list* (u16) | max | | | count | `miku_getgroups` | `getgroups` | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 90 | capget | buf* (3 x u64) | | | | 0 | `miku_capget` | - | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 91 | capset | permitted | effective | bounding | | 0 | `miku_capset` | - | | [4.9](#49-identity-and-capabilities-47-55-86-91) |
| 92 | session | op | arg | ptr | len | per op | `miku_session` | - | (`setuid`) | [4.10](#410-sessions-and-power-92-93) |
| 93 | power | action | | | | 0 | `miku_power` | - | `sys_boot` | [4.10](#410-sessions-and-power-92-93) |
| 94 | gpu_info | buf | len | | | bytes | libmay `gpu_info` | - | | [4.16](#416-the-gpu-94-98-121) |
| 95 | gpu_submit | args* (6 x u64) | | | | fence | libmay `gpu_blit(s)`, `gpu_dispatch`, `gpu_chain` | - | | [4.16](#416-the-gpu-94-98-121) |
| 96 | gpu_bind | handle | flags (0) | out* (2 x u64) | | 0 | libmay `gpu_bind` | - | | [4.16](#416-the-gpu-94-98-121) |
| 97 | gpu_wait | fence | timeout_us | | | 0 | libmay `gpu_wait`, `gpu_wait_code` | - | | [4.16](#416-the-gpu-94-98-121) |
| 98 | gpu_program | args* | | | | program id | libmay `gpu_program` | - | `gpu` | [4.16](#416-the-gpu-94-98-121) |
| 99 | tty | op | arg | | | per op | `miku_tty` | `tcsetpgrp`, `tcgetpgrp`, `ttyname_r` | (`sys_admin`) | [4.13](#413-terminals-and-jobs-99-101-102) |
| 100 | sigdisp | sig | disp | | | previous disp | `miku_sigdisp` | `sigaction`, `signal` | | [4.11](#411-signals-68-69-100-115) |
| 101 | setpgid | pid | pgid | | | 0 | `miku_setpgid` | `setpgid`, `setsid`* | | [4.13](#413-terminals-and-jobs-99-101-102) |
| 102 | getpgid | pid | | | | pgid | `miku_getpgid` | `getpgid`, `getpgrp` | | [4.13](#413-terminals-and-jobs-99-101-102) |
| 103 | procinfo | buf | max | | | count | `miku_procinfo` | (`sysinfo`) | | [4.1](#41-process-lifetime) |
| 104 | kcmd | text | len | | | 0 | `miku_kcmd` | - | per command | [4.17](#417-the-administrative-channel-104) |
| 105 | proc_path | pid | buf | len | | bytes | `miku_proc_path` | - | | [4.1](#41-process-lifetime) |
| 106 | thread_create | args* (8 x u64) | | | | tid | `miku_thread_spawn*` (`lib/runtime/libmiku/core/thread.rs`) | `pthread_create` | | [4.12](#412-threads-and-the-futex-106-110) |
| 107 | thread_exit | code | | | | does not return | `miku_thread_exit` | `pthread_exit` | | [4.12](#412-threads-and-the-futex-106-110) |
| 108 | gettid | | | | | tid | `miku_gettid` | `pthread_self`* | | [4.12](#412-threads-and-the-futex-106-110) |
| 109 | sched_yield | | | | | 0 | `miku_sched_yield` | - | | [4.12](#412-threads-and-the-futex-106-110) |
| 110 | futex | uaddr | op | val | timespec* | 0 / woken | `miku_futex_wait`, `miku_futex_wake` | (pthreads) | | [4.12](#412-threads-and-the-futex-106-110) |
| 111 | getdents | fd | dirent* | max | offset | count | `miku_getdents`, `miku_dir_*` | `readdir`, `opendir` | | [4.5](#45-the-file-namespace) |
| 112 | lstat | path | pw | stat* | | 0 | `miku_lstat` | `lstat`, `fstatat` | | [4.5](#45-the-file-namespace) |
| 113 | fcntl | fd | cmd | arg | | per cmd | `miku_fcntl` | `fcntl`, `flock`* | | [4.4](#44-descriptors-and-open-files) |
| 114 | getsockopt | fd | level | optname | | value | `miku_getsockopt` | `getsockopt` | | [4.14](#414-sockets) |
| 115 | sigaltstack | new* | old* | | | 0 | - | `sigaltstack` | | [4.11](#411-signals-68-69-100-115) |
| 116 | inotify_init1 | flags | | | | fd | - | `inotify_init`, `inotify_init1` | | [4.7](#47-watching-files-inotify-116-118) |
| 117 | inotify_add_watch | fd | path | pw | mask | wd | - | `inotify_add_watch` | | [4.7](#47-watching-files-inotify-116-118) |
| 118 | inotify_rm_watch | fd | wd | | | 0 | - | `inotify_rm_watch` | | [4.7](#47-watching-files-inotify-116-118) |
| 119 | realpath | path | pw | buf | size | length | `miku_realpath`, `miku_fd_path` | `realpath`, `fcntl(F_GETPATH)` | | [4.5](#45-the-file-namespace) |
| 120 | pipe2 | fds* (2 x u64) | flags | | | 0 | `miku_pipe2` | `pipe2` | | [4.4](#44-descriptors-and-open-files) |
| 121 | gpu_alloc | size | flags (0) | out* (2 x u64) | | 0 | libmay `Pool::video` | - | | [4.16](#416-the-gpu-94-98-121) |
| 122 | sleep_ns | ns | | | | 0 | `miku_sleep_ns` | `nanosleep`, `usleep`, `sleep` | | [4.8](#48-time-and-entropy) |
| 123 | poll_ns | pollfd* | nfds | timeout_ns | | ready count | - | - | | [4.3](#43-reading-writing-and-waiting) |

A capability in parentheses is needed only for some uses (for example `kill`
of another user's process). An asterisk in the libc column marks a function
built from this call and others (`setsid` is `setpgid(0, 0)`, `pthread_self`
reads the thread's own block, `flock` is built on record locks, `truncate`
opens the file and calls 30); see [wiki/sdk.md](wiki/sdk.md) for how `libc_miku` composes them.

---

## 4. System call reference

Each entry gives the call as a C-like signature with the registers in order
(`rdi`, `rsi`, `rdx`, `r10`), what it returns, every error the code can
produce, what it does and where it differs from Linux. Errors from the common
checks are listed once here and not repeated:

- `EFAULT`: a pointer failed the check in [§2.3](#23-user-pointers);
- `EINVAL` from a path: empty without a base, longer than 4096 bytes, or not
  UTF-8; `EBADF`/`ENOTDIR` from a path word's base ([§2.4](#24-paths-and-the-path-word));
- any errno of the VFS table in [§2.2](#22-return-values-and-errno) for calls
  that resolve a path, the commonest being `ENOENT`, `ENOTDIR`, `EACCES`,
  `ELOOP`, `ENAMETOOLONG` and `EROFS`.

### 4.1 Process lifetime

The process and thread model behind these calls is described in
[wiki/processes.md](wiki/processes.md).

#### 0 `exit`

```
exit(u64 code) -> does not return
```

Ends the **whole process**: every thread of the group, not only the caller.
In order: the other threads are killed, the process's descriptors are closed
(so a pipe's reader sees end of file before the parent is told), the group
is marked dead with `code`, the parent gets `SIGCHLD`, and the task yields
for the last time. The parent's `wait4` sees `(code & 0xff) << 8`.

`thread_exit` (107) ends one thread. libmiku: `miku_exit`; libc: `exit` (runs
`atexit` handlers and flushes stdio first), `_exit`, `_Exit`.

#### 7 `getpid`

```
getpid() -> u64 id
```

Never fails. Returns the process id: the thread group's id, which is the id
of its first thread, from every thread of the process. A thread's own id is
`gettid` (108). Signals, `wait4`, `kill` and `procinfo` name the process by
the same number. (Until 2026-11-03 the dispatcher answered with the calling
task, so a thread other than the first got its own id here.) libmiku:
`miku_getpid`; libc: `getpid`.

#### 43 `fork`

```
fork() -> u64 child_pid (parent), 0 (child)
```

| errno | Cause |
|---|---|
| `EPERM` | called from a kernel thread (no user address space) |
| `ENOMEM` | the address space could not be copied |
| `EAGAIN` | no process slot or pid could be allocated |

The child is a copy of the calling process with:

- the address space shared **copy-on-write**: both sides map every private
  page read-only and the first write copies it; pages mapped `MAP_SHARED` from
  a tmpfs file stay shared; Mayland pool windows are lent, not copied;
- the same VMAs and program break;
- a copy of the descriptor table (each descriptor references the same open
  file; sockets are not duplicated into the child's ownership, see
  [§4.14](#414-sockets));
- the parent's signal entry point and dispositions, no pending signals;
- the parent's credential, working directory, umask, process group and
  controlling terminal;
- the calling thread's registers, so the child resumes at the instruction after
  `syscall` with `rax` = 0. Only the calling thread is copied.

Sibling CPUs that have the parent's page tables loaded are flushed before a
COW page can be written ([wiki/memory.md](wiki/memory.md)). There is no
`vfork` and no `clone` with flags: `fork` (43) makes processes and
`thread_create` (106) makes threads. libmiku: `miku_fork`; libc: `fork`
(which also runs `pthread_atfork` handlers and holds libmiku's heap and
environment locks across the call, see [wiki/sdk.md](wiki/sdk.md)).

#### 44 `wait4`

```
wait4(u64 pid, u64 *status, u64 options) -> u64 pid | 0
```

| errno | Cause |
|---|---|
| `ECHILD` | the caller has no children (none live, none unreaped) |
| `EINTR` | a deliverable signal arrived while it was waiting |

`pid` is a child's pid, or `-1` (`u64::MAX`) for any child. **There are no
process-group waits**: 0 and other negative values match no child, so they
block until a signal or, with `WNOHANG`, return 0.

`options`: `WNOHANG` (1) returns 0 at once when no child has changed state;
`WUNTRACED` (2) also reports a child that has stopped (and has not been
reported since it last stopped). A stopped child is reported only to a caller
that asked.

`status`, when non-zero, receives a **64-bit** word in POSIX's encoding:

| Outcome | Status |
|---|---|
| exited with `code` | `(code & 0xff) << 8` |
| killed by signal `sig` | `sig & 0x7f` |
| stopped by signal `sig` | `0x7f \| (sig << 8)` |

So `WIFEXITED`, `WEXITSTATUS`, `WIFSIGNALED`, `WTERMSIG`, `WIFSTOPPED` and
`WSTOPSIG` work unchanged on the low 32 bits. There is no core-dump bit and no
`WIFCONTINUED`. A child that was reaped is gone; a status that cannot be
written is logged and the child is reaped anyway.

The wait sleeps on the death queue with a 100 ms backstop. libmiku:
`miku_wait4`, `miku_waitpid` (no options), `miku_try_wait` (`WNOHANG`); libc:
`waitpid`, `waitid`; `lib/sdk/miku`'s `proc::exited`, `signalled`, `stopped`,
`exit_code`, `term_sig`, `stop_sig` and `wait_status_code` (the `128+sig`
number a shell prints) decode it.

#### 45 `kill`

```
kill(i64 pid, u64 sig) -> 0
```

| errno | Cause |
|---|---|
| `EINVAL` | `pid` is 0, or `sig` > 64 |
| `ESRCH` | no such process (or, for a group, no member) |
| `EPERM` | the caller may not signal it (every member, for a group) |

`pid > 0` names a process; a thread id is redirected to its process, because
signals are delivered to the process ([§4.11](#411-signals-68-69-100-115)).
`pid < 0` signals every member of process group `-pid`, each checked
separately; the call succeeds if any member could be signalled. **`pid` 0
(the caller's own group) and -1 (everybody) are refused** with `EINVAL`.

`sig` 0 checks existence and permission and sends nothing. Signals 32-64 are
accepted and dropped (`NSIG` is 32).

Permission: `CAP_KILL`, or the caller's effective uid equal to the target's
real or saved uid. `SIGKILL` (9) kills at once and sends `SIGCHLD` to the
parent; every other signal goes through the target's disposition. libmiku:
`miku_kill`, `miku_killpg`; libc: `kill`, `killpg`, `raise`.

#### 46 `exec`

```
exec(const char *path, u64 path_len, const char **argv, u64 argc) -> does not return
```

Replaces the calling process's image, with the **kernel's default
environment**: `PATH=/bin`, `HOME=/`, `TERM=miku`, `OS=MikuOS`
(`process::exec::DEFAULT_ENV`). `argv` is an array of `argc` pointers to
NUL-terminated strings; with `argc` 0 the path is passed as `argv[0]`. Use
`execve` (67) to pass an environment.

| errno | Cause |
|---|---|
| `EPERM` | the caller is a thread other than the process's first |
| `EACCES` | no execute permission, or the path is a directory |
| `ENOENT` | the file does not exist, cannot be read, its `#!` line is bad, or **the ELF image was refused** by the loader |
| `ENOMEM` | no address space for the new image |
| `EINVAL` | `argc` > 256, an argument longer than 32 KiB, too many argument bytes in total, an argument not valid UTF-8 |

The path is not a path word; a relative path is resolved from the working
directory. A file starting with `#!` is run by its interpreter, one level
deep: the interpreter must be an absolute path, and everything after it on
the line is passed as a single argument (`process::exec::resolve_shebang`,
at most 128 bytes).

A refused image is `ENOENT` rather than `ENOEXEC`: the loader's reason is on
the serial line (`[exec] ELF load failed: ...`).

On success, in order: the other threads of the process are killed and
reaped, the new image is loaded into a fresh address space, close-on-exec
descriptors and sockets are closed, the image is registered for crash reports
(`process::image`), the process name becomes the file's name, the credential
becomes `permitted &= bounding; effective = permitted` (no setuid or setgid
bit is honoured, [§4.9](#49-identity-and-capabilities-47-55-86-91)), the signal
entry point is cleared, caught signals revert to default (ignored ones stay
ignored), pending signals and the alternate stack are dropped, the FPU state
is reset, the old address space is freed and `FS_BASE` is set to the new
thread block. The new image starts with `rdi`, `rsi`, `rdx` and `rax` zero.

libmiku: `miku_exec`. Programs normally use `execve`.

#### 67 `execve`

```
struct ExecveArgs { u64 path, path_len, argv, argc, envp, envc; };   /* 48 bytes */
execve(const struct ExecveArgs *args) -> does not return
```

`exec` with an environment: `envp` is an array of `envc` pointers to
`KEY=value` strings, checked like `argv`. Errors are those of `exec`, plus
`EFAULT` for a null or unreadable `args`. libmiku: `miku_execve`; libc:
`execve`, `execvpe` (searches `PATH`), `fexecve`.

#### 103 `procinfo`

```
procinfo(struct ProcInfo *buf, u64 max) -> u64 count
```

Fills up to `max` 72-byte records (at most 512 per call) describing every
**process** on the machine (threads are left out). `max` 0 returns 0.
Anyone may call it; it reveals nothing about any process's memory.

```
  0  u64  pid
  8  u64  ppid
 16  u64  pgid
 24  u64  cpu_time        ticks of CPU used
 32  u16  uid
 34  u8   state           one letter: R S B D Z
 35  u8   priority
 36  u32  cpu_pct_x10     tenths of a percent
 40  u32  tty             raw terminal id, 0 = none
 44  [24] name            NUL-padded, 23 usable bytes
 68  [4]  zero
```

`ps` and `top` are this call. libmiku: `miku_procinfo`. There are no
`/proc/<pid>` directories ([wiki/diagnostics.md](wiki/diagnostics.md)).

#### 105 `proc_path`

```
proc_path(u64 pid, char *buf, u64 len) -> u64 bytes
```

The path the process was started from, as `exec` was given it. `pid` 0 is the
caller's process (a thread asking gets its process's image). Writes at most
`min(len, 4096)` bytes, **without a terminating NUL**, and returns how many.
A pid with no image (a kernel thread, a pid nobody holds) returns 0. `len` 0
returns 0. libmiku: `miku_proc_path`; Rust `std::env::current_exe` uses it.

### 4.2 Memory

Virtual memory, page faults and the heap are described in
[wiki/memory.md](wiki/memory.md).

#### 3 `mmap`

```
mmap(u64 addr, u64 len, u64 prot, u64 flags) -> u64 address
```

An **anonymous** mapping only (the kernel forces `MAP_ANONYMOUS`; file
mappings are 60). `len` is rounded up to whole pages.

| errno | Cause |
|---|---|
| `EINVAL` | `len` 0; `prot` has a bit other than `PROT_READ`/`PROT_WRITE`/`PROT_EXEC`, or both `PROT_WRITE` and `PROT_EXEC` (W^X); with `MAP_FIXED`, `addr` 0, not page-aligned, or the range leaves the user half |
| `ENOMEM` | no free range of that size |

`flags`: only `MAP_FIXED` (0x10) changes anything. With it, whatever was
mapped in the range is unmapped first and the mapping is placed exactly at
`addr`. Without it **`addr` is ignored** and the kernel picks a range in
`0x1_0000_0000 .. 0x7F00_0000_0000`. `MAP_SHARED`, `MAP_PRIVATE` and the rest
are accepted and ignored. Pages are zero-filled on first touch.

libmiku: `miku_mmap`; libc: `mmap` with `MAP_ANONYMOUS`.

#### 4 `munmap`, 5 `mprotect`

```
munmap(u64 addr, u64 len) -> 0
mprotect(u64 addr, u64 len, u64 prot) -> 0
```

`addr` must be page-aligned and the range must stay inside the user half
(`EINVAL` otherwise); `mprotect` refuses unknown bits and W+X like `mmap`.
`munmap` writes dirty `MAP_SHARED` file pages back before dropping them.
Other CPUs running the same address space are flushed before the call returns.
libmiku: `miku_munmap`, `miku_mprotect`; libc: `munmap`, `mprotect`.

#### 6 `brk`

```
brk(u64 new_break) -> u64 current_break
```

Never returns an errno. `new_break` 0 asks for the current break. A larger
value maps zeroed, non-executable pages up to it (rounded up to a page); a
smaller one unmaps down to it, but never below the initial break. A request
that cannot be satisfied returns the unchanged break, which is how a caller
learns it failed. The initial break is the end of the image plus a random gap
of up to 32 MiB. libmiku: `miku_brk`; libc: `brk`, `sbrk`. libmiku's own heap
does not use it ([wiki/sdk.md](wiki/sdk.md)).

#### 9 `set_tls`, 10 `get_tls`

```
set_tls(u64 addr) -> 0
get_tls() -> u64 fs_base
```

Write and read the calling thread's `FS_BASE`, the thread pointer of the
x86-64 TLS ABI. `set_tls` refuses an address above the user half or a
non-canonical one with `EINVAL`. These are what `arch_prctl(ARCH_SET_FS)` and
`ARCH_GET_FS` are on Linux; there is no `arch_prctl` and no `prctl`. Each
thread has its own `FS_BASE`, saved on every switch. libmiku: `miku_set_tls`,
`miku_get_tls`. What the thread pointer points at is in
[§7.6](#76-the-thread-control-block).

#### 15 `map_lib`

```
map_lib(const char *soname, u64 len) -> u64 load_base
```

Maps a shared object into the caller by name and returns its load base. The
name is at most 256 bytes (`EINVAL` for 0 or more), trailing NULs are ignored,
and it must be UTF-8. The object comes from the kernel's shared-object cache
(which preloads the ten miku libraries) or from the search path `/lib`,
`/usr/lib` (`process::solib`). On failure it returns the loader's negative
errno. This is ld-miku's call; a program does not normally make it. libmiku:
`miku_map_lib`.

#### 60 `mmap_file`

```
struct MmapArgs { u64 addr, len, prot, flags, fd, offset; };    /* 48 bytes */
mmap_file(const struct MmapArgs *args) -> u64 address
```

A file-backed mapping. Pages are filled from the file on first touch; reading
past the end of the file gives zeroes.

| errno | Cause |
|---|---|
| `EFAULT` | `args` unreadable |
| `EINVAL` | `len` 0; `fd` is not an open file with a backing vnode; `offset` not page-aligned; bad `prot` or W+X; a bad `MAP_FIXED` address |
| `ENOMEM` | no free range |

`flags`: `MAP_SHARED` (0x01) writes dirty pages back to the file on `msync`
and `munmap`; without it the mapping is private (copy-on-write, never written
back). `MAP_FIXED` (0x10) as for 3. The mapping keeps the vnode, not the
descriptor, so closing `fd` afterwards is fine. A `MAP_SHARED` mapping of a
**tmpfs** file maps the file's own frames, so two processes see each other's
writes and a futex in it is shared between them; on ext2/3/4 each mapping is
its own copy that `msync` writes back. libmiku: `miku_mmap_file`; libc:
`mmap` with a descriptor.

#### 61 `msync`

```
msync(u64 addr, u64 len) -> 0
```

Writes the dirty pages of `MAP_SHARED` file mappings in the range back to
their files. `addr` must be page-aligned (`EINVAL`). There are no flags:
the write is synchronous. libmiku: `miku_msync`; libc: `msync`.

### 4.3 Reading, writing and waiting

#### 1 `write`

```
write(u64 fd, const void *buf, u64 len) -> u64 bytes
```

At most 1 MiB moves per call; a longer `len` is clamped and the short count
returned. `len` 0 returns 0 without looking at `fd`. What happens depends on
what `fd` is ([§2.5](#25-descriptors)):

| `fd` is | Effect | Errors |
|---|---|---|
| unredirected 1 or 2 | text to the caller's controlling terminal (non-UTF-8 bytes as Latin-1) | none |
| a terminal (`/dev/tty`, `/dev/console`, a pty slave) | text to **that** terminal | `ENXIO` if it is gone, `EIO` if the write failed |
| a pty master | the bytes are the slave's **input**, through the line discipline (echo, line editing, Ctrl-C) | `EIO` if the terminal is gone |
| a socket | `send(fd, buf, len, 0)` | as `send` |
| a pipe | all of it, waiting for room as needed | see below |
| an inotify descriptor | refused | `EINVAL` |
| anything else | the VFS write at the descriptor's offset (or at the end with `O_APPEND`) | VFS errors, `EBADF` |

**Pipes.** A write of at most 4096 bytes (`pipe::ATOMIC_WRITE`) is not
interleaved with other writers; a longer one may be. A pipe holds 64 KiB. With
no reader left the call sends `SIGPIPE` to the writer and returns `-EPIPE`
(or the count already written, if any); `SIGPIPE`'s default kills the writer,
which is why `yes | head` ends. On a full pipe a non-blocking descriptor
returns `-EAGAIN` (or the partial count), and a pending signal returns
`-EINTR` (or the partial count).

A write to a file over the system's dirty-page limit makes the writer write
some of the cache back before returning
([wiki/vfs.md](wiki/vfs.md)). libmiku: `miku_write`, `miku_write_fd`; libc:
`write`, `writev` (a loop of writes), `pwrite` (`seek`, `write`, `seek` back:
**not atomic** against another thread using the same descriptor).

#### 2 `read`

```
read(u64 fd, void *buf, u64 len) -> u64 bytes   (0 = end of file)
```

At most 1 MiB per call; `len` 0 returns 0.

| `fd` is | Effect |
|---|---|
| unredirected 0, or a terminal descriptor | terminal input through the line discipline (below); `ENXIO` if the terminal is gone |
| a pty master | what the program on the slave side wrote; waits for output; 0 once the terminal is gone; `EAGAIN` when non-blocking and empty |
| a pipe | waits for data; 0 when empty and no writer is left; `EAGAIN` when non-blocking and empty |
| a socket | `recv(fd, buf, len, 0)` |
| an inotify descriptor | whole `inotify_event` records ([§4.7](#47-watching-files-inotify-116-118)) |
| anything else | the VFS read at the descriptor's offset |

**Terminal reads.** In canonical mode (`icanon`) a read returns one line. In
raw mode `vmin` and `vtime` from `TCSETS` apply: `vmin 0, vtime 0` returns at
once with whatever is there, even nothing; `vmin n, vtime 0` waits for `n`
characters; `vtime t` gives up after `t` tenths of a second. At most 1024
bytes come back per call. A process whose job is not the terminal's
foreground gets `SIGTTIN`, which stops it; if it ignores or catches
`SIGTTIN` the read returns 0 instead. libmiku: `miku_read`; libc: `read`,
`readv`, `pread` (not atomic, like `pwrite`).

#### 31 `write_file`

```
write_file(u64 fd, const void *buf, u64 len) -> u64 bytes
```

The VFS write without the terminal and socket routing: a pipe still takes the
pipe path, everything else goes straight to the VFS. Older programs used it to
write files when `write` meant the screen. No library wraps it; use `write`.

#### 71 `ioctl`

```
ioctl(u64 fd, u64 request, void *arg) -> 0 | value
```

Terminal requests only. `fd` must name a terminal: an unredirected 0-2 (the
caller's controlling terminal, or the system console if it has none), a
terminal descriptor, or a pty master. Anything else is `-ENOTTY`, including a
file redirected onto fd 0.

| Request | Value | `arg` | Effect | Errors |
|---|---|---|---|---|
| `TCGETS` | 0x5401 | `MikuTermios *` (14 bytes) | read the settings | `EFAULT` |
| `TCSETS` | 0x5402 | `const MikuTermios *` | change them, at once | `EFAULT`; `EINTR` when a background job is stopped for it (`SIGTTOU`, always, not only with `tostop`) |
| `TIOCGWINSZ` | 0x5413 | `struct { u16 rows, cols; } *` | the terminal's size | `EFAULT` |
| `TIOCSWINSZ` | 0x5414 | same | resize it and send `SIGWINCH` to its foreground job | `EPERM` unless `fd` is the **master**; `EINVAL` for a size the terminal refuses |
| `TIOCSCTTY` | 0x540E | ignored | make this terminal the caller's controlling one and give it a new foreground job led by the caller (`TTY_ATTACH`) | `EBADF`, `EPERM` |
| `TIOCGPTN` | 0x80045430 | `u32 *` | which terminal a master controls (its `TtyId`) | `ENOTTY` unless `fd` is a master |

Any other request is `-EINVAL`. **The termios layout is MikuOS's own**, not
Linux's 60-byte struct ([§5](#5-structures)); `libc_miku`'s `tcgetattr`,
`tcsetattr` and `ioctl(TCGETS)` translate to and from Linux's layout, and
`FIONBIO` is answered in libc with `fcntl(F_SETFL, O_NONBLOCK)`. Line
discipline semantics are in
[wiki/input-and-terminals.md](wiki/input-and-terminals.md). libmiku:
`miku_ioctl`; libc: `ioctl`, `tcgetattr`, `tcsetattr`, `isatty`, `cfmakeraw`.

#### 75 `klog`

```
klog(const char *buf, u64 len) -> u64 len
```

Adds one line to the kernel log at level `info`, tagged as coming from user
space, without printing it on any terminal. A trailing newline is dropped;
invalid UTF-8 is replaced. `len` above 1 MiB is `EINVAL`; 0 returns 0. `dmesg`
shows these lines ([wiki/diagnostics.md](wiki/diagnostics.md)). libmiku:
`miku_klog`.

#### 76 `poll`

```
struct pollfd { i32 fd; i16 events; i16 revents; };      /* 8 bytes */
poll(struct pollfd *fds, u64 nfds, i64 timeout_ms) -> u64 ready
```

Waits until at least one descriptor is ready or the timeout passes, and
returns how many entries have a non-zero `revents` (0 on timeout).

| errno | Cause |
|---|---|
| `EINVAL` | `nfds` > 256; `nfds` 0 with an infinite timeout |
| `EFAULT` | the array is not readable and writable |

- `timeout_ms` < 0 waits for ever, 0 polls without sleeping. The deadline is
  kept to the timer's precision (microseconds), not rounded to a tick.
- `nfds` 0 with a timeout is a sleep.
- A negative `fd` is skipped and gets `revents` 0.
- `POLLHUP` is reported whether or not it was asked for; `POLLNVAL` marks a
  descriptor with nothing open on it, and the call still reports the rest.
- End of file counts as readable: a drained pipe with no writer, a socket whose
  peer closed.
- Readiness is computed by the same routing `read` and `write` use, so a
  descriptor reported readable will not block a read.
- A pipe, an AF_UNIX socket, a terminal, a pty master and a display
  connection wake their own pollers; a network socket, an inotify instance and
  `/dev/net0` wake every poller, and a 50 ms backstop bounds any missed wake.
- `poll` does not return `EINTR`; a signal's handler runs when it returns.

Event bits: `POLLIN` 0x001, `POLLOUT` 0x004, `POLLERR` 0x008, `POLLHUP`
0x010, `POLLNVAL` 0x020. There is no `select`, `epoll` or `ppoll` in the
kernel: `libc_miku`'s `select` is built on `poll`, and its `fd_set` is the
usual 1024 bits, so it cannot name a socket (they are numbered from 4096): use
`poll` for sockets. libmiku: `miku_poll`; libc: `poll`, `select`.

#### 123 `poll_ns`

```
poll_ns(struct pollfd *fds, u64 nfds, i64 timeout_ns) -> u64 ready
```

`poll` with the timeout in nanoseconds; everything else is identical. No
library wraps it yet.

### 4.4 Descriptors and open files

How descriptors, open files and the page cache work inside the kernel is in
[wiki/vfs.md](wiki/vfs.md).

#### 11 `open`

```
open(const char *path, u64 path_word, u64 flags, u64 mode) -> u64 fd
```

Opens or creates a file and returns the lowest free descriptor (3 or above
unless 0-2 are free in the table). `flags` 0 means `O_READ`; `mode` 0 means
0644 for a new file. The umask is applied to `mode`.

| Flag | Value | Meaning |
|---|---|---|
| `O_READ` | 0x0001 | open for reading |
| `O_WRITE` | 0x0002 | open for writing (`O_RDWR` = 0x0003) |
| `O_APPEND` | 0x0004 | every write goes to the end |
| `O_CREATE` | 0x0008 | create if missing |
| `O_EXCLUSIVE` | 0x0010 | with `O_CREATE`: fail with `EEXIST` if it exists |
| `O_TRUNC` | 0x0020 | truncate to 0 |
| `O_DIRECTORY` | 0x0040 | fail unless it is a directory |
| `O_NOFOLLOW` | 0x0080 | do not follow a final symlink |
| `O_DIRECT` | 0x0100 | bypass the page cache where the filesystem supports it |
| `O_SYNC` | 0x0200 | write through |
| `O_NONBLOCK` | 0x0400 | pipe, pty master, inotify and socket reads and writes do not wait |
| `O_CLOEXEC` | 0x0800 | close on `exec` |
| `O_NOATIME` | 0x1000 | do not update the access time |

**These are not Linux's values.** `libc_miku`'s `open` translates Linux's
`O_*` numbers to these; a program making the raw call must use the table
above.

Errors: the VFS table ([§2.2](#22-return-values-and-errno)), notably
`ENOENT`, `EEXIST`, `EISDIR` (writing a directory), `ENOTDIR`, `EACCES`,
`EROFS`, `EMFILE` (table full), `ENXIO` (`/dev/tty` with no controlling
terminal), and `EACCES` for `/dev/net0` without `CAP_NET_RAW`. A relative
path that is not found in the VFS and has no descriptor base is tried once
more on the active ext filesystem.

Special files: `/dev/ptmx` creates a pseudo-terminal and returns its master
([§4.13](#413-terminals-and-jobs-99-101-102)); `/dev/tty` is the caller's
controlling terminal; `/dev/ptsN` is a pty's slave. libmiku: `miku_open`;
libc: `open`, `openat`, `creat`.

#### 12 `close`

```
close(u64 fd) -> 0
```

Releases a descriptor. On an unredirected 0-2 it succeeds and does nothing.
On a socket it checks ownership (`EBADF`) and releases the socket when no
other descriptor names it. Closing any descriptor on a file drops **all** of
the process's record locks on that file (POSIX). Closing the last copy of a
pty master hangs the terminal up. Errors: `EBADF`. libmiku: `miku_close`;
libc: `close`.

#### 13 `seek`

```
seek(u64 fd, i64 offset, u64 whence) -> u64 new_offset
```

`whence`: 0 `SEEK_SET`, 1 `SEEK_CUR`, 2 `SEEK_END`; anything else is
`EINVAL`. Errors: `EBADF`, `EINVAL` (a negative result). libmiku: `miku_seek`,
`miku_lseek`; libc: `lseek`.

#### 14 `fsize`

```
fsize(u64 fd) -> u64 size
```

The size of the open file, as `fstat` would report it. Errors: `EBADF`.
libmiku: `miku_fsize`.

#### 28 `dup`

```
dup(u64 fd) -> u64 new_fd
```

A new descriptor for the same open file (shared offset and status flags),
in the lowest free slot, with `FD_CLOEXEC` clear. On a socket it makes an
**alias** in the socket range that names the same socket; the socket is
released when its last descriptor closes. Errors: `EBADF`, `EMFILE`. libmiku:
`miku_dup`; libc: `dup`.

#### 29 `dup2`

```
dup2(u64 old_fd, u64 new_fd) -> u64 new_fd
```

Makes `new_fd` a copy of `old_fd`, closing whatever `new_fd` held. `old_fd ==
new_fd` returns it unchanged without checking it. VFS descriptors only:
neither argument may be a socket number. Errors: `EBADF`. libmiku:
`miku_dup2`; libc: `dup2`, `dup3`.

#### 30 `truncate`

```
truncate(u64 fd, u64 length) -> 0
```

Sets the size of the file open on `fd` (it takes a descriptor, despite the
name). Errors: `EBADF`, VFS errors (`EISDIR`, `EROFS`, `EFBIG`, `ENOSPC`).
libmiku: `miku_ftruncate`; libc: `ftruncate`, `truncate` (opens the path and
calls this).

#### 34 `pipe`, 120 `pipe2`

```
pipe(u64 fds[2]) -> 0
pipe2(u64 fds[2], u64 flags) -> 0
```

A pipe; `fds[0]` is the read end and `fds[1]` the write end, written as two
**64-bit** words. `pipe2` takes `O_CLOEXEC` (0x800) and `O_NONBLOCK` (0x400);
any other bit is `EINVAL`. A pipe holds 64 KiB. Errors: `EFAULT`, `ENOMEM`,
`EMFILE`. libmiku: `miku_pipe`, `miku_pipe2`; libc: `pipe`, `pipe2` (which
translate Linux's flag values).

#### 37 `fallocate`

```
fallocate(u64 fd, u64 offset, u64 len) -> 0
```

Reserves blocks for `[offset, offset+len)` on an ext2/3/4 file. `len` 0 is
`EINVAL`; a file not on ext is `ENOSYS`; a failure in the filesystem is
`ENOSPC`. libmiku: `miku_fallocate`; libc: `posix_fallocate`.

#### 41 `fsync`

```
fsync(u64 fd) -> 0
```

Writes the file's dirty cached pages and then syncs the ext filesystem it is
on. A file that is not on ext succeeds after the page write-out. Errors:
`EBADF`, `EIO`. libmiku: `miku_fsync`; libc: `fsync`, `fdatasync` (same
call), `syncfs`.

#### 42 `punch_hole`

```
punch_hole(u64 fd, u64 offset, u64 len) -> 0
```

Zeroes the whole blocks inside `[offset, offset+len)` of an ext file (on ext4
with extents the blocks are zeroed in place; ext2/3 free them). Errors as for
`fallocate`. No library wraps it.

#### 113 `fcntl`

```
fcntl(u64 fd, u64 cmd, u64 arg) -> per command
```

| Command | Value | `arg` | Returns |
|---|---|---|---|
| `F_DUPFD` | 0 | lowest acceptable number | new descriptor, `FD_CLOEXEC` clear |
| `F_GETFD` | 1 | - | `FD_CLOEXEC` (1) or 0 |
| `F_SETFD` | 2 | `FD_CLOEXEC` or 0 | 0 |
| `F_GETFL` | 3 | - | the open flags ([4.4 `open`](#11-open)) |
| `F_SETFL` | 4 | new flags | 0; only `O_NONBLOCK` and `O_APPEND` change |
| `F_GETLK` | 5 | `struct flock *` | 0; the struct describes the first conflicting lock, or `l_type = F_UNLCK` |
| `F_SETLK` | 6 | `struct flock *` | 0, or `EAGAIN` on a conflict |
| `F_SETLKW` | 7 | `struct flock *` | 0 when granted; `EINTR` if a signal arrives first |
| `F_DUPFD_CLOEXEC` | 1030 | lowest acceptable number | new descriptor with `FD_CLOEXEC` set |

Any other command is `EINVAL`. The access mode can never be changed.

**Record locks** are POSIX advisory byte-range locks with POSIX's rules:
owned by the process, never conflicting with the same process's own locks, a
new lock replacing what the process held over the same bytes, and all of a
process's locks on a file dropped when it closes **any** descriptor on that
file, and at exit. `struct flock` is 32 bytes: `l_type` (i16 at 0:
`F_RDLCK` 0, `F_WRLCK` 1, `F_UNLCK` 2), `l_whence` (i16 at 2, `SEEK_*`),
`l_start` (i64 at 8), `l_len` (i64 at 16; 0 means to the end of the file
however it grows, a negative length covers the bytes before `l_start`),
`l_pid` (i32 at 24, written by `F_GETLK`). A read lock
needs a readable descriptor and a write lock a writable one (`EBADF`). An
unknown `l_type` or a range that overflows is `EINVAL`. `F_SETLKW` has no
deadlock detection. The lock table is `kernel/fs/vfs/lock.rs`.

**On a socket** `fcntl` is routed to the socket layer: `F_GETFL` returns
`O_RDWR` plus `O_NONBLOCK` if set; `F_SETFL` sets or clears `O_NONBLOCK`
(shared by every alias of the socket); `F_GETFD`/`F_SETFD` read and write a
per-descriptor `FD_CLOEXEC`; `F_DUPFD` and `F_DUPFD_CLOEXEC` make an alias
(the floor is ignored, `EMFILE` if the table is full); record locks are
`EINVAL`.

libmiku: `miku_fcntl`, `miku_set_nonblocking`; libc: `fcntl` (which also
answers `F_GETPATH`, 50, through `realpath` (119) of the descriptor), `flock`.

### 4.5 The file namespace

Filesystems and the VFS are in [wiki/vfs.md](wiki/vfs.md) and
[wiki/filesystems.md](wiki/filesystems.md). Every call here that takes a path
reads a path word.

#### 8 `getcwd`

```
getcwd(char *buf, u64 size) -> u64 buf
```

Writes the working directory's absolute path with a terminating NUL and
returns `buf` (the pointer, as a number). `size` < 2, or a path that does not
fit with its NUL, is `EINVAL` (Linux says `ERANGE`). libmiku: `miku_getcwd`;
libc: `getcwd`.

#### 18 `stat`, 112 `lstat`, 19 `fstat`

```
stat(const char *path, u64 path_word, struct stat *buf) -> 0
lstat(const char *path, u64 path_word, struct stat *buf) -> 0
fstat(u64 fd, struct stat *buf) -> 0
```

Fill the 64-byte MikuOS `stat` ([§5](#5-structures)). `stat` follows a final
symlink, `lstat` does not, `fstat` describes the open file. Errors: the VFS
table, `EBADF` for `fstat`.

The fields: `size` in bytes; `mode` the permission bits and file type bits;
`nlinks`; `uid`, `gid` as 16-bit ids; `kind` (0 regular, 1 directory, 2
symlink, 3 character device, 4 block device, 5 pipe, 6 socket, 7 FIFO);
`fs_type` (0 tmpfs, 1 devfs, 2 procfs, 3 ext2, 4 ext3, 5 ext4, 8 btrfs, 9
memfs); device major and minor for device nodes; `atime`, `mtime`, `ctime` in
**whole seconds** since the epoch; `inode_id`; `blocks` in 512-byte units.

`inode_id` names the file, not a cache slot: on a mounted filesystem it is the
filesystem's own inode number in the low 32 bits with the mount above it, so
every hard link of a file gives the same number and the number survives the
VFS evicting and re-reading the file; on tmpfs, `/dev` and `/proc` it is the
vnode's number, which never changes because those vnodes are never evicted.
The number alone is unique on the machine. `getdents` reports the same number.

libmiku: `miku_stat`, `miku_lstat`, `miku_fstat`; libc: `stat`, `lstat`,
`fstat`, `fstatat` (with `AT_SYMLINK_NOFOLLOW` it calls 112), `access` and
`faccessat` (which check the mode bits against the caller in the library).
`libc_miku` translates into a Linux-shaped `struct stat`.

#### 20 `mkdir`

```
mkdir(const char *path, u64 path_word, u64 mode) -> 0
```

`mode` 0 means 0755; the umask applies. Trailing slashes are ignored. Errors:
`EEXIST`, `ENOENT` (missing parent), `ENOTDIR`, `EACCES`, `EROFS`, `ENOSPC`.
libmiku: `miku_mkdir`; libc: `mkdir`, `mkdirat`.

#### 21 `rmdir`, 22 `unlink`

```
rmdir(const char *path, u64 path_word) -> 0
unlink(const char *path, u64 path_word) -> 0
```

Remove an empty directory, or a name that is not a directory. An unlinked
file stays alive while a descriptor or a mapping holds it. Errors:
`ENOTEMPTY`, `EISDIR` (`unlink` of a directory), `ENOTDIR` (`rmdir` of a
file), `EBUSY` (a mount point), `EACCES`, `EROFS`. libmiku: `miku_rmdir`,
`miku_unlink`; libc: `rmdir`, `unlink`, `unlinkat` (`AT_REMOVEDIR` picks 21),
`remove`.

#### 23 `readdir`

```
readdir(const char *path, u64 path_word, struct dirent *buf, u64 max) -> u64 count
```

Resolves `path` and writes up to `min(max, 120)` 272-byte entries from the
**start** of the directory. It has no cursor, so a directory larger than one
call cannot be read past the first 120 entries: use `getdents`. `max` 0
returns 0. libmiku: `miku_readdir`.

#### 111 `getdents`

```
getdents(u64 fd, struct dirent *buf, u64 max, u64 offset) -> u64 count
```

Reads up to `min(max, 120)` entries of the directory open on `fd` (open it
with `O_DIRECTORY`), starting at logical entry `offset`, where `.` is 0 and
`..` is 1. The caller adds the returned count to its offset and stops when a
call returns 0. Errors: `EBADF`, `ENOTDIR`, `EFAULT` (the buffer must hold
`max * 272` bytes).

The entry ([§5](#5-structures)) holds the name (NUL-terminated, up to 255
bytes), the 64-bit inode number (the one `stat` reports), the kind (as in
`stat`) and the name's length.

**The offset is a position, not a cookie.** It counts entries in the
directory as it is at the time of the call. Adding or removing entries between
two calls moves what the offset points at, so a walk that changes the
directory must either read the whole listing first (what `std::fs::read_dir`
does) or start again from 0 after each change. The order is stable while the
directory is not modified. libmiku: `miku_getdents`, `miku_dir_open`,
`miku_dir_close`; libc: `opendir`, `readdir`, `fdopendir`, `rewinddir`,
`seekdir`, `telldir`.

#### 24 `rename`, 25 `link`

```
rename(const char *old, u64 old_word, const char *new, u64 new_word) -> 0
link(const char *old, u64 old_word, const char *new, u64 new_word) -> 0
```

Each path has its own word and so its own base. `rename` moves a name,
replacing an existing target as POSIX does; `link` adds a hard link to an
existing file. Errors: `EXDEV` (different filesystems), `EEXIST` (`link`
target exists), `ENOTEMPTY` (replacing a non-empty directory), `EISDIR`,
`ENOTDIR`, `EMLINK`, `EACCES`, `EROFS`, `EINVAL` (moving a directory into
itself). libmiku: `miku_rename`, `miku_link`; libc: `rename`, `renameat`,
`link`, `linkat`.

#### 26 `chmod`

```
chmod(const char *path, u64 path_word, u64 mode) -> 0
```

Sets the permission bits (the low 12 bits of `mode`). The caller must own the
file or hold `CAP_FOWNER`. An empty path with a descriptor base is `fchmod`.
Errors: `EPERM`/`EACCES`, `EROFS`, VFS lookup errors. The setuid and setgid
bits are stored and shown, and **never honoured** by `exec`. libmiku:
`miku_chmod`, `miku_fchmod`; libc: `chmod`, `fchmod`, `fchmodat`.

#### 27 `chown`

```
chown(const char *path, u64 path_word, u64 uid, u64 gid) -> 0
```

Changes the owner and group; `0xFFFF` leaves either unchanged. Giving a file
away needs `CAP_CHOWN`. An empty path with a descriptor base is `fchown`.
libmiku: `miku_chown`, `miku_fchown`; libc: `chown`, `fchown`, `fchownat`.

#### 32 `symlink`, 33 `readlink`

```
symlink(const char *target, u64 target_len, const char *link, u64 link_word) -> 0
readlink(const char *path, u64 path_word, char *buf, u64 size) -> u64 length
```

`symlink` stores `target` as text (its length is a plain length, not a path
word) under the name `link`. `readlink` copies at most `size` bytes of the
link's text and adds a NUL only if there is room; it returns the number of
bytes copied. `size` 0 is `EINVAL`; a path that is not a symlink is `EINVAL`.
libmiku: `miku_symlink`, `miku_readlink`; libc: `symlink`, `symlinkat`,
`readlink`, `readlinkat`.

#### 35 `chdir`

```
chdir(const char *path, u64 path_word) -> 0
```

Changes the process's working directory. `ENOTDIR` if the path is not a
directory. An empty path with a descriptor base is `fchdir`. libmiku:
`miku_chdir`; libc: `chdir`, `fchdir`.

#### 36 `statfs`

```
statfs(const char *path, u64 path_word, struct statfs *buf) -> 0
```

Describes the filesystem `path` is on: a 48-byte struct ([§5](#5-structures))
with the filesystem's magic number (ext 0xEF53, tmpfs 0x01021994, devfs
0x1373, procfs 0x9FA0, btrfs 0x9123683E, memfs 0x4D454D46), block size, total
and free blocks, total and free inodes, longest name and flags. An empty path
with a descriptor base asks about the file's filesystem. libmiku:
`miku_statfs`; libc: `statfs`, `fstatfs`, `statvfs`, `fstatvfs`.

#### 40 `utimensat`

```
utimensat(const char *path, u64 path_word, const struct timespec times[2], u64 flags) -> 0
```

Sets the access time (`times[0]`) and modification time (`times[1]`). `times`
0 sets both to now. A `tv_nsec` of `UTIME_NOW` ((1 << 30) - 1) means now and
`UTIME_OMIT` ((1 << 30) - 2) leaves that time alone. `flags` may hold
`AT_SYMLINK_NOFOLLOW` (0x100), anything else is `EINVAL`. An empty path with a
descriptor base is `futimens`. Times are stored in whole seconds; a negative
`tv_sec` becomes 0. Works on every filesystem that keeps times (tmpfs and ext
alike). libmiku: `miku_utimensat`; libc: `utimensat`, `futimens`, `utimes`,
`lutimes`.

#### 119 `realpath`

```
realpath(const char *path, u64 path_word, char *buf, u64 size) -> u64 length
```

Resolves `path`, following every symlink, and writes the absolute path it
names with a NUL; returns its length without the NUL. An empty path with a
descriptor base names the open file itself, which is how `fcntl(F_GETPATH)`
and `/proc/self/fd` style lookups are answered. `ERANGE` if the name and its
NUL do not fit in `size`. libmiku: `miku_realpath`, `miku_fd_path`; libc:
`realpath`, `fcntl(F_GETPATH)`.

### 4.6 Extended attributes (38, 39, 81, 82)

```
getxattr(u64 ino, const char *name, u64 name_len, void *buf) -> u64 length
setxattr(u64 ino, const char *name, const void *value, u64 (name_len << 16) | value_len) -> 0
listxattr(u64 ino, char *buf, u64 len) -> u64 length
removexattr(u64 ino, const char *name, u64 name_len) -> 0
```

**These take an ext inode number, not a path**, and work only on the active
ext filesystem; they reach only the `user.` namespace (the name is given
without the prefix). There is no library wrapper. Limits: names 1 to 64
bytes, values at most 256 bytes (`getxattr` always needs a 256-byte buffer),
`listxattr`'s buffer at most 4096 bytes.

| errno | Cause |
|---|---|
| `EINVAL` | `ino` 0, a name length out of range, a value over 256 bytes, a name that is not UTF-8 |
| `EACCES` | the caller may not read (get, list) or write (set, remove) the inode, by its owner and mode |
| `ENOENT` | no such attribute (get, remove), or the filesystem refused |
| `ERANGE` | `listxattr`: the names do not fit in `len` |
| `ENOSYS` | no ext filesystem is mounted |

`listxattr` with `len` 0 returns the size needed; the names come back
NUL-separated. The permission check happens inside the call, because a raw
inode number has passed no path permission check on its way in. These calls
raise no inotify events.

### 4.7 Watching files: inotify (116-118)

Linux's inotify, with Linux's semantics, flags and record layout, so code
written against `<sys/inotify.h>` (and the `notify` crate's inotify backend)
runs unchanged.

```
inotify_init1(u64 flags) -> u64 fd
inotify_add_watch(u64 fd, const char *path, u64 path_word, u64 mask) -> u64 wd
inotify_rm_watch(u64 fd, u64 wd) -> 0
```

- `inotify_init1` takes `O_NONBLOCK` (0x400) and `O_CLOEXEC` (0x800) in
  MikuOS's values; anything else is `EINVAL`. `EMFILE`/`ENOSPC` when the
  table of 128 instances on the machine is full.
- `inotify_add_watch` resolves the path (following a final symlink unless
  `IN_DONT_FOLLOW`; `IN_ONLYDIR` refuses a non-directory) and returns a watch
  descriptor. One watch per (instance, file): adding one again returns the
  same `wd` and replaces the mask, or ORs into it with `IN_MASK_ADD`;
  `IN_MASK_CREATE` makes a second add fail with `EEXIST`. A mask with no event
  bits, or with both `IN_MASK_ADD` and `IN_MASK_CREATE`, is `EINVAL`; more than
  8192 watches on an instance is `ENOSPC`; an `fd` that is not an instance is
  `EINVAL`.
- `inotify_rm_watch` removes a watch and queues `IN_IGNORED`; an unknown `wd`
  is `EINVAL`.
- `read` returns whole records: `struct inotify_event { i32 wd; u32 mask; u32
  cookie; u32 len; char name[len]; }`, the name NUL-padded to a multiple of 16
  bytes. A buffer too small for the next record is `EINVAL`; an empty queue
  blocks (or `EAGAIN`, or `EINTR` on a signal). `write` is `EINVAL`. `poll`
  reports the descriptor readable when an event is queued.

Raised: `IN_CREATE`, `IN_DELETE`, `IN_MOVED_FROM`/`IN_MOVED_TO` (one cookie
per rename), `IN_MODIFY`, `IN_ATTRIB`, `IN_CLOSE_WRITE`, `IN_CLOSE_NOWRITE`,
`IN_DELETE_SELF`, `IN_MOVE_SELF`, `IN_IGNORED`, `IN_Q_OVERFLOW`, with
`IN_ISDIR` when the subject is a directory. Accepted and kept: `IN_ONESHOT`,
`IN_EXCL_UNLINK`. **Not raised**: `IN_ACCESS`, `IN_OPEN`, `IN_UNMOUNT`, and
nothing for changes made through the inode-numbered xattr calls. An event
identical to the last one queued is merged into it. At most 4096 events wait
per instance; past that one `IN_Q_OVERFLOW` is queued. The bit values are in
[§6](#6-constants-ioctls-and-fcntl-commands). libc: `inotify_init`,
`inotify_init1`, `inotify_add_watch`, `inotify_rm_watch` (which translate
Linux's `IN_NONBLOCK`/`IN_CLOEXEC`); `libc_miku` also answers
`kcmp(KCMP_FILE)` for the caller's own descriptors, which the `inotify` crate
needs. The design is in `dev/fs-architecture.md`, "Watching".

### 4.8 Time and entropy

The clock every wait is measured on is the TSC, calibrated at boot; the same
clock `CLOCK_MONOTONIC` reads. A machine whose TSC could not be calibrated,
or one booted with `tickonly`, keeps the periodic 250 Hz timer, and every
deadline then ends on a tick. Time and the scheduler are in
[wiki/scheduler.md](wiki/scheduler.md).

#### 16 `sleep`

```
sleep(u64 ticks) -> 0
```

Sleeps until the `ticks`-th timer tick from now. A tick is 4 ms (250 Hz).
`ticks` 0 yields the CPU; values above 100 000 are clamped. Never fails.
libmiku: `miku_sleep`, `miku_yield` (0 ticks); libc: `sched_yield`.

#### 122 `sleep_ns`

```
sleep_ns(u64 ns) -> 0
```

Sleeps that many nanoseconds, kept to the timer's precision (a few
microseconds, not a tick). 0 yields; more than a day is clamped to a day.
libmiku: `miku_sleep_ns`, `miku_sleep_ms`; libc: `nanosleep` (never writes
`rem`), `usleep`, `sleep`; Rust's `std::thread::sleep`.

#### 17 `uptime`

```
uptime() -> u64 ticks
```

Timer ticks since boot (4 ms each). libmiku: `miku_uptime`,
`miku_uptime_ms`.

#### 70 `clock_gettime`

```
struct timespec { i64 tv_sec; i64 tv_nsec; };     /* 16 bytes */
clock_gettime(u64 clock, struct timespec *ts) -> 0
```

`clock` 0 `CLOCK_REALTIME` (the wall clock) or 1 `CLOCK_MONOTONIC` (time
since boot); anything else is `EINVAL`. **`CLOCK_REALTIME` is `EINVAL` until
the wall clock has been set** (from the RTC at boot, NTP, or 79). The vDSO
answers the same question without a system call ([§7.7](#77-the-vdso)), and
the library uses it when present. libmiku: `miku_clock_gettime`,
`miku_realtime`; libc: `clock_gettime` (passes the clock id through, so only
0 and 1 work), `gettimeofday`, `time`.

#### 79 `clock_settime`

```
clock_settime(u64 clock, const struct timespec *ts) -> 0
```

Sets the wall clock to `ts->tv_sec` (the nanoseconds are ignored). Only
`CLOCK_REALTIME` (`EINVAL` otherwise), only with `CAP_SYS_TIME` (`EPERM`), and
a negative time is `EINVAL`. `ntpdate` is the program that calls it. libmiku:
`miku_set_realtime`; libc: `clock_settime`.

#### 78 `getrandom`

```
getrandom(void *buf, u64 len) -> u64 len
```

Fills `buf` with unpredictable bytes from the kernel's generator (RDRAND where
the CPU has it, a TSC-derived fallback where it does not). Never blocks and never returns short;
`len` above 1024 is `EINVAL`, 0 returns 0. This, not `miku_rand` (a xorshift
seeded from uptime and pid), is the source for keys, nonces and DNS query ids.
libmiku: `miku_getrandom`; libc: `getrandom` (the flags are ignored).

### 4.9 Identity and capabilities (47-55, 86-91)

A process's identity is one value, changed under one lock so a concurrent
permission check never sees half of a change: real, effective and saved user
ids (`uid`, `euid`, `suid`), the same three group ids, up to 16
supplementary groups, three capability sets (permitted, effective, bounding)
and the umask. Ids are 16-bit; `0xFFFE` is the highest valid id and every id
above it is refused with `EINVAL` rather than truncated (`setuid(0x10000)`
once stored uid 0). The model and its policy are in
[MikuOS_Security.md](MikuOS_Security.md).

`euid == 0` grants nothing by itself: each privileged operation checks the
capability it needs ([§2.7](#27-capabilities)). Every call below returns
`ESRCH` if the calling process vanished mid-call, which cannot happen to a
live caller.

#### 47 `umask`

```
umask(u64 mask) -> u64 old_mask
```

Sets the file creation mask to `mask & 0777` and returns the previous one.
Never fails. libmiku: `miku_umask`; libc: `umask`.

#### 48 `getuid`, 49 `getgid`, 50 `geteuid`, 51 `getegid`

Return the real or effective user or group id. Never fail. libmiku:
`miku_getuid` and so on; libc: `getuid`, `getgid`, `geteuid`, `getegid`.

#### 52 `setuid`, 53 `setgid`

```
setuid(u64 uid) -> 0
setgid(u64 gid) -> 0
```

With `CAP_SETUID` (`CAP_SETGID` for `setgid`): all three ids become `uid`,
and **for `setuid` to any id other than 0, every permitted and effective
capability is dropped**. Without it: only the effective id changes, and only
to an id the process already holds as real, effective or saved; anything else
is `EPERM`. libmiku: `miku_setuid`, `miku_setgid`. libc exports neither.

#### 54 `seteuid`, 55 `setegid`

```
seteuid(u64 euid) -> 0
setegid(u64 egid) -> 0
```

Change only the effective id: allowed with the capability or to an id already
held; `EPERM` otherwise. Capabilities are not touched. libmiku:
`miku_seteuid`, `miku_setegid`.

#### 86 `setresuid`, 87 `setresgid`

```
setresuid(u64 ruid, u64 euid, u64 suid) -> 0
setresgid(u64 rgid, u64 egid, u64 sgid) -> 0
```

All three ids in one step, so there is no moment at which the process is half
one identity and half another. `0xFFFF` (or `-1`) leaves a field alone.
Without the capability each new id must be one already held (`EPERM`).
`setresuid` with `CAP_SETUID` drops every permitted and effective capability
when **all three** resulting ids are non-zero. libmiku: `miku_setresuid`,
`miku_setresgid`.

#### 88 `setgroups`, 89 `getgroups`

```
setgroups(const u16 *list, u64 count) -> 0
getgroups(u16 *list, u64 max) -> u64 count
```

The supplementary groups, as **16-bit** ids. `setgroups` needs `CAP_SETGID`
(`EPERM`), takes at most 16 (`EINVAL`) and refuses an id above `0xFFFE`.
`getgroups` with `max` 0 returns the count; a `max` smaller than the count is
`EINVAL`. libmiku: `miku_setgroups`, `miku_getgroups`; libc: `setgroups`,
`getgroups`, `initgroups`, `getgrouplist` (which widen to `gid_t`).

#### 90 `capget`, 91 `capset`

```
capget(u64 out[3]) -> 0                 /* permitted, effective, bounding */
capset(u64 permitted, u64 effective, u64 bounding) -> 0
```

`capget` writes the three sets as bit masks (bit n = capability n). `capset`
**only ever narrows**: the new permitted set must be a subset of the current
permitted set, the new bounding set a subset of the current bounding set, and
the new effective set a subset of the new permitted set (`EPERM`
otherwise). Bits above 15 are ignored. After the change permitted and
effective are intersected with bounding. Narrowing the bounding set confines
every descendant, because it survives `fork` and `exec`. libmiku:
`miku_capget`, `miku_capset`.

**On `exec`**: `permitted &= bounding; effective = permitted`, and nothing
else. There is no setuid bit, no setgid bit and no file capability; a mode
bit on a file never raises anybody. Privilege is handed to a child by a
privileged parent (mikuD for services, `mikudod` for `mikudo`).

### 4.10 Sessions and power (92, 93)

#### 92 `session`

```
session(u64 op, u64 arg, void *ptr, u64 len) -> per op
```

Who is logged in, on which seat (`kernel/process/session.rs`).

| op | Name | `arg` | `ptr`, `len` | Returns | Needs |
|---|---|---|---|---|---|
| 0 | `SESSION_OPEN` | leader pid, or 0 | `SessionRequest`, its size (40) | new session id | `CAP_SETUID` |
| 1 | `SESSION_CLOSE` | session id, or 0 for the session the caller leads | - | 0 | `CAP_SETUID` |
| 2 | `SESSION_LIST` | - | array of `SessionInfo`, its length in entries | entries written | - |
| 3 | `SESSION_CURRENT` | pid, or 0 for the caller | - | that process's session id, 0 for none | - |

```c
struct SessionRequest {            /* 40 bytes */
    uint16_t uid, gid;             /* 0, 2 */
    uint8_t  kind;                 /* 4: 0 console, 1 graphical */
    uint8_t  seat;                 /* 5 */
    uint8_t  ngroups, _pad;        /* 6, 7 */
    uint16_t groups[16];           /* 8 */
};
struct SessionInfo {               /* 32 bytes */
    uint32_t id;                   /* 0 */
    uint16_t uid, gid;             /* 4, 6 */
    uint8_t  kind, seat;           /* 8, 9 */
    uint8_t  _pad[2];              /* 10; 4 more bytes of padding follow */
    uint64_t leader;               /* 16 */
    uint64_t opened_tick;          /* 24: timer tick it was opened at */
};
```

Errors: `EPERM` (no capability), `EINVAL` (wrong `len`, an id above `0xFFFE`,
more than 16 groups, unknown `kind`, an unknown op, or the session table
refused the request), `ESRCH` (close of a session that does not exist, or
`arg` 0 from a process that leads none), `EFAULT`.

`SESSION_OPEN` authenticates nothing: it publishes a decision `mikudod` made
after checking `/etc/shadow`. Opening a session on a seat moves `CAP_MAY_PRIV`
(permitted, effective and bounding) to the session's leader, taking it from
whoever held it on that seat (the greeter), and names the session in the
kernel console. `SESSION_CURRENT` walks the process's parent chain to the
process that leads a session. libmiku: `miku_session`. The login flow is in
[wiki/system.md](wiki/system.md).

#### 93 `power`

```
power(u64 action) -> does not return on success
```

`action` 0 powers off, 1 reboots; anything else is `EINVAL`. Needs
`CAP_SYS_BOOT` (`EPERM`). With mikuD running, the request goes through it
(services are stopped first); otherwise the kernel acts at once. libmiku:
`miku_power`. No program in `/bin` calls it today; the kernel console's
`poweroff` and `reboot` commands (reached through `mikudo`) do the same thing
from ring 0.

### 4.11 Signals (68, 69, 100, 115)

Signals are split between the kernel and `sys_miku.so`. The kernel keeps, per
process, a **disposition** for each of signals 1 to 31 (default, ignore or
catch, two bits each), a pending bit set, one **entry point** for the
userspace dispatcher, an alternate stack per task and an "in a handler" flag.
The library keeps the handler table, `sa_mask` and the process-wide blocked
mask. Delivery, frames and the rules are described in
[wiki/processes.md](wiki/processes.md#signals) and
`dev/signal-architecture.md`.

How a signal is sent (`kill`, a terminal key, `SIGPIPE`, `SIGCHLD`):

1. It is addressed to the process (the first thread), never to a thread.
2. `SIGKILL` kills at once.
3. Otherwise, unless the signal is `SIGKILL` or `SIGSTOP`: an ignored signal
   is dropped; a caught one, when an entry point is registered, sets its
   pending bit.
4. Anything else takes its default: stop (`SIGSTOP`, `SIGTSTP`, `SIGTTIN`,
   `SIGTTOU`), continue (`SIGCONT`), ignore (`SIGCHLD`, `SIGWINCH`, 23
   `SIGURG`) or kill (everything else).

A pending caught signal is delivered at the end of the next system call the
process makes, one handler at a time (no new frame is pushed while a handler
is running). A CPU-bound loop that makes no system call does not see its
handler run until it makes one. A CPU fault (`#PF`, `#GP`, `#UD`, `#DE`) is
delivered at the faulting instruction instead.

#### 68 `sigentry`

```
sigentry(u64 entry) -> 0
```

Registers the address of the userspace dispatcher, the stub every handler is
entered through. `entry` 0 or above the user half is `EINVAL`. One per
process; `fork` copies it, `exec` clears it. `sys_miku.so` registers its own
stub the first time a handler is installed (`miku_signal_init`); a program
does not call this itself.

The stub is entered with `rsp` pointing at a 1072-byte frame
([§5](#5-structures)): Linux's `siginfo_t` (128 bytes), Linux's
`ucontext_t` up to `uc_sigmask` (424 bytes), 8 bytes of padding and a
512-byte area at offset 560 that the stub fills with `fxsave64` and restores
itself. It calls the handler as `handler(sig, siginfo_t *, ucontext_t *)` and
leaves through `sigreturn`. The frame is written below the interrupted `rsp`
past the 128-byte red zone, or on the alternate stack for a handler installed
with `SA_ONSTACK`. If the frame cannot be written, the process is killed by
the signal.

#### 69 `sigreturn`

```
sigreturn(u64 frame) -> does not return
```

Leaves a handler. The entry stub sends number 69 to a separate path before
saving anything, so it never reaches the dispatcher. It restores **every**
general register from the frame's `gregs`, including any the handler changed,
and returns through `iretq`, so `rcx` and `r11` come back too. `rip` and
`rsp` must be in the user half, and only the status bits of `rflags` are
taken (with interrupts forced on); a frame that fails this kills the process
with `SIGSEGV`. The frame's `uc_flags` say whether the task was already in a
handler when the frame was written, and that state is restored.

#### 100 `sigdisp`

```
sigdisp(u64 sig, u64 disp) -> u64 previous
```

| `disp` | Meaning |
|---|---|
| 0 `DISP_DEFAULT` | the default action |
| 1 `DISP_IGNORE` | drop it |
| 2 `DISP_CATCH` | queue it for the dispatcher |
| 3 `DISP_QUERY` | change nothing, answer with the current value |
| `\| 4` `DISP_ONSTACK` | with 2: run the handler on the alternate stack (`SA_ONSTACK`) |

Returns the previous disposition, with bit 2 set if the handler had
`SA_ONSTACK`. `sig` 0 or 32 and above, a `disp` above 3, or any change to
`SIGKILL` or `SIGSTOP`, is `EINVAL`. libmiku: `miku_sigdisp`; libc's
`sigaction` and `signal` call it along with the library's own handler table.
`SA_RESTART` is accepted by libc and not honoured
([§2.6](#26-blocking-signals-and-eintr)); `SA_NOCLDSTOP` is accepted and
`SIGCHLD` is never sent for a stop anyway; `SA_SIGINFO`, `SA_NODEFER` and
`SA_RESETHAND` are implemented in the library.

#### 115 `sigaltstack`

```
typedef struct { void *ss_sp; i32 ss_flags; u64 ss_size; } stack_t;   /* 24 bytes: sp 0, flags 8, size 16 */
sigaltstack(const stack_t *new, stack_t *old) -> 0
```

The calling task's alternate signal stack. `old`, when given, receives the
current one with `SS_ONSTACK` (1) set if the task is running on it, or
`SS_DISABLE` (2) if there is none. `new`, when given, installs one or, with
`SS_DISABLE`, removes it.

| errno | Cause |
|---|---|
| `EPERM` | changing it while running on it |
| `EINVAL` | a flag other than `SS_ONSTACK`/`SS_DISABLE`; `ss_sp` 0; the range leaves the user half |
| `ENOMEM` | `ss_size` below 2048 (`MINSIGSTKSZ`) |

`exec` removes it. libc: `sigaltstack`.

**Signals the kernel uses** (Linux's numbers):

| Signal | Nr | Default | Sent by the kernel when |
|---|---|---|---|
| `SIGHUP` | 1 | kill | the terminal hangs up (to its foreground job) |
| `SIGINT` | 2 | kill | Ctrl-C (`VINTR`), to the foreground job |
| `SIGQUIT` | 3 | kill | Ctrl-\\ (`VQUIT`) |
| `SIGILL` | 4 | kill | `#UD` in ring 3 (`si_code` `ILL_ILLOPN` 2) |
| `SIGFPE` | 8 | kill | `#DE` in ring 3 (`FPE_INTDIV` 1) |
| `SIGKILL` | 9 | kill, uncatchable | - |
| `SIGSEGV` | 11 | kill | `#PF` (`SEGV_MAPERR` 1, `SEGV_ACCERR` 2, address in `si_addr`) or `#GP` (`SI_KERNEL` 0x80) in ring 3 |
| `SIGPIPE` | 13 | kill | a write to a pipe with no reader |
| `SIGTERM` | 15 | kill | mikuD stopping a service (catchable) |
| `SIGCHLD` | 17 | ignore | a child exits or is killed |
| `SIGCONT` | 18 | continue | - |
| `SIGSTOP` | 19 | stop, uncatchable | - |
| `SIGTSTP` | 20 | stop | Ctrl-Z (`VSUSP`) |
| `SIGTTIN` | 21 | stop | a background job reads its terminal |
| `SIGTTOU` | 22 | stop | a background job changes its terminal's settings, or writes with `tostop` |
| `SIGURG` | 23 | ignore | - |
| `SIGWINCH` | 28 | ignore | the terminal is resized |

Every other number up to 31 kills by default. A process killed by a signal
reports `WTERMSIG` = that signal.

### 4.12 Threads and the futex (106-110)

A thread is a task that shares its creator's address space, descriptors,
working directory, credential, terminal and process group, and has its own
kernel stack, registers, `FS_BASE`, FPU state and alternate signal stack.
[wiki/processes.md](wiki/processes.md#threads) has the model.

#### 106 `thread_create`

```
struct ThreadArgs {        /* 64 bytes */
    u64 entry;             /* 0: where the thread starts */
    u64 stack;             /* 8: its initial rsp, 16-byte aligned */
    u64 arg;               /* 16: passed in rdi */
    u64 tls;               /* 24: its FS_BASE, or 0 */
    u64 flags;             /* 32: THREAD_STACK_OWNED (1) or 0 */
    u64 stack_base;        /* 40: with THREAD_STACK_OWNED, the stack mapping */
    u64 stack_len;         /* 48 */
    u64 reserved;          /* 56: must be 0 */
};
thread_create(const struct ThreadArgs *args) -> u64 tid
```

| errno | Cause |
|---|---|
| `EPERM` | called from a kernel thread |
| `EFAULT` | `args` unreadable; `entry`, `stack` or a non-zero `tls` outside the user half; the owned stack range outside it |
| `EINVAL` | `reserved` non-zero; an unknown flag; `stack` not 16-byte aligned; with `THREAD_STACK_OWNED`, `stack_base` not page-aligned, `stack_len` 0, or `stack` outside the range |
| `ENOMEM` | no task could be made |
| `EAGAIN` | the process began exiting while the thread was being made |

The new task starts at `entry` with `arg` in `rdi`, `rsp` = `stack` and
**nothing on the stack**, so `entry` must never return; libmiku's trampoline
calls `thread_exit` when the function does. With `THREAD_STACK_OWNED` the
kernel unmaps `[stack_base, stack_base + stack_len)` when the thread dies,
which is how a detached thread's stack is freed (the thread cannot unmap the
stack it is standing on). The thread joins its process's group and terminal.
libmiku: `miku_thread_spawn` and `miku_thread_spawn_owned`
(`lib/runtime/libmiku/core/thread.rs`, which builds the TLS block and a guard page); libc:
`pthread_create`; Rust: `std::thread::spawn`.

#### 107 `thread_exit`

```
thread_exit(u64 code) -> does not return
```

Ends the calling thread. **Called from the process's first thread it ends
the whole process** (as `exit`); POSIX's `pthread_exit` from `main` leaving the
other threads running is not provided. libmiku: `miku_thread_exit`; libc:
`pthread_exit`.

#### 108 `gettid`, 109 `sched_yield`

`gettid` returns the calling task's id (the process id in the first thread).
`sched_yield` gives up the CPU and returns 0. Neither fails. libmiku:
`miku_gettid`, `miku_sched_yield`.

#### 110 `futex`

```
futex(u32 *uaddr, u64 op, u64 val, const struct timespec *timeout) -> 0 | woken
```

| op | Name | Effect | Returns |
|---|---|---|---|
| 0 | `FUTEX_WAIT` | if `*uaddr == (u32)val`, sleep until woken or `timeout` (relative; 0 = none) | 0 when the sleep ended; `EAGAIN` at once if the word differs; `ETIMEDOUT` |
| 1 | `FUTEX_WAKE` | wake up to `val` waiters | how many were woken |

Other errors: `EINVAL` (`uaddr` not 4-byte aligned, an unknown op, a timeout
with negative fields or `tv_nsec` >= 10^9), `EFAULT` (`uaddr` or the timeout
unreadable). **A return of 0 from WAIT does not mean the word changed**: every
caller re-checks in a loop, as on any system. A futex wait is not interrupted
by signals.

The key is the address space and the virtual address, which makes it a
private futex: right for threads, and immune to a copy-on-write fault moving
the word. A word in a page mapped `MAP_SHARED` from a **tmpfs** file is keyed
on the file's frame instead, so a wait in one process is woken from another
and `PTHREAD_PROCESS_SHARED` mutexes work there; in an ext file mapping it is
still per process. There is no `FUTEX_REQUEUE`, no priority inheritance and no
bitset ops. libmiku: `miku_futex_wait`, `miku_futex_wake`; `std`'s `Mutex`,
`Condvar` and `RwLock` and `libc_miku`'s pthread primitives are built on it.

### 4.13 Terminals and jobs (99, 101, 102)

A terminal is a character grid and a byte stream. A terminal's id (`TtyId`,
a `u32`) carries the slot in its low 8 bits and a generation above it, so an
id kept across a hangup names nothing rather than the next terminal in that
slot; 0 always means "no terminal", and the console is `0x0100`. There are 8
slots, the console included, so at most 7 pseudo-terminals exist at once. The
terminal subsystem is described in
[wiki/input-and-terminals.md](wiki/input-and-terminals.md).

**Making a terminal.** `open("/dev/ptmx", O_RDWR)` creates a pseudo-terminal
and returns its master (open it close-on-exec: a child holding the master
keeps the terminal alive for ever). `ioctl(master, TIOCGPTN, &id)` says which
terminal it is. Reading the master returns what programs on the terminal
wrote; writing it is their input, through the line discipline. The terminal
hangs up when the last copy of the master closes, its holder's death included,
and the foreground job gets `SIGHUP`. `/dev/ptsN` (flat, N = the id's slot)
is the slave; only a process `TTY_ATTACH` would admit may open it, and
`ioctl(slave, TIOCSCTTY, 0)` makes it the controlling terminal. libc's
`posix_openpt`, `grantpt`, `unlockpt`, `ptsname`, `openpty` and `login_tty`
are built from these (`lib/runtime/libmiku/libc/pty.rs`).

#### 99 `tty`

```
tty(u64 op, u64 arg) -> per op
```

| op | Name | `arg` | Effect and result |
|---|---|---|---|
| 1 | `TTY_ATTACH` | terminal id | make it the caller's controlling terminal, put the caller in a new process group of its own and make that group the terminal's foreground job; 0 |
| 3 | `TTY_SELF` | - | the caller's controlling terminal id, 0 for none |
| 4 | `TTY_SETFG` | pgid, 0 = the caller's own | `tcsetpgrp`: hand the terminal's keys to that job; 0 |
| 5 | `TTY_GETFG` | - | `tcgetpgrp`: the foreground pgid |
| 6 | `TTY_CLEAR` | - | blank the caller's terminal and home the cursor; 0 |
| 7 | `TTY_OF_PID` | pid | that process's controlling terminal id |

Ops 0 and 2 (once `TTY_OPEN` and `TTY_CLOSE`) are retired and answer
`EINVAL`, like any unknown op. For ops 4-6 a caller with no controlling
terminal acts on the console.

| errno | Cause |
|---|---|
| `EBADF` | the terminal does not exist (attach, setfg, getfg, clear); `TTY_OF_PID` of pid 0 or a missing process |
| `EPERM` | attach: the caller is not the terminal's master holder, a descendant of it, or `CAP_SYS_ADMIN`; setfg: the target is not a group leader or is on another terminal; `TTY_OF_PID`: the target is neither the caller, its child, nor is the caller `CAP_SYS_ADMIN` |
| `EINVAL` | setfg with no group to name; unknown op |

There is no input-injection request (no `TIOCSTI`). libmiku: `miku_tty`;
libc: `tcsetpgrp`, `tcgetpgrp`, `ttyname_r`.

**Jobs.** A process group is the unit a terminal's keys belong to. A new
process leads a group of its own; `fork` puts the child in its parent's;
`exec` keeps it. A shell starting a job calls `setpgid` on both sides of the
`fork`, hands the terminal over with `TTY_SETFG`, waits, and takes it back
with `TTY_SETFG` 0. Ctrl-C, Ctrl-\\ and Ctrl-Z signal the foreground group;
`kill(-pgid, sig)` signals a whole group.

#### 101 `setpgid`

```
setpgid(u64 pid, u64 pgid) -> 0
```

`pid` 0 is the caller; `pgid` 0 means a new group led by `pid`. The target
must be the caller or its child (`EPERM`). Joining an existing group needs
that group's leader to exist, to lead its own group and to be on the same
controlling terminal (`EPERM` otherwise). `ESRCH` if the target does not exist.
libmiku: `miku_setpgid`; libc: `setpgid`, and `setsid` (as `setpgid(0, 0)`:
MikuOS has jobs but no sessions in the POSIX sense, and nothing drops a
controlling terminal).

#### 102 `getpgid`

```
getpgid(u64 pid) -> u64 pgid
```

`pid` 0 is the caller; `ESRCH` for a missing process. libmiku:
`miku_getpgid`; libc: `getpgid`, `getpgrp`.

### 4.14 Sockets

The network stack is described in [wiki/network.md](wiki/network.md). Socket
descriptors are numbered from 4096 in one table for the whole machine (256
sockets, at most 160 for one account) and work with `read`, `write`,
`close`, `poll`, `dup` and `fcntl`. Every call checks that the socket belongs
to the caller's process (`EBADF` otherwise); **a socket does not cross
`fork`**: the child cannot use its parent's socket numbers. A socket survives
`exec` unless it is close-on-exec, and all of a process's sockets are closed
when it exits.

Blocking calls give up after 7500 ticks (30 s), so a dead peer cannot wedge a
process for ever. A non-blocking socket (`fcntl(F_SETFL, O_NONBLOCK)`)
answers `EAGAIN` where it would wait.

Addresses are Linux's: `sockaddr_in` is 16 bytes (family `AF_INET` = 2 as a
little-endian `u16`, port big-endian at 2, address at 4, 8 bytes of zero);
`sockaddr_un` is a family `AF_UNIX` = 1 and up to 108 bytes of path. The
family in the first two bytes decides which one a call is given.

#### 56 `socket`

```
socket(u64 domain, u64 type, u64 protocol) -> u64 fd
```

| domain | type | protocol | Result |
|---|---|---|---|
| `AF_INET` 2 | `SOCK_STREAM` 1 | 0 or `IPPROTO_TCP` 6 | TCP |
| `AF_INET` 2 | `SOCK_DGRAM` 2 | 0 or `IPPROTO_UDP` 17 | UDP |
| `AF_INET` 2 | `SOCK_RAW` 3 | `IPPROTO_ICMP` 1 | ICMP, unprivileged (below) |
| `AF_UNIX` 1 | `SOCK_STREAM` 1 | ignored | local stream socket |
| `AF_MAYLAND` 42 | ignored | ignored | a connection to the display server |

Errors: `EAFNOSUPPORT` (another domain), `EPROTONOSUPPORT` (another type or
protocol, including `AF_UNIX` datagrams and raw IP of any other protocol),
`EMFILE` (table, account or display-connection share full). `SOCK_NONBLOCK`
and `SOCK_CLOEXEC` are not kernel flags; `libc_miku`'s `socket` strips them
and applies them with `fcntl`. libmiku (`net_miku.so`): `miku_socket` (TCP),
`miku_socket_udp`, `miku_socket_icmp`, `miku_socket_unix`; libc: `socket`.

#### 57 `connect`

```
connect(u64 fd, const void *addr, u64 addrlen) -> 0
```

TCP: starts the handshake to the `sockaddr_in`. A blocking socket waits for
it; a non-blocking one sends the SYN and returns `EINPROGRESS`, and whoever
looks at the socket next (`poll`, `connect`, `send`, `recv`, `getsockopt`)
moves the handshake along; `poll` reports it writable when established, or
writable with `POLLHUP` when it failed, and the reason waits in `SO_ERROR`. A
second `connect` answers `EALREADY` while it is in progress, `EISCONN` once
done, or the failure. ARP for an uncached neighbour is the one wait a
non-blocking connect still makes. UDP: sets the default destination.

`AF_UNIX`: connects to the socket bound at that path. A blocking connect
waits until the server accepts and fails at once if nothing listens there; a
non-blocking one queues in the listener's backlog and returns
`EINPROGRESS`, completing when the server accepts (`ECONNREFUSED` in
`SO_ERROR` if the listener closes first).

Errors: `EBADF`, `EINVAL` (`addrlen` < 16 for `AF_INET`, an empty path),
`EAFNOSUPPORT`, `ECONNREFUSED`, `ETIMEDOUT`, `EISCONN`, `EALREADY`,
`EINPROGRESS`. libmiku: `miku_connect(fd, ip, port)`, `miku_connect_unix`;
libc: `connect`.

#### 58 `send`, 59 `recv`

```
send(u64 fd, const void *buf, u64 len, u64 flags) -> u64 bytes
recv(u64 fd, void *buf, u64 len, u64 flags) -> u64 bytes   (0 = peer closed)
```

`flags` are ignored (no `MSG_PEEK`, `MSG_DONTWAIT`, `MSG_NOSIGNAL`). `len`
above 16 MiB is `EINVAL`; 0 returns 0. `send` on a TCP connection whose peer
has gone is `EPIPE` or `ECONNRESET`, **without `SIGPIPE`**. A local socket
holds 64 KiB unread per direction; a writer blocks when it is full. Errors:
`EBADF`, `ENOTCONN`, `EPIPE`, `ECONNRESET`, `EAGAIN`, `ETIMEDOUT`. libmiku:
`miku_send`, `miku_recv`; libc: `send`, `recv`, `sendmsg`, `recvmsg` (no
control messages: `SCM_RIGHTS` is `EOPNOTSUPP`; use `sendfd`).

#### 62 `bind`, 63 `listen`, 64 `accept`

```
bind(u64 fd, const void *addr, u64 addrlen) -> 0
listen(u64 fd, u64 backlog) -> 0
accept(u64 fd, void *addr, u64 addrlen) -> u64 new_fd
```

`bind` on `AF_INET` uses only the port (the address is ignored: a socket
listens on every address); a port below 1024 needs `CAP_NET_BIND` (`EACCES`);
a port in use is `EADDRINUSE`. `bind` on `AF_UNIX` **creates a node in the
filesystem** at the path; binding a name that exists fails, and the node is
removed when the socket closes. `listen` ignores `backlog`. `accept` waits for
a connection and returns a new socket; for TCP it writes the peer's
`sockaddr_in` to `addr` if `addr` is non-zero (`addrlen` is not read or
written); for `AF_UNIX` `addr` is left untouched. libmiku: `miku_bind(fd,
port)`, `miku_bind_unix`, `miku_listen`, `miku_accept`; libc: `bind`,
`listen`, `accept`, `accept4` (flags applied with `fcntl`). `getsockname`
and `getpeername` have no kernel call behind them and answer `EOPNOTSUPP`.

#### 65 `sendto`, 66 `recvfrom`

```
sendto(u64 fd, const void *buf, u64 len, const struct sockaddr_in *dest) -> u64 bytes
recvfrom(u64 fd, void *buf, u64 len, struct sockaddr_in *src) -> u64 bytes
```

The destination is the fourth argument and has no length (16 bytes are read).
`dest` 0 makes `sendto` a `send`. `src` 0 discards the source address. UDP
datagrams and ICMP only. libmiku: `miku_sendto`, `miku_recvfrom`; libc:
`sendto`, `recvfrom`.

**ICMP sockets.** `socket(AF_INET, SOCK_RAW, IPPROTO_ICMP)` needs no
privilege, because the kernel writes the IP header: a program can send an echo
request and cannot claim another source address. `sendto` takes the ICMP
message from its type byte on, with its checksum already filled in (the port
is ignored); `recvfrom` returns one whole IP packet and its source. Every
ICMP socket receives a copy of every ICMP message (there are no ports), at
most 8 at once, so a program must recognise its own replies; draw the echo
identifier from `getrandom`.

#### 77 `socketpair`

```
socketpair(u64 fds[2]) -> 0
```

A connected pair of `AF_UNIX` stream sockets with no name, written as two
64-bit words. No arguments for domain or type. Errors: `EFAULT`, `EMFILE`.
libmiku: `miku_socketpair`; libc: `socketpair`.

#### 80 `setsockopt`, 114 `getsockopt`

```
setsockopt(u64 fd, u64 level, u64 optname, u64 value) -> 0
getsockopt(u64 fd, u64 level, u64 optname) -> u64 value
```

Every option is a small integer, so the value travels in a register (in for
`setsockopt`, as the return for `getsockopt`) instead of through a pointer.

| level | option | set | get |
|---|---|---|---|
| `SOL_IP` 0 | `IP_TTL` 2 | 1..255 (`EINVAL` otherwise) | current TTL |
| `SOL_SOCKET` 1 | `SO_TYPE` 3 | - | `SOCK_STREAM`, `SOCK_DGRAM` or `SOCK_RAW` |
| `SOL_SOCKET` 1 | `SO_ERROR` 4 | - | the pending error as a **positive** errno, 0 for none; reading clears it |
| `SOL_SOCKET` 1 | `SO_KEEPALIVE` 9 | 0 or 1, TCP | 0 or 1 |
| `SOL_SOCKET` 1 | `SO_ACCEPTCONN` 30 | - | 1 for a listener |
| `IPPROTO_TCP` 6 | `TCP_KEEPIDLE` 4 | seconds | seconds |
| `IPPROTO_TCP` 6 | `TCP_KEEPINTVL` 5 | seconds | seconds |
| `IPPROTO_TCP` 6 | `TCP_KEEPCNT` 6 | probes | probes |

`setsockopt` of anything else is `EINVAL`; a keepalive option on a socket
that is not TCP is `ENOPROTOOPT`. `getsockopt` of anything else is
`ENOPROTOOPT`. libmiku: `miku_set_ttl`, `miku_getsockopt`; libc:
`setsockopt` and `getsockopt` take Linux's pointer-and-length form and
translate. libc's `setsockopt` passes `IP_TTL` and the keepalive options
through, accepts `TCP_NODELAY` = 1 (the kernel's TCP has no Nagle delay, so
every socket already behaves that way) and answers `ENOPROTOOPT` for anything
else, `SO_REUSEADDR` included.

#### 83 `peercred`

```
struct ucred { u64 pid; u64 start_tick; u32 uid, gid, euid, egid; u64 reserved; };   /* 40 bytes */
peercred(u64 fd, struct ucred *out) -> 0
```

Who is at the other end of a connected `AF_UNIX` socket, **as they were when
the connection was made**: the kernel recorded it before the server saw the
connection, and the client cannot influence it. `start_tick` is there because
a pid alone is not an identity; a caller that stores a peer must compare both.
Errors: `EFAULT`, `ENOTCONN` (not a connected local socket of the caller's).
This is the basis of authentication for `mikudod`. libmiku: `miku_peercred`.

#### 84 `sendfd`, 85 `recvfd`

```
sendfd(u64 sockfd, u64 fd) -> 0
recvfd(u64 sockfd) -> u64 fd
```

Pass one open VFS descriptor to the peer of a connected `AF_UNIX` socket, and
take the oldest one waiting. The received descriptor lands in the lowest free
slot with the sender's offset and flags. At most 8 descriptors are in flight
on one socket. `recvfd` never blocks (`EAGAIN` when none is waiting). A socket
descriptor cannot be sent (`EINVAL`), and an unredirected fd 0-2 is not a
descriptor (`EBADF`). Errors: `EBADF`, `EINVAL`, `EMFILE` (receiver's table
full; the descriptor is dropped), `EAGAIN`, `ENOTCONN`. libmiku:
`miku_sendfd`, `miku_recvfd`.

### 4.15 Display pools (72-74)

Shared memory between a client and the Mayland display server, which runs in
the kernel. A pool is named by a handle instead of a descriptor. The protocol
that uses it is in [wiki/mayland.md](wiki/mayland.md).

```
may_pool(u64 size, u64 out[2]) -> 0      /* out = { handle, address } */
may_map(u64 handle, u64 out[1]) -> 0     /* out = { address } */
may_unpool(u64 handle) -> 0
```

`may_pool` allocates a pool of `size` bytes (1 to 64 MiB), maps it into the
caller and writes its handle and address. `may_map` maps a pool that was made
for the caller (for example one the server lent it) and writes the address.
`may_unpool` unmaps and frees a pool the caller owns, unbinding it from the GPU
first. There are at most 64 pools on the machine and 16 per process.

| errno | Cause |
|---|---|
| `EINVAL` | size 0 or over 64 MiB; handle 0 or above `u32::MAX`, or no such pool; a video-memory pool where only system memory will do |
| `ENOMEM` | no memory for the pool or no room to map it |
| `EMFILE` | the machine's or the process's pool table is full |
| `EACCES` | the pool belongs to another process |
| `EBUSY` | already mapped |
| `EFAULT` | `out` not writable |

Ownership is by process and is checked here and again in the server, so a
connection cannot name a pool its process does not own. Pools are destroyed
when their owner's connection or process goes. libmay: `Pool::new`,
`Pool::adopt`, `Pool::free` (`lib/gfx/mayland/src/conn.rs`).

### 4.16 The GPU (94-98, 121)

The NVIDIA GB206 (RTX 5060 Ti) from ring 3. The driver is described in
[Nvidia_gb206.md](Nvidia_gb206.md). All six calls answer `ENODEV` where they
need a card and there is none.

**94-97 and 121 need no capability.** A submission names memory by pool
handle and byte offset; the kernel checks the handle belongs to the caller,
computes every address itself and refuses any rectangle or buffer that leaves
its pool. Userspace never hands the engine an address, and every program it
can run without `CAP_GPU` is one the kernel compiled into itself.

#### 94 `gpu_info`

```
gpu_info(char *buf, u64 len) -> u64 bytes
```

Writes a text description of the card and of what this process may run:
the program list (the built-in programs, plus any this process loaded),
`program_slots`, `max_binds` and, while the card reports them, its live state
(`dev/gpu-perf.md`), one line each and only lines whose numbers are current:

```text
perf P8                                    the P-state, P0 the fastest
clock gr 180 mem 405 video 600             MHz per domain; "sm N" after gr where reported
util gpu 3 mem 1                           percent busy
power 12.4 limit 180.0                     watts, average draw and enforced limit
temp gpu 41 mem 50 board 38                degrees Celsius
fan 1200 1180                              rpm ("fan 0" when none turns)
```

GSP-RM refreshes these numbers only while somebody reads them, so the first
call after a quiet spell may lack them and one half a second later has them;
the call never waits on the card. `len` 0 or above 4096 is `EINVAL`. libmay:
`gpu_info`.

#### 95 `gpu_submit`

```
gpu_submit(const u64 args[6]) -> u64 fence      /* args = { op, a1, a2, 0, 0, 0 } */
```

Words 3-5 must be zero (`EINVAL`).

**op 1 `GPU_OP_BLIT`**: `a1` points at `a2` (1 to 48) 48-byte rectangles,
copied on the copy engine:

```text
  0  u32 src_handle    4  u32 dst_handle
  8  u64 src_off      16  u64 dst_off
 24  u32 pitch_in     28  u32 pitch_out
 32  u32 line_len     36  u32 lines
 40  u64 reserved, zero
```

Each rectangle must stay inside both pools (`EINVAL`), both pools must be the
caller's (`EACCES`), and one submission copies between one pair of memory
kinds only (system to video, video to system, or within one kind; `EINVAL`
otherwise).

**op 2 `GPU_OP_DISPATCH`**: `a1` points at one 40-byte launch:

```text
  0  u32 program       4  u32 nparams
  8  u32 grid[3]      20  u32 block[3]
 32  u64 params_ptr    -> nparams (at most 16) 16-byte { u32 kind; u32 handle; u64 value; }
```

A parameter's kind is 0 (`u32` value), 1 (`u64` value), 2 (a buffer: pool
`handle` at offset `value`, passed as a GPU address) or 3 (a sized buffer: the
address followed by a `u64` count of bytes from the offset to the end of the
pool). The kernel's own programs take sized buffers and stay inside them.

**op 3 `GPU_OP_CHAIN`**: `a1` points at `a2` (1 to 16) launches, run in
order, each finished before the next starts (the shader caches are
invalidated between them), as one submission with one fence. One refused
launch refuses the chain.

**One submission in flight per process**: a second before the first's fence
has passed is `EAGAIN`. Other errors: `EFAULT`, `EINVAL`, `EACCES`,
`ENODEV`, `ENOEXEC` (a launch that cannot be built for an existing program),
`EIO` (the channel refused it). The refusal reason is on the serial line.

**A fence** is `engine << 62 | epoch << 32 | seq`, with bit 63 always clear so
it never reads as an errno. The epoch (30 bits) changes every time a channel is
rebuilt, so a fence held across a teardown can never be mistaken for a passed
one; a fence of a channel that no longer exists reads as passed. Bit 62 says
whether it counts on the copy engine (0) or the compute engine (1).

libmay: `gpu_blit`, `gpu_blits`, `gpu_dispatch`, `gpu_chain`.

#### 96 `gpu_bind`

```
gpu_bind(u64 handle, u64 flags, u64 out[2]) -> 0      /* out = { gpu_va, size } */
```

Maps one of the caller's pools into the GPU address space so programs can
reach it, and returns its GPU address and size. `flags` must be 0. At most 16
pools are bound at once, all processes together (`EMFILE` past that). Errors:
`EINVAL`, `EFAULT`, `EACCES`, `ENODEV`, `EMFILE`, `EIO`. libmay: `gpu_bind`.

#### 97 `gpu_wait`

```
gpu_wait(u64 fence, u64 timeout_us) -> 0
```

Waits until the fence has passed: 0, `ETIMEDOUT` after `timeout_us` (clamped
to 1 us .. 2 s), or `EIO` if the work was lost. Sleeps on the fence interrupt
where the card raises one (`nonstall`/`nointr` boot flags fall back to
polling). libmay: `gpu_wait`, `gpu_wait_code`.

#### 98 `gpu_program`

```
struct GpuProgramArgs { u64 code_ptr; u64 code_len; u32 regs; u32 shared; };   /* 24 bytes */
gpu_program(const struct GpuProgramArgs *args) -> u64 program_id
```

**Needs `CAP_GPU`** (`EPERM`): it loads machine code. The cubin must hold
exactly one kernel, be at most 1 MiB, and be as `nvcc -cubin` wrote it; it is
read by the same parser the build uses for the kernel's own shaders. `regs`
(1 to 255) and `shared` (at most 228 KiB) are the kernel's register and
shared-memory use, from `cuobjdump -res-usage`. On first load the process
gets a GPU address space of its own where the driver can provide one;
otherwise the program runs in the shared space, which `CAP_GPU` is what
guards. Errors: `EPERM`, `EFAULT`, `EINVAL` (sizes, more or fewer than one
kernel), `ENOEXEC` (an unparsable cubin, or a load the engine refused),
`ENODEV`. The returned id joins this process's list in `gpu_info`. libmay:
`gpu_program`.

#### 121 `gpu_alloc`

```
gpu_alloc(u64 size, u64 flags, u64 out[2]) -> 0       /* out = { handle, address } */
```

A pool in **video memory**: the handle is a pool handle that `gpu_bind`,
`gpu_submit`, `may_unpool` and `may_shm.create_pool` accept like any other.
The kernel picks the memory (the part of video memory BAR1 reaches), zeroes
it and maps it into the caller write-combining: stores are fast and loads
cross the bus one at a time, so a program fills it and lets the GPU read it.
The block is at least 64 KiB and padded to its GPU page (64 KiB, or 2 MiB
from 2 MiB up). `flags` must be 0; `size` is 1 byte to 64 MiB.

| errno | Cause |
|---|---|
| `EINVAL` | flags non-zero, size out of range |
| `ENODEV` | no card, or BAR1 not usable |
| `EDQUOT` | past the 128 MiB all programs share |
| `ENOMEM` | no video memory, or no room in the address space |
| `EMFILE` | the process holds as many pools as it may |
| `EFAULT` | `out` not writable |

A window buffer cut from such a pool is read by the compositor in place
(`dev/gpu-window-buffers.md`). `apps/src/gpudemo.rs` is the worked example.
libmay: `Pool::video`.

### 4.17 The administrative channel (104)

#### 104 `kcmd`

```
kcmd(const char *line, u64 len) -> 0
```

Runs one command of the in-kernel console (the emergency shell's command
table, `kernel/shell/commands/`) on the caller's thread, with the caller's
credential and terminal, so output lands where the caller is and every check
is made as the caller. Each command gates itself (for example `nvidia list`
needs nothing, `mount` needs `CAP_SYS_ADMIN`), and the gate is checked before
the arguments are parsed.

| errno | Cause |
|---|---|
| `EINVAL` | `len` 0 or over 512; not UTF-8; a control character other than tab; an empty line |
| `EPERM` | `login` or `logout` (the console's own) |
| `ENOENT` | no such command |
| `EFAULT` | unreadable |

`msh` sends any name it cannot find in `PATH` here, and `mikudo` does the same
(named `kernel:<name>` in `/etc/mikudo.conf`). The command list is in
[wiki/commands.md](wiki/commands.md). This channel shrinks as commands move to
ring-3 programs. libmiku: `miku_kcmd`.

---

## 5. Structures

Every structure that crosses the boundary, with byte offsets. All integers
are little-endian; `sin_port` and the IPv4 address are in network order.
Structures the kernel reads are read unaligned, so the caller need not align
them beyond what the table implies.

**`stat`** (18, 19, 112), 64 bytes, `lib/abi/src/kstat.rs`:

```
  0  u64  size
  8  u32  mode         permission and type bits
 12  u32  nlinks
 16  u16  uid
 18  u16  gid
 20  u8   kind         0 reg, 1 dir, 2 symlink, 3 chr, 4 blk, 5 pipe, 6 socket, 7 fifo
 21  u8   fs_type      0 tmpfs, 1 devfs, 2 procfs, 3 ext2, 4 ext3, 5 ext4, 8 btrfs, 9 memfs
 22  u8   dev_major
 23  u8   dev_minor
 24  u64  atime        seconds
 32  u64  mtime        seconds
 40  u64  ctime        seconds
 48  u64  inode_id
 56  u32  blocks       512-byte units
 60  [4]  reserved
```

**Directory entry** (23, 111), 272 bytes:

```
  0  [256] name        NUL-terminated, 255 usable
256  u64   inode_id    the number stat reports
264  u8    kind        as in stat
265  u8    name_len
266  [6]   reserved
```

**`statfs`** (36), 48 bytes:

```
  0  u32  fs_type      the filesystem's magic number
  4  u32  block_size
  8  u64  total_blocks
 16  u64  free_blocks
 24  u64  total_inodes
 32  u64  free_inodes
 40  u32  max_name_len
 44  u32  flags
```

**`timespec`** (40, 70, 79, 110): `{ i64 tv_sec; i64 tv_nsec; }`, 16 bytes.
`utimensat` takes two of them (32 bytes).

**`pollfd`** (76, 123): `{ i32 fd; i16 events; i16 revents; }`, 8 bytes,
Linux's layout.

**`flock`** (113), 32 bytes: `i16 l_type` at 0, `i16 l_whence` at 2,
`i64 l_start` at 8, `i64 l_len` at 16, `i32 l_pid` at 24.

**`MikuTermios`** (71 `TCGETS`/`TCSETS`), 14 bytes, not Linux's 60-byte
termios:

```
  0  u8   icanon       canonical (line) mode
  1  u8   echo
  2  u8   isig         control characters raise signals
  3  u8   vmin         raw mode: fewest characters a read returns with
  4  u8   vtime        raw mode: tenths of a second a read waits
  5  u8   tostop       stop background jobs that write
  6  [7]  c_cc         VINTR 0, VQUIT 1, VERASE 2, VKILL 3, VEOF 4, VWERASE 5, VSUSP 6
 13  u8   _pad         zero
```

The kernel copies `size_of::<MikuTermios>()` bytes, which is 14; comments in
`kernel/syscall/adapters/io.rs` and `lib/runtime/libmiku/libc/termios.rs` still say
twelve, from before `c_cc` grew to seven slots.

Defaults: `c_cc` = Ctrl-C (0x03), Ctrl-\\ (0x1c), DEL (0x7f, with 0x08
accepted too), Ctrl-U (0x15), Ctrl-D (0x04), Ctrl-W (0x17), Ctrl-Z (0x1a);
`vmin` 1, `vtime` 0, canonical, echo and signals on. A `c_cc` of 0
(`VDISABLE`) disables that character.

**`winsize`** (71 `TIOCGWINSZ`/`TIOCSWINSZ`): `{ u16 rows; u16 cols; }`, 4
bytes (Linux's struct has two more fields, which the kernel neither reads nor
writes).

**`sockaddr_in`**, 16 bytes: `u16 family` (2) at 0, `u16 port` big-endian
at 2, `u8 addr[4]` at 4, 8 zero bytes at 8. **`sockaddr_un`**: `u16 family`
(1) at 0, `char path[108]` at 2, NUL-terminated or ended by `addrlen`.

**`ucred`** (83), 40 bytes: `u64 pid` 0, `u64 start_tick` 8, `u32 uid` 16,
`u32 gid` 20, `u32 euid` 24, `u32 egid` 28, `u64 reserved` 32.

**`SessionRequest`** (92 open), 40 bytes, and **`SessionInfo`** (92 list),
32 bytes: see [§4.10](#410-sessions-and-power-92-93).

**Process record** (103), 72 bytes: see [`procinfo`](#103-procinfo).

**`stack_t`** (115), 24 bytes: `u64 ss_sp` 0, `i32 ss_flags` 8, `u64 ss_size`
16.

**Signal frame** (68/69), 1072 bytes, `kernel/process/signal/frame.rs`, at
the handler's `rsp`:

```
   +0    siginfo_t     128   si_signo i32 0, si_errno i32 4, si_code i32 8, si_addr u64 16
 +128    ucontext_t    424   Linux's layout up to the end of uc_sigmask:
           +0    uc_flags        UC_MIKU_FAULT (1): raised for a faulting instruction;
                                 bit 32: the task was already in a handler
           +8    uc_link         0
           +16   uc_stack        ss_sp, ss_flags (+8), ss_size (+16)
           +40   gregs[23]       Linux's REG_* order: R8 R9 R10 R11 R12 R13 R14 R15
                                 RDI RSI RBP RBX RDX RAX RCX RSP RIP EFL CSGSFS ERR
                                 TRAPNO OLDMASK CR2
           +224  fpregs          pointer to the fpstate below
           +232  reserved[8]
           +296  uc_sigmask      128 bytes, zero
 +552    padding         8
 +560    fpstate       512   fxsave64 image, filled and restored by the stub
+1072    end
```

`CSGSFS` holds CS in its low 16 bits and SS in its top 16. `ERR`, `TRAPNO`
and `CR2` are the fault's error code, vector and faulting address. The layout
matches `lib/abi/src/ucontext.rs`, which is what lets crates
that read a machine context (wasmtime, for one) build for MikuOS.

**`inotify_event`** (read of 116's descriptor): `i32 wd` 0, `u32 mask` 4,
`u32 cookie` 8, `u32 len` 12, `char name[len]` 16, the name NUL-padded so
`len` is a multiple of 16 (0 when there is no name).

**`ThreadArgs`** (106), 64 bytes: see [§4.12](#106-thread_create).
**`MmapArgs`** (60) and **`ExecveArgs`** (67), 48 bytes each: six `u64`s, see
their entries. **GPU blocks** (95, 98): see [§4.16](#416-the-gpu-94-98-121).

There is no `epoll_event`, `sigset_t` in the kernel (the blocked mask is the
library's), `rlimit`, `rusage` or `utsname` at this boundary; `libc_miku`
provides those types and fills them itself.

---

## 6. Constants, ioctls and fcntl commands

**Open flags** (11, 113, 120, 116): `O_READ` 0x1, `O_WRITE` 0x2, `O_RDWR`
0x3, `O_APPEND` 0x4, `O_CREATE` 0x8, `O_EXCLUSIVE` 0x10, `O_TRUNC` 0x20,
`O_DIRECTORY` 0x40, `O_NOFOLLOW` 0x80, `O_DIRECT` 0x100, `O_SYNC` 0x200,
`O_NONBLOCK` 0x400, `O_CLOEXEC` 0x800, `O_NOATIME` 0x1000. (MikuOS values;
`libc_miku` translates Linux's.)

**Memory**: `PROT_READ` 1, `PROT_WRITE` 2, `PROT_EXEC` 4 (W+X refused);
`MAP_SHARED` 0x01 (60 only), `MAP_FIXED` 0x10, `MAP_ANONYMOUS` 0x20 (implied
by 3).

**Seek**: `SEEK_SET` 0, `SEEK_CUR` 1, `SEEK_END` 2.

**`fcntl` commands**: `F_DUPFD` 0, `F_GETFD` 1, `F_SETFD` 2, `F_GETFL` 3,
`F_SETFL` 4, `F_GETLK` 5, `F_SETLK` 6, `F_SETLKW` 7, `F_DUPFD_CLOEXEC` 1030;
`FD_CLOEXEC` 1; lock types `F_RDLCK` 0, `F_WRLCK` 1, `F_UNLCK` 2. `F_GETPATH`
(50) exists only in `libc_miku`.

**`ioctl` requests** (terminals only): `TCGETS` 0x5401, `TCSETS` 0x5402,
`TIOCSCTTY` 0x540E, `TIOCGWINSZ` 0x5413, `TIOCSWINSZ` 0x5414, `TIOCGPTN`
0x80045430. `FIONBIO` is answered in `libc_miku`. No `FIONREAD`, `TIOCSTI`,
`TCSETSW`/`TCSETSF` (libc maps `tcsetattr`'s actions onto `TCSETS`) or
`TIOCNOTTY`.

**`prctl` and `arch_prctl`**: neither exists. `set_tls` (9) and `get_tls`
(10) are `ARCH_SET_FS` and `ARCH_GET_FS`; nothing else is offered.

**`poll` events**: `POLLIN` 0x1, `POLLOUT` 0x4, `POLLERR` 0x8, `POLLHUP`
0x10, `POLLNVAL` 0x20.

**Clocks**: `CLOCK_REALTIME` 0, `CLOCK_MONOTONIC` 1. `UTIME_NOW` 0x3FFFFFFF,
`UTIME_OMIT` 0x3FFFFFFE, `AT_SYMLINK_NOFOLLOW` 0x100.

**`wait4` options**: `WNOHANG` 1, `WUNTRACED` 2.

**Signal dispositions** (100): `DISP_DEFAULT` 0, `DISP_IGNORE` 1, `DISP_CATCH`
2, `DISP_QUERY` 3, `DISP_ONSTACK` 4 (or'd). `SS_ONSTACK` 1, `SS_DISABLE` 2,
`MINSIGSTKSZ` 2048. `si_code`: `SEGV_MAPERR` 1, `SEGV_ACCERR` 2,
`ILL_ILLOPN` 2, `FPE_INTDIV` 1, `SI_KERNEL` 0x80, 0 for `kill`.

**Futex**: `FUTEX_WAIT` 0, `FUTEX_WAKE` 1. **Threads**: `THREAD_STACK_OWNED` 1.

**`tty` ops** (99): `TTY_ATTACH` 1, `TTY_SELF` 3, `TTY_SETFG` 4, `TTY_GETFG`
5, `TTY_CLEAR` 6, `TTY_OF_PID` 7. **`session` ops** (92): 0 open, 1 close, 2
list, 3 current; kinds 0 console, 1 graphical. **`power`** (93): 0 off, 1
reboot.

**inotify** (Linux's values): `IN_ACCESS` 0x1, `IN_MODIFY` 0x2, `IN_ATTRIB`
0x4, `IN_CLOSE_WRITE` 0x8, `IN_CLOSE_NOWRITE` 0x10, `IN_OPEN` 0x20,
`IN_MOVED_FROM` 0x40, `IN_MOVED_TO` 0x80, `IN_CREATE` 0x100, `IN_DELETE`
0x200, `IN_DELETE_SELF` 0x400, `IN_MOVE_SELF` 0x800, `IN_UNMOUNT` 0x2000,
`IN_Q_OVERFLOW` 0x4000, `IN_IGNORED` 0x8000, `IN_ONLYDIR` 0x01000000,
`IN_DONT_FOLLOW` 0x02000000, `IN_EXCL_UNLINK` 0x04000000, `IN_MASK_CREATE`
0x10000000, `IN_MASK_ADD` 0x20000000, `IN_ISDIR` 0x40000000, `IN_ONESHOT`
0x80000000.

**Sockets**: `AF_UNIX` 1, `AF_INET` 2, `AF_MAYLAND` 42; `SOCK_STREAM` 1,
`SOCK_DGRAM` 2, `SOCK_RAW` 3; `IPPROTO_ICMP` 1, `IPPROTO_TCP` 6,
`IPPROTO_UDP` 17; levels `SOL_IP` 0, `SOL_SOCKET` 1, `IPPROTO_TCP` 6;
options `IP_TTL` 2, `SO_TYPE` 3, `SO_ERROR` 4, `SO_KEEPALIVE` 9,
`SO_ACCEPTCONN` 30, `TCP_KEEPIDLE` 4, `TCP_KEEPINTVL` 5, `TCP_KEEPCNT` 6;
`SOCK_FD_BASE` 4096.

**GPU**: `GPU_OP_BLIT` 1, `GPU_OP_DISPATCH` 2, `GPU_OP_CHAIN` 3; parameter
kinds 0 `u32`, 1 `u64`, 2 buffer, 3 sized buffer.

**Capabilities**: [§2.7](#27-capabilities). **Errno values**:
[§2.2](#22-return-values-and-errno). **Signals**:
[§4.11](#411-signals-68-69-100-115).

**Timer**: the local APIC timer at 250 Hz, so one tick is 4 ms (`AT_CLKTCK`
reports 250). Deadlines of `sleep_ns`, `poll`, `futex` and the socket layer
are kept on the TSC to microseconds; `sleep` (16) and `uptime` (17) count
ticks.

---

## 7. Starting a program

### 7.1 Binary requirements

- ELF64, x86-64, `ET_EXEC` or `ET_DYN` (PIE). PIE binaries load at a random
  base (28 bits of entropy); `ET_EXEC` keeps its link addresses.
- A dynamically linked program has `PT_INTERP` naming `/lib/ld-miku.so` (at
  most 128 bytes) and `DT_NEEDED` entries for the miku libraries it uses.
  A program with no `PT_INTERP` is started directly by the kernel.
- A segment that is both writable and executable is refused (W^X); a
  `PT_GNU_STACK` request for an executable stack is ignored, the stack is
  always non-executable.
- `PT_TLS` alignment up to a page is honoured; more is refused.
- Programs are built with frame pointers and `--build-id=sha1` so crash
  reports can walk and name frames ([§7.9](#79-when-a-process-dies)).
- The file is at most 64 MiB, and the mapped segments at most 1 GiB.

How to produce such a binary is in [wiki/sdk.md](wiki/sdk.md).

### 7.2 Loading sequence

1. The kernel opens the file and reads it **by range** through an
   `ElfSource` (`kernel/process/elf_source.rs`): headers, notes, `PT_INTERP`,
   the `PT_TLS` image, each `PT_LOAD` straight into the frames it fills, and
   the dynamic and relocation tables through a 64 KiB window. Nothing holds
   the whole file in the kernel heap.
2. Read-only segments of a binary already running are mapped from the same
   frames ([§7.8](#78-shared-image-pages)); writable ones are private.
3. If there is a `PT_INTERP`, the interpreter is loaded too (the kernel embeds
   `ld-miku.so`, so the usual one costs no disk read) at its own random base.
4. The stack, the auxiliary vector, the vDSO and, for a program without an
   interpreter, the thread control block are set up.
5. Control goes to the interpreter's entry (or the program's). `ld-miku`
   maps each `DT_NEEDED` library with `map_lib` (15), recursively pulls in the
   libraries' own dependencies, relocates, applies `PT_GNU_RELRO`, builds the
   thread control block, hands the auxiliary vector to libmiku
   (`miku_set_auxv`) and jumps to the program's entry.

The loader's side is described in [wiki/processes.md](wiki/processes.md#the-elf-loader)
and the dynamic linker in [wiki/sdk.md](wiki/sdk.md#dynamic-linking).

### 7.3 Address space layout

Every base is randomized per `exec`:

```
0x0000_0000_0040_0000                         ET_EXEC program (its link addresses)
image end + random gap (up to 32 MiB)         heap (brk)
0x0000_0001_0000_0000 .. 0x0000_7F00_0000_0000 mmap area, miku libraries
0x0000_5555_0000_0000 + up to 1 TiB           PIE program base
0x0000_7F00_0000_0000 + up to 2 GiB           ld-miku base
16 MiB below the stack's low end              TLS block of the first thread
0x0000_7FFF_FFFF_0000 - up to 16 MiB          top of the user stack (8 MiB mapping, grows on demand)
two pages, random address                     vvar page, then the vDSO page
```

The stack is an 8 MiB mapping of which the top 256 KiB is populated at
`exec`; the rest is zero-filled on first touch. The page below it is an
unmapped guard: touching it is a `SIGSEGV`. The constants are in
`kernel/process/elf_loader.rs` (`PIE_BASE`, `INTERP_BASE`, `USER_STACK_TOP`,
`STACK_SIZE`) and `kernel/mm/mmap.rs` (`MMAP_BASE`, `MMAP_LIMIT`).

### 7.4 Initial stack

At the entry point `rsp` is 16-byte aligned and points at:

```
[rsp]        argc
[rsp+8]      argv[0] .. argv[argc-1], NULL
             envp[0] .. envp[envc-1] ("KEY=value"), NULL
             auxv pairs (key, value) .. AT_NULL
             padding, then the strings: argv and envp bytes, "x86_64",
             the exec path, 16 random bytes
```

`rdi`, `rsi`, `rdx` and `rax` are zero. Only the first code to run sees this
stack: for a dynamically linked program that is `ld-miku`. The userspace
`_start` shim passes `argc`, `argv` and `envp` to `_start_main` (or `main`) in
`rdi`, `rsi`, `rdx`. Limits: 256 entries each for argv and envp, 32 KiB per
string, 128 KiB of strings in all; an `exec` that does not fit is refused
before anything is replaced.

### 7.5 Auxiliary vector

21 entries, in this order:

| Key | Nr | Value |
|---|---|---|
| `AT_PHDR` | 3 | address of the program headers |
| `AT_PHENT` | 4 | size of one program header |
| `AT_PHNUM` | 5 | number of program headers |
| `AT_PAGESZ` | 6 | 4096 |
| `AT_BASE` | 7 | interpreter load base (0 without one) |
| `AT_FLAGS` | 8 | 0 |
| `AT_ENTRY` | 9 | the program's entry point |
| `AT_UID` | 11 | real uid |
| `AT_EUID` | 12 | effective uid |
| `AT_GID` | 13 | real gid |
| `AT_EGID` | 14 | effective gid |
| `AT_SECURE` | 23 | 1 when the effective ids differ from the real ones |
| `AT_RANDOM` | 25 | address of 16 random bytes |
| `AT_HWCAP` | 16 | CPUID.1:EDX |
| `AT_HWCAP2` | 26 | 0 |
| `AT_CLKTCK` | 17 | 250 |
| `AT_PLATFORM` | 15 | address of `"x86_64"` |
| `AT_EXECFN` | 31 | address of the path given to `exec` |
| `AT_MINSIGSTKSZ` | 51 | 2048 |
| `AT_SYSINFO_EHDR` | 33 | address of the vDSO's ELF header, 0 if there is none |
| `AT_NULL` | 0 | end |

From userspace: `ld-miku` passes the vector to libmiku before any
constructor runs, and `miku_getauxval(key)` (core_miku) and `getauxval(key)`
(libc_miku, `<sys/auxv.h>`) answer a key, 0 when absent.

### 7.6 The thread control block

(Earlier versions of this document called this §3.2d; comments in the code
still use that number.)

`FS_BASE` points at a 64-byte block. The x86-64 ABI fixes only the first word
(`%fs:0` is the block's own address); the rest is MikuOS's:

```
 tp +  0   self        this address
 tp +  8   tls_size    bytes of thread-local data below tp
 tp + 16   tls_align   what tp is aligned to
 tp + 24   tls_image   address of the initial data (.tdata)
 tp + 32   tls_filesz  how much of tls_size the image covers; the rest is zero (.tbss)
 tp + 40   errno       this thread's errno (an int); zero at thread start
 tp + 48   pthread     libc_miku's pthread block, made on first use
 tp + 56   heap cache  core_miku's per-thread heap cache, made on first free
```

Thread-local data sits below the thread pointer (TLS variant II), so a
thread-local access is a small negative `%fs:` offset. Words 8-32 are what
`thread_spawn` copies to build another thread's block. The offsets are in
`lib/abi/src/tcb.rs`. The block is built in three places:
`lib/runtime/ld-miku/src/loader.rs` (dynamically linked programs),
`kernel/process/elf_loader.rs` (programs without an interpreter) and
`lib/runtime/libmiku/core/thread.rs` (every new thread). A dynamically linked program without `PT_TLS` still gets a block, so `errno` has a home; a
**statically linked** program without `PT_TLS` starts with `FS_BASE` = 0.

`errno` at `tp + 40` is written only by the C-convention functions of
`libc_miku`; `__errno_location` (and `errno_location`, the name the Rust
`errno` crate links) returns `%fs:0 + 40`.

### 7.7 The vDSO

Every process gets two extra pages at a random address, the same physical
frames in every process:

```
base - 0x1000   vvar   read-only, no-execute   the published time data
base            vdso   read + execute          the code that reads it (an ELF image)
```

`base` is `AT_SYSINFO_EHDR`. The vDSO exports three functions, found through
its `.dynsym` (it is linked `--hash-style=sysv`):

| Symbol | Signature | Notes |
|---|---|---|
| `__vdso_clock_gettime` | `i32 (i32 clock, struct timespec *)` | clocks 0 and 1; same results as syscall 70 |
| `__vdso_gettimeofday` | `i32 (struct timeval *, void *tz)` | the timezone argument is ignored |
| `__vdso_getcpu` | `i32 (u32 *cpu, u32 *node, void *)` | from `RDTSCP`, only where the CPU has it; `node` is always 0 |

Each falls back to the system call by itself when the data cannot be used
(an uncalibrated TSC, `CLOCK_REALTIME` before the wall clock is set, or a
magic or version it does not recognise), so a caller never has to check.

**The vvar page** (`lib/abi/src/vvar.rs`, used by both the kernel and the vDSO):

```
  0  u32  magic        0x4D494B55 ("MIKU")
  4  u32  version      2; readers require equality
  8  u32  seq          seqlock: odd while the kernel is writing
 12  u32  have_wall    non-zero once the wall clock has been set
 16  u64  tsc_khz      TSC frequency; 0 = uncalibrated, every clock unusable
 24  u64  anchor_tsc   TSC value the wall clock below was taken at
 32  u64  wall_sec     wall clock at anchor_tsc
 40  u32  wall_nsec
 44  u32  features     bit 0: RDTSCP available
 48  u64  boot_tsc     TSC at boot, the origin of CLOCK_MONOTONIC
```

Protocol: read `seq`; if odd, read again; read the fields; read `seq` again
and retry if it changed. The code finds the data page by taking its own base
(PC-relative) and subtracting `VVAR_OFFSET` (0x1000). libmiku's `time` module
looks the functions up once (`lib/runtime/libmiku/core/auxv.rs`) and `miku_clock_gettime`,
`std::time::Instant` and C's `clock_gettime` use them.

### 7.8 Shared image pages

Segments without `PF_W` are the same bytes in every process running a binary,
so the second `exec` of a program maps the first one's frames instead of
reading and copying its own. Writable segments are always private, because
they are relocated per process and `PT_GNU_RELRO` turns part of them
read-only afterwards. The addresses are still randomized per process. The
kernel console's `elf cache` lists what is held.

### 7.9 When a process dies

An uncaught fault prints a report on the serial line and in the kernel log,
and the whole process (every thread) dies of the signal, so `wait4` reports
`WIFSIGNALED`:

```
[crash] pid=7 'mikudo' SIGSEGV at 0x00005555a3f01c4e
[crash]   write to 0x0 (page not present)
[crash]   image /bin/mikudo bias=0x55559f000000 build-id 3f2a1c04...
[crash]   #0  0x0000000000401c4e  parse_word+0x2e
[crash]   #1  0x0000000000402a10  run_line+0x150
[crash]   rip=... rsp=... rbp=...
[crash]   maps: ...
```

Frame addresses are printed with the ASLR bias subtracted, so they can be
looked up against the binary (`elf addr <path> <hex>` in the kernel
console). Frames marked `?` came from a stack scan rather than the
frame-pointer chain.

A core file is written to `/tmp/core.<name>.<pid>`: `ET_CORE`, `PT_NOTE`
first (`NT_PRSTATUS`, `NT_PRPSINFO`, `NT_FILE`, the build-id, and a `MIKU`
note saying which registers in `NT_PRSTATUS` are real), then one `PT_LOAD` per
region: the stack around `rsp` first, then writable anonymous memory.
Read-only file-backed regions are described and not stored. A region that did
not fit has `p_filesz` 0 and its full `p_memsz`. A core is at most 16 MiB
(half of it for the stack), at most 64 regions are described, and only the
three newest cores are kept; `elf core` lists them. A caught fault produces
none of this: it is the handler's. Details are in
[wiki/diagnostics.md](wiki/diagnostics.md).

---

## 8. Changes since v0.2.10

v0.2.10 (commit `55bf988`) had system calls 0 to 71. Every number from 72 to
123 is new:

| Nrs | Added |
|---|---|
| 72-74 | `may_pool`, `may_unpool`, `may_map` |
| 75 | `klog` |
| 76 | `poll` |
| 77 | `socketpair` |
| 78 | `getrandom` |
| 79 | `clock_settime` |
| 80 | `setsockopt` |
| 81, 82 | `listxattr`, `removexattr` |
| 83-85 | `peercred`, `sendfd`, `recvfd` |
| 86-91 | `setresuid`, `setresgid`, `setgroups`, `getgroups`, `capget`, `capset` |
| 92, 93 | `session`, `power` |
| 94-98 | `gpu_info`, `gpu_submit`, `gpu_bind`, `gpu_wait`, `gpu_program` |
| 99 | `tty` |
| 100-102 | `sigdisp`, `setpgid`, `getpgid` |
| 103 | `procinfo` |
| 104 | `kcmd` |
| 105 | `proc_path` |
| 106-110 | `thread_create`, `thread_exit`, `gettid`, `sched_yield`, `futex` |
| 111 | `getdents` |
| 112 | `lstat` |
| 113 | `fcntl` |
| 114 | `getsockopt` |
| 115 | `sigaltstack` |
| 116-118 | `inotify_init1`, `inotify_add_watch`, `inotify_rm_watch` |
| 119 | `realpath` |
| 120 | `pipe2` |
| 121 | `gpu_alloc` |
| 122 | `sleep_ns` |
| 123 | `poll_ns` |

No number was renumbered or removed. Calls that kept their number and changed
shape or meaning:

| Nr | Then | Now |
|---|---|---|
| all path calls | `(path, len)`, relative to the working directory | the length word's high half is a directory descriptor ([§2.4](#24-paths-and-the-path-word)); an empty path with a base names the descriptor |
| 23 `readdir` | 72-byte entries: 64-byte name, **16-bit** inode | 272-byte entries: 256-byte name, 64-bit inode; at most 120 per call |
| 40 `utimensat` | `(ino, atime, mtime)` on the active ext filesystem | `(path, path word, times, flags)` on any filesystem |
| 69 `sigreturn` | an ordinary call returning the saved `rax` | diverted at entry, restores every register through `iretq` |
| 71 `ioctl` | fds 0-2 only | any terminal descriptor; `TIOCSWINSZ`, `TIOCSCTTY`, `TIOCGPTN` added; `ENOTTY` for non-terminals |
| 44 `wait4` | the raw exit code, a signal death as `128+sig` | POSIX's status word; `WUNTRACED` |
| 99 `tty` ops 0, 2 | `TTY_OPEN`, `TTY_CLOSE` | retired (`EINVAL`); use `/dev/ptmx` and `close` |
| entry | `rdi`, `rsi`, `rdx` clobbered | preserved (Linux's rule); only `rcx` and `r11` are clobbered |
| 46, 67 `exec` | setuid/setgid bits | never honoured; the credential follows `permitted &= bounding` |
| descriptors | 128 per process | 512 per process |
| 9 `set_tls` | `FS_BASE` only | also the start of the 64-byte TCB that libraries rely on ([§7.6](#76-the-thread-control-block)) |

The `stat` (64 bytes) and `statfs` (48 bytes) layouts are unchanged; what
`inode_id` means changed on 2026-10-27 (it now names the file, stable across
eviction and hard links).

---

### The library exports, after v1.0.0

The system calls did not change. `lib/` was reorganised (`lib/README.md`), and
exports that duplicated others went:

| Removed | Use instead |
|---|---|
| `miku_bring_*` | `miku_ring_*`, which gained `miku_ring_find`, `miku_ring_readline`, `miku_ring_capacity` |
| `miku_sb_*` | `miku_str_*`, which gained `miku_str_push_uint`, `miku_str_push_repeat` |
| `miku_getopt_*` | `miku_args_*` |
| `miku_strtod`, an integer parser under that name | `miku_strtol`, or `strtod` from `libc_miku.so` |

`miku_crc32` and `miku_crc32_update` moved from `algo_miku.so` to `codec_miku.so`.
`MikuSigaction.flags` takes Linux's `SA_*` values, so `libc_miku` no longer
translates them.

---

## 9. Programs and libraries

Earlier versions of this file also carried the library API, the minimal Rust
and C programs, the build and deploy steps and a checklist. That material is
now in [wiki/sdk.md](wiki/sdk.md), which covers:

- the three ways to write a program (no_std with libmiku, Rust `std`, C with
  `libc_miku` and `miku-cc`), each with a working example;
- the libmiku modules and the `libc_miku` headers;
- the dynamic linker `ld-miku`, the ten miku libraries (`core_miku`,
  `sys_miku`, `text_miku`, `ds_miku`, `algo_miku`, `codec_miku`, `fs_miku`,
  `net_miku`, `parse_miku`, `libc_miku`, declared in
  `lib/runtime/libmiku/libs.list`) and their dependency graph;
- building, getting a binary into `/bin`, and testing it;
- calling a system call no library wraps.

Every program in `/bin` is listed in [wiki/commands.md](wiki/commands.md).

---

## 10. Debugging

| Symptom | Likely cause | What to do |
|---|---|---|
| `[syscall] unknown nr=N` on serial, `ENOSYS` | a number the kernel does not have | check [§3](#3-index-of-all-system-calls) |
| `[exec] ELF load failed: ...` and `ENOENT` from `exec` | the loader refused the image (W+X segment, bad alignment, too large, missing interpreter) | read the reason; `readelf -l` the binary |
| `EINVAL` from a path call that looks right | the path word's high half is not 0 (a stray length type), or the path is not UTF-8 | pass `strlen` as a `u64` |
| `ENOTDIR`/`EBADF` from a relative path | the path word names a base descriptor that is closed or not a directory | |
| `EFAULT` on a buffer that exists | the buffer crosses into an unmapped page, or a write target is read-only | |
| `[crash] ... SIGSEGV` with frames | an uncaught fault | look the unbiased addresses up with `elf addr`; open the core in `/tmp` |
| a handler never runs | the program makes no system call (signals sent by others are delivered at system call exit), or no entry point is registered (`sigdisp` alone does not register one) | install it through `sigaction`/`miku_signal` |
| `wait4` returns `EINTR` repeatedly | a signal handler is installed and signals keep arriving | retry; `SA_RESTART` is not honoured |
| `CLOCK_REALTIME` is `EINVAL` | the wall clock has not been set yet | use `CLOCK_MONOTONIC`, or wait for NTP |
| `EAGAIN` from `gpu_submit` | the previous submission's fence has not passed | `gpu_wait` it first |

`dmesg`, the kernel log levels, `/proc` and the crash reports are in
[wiki/diagnostics.md](wiki/diagnostics.md). Checking a dynamically linked
binary:

```bash
readelf -l app | grep INTERP        # /lib/ld-miku.so
readelf -d app | grep NEEDED        # the miku libraries it uses
readelf -n app | grep 'Build ID'    # present, for crash reports
```

---

## 11. Limitations

- **Four argument registers.** Calls with more arguments take a block in user
  memory.
- **MikuOS numbers and flag values.** Open flags, the termios layout and
  several argument shapes differ from Linux; `libc_miku` translates.
- **No `select`, `epoll`, `ppoll` or `signalfd` in the kernel.** `poll`
  (76/123) covers files, pipes, terminals and every socket kind; libc's
  `select` is built on it.
- **Signals**: no kernel signal mask (the blocked mask is `sys_miku.so`'s,
  process-wide); no queued real-time signals (one pending bit per signal);
  no `SA_RESTART`; signals sent by another process are delivered only at
  system call exit; no `sigtimedwait` in the kernel (libc's `sigwait` and
  `sigsuspend` are library loops).
- **No process-group waits**: `wait4` takes a pid or -1. `kill(0, sig)` and
  `kill(-1, sig)` are refused.
- **No `vfork`, `clone`, `posix_spawn` system call**: `fork` + `execve`, and
  `thread_create` for threads.
- **No `prctl`, `arch_prctl`, `ptrace`, `setrlimit`, `getrusage`** in the
  kernel. No per-process resource limits.
- **Sockets**: one machine-wide table of 256; `AF_UNIX` streams only (no
  datagrams); no `SCM_RIGHTS` (use `sendfd`/`recvfd`); `send`/`recv` flags are
  ignored; a socket does not cross `fork`; blocking operations time out after
  30 s; `getsockname`/`getpeername` are not supported; `bind` ignores the
  address; raw sockets are ICMP only.
- **Files**: times are whole seconds; xattrs are by ext inode number, `user.`
  namespace only, values up to 256 bytes; `readdir` (23) cannot continue past
  one call (use `getdents`); `getdents` offsets are positions, not cookies.
- **Ids are 16-bit**, at most 16 supplementary groups.
- **Terminals**: at most 7 pseudo-terminals; the termios layout is MikuOS's.
- **GPU**: one submission in flight per process; 128 MiB of video memory for
  all programs; 16 bound pools machine-wide.
- **`SYSCALL_TABLE_SIZE`** in `kernel/syscall/mod.rs` says 119 while the
  dispatcher answers up to 123; it only feeds the boot log line.

---

## 12. Where the code is

| What | Where |
|---|---|
| Entry stub, `sigreturn` path, dispatcher | `kernel/syscall/mod.rs` |
| Syscall numbers, stubs | `lib/abi/src/sys.rs` |
| errno values | `lib/abi/src/errno.rs` |
| VFS error mapping | `kernel/syscall/errno.rs` |
| `stat`, dirent, `statfs` layouts | `lib/abi/src/kstat.rs` |
| User pointer checks, path word, argv copy-in | `kernel/syscall/usercopy.rs` |
| Process, identity, time, exec | `kernel/syscall/adapters/process.rs` |
| Threads, futex | `kernel/syscall/adapters/thread.rs`, `kernel/sched/futex.rs` |
| read, write, ioctl, poll, klog | `kernel/syscall/adapters/io.rs` |
| open, close, dup, pipe, fcntl, locks | `kernel/syscall/adapters/file.rs`, `kernel/fs/vfs/lock.rs` |
| Namespace calls, xattrs, utimensat | `kernel/syscall/adapters/fs.rs` |
| mmap, brk, TLS, map_lib | `kernel/syscall/adapters/memory.rs`, `kernel/mm/mmap.rs` |
| Sockets | `kernel/syscall/adapters/net.rs`, `kernel/net/socket.rs` |
| inotify | `kernel/syscall/adapters/notify.rs`, `kernel/fs/vfs/notify.rs` |
| tty, job control | `kernel/syscall/adapters/tty.rs`, `kernel/io/tty/` |
| Display pools | `kernel/syscall/adapters/mayland.rs`, `kernel/mayland/server/shm.rs` |
| GPU | `kernel/syscall/adapters/gpu.rs` |
| kcmd | `kernel/syscall/adapters/admin.rs` |
| Signals, frame, sigaltstack | `kernel/process/signal/` |
| Credentials, capabilities | `kernel/process/cred.rs` |
| Sessions | `kernel/process/session.rs` |
| ELF loader, auxv, stack | `kernel/process/elf_loader.rs`, `kernel/process/exec.rs` |
| vDSO | `kernel/process/vdso.rs`, `lib/runtime/vdso/` |
| Crash reports, cores | `kernel/process/crash.rs`, `kernel/process/coredump.rs` |
| Syscall selftest | `kernel/syscall/selftest.rs` (`ipc test` in the kernel console) |
| libc_miku | `lib/runtime/libmiku/libc/`, `lib/runtime/libmiku/libc/` |
| C's constants and structs, by header | `lib/abi/src/<header>.rs` |
| Types `miku_*` functions expose | `lib/abi/src/libtypes/` |
| ELF, the auxiliary vector | `lib/abi/src/elf.rs`, `lib/abi/src/auxv.rs` |
| Rust std's view | `lib/sdk/rust-std/`, `lib/sdk/rust-libc/` |
