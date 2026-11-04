# Processes, threads and signals

How MikuOS runs programs: what a process and a thread are inside the kernel,
how pids are handed out, how `fork`, `exec`, `exit` and `wait4` fit together,
how the kernel loads an ELF image and what it puts on the new stack, how
futexes and signals work (including CPU faults delivered as signals), the
vDSO, the FPU state, credentials, and how a program enters the kernel at all.
Each system call mentioned here is documented in full, with its errors, in
[the ABI reference](../MikuOS_ABI.md); this page is about the machinery and
how the pieces relate.

## Contents

- [The model: tasks, processes and threads](#the-model-tasks-processes-and-threads)
- [Process ids](#process-ids)
- [Task states](#task-states)
- [Creating processes: fork, exec and the kernel's spawn](#creating-processes-fork-exec-and-the-kernels-spawn)
- [Ending: exit, kill, zombies and reaping](#ending-exit-kill-zombies-and-reaping)
- [wait4 and the status word](#wait4-and-the-status-word)
- [Threads](#threads)
- [The ELF loader](#the-elf-loader)
- [The initial stack and the auxiliary vector](#the-initial-stack-and-the-auxiliary-vector)
- [Futexes](#futexes)
- [Signals](#signals)
- [The vDSO](#the-vdso)
- [FPU state](#fpu-state)
- [Credentials](#credentials)
- [Resource limits](#resource-limits)
- [Syscall entry](#syscall-entry)
- [Reference tables](#reference-tables)
- [Diagnostics](#diagnostics)
- [Where the code is](#where-the-code-is)

---

## The model: tasks, processes and threads

The kernel schedules **tasks**. Every task is one `Process` struct
(`kernel/process/mod.rs`) with its own pid, kernel stack, saved registers,
`FS_BASE`, FPU area, alternate signal stack, scheduling state and run-queue
position. Kernel threads (drivers, mikuD's workers, the Mayland server) are
tasks too, with no user address space.

A **process** is a thread group: a set of tasks sharing one address space
(`cr3`) and identified by the pid of its first task, the **thread-group id**
(`tgid`). A **thread** is a task whose `tgid` names another task, the
group's leader. The kernel keeps every process-wide resource on the leader or
keyed by the tgid and reaches it through the leader:

| Per process (keyed by tgid or on the leader) | Per task |
|---|---|
| address space, VMAs, program break | pid (the thread id) |
| descriptor table (`fd_tables` in the VFS, one per tgid) | kernel stack (`mm::kstack`, 128 KiB for programs) |
| sockets (owned by tgid) | registers, `rsp`, `saved_user_rsp` |
| working directory, umask | `FS_BASE` (`saved_fs_base`) |
| credential (`cred`: ids, groups, capabilities) | FPU/SSE state (`fpu`) |
| controlling terminal (`ctty`), process group (`pgid`) | alternate signal stack (`sig_alt_sp`, `sig_alt_size`) |
| signal entry point, dispositions, pending set, `SA_ONSTACK` mask | scheduling: `state`, `vruntime`, `priority`, `cpu_mask`, run-queue slot |
| record locks, Mayland pools, GPU fences | an owned stack mapping to unmap at death (`owned_stack`) |
| parent (`ppid`), exit code, stop state | CPU accounting (`cpu_time`, switch counts) |

There is no `clone` with flags. Two calls make tasks: `fork` (43) makes a new
process, and `thread_create` (106) makes a thread in the caller's process.
Nothing in between (a task with its own descriptors but a shared address
space, and so on) can be expressed.

Each thread has its own **thread pointer** in `FS_BASE`, set at creation from
`ThreadArgs.tls` and changed with `set_tls` (9). It points at a 64-byte
thread control block whose layout the kernel, `ld-miku` and libmiku share
([ABI §7.6](../MikuOS_ABI.md#76-the-thread-control-block)); `errno` lives in it
at `tp + 40`.

---

## Process ids

Pids are 1 to 32767 (`MAX_PROCS` = 32768, Linux's default `pid_max`), shared
by processes and threads alike (a thread id is a pid).

- **`PROC_TABLE`** (`kernel/sched/proc_index.rs`) owns every boxed
  `Process`, in a `BTreeMap` under a mutex.
- **`PROC_INDEX`** is a flat array of 32768 pointers (256 KiB, static) for
  lock-free lookup from any context, the timer interrupt included. It is
  updated with interrupts off.
- **`PID_USED`** is a bitmap with one atomic bit per pid. It is both the
  allocator and the list of live tasks: `for_each_task` walks the set bits
  without a lock.
- **`alloc_pid`** takes the first free bit at or after a rotating cursor and
  wraps to 1, so a pid is reused **as late as possible**: the further apart two
  uses of a number are, the smaller the window in which a stale pid means
  somebody else. `free_pid` clears the bit only when nothing can name the task
  any more (after reaping).

Before 2026-09-22 the allocator was a counter that never wrapped, and the
4096th task of an uptime was registered nowhere; before 2026-10-23 the space
was 4096. Pid 1 is **mikuD**, which inherits orphans
([system.md](system.md#mikud)).

---

## Task states

| State | Value | Meaning |
|---|---|---|
| `READY` | 0 | on a run queue |
| `RUNNING` | 1 | on a CPU |
| `SLEEPING` | 2 | on the timer, with a deadline |
| `BLOCKED` | 3 | on a wait queue, woken by an event (and a backstop) |
| `DEAD` | 4 | finished; a zombie until its parent collects it |
| `STOPPED` | 5 | off the run queue and the timer until `SIGCONT` |

A task leaves `RUNNING` and is claimed from `READY` only by compare-and-swap,
and a reaper frees only a task that is *quiesced* (off every run queue, not
switching, not on a CPU, not current anywhere). `ps` shows one letter per
process: R, S, B, D, Z. Scheduling, run queues and waiting are in
[scheduler.md](scheduler.md).

---

## Creating processes: fork, exec and the kernel's spawn

### fork

`fork` (43, `kernel/syscall/adapters/process.rs` `sys_fork`):

1. reads the caller's user `rip`, `rflags`, `rsp` and the registers the
   syscall entry saved;
2. clones the address space **copy-on-write** (`AddressSpace::clone_cow`):
   private pages become read-only in both, and the first write copies;
   `MAP_SHARED` tmpfs pages stay shared; Mayland pool windows are lent, not
   copied ([memory.md](memory.md));
3. copies the VMA table and the break;
4. builds the child (`Process::new_fork`) under the caller's **tgid** as
   parent, with the caller's credential, cwd, umask, pgid and terminal, and
   returns into user mode with `rax` = 0;
5. copies the signal entry point (dispositions are on the process and are
   copied with it), and the descriptor table (`fork_fds`);
6. queues the child and returns its pid.

Only the calling thread is copied. Sibling CPUs that have the parent's page
tables loaded are flushed before any COW page can be written (the TLB
shootdown in [memory.md](memory.md)). Sockets are **not** inherited: they
are owned by the parent's tgid, and the child gets `EBADF` on their numbers.

There is no `vfork`.

### exec

`exec` (46) and `execve` (67) replace the calling process's image. The order
in `do_exec` matters:

1. Refuse if the caller is not the group leader (`EPERM`): POSIX would have
   the calling thread take over the process's pid, and every table here is
   keyed on pid.
2. Kill the other threads and wait until none is on a CPU; reap them.
3. Check execute permission on the path; open it; resolve a `#!` line
   (one level, absolute interpreter, the rest of the line as one argument).
4. Load the image into a **new** address space (`elf_loader::load`). Until
   this succeeds nothing about the process has changed, and every failure
   returns to the old image.
5. Close close-on-exec descriptors and sockets (waking pipe readers);
   register the image for crash reports; rename the process.
6. Switch to the new address space and free the old one.
7. Apply the credential rule `permitted &= bounding; effective = permitted`;
   clear the signal entry point; reset caught signals to default (ignored
   ones stay ignored); drop pending signals and the alternate stack; reset
   the FPU.
8. Set `FS_BASE`, point the return at the new entry with a fresh stack, and
   zero `rdi`, `rsi` and `rdx`.

`exec` (46) passes the kernel default environment (`PATH=/bin`, `HOME=/`,
`TERM=miku`, `OS=MikuOS`); `execve` (67) passes the caller's.

### What programs use

Ring-3 programs use **`fork` + `execve`**. Rust's
`std::process::Command` (`lib/rust-std/library/std/src/sys/process/miku.rs`)
builds argv, envp and every path before the `fork` (the child is
single-threaded and must not allocate), and reports an `execve` failure to the
parent over a close-on-exec pipe. `msh`, `mikudod`, `mayterm` and `mikupkg`
do the same through libmiku. `libc_miku` has no `posix_spawn`.

### The kernel's own spawn

The kernel starts programs without a parent process through
`process::exec::exec` and `exec_as(path, args, env, cred)`
(`kernel/process/exec.rs`): a fresh address space, the ELF load, a new
`Process` (`Process::new_elf`), the credential applied **before** the task is
queued (so it never runs a single instruction as anyone else), and a
`[exec] spawned pid=...` line in the log. mikuD starts its services this way,
and so do the kernel console's program commands. A program started this way
is a child of whoever the kernel names (mikuD for services). Kernel threads
are made with `sched::spawn_named` (`kernel/sched/lifecycle.rs`).

---

## Ending: exit, kill, zombies and reaping

### exit

`exit` (0) ends the whole process, in this order (`sys_exit`):

1. kill every other thread of the group (`kill_group_threads`);
2. **close the process's descriptors** (`release_fds_on_exit`), so a pipe's
   reader sees end of file and a terminal whose last master this was hangs up
   before the parent hears anything;
3. mark the group dead with the code (`kill_group_with_code`);
4. send `SIGCHLD` to the parent;
5. yield for the last time.

Descriptors are closed while the task is still alive because a `DEAD` task
that blocks is never resumed: closing can wait (a pipe wake, a flush), and it
must happen before the task is dead. `thread_exit` (107) from the leader is
`exit`; from any other thread it marks only that task dead.

### Killing

`kill_with_code` (`kernel/sched/lifecycle.rs`) is where every death ends up:
it stores the exit code, swaps the state to `DEAD`, unlinks the task from the
timer and the run queue, and puts it on the dead list. For a process (not a
thread) it then:

- **reparents its children to pid 1** (mikuD), which collects them;
- wakes the parent and the death queue (`wait::DEATH`);
- ends the session the process led, if any (`session::on_process_exit`), so a
  desktop that quits does not leave its login open;
- clears it as a terminal's foreground job, so the next Ctrl-C there does not
  reach whoever gets the pid next.

A death by signal goes through `kill_by_signal`, which records the signal
(`killed_by_sig`) so `wait4` reports `WIFSIGNALED` instead of inferring it from
the code. A task killed while inside a system call stops at the end of the
call (`stop_if_dead` in the dispatcher) and never returns to ring 3.

### Zombies and reaping

A dead process stays in `PROC_TABLE` as a **zombie** until its parent
collects it with `wait4` (`collected` set), because the status is owed to the
parent. A thread is never a zombie: `new_thread` sets its parent to 0 and
`collected` to true. Reaping (`reap_dead`, run on every spawn on every CPU,
and `reap_zombie` from `wait4`) frees a task in three steps, in this order
(`reap_one`):

1. **Claim** it with a compare-and-swap on `reaping`, so exactly one reaper
   handles it;
2. unmap the stack the thread asked the kernel to own, while the task is still
   in the table (so its group's address space cannot be torn down under it);
3. remove it from the table and, **under the same lock**, ask whether it was
   the last task of its group.

The last task of a group, whoever it is (the leader may go first), tears the
process down (`free_process_resources`): sockets, record locks, Mayland pools,
GPU fences, the descriptor table (hanging up terminals whose last master it
held), the VMA table and the address space. Every task frees its own kernel
stack and returns its pid.

---

## wait4 and the status word

`wait4(pid, &status, options)` (44) finds a child of the caller's **process**
(any thread of a process may wait for its children):

- `pid` is a child pid or -1 for any child; there are no process-group
  waits.
- With `WUNTRACED` (2) a stopped child not yet reported is returned first.
- Otherwise a dead child not yet collected is returned and reaped.
- No such child: `ECHILD` if the caller has no children at all, 0 with
  `WNOHANG` (1), `EINTR` if a signal the process will handle is pending, else
  sleep on the death queue (100 ms backstop) and look again.

The status is written as a **64-bit** POSIX word: `code << 8` for an exit,
the signal number for a death by signal, `0x7f | sig << 8` for a stop. This
replaced, on 2026-09-17, the old practice of reporting the raw code and
spelling a signal death as `128 + sig`, which could not tell `exit(137)` from
`SIGKILL`. `lib/miku`'s `proc::exited`, `signalled`, `stopped`,
`exit_code`, `term_sig`, `stop_sig` decode it, and `wait_status_code` gives
back the `128 + sig` number a shell prints.

---

## Threads

`thread_create` (106) takes a 64-byte block: entry, stack, argument, TLS
pointer, flags, an optional owned stack range, and a reserved word that must be
zero. The new task:

- shares the leader's `cr3` and carries the leader's pid as its tgid;
- starts at `entry` with the argument in `rdi`, `rsp` = `stack` (16-byte
  aligned), nothing on the stack, and `FS_BASE` = `tls`;
- inherits the leader's controlling terminal and process group;
- has its own kernel stack, registers, FPU state and alternate signal stack.

**Owned stacks.** With `THREAD_STACK_OWNED` the kernel unmaps
`[stack_base, stack_base + stack_len)` when the thread is reaped. A thread
cannot unmap the stack it stands on, and a detached thread has nobody to do it
later, so this is how `std::thread` frees a dropped thread's stack.

**libmiku** (`lib/libmiku/thread.rs`): `miku_thread_spawn` and
`miku_thread_spawn_owned` map a stack with a 4096-byte guard page below it
(at least 8 KiB usable), build the new thread's TLS block by copying the
template the caller's own block describes (`tp + 8 .. tp + 32`), and start the
thread in a trampoline that calls `thread_exit` when the function returns.
`libc_miku`'s pthreads (`lib/libmiku/c/pthread.rs`) and Rust's `std::thread`
sit on top; see [sdk.md](sdk.md).

Rules that follow from the model:

- `exit` from any thread ends the process; `thread_exit` from the leader
  ends the process too (POSIX's `pthread_exit` from `main` keeping the others
  alive is not provided).
- `exec` from a thread other than the leader is refused.
- A signal is delivered to the process, which in practice means its leader;
  a fatal default ends the whole group.
- `getpid` (7) answers the process from every thread; `gettid` (108) is the
  thread's own id (see [ABI §4.1](../MikuOS_ABI.md#7-getpid)).
- `procinfo` lists processes only; threads are not shown by `ps`.

If the process starts exiting while a thread is being created, the new thread
is killed at once and `thread_create` returns `EAGAIN`.

---

## The ELF loader

`kernel/process/elf_loader.rs` loads every user image, for `exec`, `execve`
and the kernel's own spawn. It reads the file **by range** through an
`ElfSource` (`kernel/process/elf_source.rs`): a file open for the duration of
the load, or a byte slice for images already in memory (the embedded
`ld-miku.so`, the vDSO, the selftest's hostile images). No file is held whole
in the kernel heap.

The flow of `elf_loader::load`:

1. **Parse** (`kernel/process/elf.rs`): headers, program headers, notes,
   `PT_INTERP`, `PT_TLS`, `PT_GNU_STACK`, `PT_GNU_RELRO`, and the total mapped
   size (at most 1 GiB, `MAX_IMAGE_MEM`).
2. **Reject** overlapping `PT_LOAD` segments, a segment that is writable and
   executable (W^X), segments that would land outside the user range or above
   the image ceiling (the space the loader reserves for the stack and TLS), and
   an entry point that is not inside an executable segment. All of this
   happens before the first frame is allocated, so a hostile image costs
   nothing.
3. **Place**: an `ET_DYN` (PIE) image goes to `0x5555_0000_0000` plus a random
   offset (28 bits of 4 KiB steps); an `ET_EXEC` image keeps its addresses.
   A request for an executable stack in `PT_GNU_STACK` is logged and ignored.
4. **Map** every `PT_LOAD` straight from the file into the frames it fills,
   in physically contiguous runs of up to 2 MiB. Read-only segments are shared
   with any process already running the same file (`process::imgcache`).
5. **Relocate** a PIE image's own relocations (`process::reloc`).
6. **RELRO**: for an image with no interpreter, every `PT_GNU_RELRO` range is
   made read-only now; with an interpreter, `ld-miku` does it after its own
   relocations. A RELRO range outside the image is ignored with a log line.
7. **Heap**: the break starts at the image end plus a random gap of up to
   32 MiB.
8. **Stack**: the top is `0x7FFF_FFFF_0000` minus up to 16 MiB; an 8 MiB
   mapping of which the top 256 KiB is mapped now.
9. **TLS**: for a program without an interpreter, the first thread's TLS block
   and 64-byte TCB are built 16 MiB below the stack (alignment above a page is
   refused).
10. **Interpreter**: `PT_INTERP` (at most 128 bytes) is loaded at
    `0x7F00_0000_0000` plus a random offset of up to 2 GiB. It must be
    position-independent and must not have an interpreter of its own.
    `/lib/ld-miku.so` is embedded in the kernel and never read from disk.
11. **Stack contents and auxv** (next section), and the vDSO pages.

The result (`ElfImage`) carries the entry to jump to (the interpreter's, if
any), the stack top, the break, the load bias and the TLS base. Errors
(`LoadError`) are printed as `[exec] ELF load failed: <reason>`, and the
system call returns `ENOENT`:

| Reason | Meaning |
|---|---|
| parse error | not a valid x86-64 ELF64, or a header out of range |
| out of memory / page mapping failed | frames or page tables ran out |
| ELF file too large | over the mapped-size limit |
| overlapping PT_LOAD segments | |
| W+X segment rejected (W^X policy) | |
| cannot read / load ELF interpreter | `PT_INTERP` names a missing or bad file |
| relocation failed | a relocation outside the image or of an unsupported type |
| argv/envp/auxv too large for user stack | over 256 entries, 32 KiB per string, 128 KiB in all |
| segment lands outside the user address range | |
| entry point is not inside an executable segment | |
| ELF interpreter has an interpreter of its own / is not position-independent / bad PT_INTERP path | |

**Dynamic linking.** For a program with `PT_INTERP`, control goes to
`ld-miku`, which maps the `DT_NEEDED` libraries through `map_lib` (15) from
the kernel's shared-object cache or `/lib` and `/usr/lib`, relocates, applies
RELRO, builds the TCB, hands the auxiliary vector to libmiku and jumps to the
program. The program's side of this is in [sdk.md](sdk.md#dynamic-linking).

**The hostile-image selftest.** `kernel/process/selftest.rs` builds
adversarial ELF images in memory and feeds them to the real parser and loader,
each case a bug that was once reachable: a segment mapped into the kernel half,
a relocation pointing at kernel memory, a `DT_RELASZ` large enough to hang the
machine, an entry point that would fault `sysret` in ring 0, RELRO outside the
image, and more. Placement cases load into a throwaway address space that is
freed after each case. Run it with `elf test` in the kernel console or
`run=elf_test` on the boot line ([testing.md](testing.md)). One RELRO case is
a known failure that predates the current work.

---

## The initial stack and the auxiliary vector

The loader builds the stack top-down and refuses up front (not halfway) if the
strings and vectors do not fit the 256 KiB mapped at exec:

```
low  [rsp]       argc
     [rsp+8]     argv pointers, NULL
                 envp pointers, NULL
                 auxv (key, value) pairs, AT_NULL
                 alignment padding
                 strings: argv, envp, "x86_64", the exec path, 16 random bytes
high             (stack top)
```

`rsp` is 16-byte aligned at entry. The 21 auxv entries, in order: `AT_PHDR`,
`AT_PHENT`, `AT_PHNUM`, `AT_PAGESZ` (4096), `AT_BASE` (interpreter base),
`AT_FLAGS` (0), `AT_ENTRY`, `AT_UID`, `AT_EUID`, `AT_GID`, `AT_EGID` (from the
credential the image will run with), `AT_SECURE` (1 when the effective ids
differ from the real ones), `AT_RANDOM`, `AT_HWCAP` (CPUID.1:EDX), `AT_HWCAP2`
(0), `AT_CLKTCK` (250), `AT_PLATFORM` ("x86_64"), `AT_EXECFN`,
`AT_MINSIGSTKSZ` (2048), `AT_SYSINFO_EHDR` (the vDSO), `AT_NULL`. The values
and key numbers are tabulated in
[ABI §7.5](../MikuOS_ABI.md#75-auxiliary-vector).

---

## Futexes

`futex` (110, `kernel/sched/futex.rs`) is `FUTEX_WAIT` and `FUTEX_WAKE`, and
it is all `std::sync` needs: upstream's `Mutex`, `Condvar`, `RwLock`, `Once`
and thread parking compile unchanged on a futex target.

- **Wait** compares the 32-bit word with the expected value under the bucket
  lock and sleeps if they match (`EAGAIN` at once if not). It sleeps on its
  bucket with the caller's deadline (relative `timespec`, nanosecond precision
  on the TSC timer) and a 100 ms backstop, so it may return 0 without anybody
  having woken it. Callers loop, as everywhere.
- **Wake** wakes up to `n` waiters on the same key and returns how many.
- **Keys**: the pair (address space, virtual address) for an ordinary page, a
  *private* futex, exact for threads and immune to a copy-on-write fault
  moving the word. For a page mapped `MAP_SHARED` from a tmpfs file
  (`PTE_SHARED`) the key is the physical frame, so processes mapping the same
  file meet on it; that is what `PTHREAD_PROCESS_SHARED` mutexes need (LMDB's
  reader table, for example). An ext file mapping is private per process, and
  so is a futex in it.
- 64 hash buckets, each a spinlock over a list of waiters.
- A signal does not interrupt a futex wait.

There is no requeue, no priority inheritance, no bitset and no robust list.

---

## Signals

The design, decisions and history are in
[dev/signal-architecture.md](../../dev/signal-architecture.md).

### Who keeps what

| Kernel (`kernel/process/signal/`) | `sys_miku.so` (`lib/libmiku/signal.rs`) |
|---|---|
| disposition per signal, 2 bits each in one `u64` (`sig_disp`): default, ignore, catch | the handler table (`miku_signal`, `miku_sigaction`) |
| `SA_ONSTACK` per signal (`sig_onstack`) | `sa_mask`, `SA_NODEFER`, `SA_RESETHAND`, `SA_SIGINFO` |
| pending bits (`pending_sig`, one per signal) | the **blocked mask**, one per process (`BLOCKED_MASK`) |
| the dispatcher's entry address (`sig_entry`, set by 68) | the entry stub that saves the FPU and calls the handler |
| "in a handler" flag (`sig_in_delivery`) | signals held while blocked, delivered on unblock |
| per-task alternate stack | |

Signals 1 to 31 exist (`NSIG` = 32).

### Sending

`send_signal(pid, sig)` addresses the **process** (it redirects a thread id to
its tgid) and decides at once, with interrupts off:

1. `SIGKILL`: kill now.
2. For any signal but `SIGKILL` and `SIGSTOP`: ignored, drop it; caught and an
   entry point registered, set the pending bit and stop here.
3. Otherwise the default: stop, continue, ignore or kill
   ([table below](#signals-and-their-defaults)).

Because fatal defaults are applied at send time, Ctrl-C kills a CPU-bound loop
that never makes a system call. Group sends (`kill(-pgid, sig)`, terminal keys
to the foreground job) go member by member through the same function.

### Delivery

A **pending caught signal** is delivered at the end of a system call:
`dispatch` calls `signal::deliver_pending(ret)` on every return. If a signal is
deliverable (an entry is registered, no handler is running, a bit is pending)
the kernel builds a `TrapFrame` from what the entry saved (all general
registers, including the `rdi`, `rsi`, `rdx` the program called with, and
`rax` = the call's return value), writes the signal frame on the user stack,
and rewrites the `sysret` target to the entry stub. All of this happens with
interrupts off, so the task cannot migrate between reading its user `rsp` and
redirecting it (a migration once put two threads on one stack). A program that
never makes a system call never runs a handler for a signal somebody sent.

**One handler at a time**: while `sig_in_delivery` is set, no new frame is
pushed for a sent signal; it waits for `sigreturn`.

### The frame

1072 bytes on the user stack, below the interrupted `rsp` past the 128-byte red
zone, or on the alternate stack:

| Offset | Size | Content |
|---|---|---|
| 0 | 128 | `siginfo_t`: `si_signo`, `si_errno`, `si_code`, `si_addr` at +16 |
| 128 | 424 | `ucontext_t` in Linux's layout up to `uc_sigmask`: `uc_flags`, `uc_link`, `uc_stack`, `gregs[23]` (Linux `REG_*` order), `fpregs` pointer, `uc_sigmask` (zero) |
| 552 | 8 | padding |
| 560 | 512 | `fxsave64` area, filled and restored by the stub |

The full offsets are in [ABI §5](../MikuOS_ABI.md#5-structures). The shape is
Linux's so that crates reading a machine context (wasmtime is the motivating
one) need only `target_os = "miku"` in a `cfg`; the reader's side is
`lib/rust-libc/libc/src/unix/miku/mod.rs`.

The kernel owns the general registers; the stub owns the FPU: it `fxsave`s
into the frame, calls `handler(sig, siginfo_t *, ucontext_t *)`, `fxrstor`s,
and makes `sigreturn`.

### sigreturn

`sigreturn` (69) is diverted by the syscall entry before anything is saved.
`sigreturn_full` reads the frame, checks that `rip` and `rsp` are in the user
half, keeps only the status bits of `rflags` with IF forced on, forces the
user selectors, and builds a `TrapFrame` that `iretq` leaves through. Every
general register comes from the frame, including any the handler changed
(that is how a handler can redirect execution or fix up a faulting
instruction), and `rcx`/`r11` come back intact, which `sysret` could not
do. A bad frame kills the process with `SIGSEGV`.

### SA_RESTART and interrupted calls

There is no restart. `libc_miku` accepts `SA_RESTART` and ignores it. The
calls that return `EINTR` when a deliverable signal is pending are `wait4`,
a write to a full pipe, a read of an inotify descriptor, `fcntl(F_SETLKW)` and
`TCSETS` from a background job; every other blocking call finishes on its own
terms and the handler runs afterwards
([ABI §2.6](../MikuOS_ABI.md#26-blocking-signals-and-eintr)).

### Alternate stacks

`sigaltstack` (115) sets the calling task's alternate stack (at least 2048
bytes). A handler installed with `SA_ONSTACK` runs on it when the task has
one and is not already on it. Changing it while on it is `EPERM`. `exec`
removes it.

### Faults are signals

Since 2026-09-23 a CPU exception in ring 3 is a signal with a full frame:

| Exception | Signal | `si_code` | `si_addr` |
|---|---|---|---|
| `#PF` page fault (not resolved by demand paging or COW) | `SIGSEGV` | `SEGV_MAPERR` (1) no mapping, `SEGV_ACCERR` (2) protection | the faulting address (`CR2`) |
| `#GP` general protection | `SIGSEGV` | `SI_KERNEL` (0x80) | 0 |
| `#UD` invalid opcode | `SIGILL` | `ILL_ILLOPN` (2) | the instruction |
| `#DE` divide error | `SIGFPE` | `FPE_INTDIV` (1) | the instruction |

The exception entry (`kernel/arch/x86_64/trap.rs`) first moves off the IST
stack onto the task's own kernel stack, then `trap::user_trap` asks
`deliver_fault`: if the process catches that signal and has an entry point, the
frame is written at the faulting instruction (`gregs` hold the exact registers
of the fault, `ERR`, `TRAPNO` and `CR2` filled) with `UC_MIKU_FAULT` in
`uc_flags`, and the handler runs. Returning from such a frame without changing
`rip` re-executes the instruction, which faults again; the stub knows not to
do that blindly. A fault inside a handler is delivered too (bounded by the
stack: a frame that cannot be written is the crash). If the signal is not
caught, it is a crash: a report, a core file, and the whole process dies of
the signal ([diagnostics.md](diagnostics.md#ring-3-crash-reports)).

### The direction flag on kernel entry

A program may leave the direction flag set (`std`, for a backwards copy). The
`syscall` instruction clears it through `SFMASK`, but an exception or
interrupt does not, and every `rep movs` the kernel runs with DF set copies
backwards from its destination. So every naked entry stub from ring 3 executes
`cld` before calling Rust: `user_trap_entry` in `trap.rs` and the timer stub
in `interrupts.rs` (the `x86-interrupt` handlers get it from the compiler).
`iretq` gives the program its flag back. The missing `cld` was found on
2026-10-13, as aosh crashes and `no signal for vector <addr>` lines.

### Job control signals

`SIGTSTP` (Ctrl-Z), `SIGSTOP`, `SIGTTIN` and `SIGTTOU` stop a process:
`STATE_STOPPED`, off the run queue and off the timer, until `SIGCONT`. A stop
is reported once to a parent waiting with `WUNTRACED`. `SIGCHLD` is not sent
for a stop. Terminal-side rules (who gets Ctrl-C, background reads and writes)
are in [input-and-terminals.md](input-and-terminals.md).

---

## The vDSO

Two pages mapped into every process at a random address, the same physical
frames everywhere (`kernel/process/vdso.rs`, `lib/vdso/`):

- **vvar** (one page below the code, read-only, no-execute) holds the time
  data: magic `MIKU`, version 2, a seqlock word, the TSC frequency, a
  wall-clock anchor (TSC value, seconds, nanoseconds), the boot TSC, and a
  feature word (bit 0: `RDTSCP`).
- **vdso** (read + execute) is a small ELF image exporting
  `__vdso_clock_gettime`, `__vdso_gettimeofday` and `__vdso_getcpu`.

The code finds its data by subtracting one page from its own base, so nothing
else needs to tell it where the page is. Readers take the seqlock: read `seq`,
retry while odd, read the fields, retry if `seq` changed. The layout is one
file, `lib/vdso/src/abi.rs`, included verbatim by both the kernel writer and
the vDSO reader. Each function falls back to the system call when the data is
unusable (TSC uncalibrated, wall clock not set, magic or version wrong), and
`__vdso_getcpu` uses `RDTSCP` only when the feature bit says the CPU has it
(QEMU's default `qemu64` CPU does not, and executing it there was a `SIGILL`).
The program finds the vDSO through `AT_SYSINFO_EHDR`; libmiku looks the symbols
up once (`lib/libmiku/auxv.rs`). The table of fields is in
[ABI §7.7](../MikuOS_ABI.md#77-the-vdso).

---

## FPU state

The x87/SSE registers (`xmm0-15`, the x87 stack, `MXCSR`) are part of a task
(`kernel/arch/x86_64/fpu.rs`):

- saved and restored **eagerly** on every context switch with `FXSAVE`/
  `FXRSTOR` (512 bytes), not lazily through `CR0.TS` and `#NM`;
- `FXSAVE`, not `XSAVE`: `CR4.OSXSAVE` is not set, so AVX state cannot exist
  on this machine and 512 bytes is the whole state;
- a new process starts from a clean state; `exec` resets it; a thread starts
  from a clean state.

The kernel itself is built soft-float and never touches `xmm`, which is why
two float-using programs used to corrupt each other's registers without
anything noticing until 2026-09-22. A signal handler's FPU state is saved by
the entry stub into the frame, not by the kernel.

---

## Credentials

Each process has one `Cred` (`kernel/process/cred.rs`) behind a reader-writer
lock, changed as a whole so no permission check sees half a change:

| Field | Meaning |
|---|---|
| `uid`, `euid`, `suid` | real, effective, saved user id (16-bit, at most `0xFFFE`) |
| `gid`, `egid`, `sgid` | the same for groups |
| `groups[16]`, `ngroups` | supplementary groups |
| `permitted`, `effective`, `bounding` | capability sets, one bit per capability |
| `umask` | file creation mask |

Every VFS permission check reads the calling thread's process credential.
`euid == 0` is not special by itself: root is simply the credential that holds
all sixteen capabilities (`setuid`, `setgid`, `kill`, `dac_override`,
`dac_read`, `fowner`, `chown`, `sys_time`, `sys_admin`, `sys_boot`, `net_raw`,
`net_bind`, `mknod`, `may_priv`, `audit`, `gpu`). The transition rules:

- `setuid(u)` with `CAP_SETUID` sets all three ids and drops every capability
  if `u != 0`; without it, only the effective id, and only to an id already
  held.
- `setresuid` changes all three in one step; with `CAP_SETUID` it drops the
  capabilities when all three resulting ids are non-zero.
- `capset` only narrows. The bounding set survives `fork` and `exec`.
- On `exec`: `permitted &= bounding; effective = permitted`. **No setuid
  bit, setgid bit or file capability exists**: privilege is granted by a
  privileged parent (mikuD for services, `mikudod` for `mikudo`), never by a
  file.
- `CAP_GPU` gates loading GPU machine code (`gpu_program`, 98).
- `CAP_MAY_PRIV` follows the seat: opening a session moves it from the greeter
  to the session's leader.

The calls are in [ABI §4.9](../MikuOS_ABI.md#49-identity-and-capabilities-47-55-86-91).
Who gets which credential (service units, `/etc/mikudo.conf`, login) is the
policy in [system.md](system.md) and
[MikuOS_Security.md](../MikuOS_Security.md).

---

## Resource limits

None in the kernel: no `RLIMIT_*`, no per-user process count, no memory
limit per process. The hard limits that exist are global or structural: 32768
pids, 512 descriptors per process, 256 sockets on the machine (160 per
account), 64 display pools (16 per process), a 128 KiB argument budget per
`exec`, an 8 MiB stack mapping. `libc_miku`'s `getrlimit` answers
`RLIMIT_NOFILE` = 512 and `RLIM_INFINITY` for everything else, and
`setrlimit` succeeds only for what `getrlimit` already reports.

---

## Syscall entry

`init` in `kernel/syscall/mod.rs` sets `EFER.SCE`, `STAR` (user and kernel
selectors), `LSTAR` (the entry) and `SFMASK` (IF, AC and DF cleared on
entry). The naked `syscall_handler`:

1. `swapgs`; saves the user `rsp` in the per-CPU slot `gs:[0x18]`; loads the
   task's kernel stack from `gs:[0x10]`;
2. if `rax` is 69, jumps to the `sigreturn` path;
3. pushes `rcx` (user `rip`), `r11` (user `rflags`), `rbp`, `rbx`, `r12`-`r15`,
   `r10`, `r9`, `r8`, `rdi`, `rsi`, `rdx` (fourteen pushes keep the stack
   16-byte aligned for the call);
4. moves the arguments into the C ABI order and calls `dispatch(nr, a1, a2, a3,
   a4)`;
5. on return: `cli` (the handler may come back with interrupts on, and an
   interrupt between the stack switch and `sysretq` would push onto the user
   stack), pops everything, restores the user `rsp`, `swapgs`, `sysretq`.

`dispatch` runs the handler, then `sched::stop_if_dead()` (a task killed during
the call stops here), then `signal::deliver_pending(ret)`.

**Register convention**: number in `rax`; arguments in `rdi`, `rsi`, `rdx`,
`r10` (four at most; larger calls pass a block); result in `rax`, negative for
an errno; only `rcx` and `r11` are clobbered. **Errno**: the kernel returns
`-errno`; `libc_miku` converts it to `-1` and the thread's `errno` at
`%fs:0 + 40`. **User memory** is touched only through the checked SMAP window
of `kernel/syscall/usercopy.rs`.

System call numbers by area (each linked to its entry in the ABI):

| Area | Calls |
|---|---|
| Process lifetime | [0 exit](../MikuOS_ABI.md#0-exit), [7 getpid](../MikuOS_ABI.md#7-getpid), [43 fork](../MikuOS_ABI.md#43-fork), [44 wait4](../MikuOS_ABI.md#44-wait4), [45 kill](../MikuOS_ABI.md#45-kill), [46 exec](../MikuOS_ABI.md#46-exec), [67 execve](../MikuOS_ABI.md#67-execve), [103 procinfo](../MikuOS_ABI.md#103-procinfo), [105 proc_path](../MikuOS_ABI.md#105-proc_path) |
| Threads | [106 thread_create](../MikuOS_ABI.md#106-thread_create), [107 thread_exit](../MikuOS_ABI.md#107-thread_exit), [108 gettid, 109 sched_yield](../MikuOS_ABI.md#108-gettid-109-sched_yield), [110 futex](../MikuOS_ABI.md#110-futex) |
| Signals | [68 sigentry](../MikuOS_ABI.md#68-sigentry), [69 sigreturn](../MikuOS_ABI.md#69-sigreturn), [100 sigdisp](../MikuOS_ABI.md#100-sigdisp), [115 sigaltstack](../MikuOS_ABI.md#115-sigaltstack) |
| Memory and TLS | [3 mmap](../MikuOS_ABI.md#3-mmap), [4 munmap, 5 mprotect](../MikuOS_ABI.md#4-munmap-5-mprotect), [6 brk](../MikuOS_ABI.md#6-brk), [9 set_tls, 10 get_tls](../MikuOS_ABI.md#9-set_tls-10-get_tls), [15 map_lib](../MikuOS_ABI.md#15-map_lib), [60 mmap_file](../MikuOS_ABI.md#60-mmap_file), [61 msync](../MikuOS_ABI.md#61-msync) |
| Identity | [47-55, 86-91](../MikuOS_ABI.md#49-identity-and-capabilities-47-55-86-91) |
| Jobs and terminals | [99 tty](../MikuOS_ABI.md#99-tty), [101 setpgid](../MikuOS_ABI.md#101-setpgid), [102 getpgid](../MikuOS_ABI.md#102-getpgid) |
| Sessions, power | [92 session](../MikuOS_ABI.md#92-session), [93 power](../MikuOS_ABI.md#93-power) |
| Time | [16 sleep](../MikuOS_ABI.md#16-sleep), [17 uptime](../MikuOS_ABI.md#17-uptime), [70 clock_gettime](../MikuOS_ABI.md#70-clock_gettime), [122 sleep_ns](../MikuOS_ABI.md#122-sleep_ns) |
| Everything else | [ABI §3, the full index](../MikuOS_ABI.md#3-index-of-all-system-calls) |

---

## Reference tables

### Signals and their defaults

| Nr | Signal | Default | Catchable | Raised by the kernel |
|---|---|---|---|---|
| 1 | `SIGHUP` | kill | yes | terminal hangup, to the foreground job |
| 2 | `SIGINT` | kill | yes | Ctrl-C |
| 3 | `SIGQUIT` | kill | yes | Ctrl-\\ |
| 4 | `SIGILL` | kill | yes | `#UD` |
| 5-7 | | kill | yes | |
| 8 | `SIGFPE` | kill | yes | `#DE` |
| 9 | `SIGKILL` | kill | **no** | |
| 10 | | kill | yes | |
| 11 | `SIGSEGV` | kill | yes | `#PF`, `#GP` |
| 12 | | kill | yes | |
| 13 | `SIGPIPE` | kill | yes | write to a pipe with no reader |
| 14 | | kill | yes | |
| 15 | `SIGTERM` | kill | yes | mikuD stopping a service |
| 16 | | kill | yes | |
| 17 | `SIGCHLD` | ignore | yes | a child exited |
| 18 | `SIGCONT` | continue | yes | |
| 19 | `SIGSTOP` | stop | **no** | |
| 20 | `SIGTSTP` | stop | yes | Ctrl-Z |
| 21 | `SIGTTIN` | stop | yes | background read of the terminal |
| 22 | `SIGTTOU` | stop | yes | background `TCSETS`, or a background write with `tostop` |
| 23 | `SIGURG` | ignore | yes | |
| 24-27 | | kill | yes | |
| 28 | `SIGWINCH` | ignore | yes | terminal resized |
| 29-31 | | kill | yes | |

A process killed by a signal has its signal recorded, and `wait4` reports it
as `WIFSIGNALED` with `WTERMSIG` = the signal.

### Task creation flags

There is no `clone`. What exists:

| Call | Flag or field | Effect |
|---|---|---|
| `thread_create` (106) | `flags` = `THREAD_STACK_OWNED` (1) | the kernel unmaps `stack_base .. +stack_len` when the thread is reaped |
| `thread_create` (106) | `tls` | the new thread's `FS_BASE` |
| `fork` (43) | none | full process copy, COW |

Every other bit of `flags`, and a non-zero `reserved`, is `EINVAL`.

### Auxiliary vector

| Key | Nr | Value |
|---|---|---|
| `AT_NULL` | 0 | end |
| `AT_PHDR` | 3 | program header address |
| `AT_PHENT` | 4 | program header size |
| `AT_PHNUM` | 5 | program header count |
| `AT_PAGESZ` | 6 | 4096 |
| `AT_BASE` | 7 | interpreter base, 0 without one |
| `AT_FLAGS` | 8 | 0 |
| `AT_ENTRY` | 9 | the program's entry |
| `AT_UID`, `AT_EUID`, `AT_GID`, `AT_EGID` | 11-14 | the image's ids |
| `AT_PLATFORM` | 15 | "x86_64" |
| `AT_HWCAP` | 16 | CPUID.1:EDX |
| `AT_CLKTCK` | 17 | 250 |
| `AT_SECURE` | 23 | effective ids differ from real |
| `AT_RANDOM` | 25 | 16 random bytes |
| `AT_HWCAP2` | 26 | 0 |
| `AT_EXECFN` | 31 | the exec path |
| `AT_SYSINFO_EHDR` | 33 | vDSO base, 0 if none |
| `AT_MINSIGSTKSZ` | 51 | 2048 |

---

## Diagnostics

| Question | Where to look |
|---|---|
| Which processes exist, their state, CPU, terminal | `ps` (built on `procinfo`) |
| A task that never runs, or runs too much | boot with `schedwatch` (dumps tasks); [scheduler.md](scheduler.md) |
| Why `exec` failed | the serial line or `dmesg`: `[exec] ELF load failed: <reason>` |
| Which program a pid runs | `ps -f` (`proc_path`) |
| A crash in a program | the `[crash]` report and `/tmp/core.<name>.<pid>`; [diagnostics.md](diagnostics.md#ring-3-crash-reports) |
| Process start and end | `[exec] spawned pid=...` and `[sched] kill pid=... code=...` in `dmesg` (debug level) |
| Per-thread creation and exit | boot with `trace`: `[thread] tgid=... tid=...`, `[sched] kill tid=...` |
| A signal frame that could not be written | `[signal] pid=N sig=S frame unwritable, killing` on serial |
| A bad `sigreturn` | `[signal] pid=N sigreturn with a bad frame at ...` |
| The loader on hostile input | `elf test` / `run=elf_test` |
| Signal delivery end to end | `run=` suites in [testing.md](testing.md); the test programs `sigtest`, `threadtest`, `polltest`, `crashtest` (`apps/src/`) and `stdsignal`, `stdthreads`, `stdproc` (`stdapps/src/`) |

---

## Where the code is

| What | Where |
|---|---|
| `Process`, states, `new_fork`, `new_thread`, `new_elf` | `kernel/process/mod.rs` |
| Pid allocator, process table and index | `kernel/sched/proc_index.rs` |
| Spawning kernel threads, killing, reaping, zombies, orphans | `kernel/sched/lifecycle.rs` |
| fork, exec, wait4, kill, setpgid, procinfo | `kernel/syscall/adapters/process.rs` |
| thread_create, thread_exit, futex adapter | `kernel/syscall/adapters/thread.rs` |
| Futex buckets and keys | `kernel/sched/futex.rs` |
| Kernel spawn (`exec`, `exec_as`), `#!`, default environment | `kernel/process/exec.rs` |
| ELF parse / load / relocate / RELRO / TLS / stack / auxv | `kernel/process/elf.rs`, `elf_loader.rs`, `elf_source.rs`, `reloc.rs`, `imgcache.rs` |
| Hostile-image selftest | `kernel/process/selftest.rs` |
| Signals: send, dispositions | `kernel/process/signal/mod.rs` |
| Signals: delivery, faults, sigreturn, sigaltstack | `kernel/process/signal/deliver.rs` |
| Signal frame layout | `kernel/process/signal/frame.rs` |
| Exception entry from ring 3, `cld` | `kernel/arch/x86_64/trap.rs`, `interrupts.rs` |
| FPU state | `kernel/arch/x86_64/fpu.rs` |
| vDSO | `kernel/process/vdso.rs`, `lib/vdso/src/abi.rs`, `lib/vdso/src/main.rs` |
| Credentials, capabilities | `kernel/process/cred.rs` |
| Sessions | `kernel/process/session.rs` |
| Crash reports and cores | `kernel/process/crash.rs`, `kernel/process/coredump.rs`, `kernel/process/symbols.rs` |
| Syscall entry and dispatch | `kernel/syscall/mod.rs` |
| libmiku threads and futex wrappers | `lib/libmiku/thread.rs` |
| libmiku signals (handler table, mask) | `lib/libmiku/signal.rs` |
| Rust std's process spawning | `lib/rust-std/library/std/src/sys/process/miku.rs` |
