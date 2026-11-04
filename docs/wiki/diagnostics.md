# Diagnostics

Everything MikuOS offers for finding out what the machine is doing and why it
went wrong: the kernel log and its viewers (`dmesg`, `nvidia log`), the serial
mirror, the persistent RAM log that survives a warm reboot (`pstore`), crash
reports for programs and for the kernel, core files, every file under `/proc`
and every node under `/dev`, the lock statistics in `/proc/lockstat`, the
debugging boot flags, the kernel console's `info` and related commands, and a
set of recipes for the usual problems (a hang, a panic, a slow call, a leak).

## Contents

- [The kernel log](#the-kernel-log)
- [Reading the log: dmesg and nvidia log](#reading-the-log-dmesg-and-nvidia-log)
- [The serial line](#the-serial-line)
- [pstore: the log that survives a warm reboot](#pstore-the-log-that-survives-a-warm-reboot)
- [Ring-3 crash reports](#ring-3-crash-reports)
- [Core files](#core-files)
- [Kernel faults and panics](#kernel-faults-and-panics)
- [/proc reference](#proc-reference)
- [/proc/lockstat](#proclockstat)
- [/dev reference](#dev-reference)
- [Console diagnostic commands](#console-diagnostic-commands)
- [Debugging boot flags](#debugging-boot-flags)
- [How to debug X](#how-to-debug-x)
- [Where the code is](#where-the-code-is)

---

## The kernel log

Since 2026-09-29 every line the kernel prints is a **record** in memory, with
a level set where the event happened (`kernel/kcore/klog/`, design in
[dev/klog.md](../../dev/klog.md)). The log exists from the first line of boot:
it is static (in `.bss`), a writer never allocates, so interrupt handlers and
the earliest boot code may log.

### A record

| Field | Content |
|---|---|
| `seq` | global sequence number; the three rings merge by it, and a reader can say how many lines it lost |
| time | microseconds since boot (TSC; 0 before calibration) |
| level | `E` error, `W` warning, `N` notice, `I` info, `D` debug, `T` trace |
| CPU, pid | who wrote it |
| module, line | `module_path!()` and `line!()` of the call site |
| flags | `echo` (a console line mirrored into the log), `cut` (truncated), `user` (from a program, syscall 75) |
| text | up to 1000 bytes (the serial line still gets all of it) |

### Three rings

| Ring | Levels | Size |
|---|---|---|
| errors | error, warning | 256 KiB |
| main | notice, info | 1 MiB |
| debug | debug, trace | 1 MiB |

Chatter evicts chatter: an error stays until 256 KiB of *errors* have come
after it, however much debug output follows. One lock covers all three (a
spinlock with interrupts off, held for a copy). A writer that cannot get it in
a bounded wait counts a drop and goes on, so a CPU halted by a panic while
holding it cannot take the panic message with it.

### Writing to it

| From | How | Level, destination |
|---|---|---|
| kernel code | `kerror!`, `kwarn!`, `knotice!`, `kinfo!` | that level; log and serial |
| kernel code | `kdebug!` | debug; log only (serial too under `trace`) |
| kernel code | `trace_println!` | trace; only formatted at all under `trace` (hot paths) |
| kernel code | `serial_println!`, `println!`, `print!`, `cprint!` | info, with the call site's module; console lines carry `echo` |
| kernel code | `log_err!` | error |
| GPU driver | `nvidia::said(Level::Error, &say, text)` | the level given, through a `say` sink (`klog::at_least`) |
| a program | `klog` (75), `miku_klog(buf, len)` | info, module `user`, flag `user`, the caller's pid; log only |

`klog::speaking_for(&SITE, f)` signs every line committed while `f` runs with
another module (how display lines printed through the boot's `say` are
attributed to `blackwell::display`), and `klog::unrecorded(f)` keeps lines off
the log and the serial line (screen only). Lines printed by the log viewer
itself are never recorded.

---

## Reading the log: dmesg and nvidia log

`dmesg` is a kernel console command. It needs `CAP_SYS_ADMIN` (the log holds
addresses and what every other user's programs did), so from a desktop
session it is **`mikudo dmesg ...`**. There is no `/dev/kmsg` and no
unprivileged reader.

```
dmesg [error|err|warn|notice|info|debug|trace] [N | -n N] [-f] [-g TEXT] [-m MODULE] [-v] [-E]
dmesg mark | new | stat | selftest | help
```

| Argument | Effect |
|---|---|
| `error`, `warn`, `notice`, `info`, `debug`, `trace` | the least severe level shown; default `info` |
| `err`, `errors` | errors **and** warnings |
| `N`, `-n N`, `--lines N` | only the last N lines |
| `-f`, `--follow`, `follow` | keep printing new lines until Ctrl-C (a pending signal under `mikudo`) |
| `-g TEXT`, `--grep` | only lines containing TEXT, case ignored; quotes group words (`-g "boot events"`) |
| `-m MODULE`, `--module` | only modules whose path contains MODULE; `MODULE$` means ending with it |
| `-v`, `--verbose` | add CPU, pid, flags and `file:line` |
| `-E`, `--no-echo` | leave out console output mirrored into the log |
| `mark` | remember the current position |
| `new` | show only what came after the last `mark` |
| `stat`, `stats` | lines written per level since boot, ring usage, lines overwritten and dropped |
| `selftest`, `test` | the log's own nine checks (`run=dmesg_selftest` on the boot line) |

The line format:

```
[    12.345678] 2 W rpc::poll            FN_GSP_RM_CONTROL failed: RecvTimeout after 2000012 us
 seconds.us    CPU level module (short)  text
```

With `-v`: `[    12.345678] cpu2 pid 41 user W module:line text`.

**`nvidia log`** is `dmesg` restricted to the NVIDIA driver's modules, with
named sections: `nvidia log [boot|gsp|fw|rpc|rm|disp|mem|chan|intr] [the
same options]`. `nvidia log stat` adds errors and warnings per section and
GSP-RM's own counts. It is privileged too: `mikudo nvidia log err`. Details
are in [Nvidia_gb206.md](../Nvidia_gb206.md).

Both viewers share one formatter and one filter (`kernel/kcore/klog/view.rs`).

---

## The serial line

COM1 at **115200 baud**, written with interrupts off for each line
(`kernel/arch/x86_64/serial.rs`). It is the one output that works before the
console exists, after the desktop has taken the screen, and when the machine
dies. In QEMU it is the `-serial` file or terminal; on the RTX 5060 Ti box a
USB serial cable.

- Every line the log records at info or above also goes down the wire, byte
  for byte as before the log existed, so old serial captures still diff.
- `kdebug!` lines and program `klog` lines stay off the wire unless the boot
  line has **`trace`**, which also turns on `trace_println!` everywhere
  (per-open, per-exec, per-thread lines). `trace` is slow: a line is about
  2.5 ms with interrupts off, so a busy boot under `trace` takes much longer.
- With the desktop up and no terminal window open, the serial line is the only
  console by design: the kernel no longer paints the framebuffer once Mayland
  owns it (a panic and `mayland stop` still do).

---

## pstore: the log that survives a warm reboot

`kernel/kcore/pstore.rs` keeps a 512 KiB region of RAM (about 6000 lines),
preferably at 256 MiB physical (moved in 16 MiB steps past a module or a hole),
out of the frame allocator. Every append is followed by `clflush` over the
bytes it touched, so a reset that comes before any natural write-back still
finds the text in DRAM. A **warm** reset does not retrain DRAM, so the next
boot reads the region back (`pstore::init`, before anything can overwrite it)
and keeps the text as "the previous boot". The header has a magic
(`MIKUPSTR`), a boot counter and a checksum over the body, so a power cycle or a
firmware that clears memory reads as "nothing from the previous boot", never
as garbage. When the body fills, one `*** pstore full - log truncated here ***`
line is written.

**What goes in it:**

- lines written with `pstore_println!` (the GB206 bring-up, the GSP health
  monitor, some `nvidia` commands);
- **the whole kernel log**, from boot, when the boot line has `klogpstore`, or
  from the moment `pstore mirror` is typed;

**Reading it** (kernel console, privileged: `mikudo pstore ...` from a
desktop):

```
pstore [err] [-g WORD] [-n LINES]     the previous boot's text, filtered
pstore clear                          drop the recovered copy
pstore mirror                         empty the region and mirror the kernel log into it from now on
```

`err` keeps lines whose level letter is `E` or `W`; `-g` keeps lines
containing the word; `-n` the last N of what is left. The GPU unload/load
test runs unattended with the log mirrored, so the result can be read after
a warm reboot; its boot line is in
[Nvidia_gb206.md](../Nvidia_gb206.md#unload-load-reset-and-recovery).

---

## Ring-3 crash reports

A program that takes a CPU fault it does not catch (or a fatal signal that
has nowhere to go) gets a report on the serial line and in the log
(`kernel/process/crash.rs`), then dies of the signal (`wait4` reports
`WIFSIGNALED`):

```
[crash] pid=7 'mikudo' SIGSEGV at 0x00005555a3f01c4e
[crash]   write to 0x0 (page not present)
[crash]   image /bin/mikudo bias=0x55559f000000 build-id 3f2a1c04...
[crash]   #0  0x0000000000401c4e  parse_word+0x2e
[crash]   #1  0x0000000000402a10  run_line+0x150
[crash]   #2  0x0000000000403f80  main+0x88
[crash]   rip=... rsp=... rbp=...
[crash]   maps: ...
```

| Line | Meaning |
|---|---|
| first | pid, process name, signal, faulting `rip` |
| cause | for a page fault: read, write or instruction fetch, the address, and why (not present, protection); "not an instruction" for `#UD`; "integer division by zero, or overflow" for `#DE`; "signal N with no handler" |
| `image` | the binary's path, its ASLR bias, its build-id; `interp base=` for a dynamic program; "no image record for this pid - addresses are raw" if the kernel has none |
| `#N` | frames, **unbiased** (bias subtracted) so they match the binary's own addresses, with `symbol+offset` from the binary's symbol table; `?` marks frames found by scanning the stack rather than following frame pointers |
| `rip= rsp= rbp=` | the registers at the fault |
| `maps:` | the mappings around the faulting address |

Names need the binary's symbol table and frame pointers; the builder builds
with `-C force-frame-pointers=yes` and `--build-id=sha1`. Look an address up
afterwards with `elf addr <path> <hex>` in the kernel console, or `elf syms
<path>` for the whole table.

A caught fault produces none of this: the handler gets the signal with a full
`ucontext_t` ([processes.md](processes.md#faults-are-signals)). `crashtest`
(`apps/src/crashtest.rs`) exercises the report on purpose.

---

## Core files

Every uncaught crash also writes **`/tmp/core.<name>.<pid>`**
(`kernel/process/coredump.rs`), an ELF `ET_CORE` file readable by `gdb` and
`readelf`:

- `PT_NOTE` first: `NT_PRSTATUS` (registers), `NT_PRPSINFO`, `NT_FILE` (which
  files are mapped where), the build-id, and a `MIKU` note saying which
  registers in `NT_PRSTATUS` are exact (an interrupt frame gives `rip`, `rsp`,
  `rbp`, `rflags`, `cs`, `ss`; the rest are zero when the fault came through a
  handler that does not see them);
- one `PT_LOAD` per region, the stack around `rsp` first, then writable
  anonymous memory; read-only file-backed regions are described and not
  stored (their bytes are in the file `NT_FILE` names);
- a region that did not fit has `p_filesz` 0 and its full `p_memsz`.

Limits: a core is at most 16 MiB (or the tmpfs file limit, if smaller), half of
it for the stack; at most 64 regions; only the **three newest** cores are kept
in `/tmp` (tmpfs is RAM, and a crash loop would otherwise fill it). `elf core`
in the kernel console lists them. `/tmp` does not survive a reboot.

---

## Kernel faults and panics

**A CPU exception in ring 0** (`#PF`, `#GP`, `#UD`, `#DE`) is reported by
`crash::report_kernel` with the kernel's own symbol table:

```
[crash] kernel page fault at 0xffffffff80123456
[crash]   read from 0x... (page not present)
[crash]   #0 ...  kernel_function+0x40
```

and that CPU halts. Page faults that are not errors (a swapped-out page, a
copy-on-write page, a lazy mapping) are resolved first and never reported. A
page fault inside the page fault handler is detected (a per-CPU nesting
depth) and reported instead of storming silently on the IST stack, which once
cost days. A **double fault** prints `[double fault] code=.. cr2=.. cr3=..`
with the TSS stack pointers and the frame, and halts.

**A Rust panic** (`panic_handler` in `kernel/main.rs`) disables interrupts,
halts every other CPU (`apic::broadcast_halt`), prints `[panic] <message>` on
serial, takes the framebuffer back from Mayland if needed
(`mayland::render::emergency`), prints `kernel panic: <message>` in red on the
screen, and halts. Nothing is written to disk. To keep a panic's context
across the reboot, boot with `klogpstore` and read `pstore` afterwards.

**Before the console exists**, `kernel/arch/x86_64/earlycon.rs` paints boot
phases into the framebuffer as coloured marks (the double-fault handler paints
one too), so a boot that dies before any text can still be placed.

---

## /proc reference

procfs (`kernel/fs/procfs.rs`) is a fixed set of twelve files made at boot.
There are **no per-process directories** (`/proc/<pid>`, `/proc/self`): the
process table is reached through `procinfo` (103) and `proc_path` (105), which
is what `ps` uses. Every file is generated on each read, is read-only, and is
readable by everyone. Files are produced into a 4 KiB buffer (8 KiB for
`slabinfo` and `lockstat`, 1 KiB for `diskstats`, 512 bytes for `net`), and
output past that is cut.

### /proc/version

```
MikuOS v1.0.0 (x86_64)
built with love <3
```

The crate version (`CARGO_PKG_VERSION`).

### /proc/uptime

```
1234.56 4567.89
```

Linux's format: seconds since boot and idle seconds summed over every CPU, to
the hundredth. Ticks divided by the timer rate (250 Hz).

### /proc/meminfo

```
MemTotal:        4031488 kB
MemFree:         3602944 kB
MemAvailable:    3602944 kB
SwapTotal:             0 kB
SwapFree:              0 kB
Slab:              21504 kB
SReclaimable:       4096 kB
SUnreclaim:        17408 kB
VmallocUsed:        8192 kB
KernelStack:        3584 kB
KernelStackMax:       40 kB
Vnodes:         1834
TmpfsPages:     251968
KernelHeap:     22020096/134217728 (112197632 free)
```

| Field | Source |
|---|---|
| `MemTotal`, `MemFree` | the frame allocator: usable frames, and those not allocated |
| `MemAvailable` | equal to `MemFree` (no reclaim estimate) |
| `SwapTotal`, `SwapFree` | always 0 here, whether or not swap is on (`info` and `swapinfo` show swap) |
| `Slab` | bytes in the kernel heap's slab caches |
| `SReclaimable` | what the heap's shrinker could give back, at most `Slab` |
| `SUnreclaim` | the difference |
| `VmallocUsed` | pages mapped by `mm::vmap` (large allocations, kernel stacks) |
| `KernelStack` | kernel stacks in use; `KernelStackMax` the deepest any went |
| `Vnodes` | vnodes in the VFS table now |
| `TmpfsPages` | the tmpfs page limit (a quarter of RAM) |
| `KernelHeap` | heap bytes used / total (free) |

### /proc/mounts

```
tmpfs on / type tmpfs (rw)
devfs on /dev type devfs (rw)
procfs on /proc type procfs (ro)
/dev/blk0 on /mnt type ext4 (rw)
```

The three built-in filesystems, then one line per mounted ext/btrfs slot with
its drive, mount point, type tag and read-only state.

### /proc/cpuinfo

```
arch: x86_64
vendor: unknown
features: vfs tmpfs devfs procfs ext2 ext3 ext4
```

A fixed text; it describes nothing about the processor. Use `nproc`-style
counts from `/proc/stat` instead.

### /proc/stat

```
cpu  1500 0 800 98000 0 0 0 0 0 0
cpu0 400 0 200 24500 0 0 0 0 0 0
cpu1 ...
ticks: 25000
vnodes: 1834
dcache: 5210 133 41 1200
filecache: 90211 3120 3120 70412 12 340 328
max_pages: 251968
max_fds: 512
heap_kb: 131072
procs: 61
hz: 250
```

| Line | Content |
|---|---|
| `cpu`, `cpuN` | Linux's ten columns in ticks: user, nice (0), system, idle, then six zeros (iowait, irq, softirq, steal, guest, guest_nice); the total line first |
| `ticks` | timer ticks since boot |
| `vnodes` | vnodes in the table |
| `dcache` | filesystem lookups, negative-entry hits, complete misses, inode cache size |
| `filecache` | page cache hits, misses, fills, hits served without the VFS lock, dirty pages now, pages dirtied, pages cleaned |
| `max_pages` | tmpfs page limit |
| `max_fds` | descriptors per process (512) |
| `heap_kb` | kernel heap size |
| `procs` | tasks (processes and threads) |
| `hz` | timer rate |

### /proc/heap

```
total: 134217728
used:  22020096
free:  112197632
```

Kernel heap bytes.

### /proc/diskstats

```
blk0 nvme 18342 401234 120012 8192 0 0 85 online
```

One line per present block device: name, driver (`ata`, `virtio`, `ahci`,
`nvme`, `ramdisk`), I/Os, sectors read, sectors written, sectors discarded,
I/O errors, retries, average latency in microseconds, state (`online`,
`degraded`, `offline`). Block devices are in [storage.md](storage.md).

### /proc/net

```
link: up
ip: 10.0.2.15
dns: 10.0.2.3
mac: 52:54:00:12:34:56
owner: kernel
```

The link state, address, DNS server and MAC, and who owns the card: `kernel`,
or `/dev/net0` when the boot line said `net=raw`, in which case a `raw:
read=.. written=.. dropped=.. undersized=..` line follows. See
[network.md](network.md).

### /proc/clock

```
unix: 1793612345
tz_min: 180
set: yes
source: rtc
```

The wall clock in Unix seconds, the timezone offset in minutes, whether the
clock has been set, and where it came from (for example `rtc`, `ntp`).

### /proc/slabinfo

Linux's `slabinfo - version: 2.1` format, so `slabtop`-style readers work:

```
slabinfo - version: 2.1
# name            <active_objs> <num_objs> <objsize> <objperslab> <pagesperslab> : tunables <limit> <batchcount> <sharedfactor> : slabdata <active_slabs> <num_slabs> <sharedavail>
kmalloc-64          4112   4608     64   64    1 : tunables    0    0    0 : slabdata     72     72      0
```

One line per kernel heap cache; the tunables are always 0 (a magazine's size
is fixed). The heap is described in [memory.md](memory.md).

### /proc/lockstat

The next section.

---

## /proc/lockstat

The cost of every **sleeping lock** in the kernel (`kcore::sched_lock`:
`SchedMutex` and its reader-writer sibling), since 2026-10-28. Spinlocks,
`IrqMutex` and the run queues are not covered; Mayland's `SERVER` lock is a
spinlock, so a long hold there shows as a long frame, not here.

```
# name taken contended slept wait_us longest_us longest_at | holds <1us <10us <100us <1ms >=1ms
vfs 812345 1204 310 52211 412 kernel/syscall/adapters/fs.rs:497 | 801200 9800 1200 140 5
fd 2401234 88 2 120 30 kernel/fs/vfs/fd.rs:210 | 2400100 1100 30 4 0
...
# calls that gave way to a filesystem's lock 41, found it taken and waited under the VFS lock 3
# disk reads under the VFS lock 120, 8410 us in all, the longest 410 us
# disk writes under the VFS lock 12, 3100 us in all, the longest 980 us
```

| Column | Meaning | Needs `lockstat` |
|---|---|---|
| `name` | the lock's name (`SchedMutex::named`); every lock of one name is merged into one line (the 64 descriptor-table shards are all `fd`); unnamed locks are `?` | |
| `taken` | times taken (reads and writes, for an rwlock) | |
| `contended` | times a taker found it held and had to wait (spun at all) | |
| `slept` | times a taker gave up spinning (after 256 spins) and slept | |
| `wait_us` | total time spent waiting, microseconds | yes |
| `longest_us` | the longest single hold | yes |
| `longest_at` | `file:line` of the code that took it for that hold (the caller of `with_vfs`, not the wrapper: `lock()` is `#[track_caller]`) | yes |
| `holds` | a histogram of hold times: under 1 us, 10 us, 100 us, 1 ms, and 1 ms or more | yes |

Without **`lockstat` on the boot line** the counts are kept (one atomic add and
one load per take) and the time columns stay 0; the header then says "(times
need lockstat on the boot line)". With it, every take and release reads the
clock. For an rwlock the times are the writers' (readers overlap). A hold of
1 ms or more is also logged as a warning, at most ten a second per lock:

```
[lockstat] vfs held 1830 us, taken at kernel/syscall/adapters/file.rs:117
```

A lock enters a fixed table of 128 entries the first time it is taken.

**The trailer lines** are about the VFS lock specifically:

- **gave way**: a call that changes a disk (`with_vfs_disk`: create,
  truncate, unlink) and finds the filesystem's own lock taken before it has
  changed anything **gives way**: it lets the VFS lock go, waits for the
  filesystem's lock with nothing held, and runs again. The count is how often
  that happened. "Found it taken and waited under the VFS lock" counts the
  calls that, after two such rounds, waited while holding the VFS lock after
  all; a high number there means long filesystem holds are stalling every VFS
  call. See [vfs.md](vfs.md) and `dev/vfs-locking.md`.
- **disk reads/writes under the VFS lock**: device commands issued by a task
  holding the VFS lock for writing, how many, total and longest time. Every
  other VFS call waits for these, so they are the first suspects for a stall
  that `vfs`'s `longest_us` points at.

Reading it: a lock with a high `slept` and a large `wait_us` is contended;
`longest_at` names the hold to shorten; a `>=1ms` bucket that keeps growing is
a latency problem even when the average looks fine. `cat /proc/lockstat` works
from any shell.

---

## /dev reference

devfs (`kernel/fs/devfs.rs`) is flat (no subdirectories) and read-only from
ring 3: nodes are made by the kernel at boot. Majors and minors follow Linux
where Linux has the device.

| Node | Mode | Major:minor | What it is |
|---|---|---|---|
| `/dev/null` | 0666 | 1:3 | reads return end of file, writes are discarded |
| `/dev/zero` | 0666 | 1:5 | reads return zeros |
| `/dev/random` | 0666 | 1:8 | the kernel's random generator (RDRAND where present), the same source as `getrandom` |
| `/dev/urandom` | 0666 | 1:8 | the same node type as `random` |
| `/dev/tty` | 0666 | 5:0 | the caller's **controlling terminal**, resolved on every call; `ENXIO` to open with none |
| `/dev/console` | 0620 | 5:1 | the system console (tty0), whoever is asking |
| `/dev/ptmx` | 0666 | 5:2 | opening it creates a pseudo-terminal and returns its master |
| `/dev/ptm1` .. `/dev/ptm7` | 0000 | 5:17..5:23 | pty masters; never opened by name, only handed out by `/dev/ptmx` |
| `/dev/pts1` .. `/dev/pts7` | 0666 | 5:65..5:71 | pty slaves; opening needs the terminal's family rule (its master holder, a descendant, or `CAP_SYS_ADMIN`) |
| `/dev/net0` | 0600 | 10:200 | whole Ethernet frames, one per read or write, never blocking (use `poll`); fed only with `net=raw` on the boot line; needs `CAP_NET_RAW` as well as the mode |
| `/dev/blkN` | 0600 | 8:N*16 | a whole block device, byte-addressable, read and write through the block layer |
| `/dev/blkNpM` | 0600 | 8:N*16+M | partition M (1-15) of device N, from its GPT |

`N` is 0 to 7. Block nodes exist only for devices present at boot (and their
used GPT entries); pty nodes exist for slots 1 to 7 whether or not a terminal
is using them. Reads of the terminal nodes never reach devfs: the syscall layer
routes them to the terminal ([ABI §2.5](../MikuOS_ABI.md#25-descriptors)).
`ls -l /dev` shows the nodes; the terminal subsystem is in
[input-and-terminals.md](input-and-terminals.md), block devices in
[storage.md](storage.md), `/dev/net0` in [network.md](network.md).

---

## Console diagnostic commands

These run in the kernel console (the emergency shell, see
[commands.md](commands.md#kernel-console)); from a desktop terminal `msh`
forwards unknown names to it through `kcmd`, and privileged ones need
`mikudo`.

### info

```
  MikuOS v1.0.0
  VNodes: 1834 cached
  Mounts: 4
  Heap:   21504 / 131072 KB
  Slabs:  21504 KB; kmalloc-256 6144 KB, vnode 4096 KB, kmalloc-64 2048 KB; mapped 8192 KB
  RAM:    412 / 3937 MB
  Usable: 3937 MB  Free: 3525 MB
  Swap:   inactive
  Uptime: 0h 5m 12s
```

| Field | Meaning |
|---|---|
| version | the kernel crate's version |
| `VNodes` | vnodes in the VFS table now (cached files and directories) |
| `Mounts` | entries in the VFS mount table |
| `Heap` | kernel heap used / total |
| `Slabs` | bytes in slab caches, the three caches holding the most, and pages mapped by `vmap` |
| `RAM` | frames in use / usable frames, in MB (slab frames included) |
| `Usable`, `Free` | usable RAM and what is not allocated |
| `Swap` | used / total / free KB when swap is on, else `inactive` |
| `Uptime` | **counts ticks as milliseconds** (it divides the 250 Hz tick count by 1000), so it reads a quarter of the real time; `/proc/uptime` and the `uptime` program are right |

`info` needs no capability.

### Others

| Command | Shows | Privileged |
|---|---|---|
| `top` | every task with pid, name, state, priority, CPU%, kernel stack used, age; refreshes every 4 s until Ctrl-C (its uptime header and age column have the same tick/ms error as `info`) | no |
| `heap` | kernel heap total, used, free, percentage | no |
| `memmap` | the physical memory map (base, length, type) from the boot loader | yes |
| `mm test` | heap and frame allocator selftest | no |
| `cred` / `cred show` | the console's credential: ids, groups, capabilities, umask | no |
| `tty` / `tty list` | terminals: index, size, who holds them | no |
| `ipc` / `ipc status` | descriptor ranges and sockets in use; `ipc test` the descriptor-layer selftest | no |
| `elf info <path>`, `elf syms <path>`, `elf addr <path> <hex>` | an ELF's headers, its symbols, the symbol at an address | no |
| `elf core` | the core files kept in `/tmp` | no |
| `elf cache` | binaries whose read-only pages are shared | no |
| `elf test` | the hostile-image loader selftest | no |
| `dmesg ...`, `pstore ...` | above | yes |
| `nvidia log ...` | the driver's part of the log | yes |
| `blkstat`, `swapinfo` | block devices, swap | no |
| `sv status`, `sv list`, `sv failed`, ... | services ([system.md](system.md)) | no (read verbs) |
| `who` | sessions | no |
| `history` | the console's command history | no |

---

## Debugging boot flags

Words on the GRUB command line that change behaviour for diagnosis. The full
list of boot flags, including the hardware escape hatches, is in
[boot.md](boot.md).

| Flag | Effect |
|---|---|
| `trace` | debug-level lines and every `trace_println!` go to the serial line (slow) |
| `klogpstore` | mirror the whole kernel log into pstore from boot |
| `lockstat` | time every sleeping lock (hold histogram, longest hold and its place, waits) |
| `schedwatch` | report tasks that sat READY or RUNNING for a second without a processor, where every CPU is, and the whole task table every five seconds |
| `slabdebug` | kernel heap: magazines off, poison and red-zone every cache |
| `bigstacks` | larger kernel stacks for programs' tasks (for chasing a stack overflow) |
| `tickonly` | keep the periodic 250 Hz timer; every deadline ends on a tick |
| `tlbalways` | flush every CPU on every unmap instead of only those with the address space loaded |
| `nopoll` | idle CPUs halt instead of polling (costs an IPI per wake) |
| `vfsmutex`, `walklocked`, `writethrough`, `nopagecache` | fall back to older, simpler VFS locking, path walks under the lock, synchronous writes, no page cache |
| `nvmepoll`, `netpoll` | poll the NVMe and network devices instead of taking interrupts |
| `nointr`, `nononstall` | every GPU wait polls, or only the fences do |
| `nosmap`, `nosmep` | turn the CPU protections off (the first thing to try when a boot dies early) |
| `fbdirect` | the console writes the framebuffer directly instead of through its shadow |
| `noauth` | boot to a root console with no login |
| `run=SUITE;SUITE;...` | run selftests on a preempted `bootline` task, report to serial (`dhcp` first for network ones, `poweroff` last); see [testing.md](testing.md) |

---

## How to debug X

### A program crashed

1. Read the `[crash]` report (serial, or `mikudo dmesg -g crash`).
2. If frames show `?` or no names, the binary lacks frame pointers or symbols:
   rebuild it with the project's build ([sdk.md](sdk.md)).
3. Look an address up: `elf addr /bin/prog 0x401c4e`.
4. For memory state, copy `/tmp/core.<name>.<pid>` off the machine and open it
   in `gdb` with the binary.

### The machine hangs

1. Is anything still printing on serial? If yes, it is a userspace or
   scheduling stall, not a dead kernel.
2. Reboot with **`schedwatch`**: after a second it names the tasks that are
   runnable and not running, and every five seconds dumps the whole table, so
   a stuck task and the CPU it is waiting for are visible.
3. Reboot with **`lockstat`** and read `cat /proc/lockstat` (from a shell that
   still answers): a lock with a huge `longest_us` names the place holding it;
   `[lockstat] ... held N us` warnings appear in the log as it happens.
4. If nothing prints at all, boot with **`klogpstore`**, wait for the hang,
   warm-reset, and read `mikudo pstore err` and `mikudo pstore -n 200`.
5. A boot that dies before the console: look at the earlycon colour marks;
   try `nosmap`.
6. A hang with no output in a page fault path is now reported (the nested
   fault check); a silent one before 2026-09-07 was exactly that.

### A kernel panic

The message is on serial (`[panic] ...`) and on screen. To see what led up to
it, reboot with `klogpstore` and reproduce: the next boot's `pstore` has the
log up to the panic. For a CPU fault in the kernel, the `[crash] kernel ...`
report names the function; `trace` adds detail if it is reproducible.

### A system call is slow

1. Time it from the program (`clock_gettime(CLOCK_MONOTONIC)` around it), or
   with `stdbench` ([testing.md](testing.md)).
2. Boot with `lockstat` and compare `/proc/lockstat` before and after: a call
   that waits on the VFS lock shows as `vfs` contention, and the trailer says
   whether disk I/O under the VFS lock is the cause.
3. `/proc/stat`'s `filecache` line says whether reads hit the page cache.
4. `/proc/diskstats` gives per-device latency.
5. A wait that always takes about 100 ms is one backstop: a lost wake-up on
   that wait queue (see [scheduler.md](scheduler.md)).

### A program leaks

1. `ps` repeatedly: a growing number of processes means children are not
   reaped (no `wait4`) or orphans pile up.
2. `info` and `/proc/meminfo` over time: `MemFree` falling while the program
   runs. `/proc/slabinfo` says which kernel cache grows (a vnode leak shows as
   the `vnode` cache and `Vnodes:` in `meminfo`; a socket leak as sockets in
   `ipc status`).
3. Descriptors: a program that leaks them reaches `EMFILE` at 512; sockets at
   the 160-per-account share.
4. Inside the program, libmiku's heap has host tests and counters
   ([sdk.md](sdk.md#heap)).

### The network does nothing

`cat /proc/net` (link, address, owner), `mikudo dmesg -m net`, and the
[network.md](network.md) troubleshooting section.

### The GPU misbehaves

`nvidia status`, `mikudo nvidia log err`, `mikudo nvidia log stat`, and
[Nvidia_gb206.md](../Nvidia_gb206.md).

---

## Where the code is

| What | Where |
|---|---|
| Kernel log: levels, macros, rings, viewer | `kernel/kcore/klog/{mod,ring,line,view,selftest}.rs` |
| `dmesg`, `pstore`, `info`, `top`, `heap`, `memmap` commands | `kernel/shell/commands/system.rs` |
| `nvidia log` | `kernel/shell/commands/nvidia_cmds.rs` |
| Serial port | `kernel/arch/x86_64/serial.rs` |
| pstore | `kernel/kcore/pstore.rs` |
| Ring-3 and kernel crash reports | `kernel/process/crash.rs`, `kernel/process/symbols.rs`, `kernel/kcore/backtrace.rs` |
| Core files | `kernel/process/coredump.rs` |
| Exception handlers, double fault | `kernel/arch/x86_64/interrupts.rs`, `trap.rs` |
| Panic handler | `kernel/main.rs` |
| Early boot marks | `kernel/arch/x86_64/earlycon.rs` |
| procfs | `kernel/fs/procfs.rs` |
| devfs nodes | `kernel/fs/devfs.rs`, `kernel/fs/vfs/core/bootstrap.rs` |
| Lock statistics | `kernel/kcore/sched_lock.rs` |
| `schedwatch` | `kernel/sched/stats.rs` |
| `slabdebug` | `kernel/mm/heap/mod.rs` |
| `elf` commands | `kernel/shell/commands/elf_cmds.rs` |
