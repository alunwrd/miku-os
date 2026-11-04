# Testing

MikuOS is tested at three levels. Pure logic that does not need the kernel (filesystem formats, cryptography, parsers, allocators, the QML engine) lives in host crates with ordinary `cargo` tests that run on the build machine in seconds. What only a boot can answer (the VFS against a real block layer, the protocol between the two halves of Mayland, the ELF loader, the terminal table, the NVIDIA driver's arithmetic) is covered by in-kernel selftest suites that run headless from the boot line and print to the serial port. Ring-3 behaviour (signals, threads, `std`, the C library, async runtimes) is covered by test programs in `apps/` and `stdapps/` run on the booted machine. This page lists every suite and crate, how to run each, the standing known failures, the benchmark `stdbench`, and what to run after changing which subsystem.

## Contents

- [Overview](#overview)
- [In-kernel selftest suites](#in-kernel-selftest-suites)
  - [The suites](#the-suites)
  - [Running them](#running-them)
  - [Reading the output](#reading-the-output)
  - [Known failures](#known-failures)
- [Ring-3 test programs](#ring-3-test-programs)
- [Host tests](#host-tests)
- [stdbench](#stdbench)
- [The developer's QEMU harness](#the-developers-qemu-harness)
- [Tests that need a host server](#tests-that-need-a-host-server)
- [Stress: the storm harness](#stress-the-storm-harness)
- [What to run after changing what](#what-to-run-after-changing-what)
- [Verification levels](#verification-levels)

## Overview

| Level | Where | Costs | Run with |
|---|---|---|---|
| Host tests | `lib/*`, `aosh/`, `builder/`, `kernel/mm/heap/hosttest` | seconds, no boot | `cargo test` or `cargo test-host` in the crate's directory |
| In-kernel selftests | `kernel/**/selftest*.rs`, `kernel/mayland/selftest/` | one QEMU boot | `run=<suite>_test` on the boot line, or the shell command |
| Ring-3 test programs | `apps/src/*test*.rs`, `stdapps/src/std*.rs` | one QEMU boot | the program's name, from a shell or in a `run=` list |
| The real card | the RTX 5060 Ti machine | a reboot of the real machine | [../../dev/nvidia-card-checklist.md](../../dev/nvidia-card-checklist.md) |

There is no CI that runs tests: `.github/workflows/release.yml` only builds and publishes. Nothing here builds the ISO or boots QEMU for you; how to build and boot is in [building.md](building.md), and the boot line is described in [boot.md](boot.md).

## In-kernel selftest suites

Every suite collects named cases through one harness, `kernel/kcore/selftest.rs`: a case passes, fails, or is skipped (a skip is a case that could not run here, for example the btrfs cases with no btrfs disk attached). Each suite prints `ok`, `FAIL` or `skip` per case with a detail line, then a summary. Most also print a tagged summary line to the serial port, which is what a harness greps.

### The suites

| Shell command | Boot-line word | Source | What it covers | Needs |
|---|---|---|---|---|
| `mm test` | `run=mm_test` | `kernel/mm/selftest.rs` | Kernel heap (size classes, per-CPU magazines, every CPU at once, memory given back), device memory (one physical run, aligned, below its limit), `vmap`, frame counts, page faults, CPU caches as copy-on-write assumes ([memory.md](memory.md)) | nothing |
| `ipc test` | `run=ipc_test` | `kernel/syscall/selftest.rs`, `kernel/sched/selftest.rs` | The descriptor contract: `read`, `write` and `poll` agree on readiness (pipe ring, ends counted, no reader, revents, invalid fds, socket range), socketpairs and Unix sockets (readiness, unconnected, names), ICMP and raw-net descriptors; timed sleeps (`time/sleep-1ms`), deadline and keyed waits; and the scheduler's structures: the run-queue heap, the pid bitmap, the job index ([scheduler.md](scheduler.md)) | nothing |
| `vfs test` | `run=vfs_test` | `kernel/fs/vfs/selftest.rs` | The VFS through its own API, twice: on an ext4 filesystem on a ramdisk and on a `MemFs`, plus cross-filesystem contamination. Every case is a bug that was once live (directories over 256 entries, names over 64 bytes, and so on) ([vfs.md](vfs.md)) | nothing |
| `ext test` | `run=ext_test` | `kernel/fs/ext/selftest.rs` | What `lib/extfs`'s host tests cannot reach: two ext filesystems mounted at once (routing in the mount table), the real block layer (write, flush, read back), the kernel's own mkfs writing to a device and the driver mounting it ([filesystems.md](filesystems.md)) | nothing (ramdisks) |
| `btrfs test` | `run=btrfs_test` | `kernel/fs/btrfs/selftest.rs` | The probe registry finding btrfs on a real block device among others, the block layer, the mount table, the graft, and a path through `MikuVFS` | a btrfs volume attached (made with `mkfs.btrfs -r` into `miku-os/btrfs.img`); without one every case skips |
| `cred test` | `run=cred_test` | `kernel/process/selftest_cred.rs` | The credential model: identities, the capability set, and that exec never raises privilege ([../MikuOS_Security.md](../MikuOS_Security.md)) | nothing; runs in milliseconds |
| `tty test` | `run=tty_test` | `kernel/io/tty/selftest.rs` | The terminal table with no screen, keyboard or disk: a `TtyId` names one terminal for ever (generations on reuse), the grid, the line discipline, the VT parser ([input-and-terminals.md](input-and-terminals.md)) | nothing |
| `elf test` | `run=elf_test` | `kernel/process/selftest.rs` | Hostile ELF images built in memory and fed to `elf::parse` and `elf_loader::load` in a throwaway address space: segments in the kernel half, non-canonical or page-zero addresses, duplicate `PT_INTERP`/`PT_DYNAMIC`, bad alignment, W+X, entry points outside an executable segment, relocations pointed outside the image, `DT_RELASZ` of `u64::MAX` (must return within a second) ([processes.md](processes.md)) | nothing |
| `net test` | `run=net_test` | `kernel/net/selftest.rs` | What needs a machine: a DHCP lease (or the existing one), the gateway answering ARP, four pings to the gateway, and zero checksum and fragment drops. Everything that is a pure function of bytes is in `lib/net`'s host tests ([network.md](network.md)) | a network card; with none the suite is one skip. Put `dhcp` first in the list |
| `dmesg selftest` | `run=dmesg_selftest` | `kernel/kcore/klog/selftest.rs` | The kernel log where it runs: levels routed, per-CPU line assembly, fragments, truncation, filters and quoted words, merge order, reading from the end, the viewer staying out of its own log ([diagnostics.md](diagnostics.md)) | nothing |
| `mayland test` | `run=mayland_test` | `kernel/mayland/selftest/` | The display protocol with nothing mocked: the client encoder's bytes consumed by the server and its replies decoded by the client. 125 test functions in 19 files: access, encoding, buffers, windows, tiling, wm, conf, console, layer, pointer, keyboard, anim, gfx, outputs, popup, clipboard, compose, below, toolkit ([mayland.md](mayland.md)) | nothing; cases that need `may_wm` or `may_console` skip if the running desktop holds them |
| `nvidia selftest` | `run=nvidia_selftest` | `kernel/drivers/gpu/nvidia/selftest.rs` | Everything about the NVIDIA driver that can be decided without the card: page-table index arithmetic, range overlap and range checks, the cubin parser, the RPC encoding, the compute-chain and sized-buffer rules. Nothing that reads or writes a page table ([../Nvidia_gb206.md](../Nvidia_gb206.md)) | nothing (QEMU has no NVIDIA card) |

Two commands are stress tests rather than suites: `nvmestress` (concurrent NVMe I/O, prints `N passed, M failed`; [storage.md](storage.md)) and `swaptest` ([memory.md](memory.md)).

### Running them

**From the boot line.** `run=` takes kernel shell commands separated by `;`, with `_` for a space, and runs them on a kernel task called `bootline` after boot (details in [boot.md](boot.md#the-boot-line)). The usual shape is:

```
noauth run=vfs_test;ext_test;tty_test;cred_test;elf_test;poweroff
noauth run=dhcp;net_test;poweroff
```

- End a headless list with `poweroff`, or the machine stays up until QEMU is killed.
- Put `dhcp` first when anything later needs the network; otherwise the test races netd's own DHCP.
- `noauth` skips the login, so nothing waits for a keyboard.
- The list runs preempted, like the real machine, on its own task. A test that makes its own job a pty's foreground and types Ctrl-C would kill `bootline`; the tty suite uses an unused process group for that reason.
- The whole boot line, `run=` included, is capped at 1024 bytes; a list that is cut drops its partial last command and says so.

**From the GRUB menu.** For one run, press `e` and add `noauth run=<suite>` to its `multiboot2` line ([boot.md](boot.md#the-boot-line)). A headless run needs a one-entry ISO ([below](#the-developers-qemu-harness)), because nothing can type at the menu in time.

**From a shell.** At the kernel console (a `noauth` boot, or the console before login) type the command, for example `vfs test`. From `msh`, kernel commands are reached through the kernel command fall-through; commands that change the machine need `mikudo` ([commands.md](commands.md)). Running a suite on a machine that is in use is safe for most suites but not neutral: `mayland test` parks the desktop's windows and switches the layout to float for the run, then puts both back.

### Reading the output

Everything a suite prints goes to the serial port as well as the screen. With QEMU, `-serial stdio` or `-serial file:serial.log` captures it.

| Suite | Serial summary line |
|---|---|
| mm | `[mm-test] N passed, N failed, N skipped` |
| vfs | `[vfs-test] N passed, N failed, N skipped` |
| ext | `[ext-test] N passed, N failed, N skipped` |
| btrfs | `[btrfs-test] N passed, N failed, N skipped` |
| cred | `[cred-test] N passed, N failed, N skipped` |
| elf | `[elf-test] N passed, N failed, N skipped` |
| tty | `[tty-test] N passed, N failed` |
| dmesg | `[klog-test] N passed, N failed` |
| ipc, net | `N passed, N failed, N skipped` after the per-case lines |
| mayland, nvidia | `N of N passed`, or `N passed, N failed` and `N skipped` |

A harness that wants one answer greps the log for `FAIL`, `passed, ` and ` of N passed`, and counts `[crash] pid` lines (a ring-3 process died of a signal) and `[crash] kernel` or `[panic]` lines (the kernel died). Per-case lines are `ok    <name>`, `FAIL  <name>` (`fail` in the nvidia suite) or `skip  <name>`, each followed by an indented detail line.

### Known failures

From the user's verification notes; compare a run against these before suspecting a regression:

| Suite | Standing result | Note |
|---|---|---|
| `mayland_test` | 164/164 since 2026-10-24 | Earlier counts (158, 162) were smaller suites, not failures. The two failures before 2026-10-20 were wrong tests, since fixed |
| `elf_test` | 35 passed, 1 failed | `elf/relro-outside-image` ("segment data beyond file end"); predates the std work and has not been investigated |
| `nvidia_selftest` | 184/184 on 2026-10-21 | QEMU |
| `threadtest` | 24/24 | On `-cpu qemu64` the mutex case ("every contender finished", "the guarded count is exact") once timed out within its 5 s window (2026-09-25); rerun before suspecting the mutex |
| crash count | `crash=4` is expected | when the std programs run: two `stdsignal` children and one `stdthreads --overflow` child (the guard-page check) die of SIGSEGV on purpose, and `stdproc` has a PROT_NONE child that does too |

## Ring-3 test programs

These are ordinary programs in `/bin`, run by name from a shell or listed in a `run=` line (a name the kernel shell does not know is looked up in `/bin`; arguments are joined with `_` on the boot line, as in `stdproc_--churn_3000_fork`). Each prints its cases and exits non-zero on failure.

| Program | Source | What it tests |
|---|---|---|
| `abitest` | `apps/src/abitest.rs` | libmiku's ABI: the allocator through `alloc` collections and over-aligned blocks, the environment, `fcntl`, `getdents` on generator-backed files read in tiny chunks, `getrandom`, the realtime clock, the vDSO |
| `sigtest` | `apps/src/sigtest.rs` | Signal delivery end to end; reports through its exit code |
| `threadtest` | `apps/src/threadtest.rs` | More than one task in one address space: shared memory, futex mutexes, join |
| `polltest` | `apps/src/polltest.rs` | Waiting on several descriptors from ring 3: the kernel's routing and readiness agree |
| `mmaptest` | `apps/src/mmaptest.rs` | File-backed `mmap` and `msync` (`MAP_SHARED` writes reach the file) |
| `envtest` | `apps/src/envtest.rs` | exec and the ELF loader's argv/envp plumbing; reports through its exit code |
| `crashtest` | `apps/src/crashtest.rs` | Crashes on purpose so the crash report can be read |
| `test_full` | `apps/src/test_full.rs` | The libc compatibility layer and libmiku helpers (bit operations, compression, strings, and so on) |
| `stdhello` | `stdapps/src/stdhello.rs` | The basics of `std`: printing, the allocator, `format!`, args, environment, entropy, exit, panics |
| `stdthreads` | `stdapps/src/stdthreads.rs` | `std::thread`, `std::sync`, `std::time`; `--overflow` checks the stack guard page |
| `stdfs` | `stdapps/src/stdfs.rs` | `std::fs` in full |
| `stdproc` | `stdapps/src/stdproc.rs` | `std::process::Command`: fork, execve, wait4, pipes; `--churn N fork` is the storm workload |
| `stdpoll` | `stdapps/src/stdpoll.rs` | The `polling` reactor over `poll` (76) |
| `stdasync` | `stdapps/src/stdasync.rs` | `smol` from crates.io, unmodified; one case connects to a host server |
| `stdtokio` | `stdapps/src/stdtokio.rs` | tokio, mio and socket2; one case connects to a host server |
| `stddns` | `stdapps/src/stddns.rs` | Name resolution through `std`, tokio and libc `getaddrinfo` |
| `stdpty` | `stdapps/src/stdpty.rs` | A terminal made the way Zed's terminal makes one (`openpty`, `/dev/ptmx`) |
| `stdwatch` | `stdapps/src/stdwatch.rs` | inotify through the `inotify` crate and mio |
| `stdsignal` | `stdapps/src/stdsignal.rs` | CPU faults as signals through libc, the way wasmtime uses them |
| `stdremote` | `stdapps/src/stdremote.rs` | Zed's `remote_server`, end to end, as a Zed client reaches it |
| `stdc` | `stdapps/src/stdc.rs` | C in a MikuOS program: `lib/c` and `libc_miku.so` (SQLite, zstd, tree-sitter) |

How the programs are built is in [building.md](building.md); writing one is in [sdk.md](sdk.md).

## Host tests

Each crate below has its own `.cargo/config.toml` that escapes the repository root's kernel target (`x86_64-unknown-none` and `build-std`), so the tests build for the host. Counts are `#[test]` functions in the source as of 2026-11-03.

| Crate | Run with | Tests | What it covers |
|---|---|---|---|
| `lib/extfs` | `cd lib/extfs && cargo test` | 234 | The ext2/3/4 driver over an in-memory disk: big directories, big volumes, checksums, crash consistency (power cut mid-transaction), dir hashing, extents, fallocate, flavours, foreign images, fsck, htree, inline data, the journal, limits, images made by `mke2fs`, mount, orphans, ownership, rename, timestamps, truncate. Images it writes are checked with `e2fsck -f -n` |
| `lib/btrfs` | `cd lib/btrfs && cargo test` | 189 | btrfs: the allocator, the chunk map, commits, extents, files, images made by `mkfs.btrfs`, mount, reads, tree shape, space accounting, transactions, the trees; images checked with `btrfs check` |
| `lib/net` | `cd lib/net && cargo test` | 152 | The network crate: ARP, DHCP, TCP wire and connection logic, HTTP wire format, packets; SHA-1/2, HMAC, AES vectors; big-number arithmetic including a 2048-bit RSA exponentiation; every root in `etc/ssl/certs/ca-bundle.pem` parsed and self-verified; signatures, chains accepted and refused, hostname rules |
| `lib/mikuauth` | `cd lib/mikuauth && cargo test` | 103 | Argon2id against RFC 9106's published tag and BLAKE2b against RFC 7693; passwd/group/shadow against a hostile corpus; the `mikudo.conf` grammar; the protocol (truncated messages, lying lengths); the login decision `session::authenticate`; the files that actually ship in `etc/` |
| `lib/mikupkg` | `cd lib/mikupkg && cargo test` | 163 | The package manager: config, containers, crash recovery, the database, fetch, host, metadata, names, version ranges, resolution, the shipped database, staging, trust, transactions, versions |
| `lib/fshost` | `cd lib/fshost && cargo test` | 29 | `MemDisk`, the in-memory disk under every filesystem test, and its promises about flushes and crashes; CRC32C |
| `lib/libmiku/hosttest` | `cd lib/libmiku/hosttest && cargo test` | 14 | libmiku's ring-3 heap (segregated bins, boundary tags), `heap_core` over a frame-shaped source, and the memory functions |
| `kernel/mm/heap/hosttest` | `cd kernel/mm/heap/hosttest && cargo test` | 11 | The kernel heap's size classes: every size and alignment lands in the smallest class that holds it |
| `lib/mayland` (libmay) | `cd lib/mayland && cargo test-host` | 16 | Keymaps (every keycode, group and modifier, compiled-in against serialised tables; xkb `ru(winkeys)` spot checks), the window shape, the paint toolkit, the config grammar |
| `lib/freetype` | `cd lib/freetype && cargo test-host` | 5 | The vendored FreeType 2.14.3 against numbers FreeType gave a C probe on the build host for the font aosh draws with |
| `aosh` | `cd aosh && cargo test-host` | 187 | The QML engine and shell: the object model, models, animation and easing, input, I/O and processes, IPC, the parser, the scene (layout, damage, painting), text, Canvas, shapes, SVG, effects, icons; renders compared with Qt's ([aosh.md](aosh.md#tests)) |
| `builder` | `cd builder && cargo test` | 8 | The image builder's packaging rules: the personal package, the image built by installing packages and carrying a database of itself, no working state in the staging tree, every file claimed by a package, modes decided in one place |

Notes:

- **`cargo test-host`** is an alias in that crate's `.cargo/config.toml`. `lib/mayland` is a member of the root workspace, whose profiles say `panic = "abort"`, so its alias builds with `panic_abort` and `-Z panic-abort-tests`; `aosh` and `lib/freetype` use `panic_unwind`. A plain `cargo test` in those directories builds for the kernel target and fails.
- **Host tools.** `lib/extfs` calls `mke2fs`, `debugfs`, `e2fsck`, `dumpe2fs` and `dd`; `lib/btrfs` calls `mkfs.btrfs` and `btrfs`. Where those are not installed the image cases print `e2fsprogs is not installed; skipping` or `btrfs-progs not available (...) - skipping` and pass.
- **aosh needs one build first.** Its `Cargo.toml` patches `rquickjs-sys` and `libc` from `target/rust-src-miku/`, which the builder assembles (`cd builder && cargo run --release -- overlay`, or any image build); a host `cargo test-host` needs that to have run once.
- **aosh's extra modes.** `AOSH_GPU_CHECK=1 cargo test-host` compiles the kernel's `paint.cu` with nvcc, runs it on the build host's NVIDIA card through `libcuda`, and compares every paint of every scene test with the processor's pixel for pixel. The benches are `#[ignore]` tests in `aosh/src/shot.rs` (`notes_flip_cost`, `desktop_drag_cost`, `desktop_session_cost`, `overview_open_cost`, `workspace_switch_cost`, `desktop_select_cost`): `CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test-host bench:: -- --ignored --nocapture --test-threads 1`; they need the gitignored `personal/` tree. `AOSH_BENCH_GPU=1` times frames as MikuOS draws them on the card. The Qt references the comparison tests read come from `aosh/tests/qtref/make.sh` (Qt 6 and `xvfb-run`); a test whose reference is missing passes over it.
- **Never set `AOSH_HOST_EXEC`** when running aosh on the host: aosh refuses to start programs or write outside `$HOME` there because a shell config's start-up commands would run on the developer's own session.

## stdbench

`stdbench` (`stdapps/src/stdbench.rs`) measures what the kernel's common paths cost, in nanoseconds an operation, through `std` alone, so the same file builds for Linux and the two can be put side by side.

```
stdbench [dir] [--net HOST:PORT]
```

- `dir` is where the file cases make their files, under `dir/stdbench`: `/tmp` (tmpfs) by default, or a mounted disk.
- `--net HOST:PORT` adds the network cases against an HTTP/1.0 server there, which must serve `small.txt` and `big64.bin`.
- `--exit` and `--echo` are its own helper modes, used by the process cases.

Each case prints `stdbench: <name>: N ns/op (M ops)`. Scaling cases run the same work on 1, 2 and 4 threads and print the total rate; a case that does not grow with threads is waiting on a lock.

| Group | Cases |
|---|---|
| Calls and clock | `getpid`; `clock (vDSO)` |
| Files | `stat, short path`; `stat, 6 components`; `stat, missing name`; `open+close`; `read 4 KiB, cached`; `write 4 KiB, same place`; `create+write 1 KB file` (2000); `read_dir entry`; `unlink`; write 64 MiB + fsync (MB/s, split into writes and fsync) and read it back (MB/s) |
| Pipes and futexes | `pipe write+read, one thread, no waiting`; pipe ping-pong between threads and between processes (round trip); `thread ping-pong (futex), round trip` |
| Processes and threads | `thread spawn+join`; `process spawn+wait`; `process spawn+wait, /bin/echo` |
| Memory | `page fault (64 MiB, touched once, freed)`, with the faults and the free split out; `random line per page over N MiB` |
| Scaling (1, 2, 4 threads) | `getpid scaling`; `stat scaling`; `read 4 KiB scaling`; `open+close scaling`; `4 KiB alloc+free scaling` |
| Network (`--net`) | `tcp connect`, `tcp close`, `http GET small.txt, connection each`, and 64 MiB |

**To compare with Linux:** build the same file on the host with `rustc -O --edition 2021 stdapps/src/stdbench.rs`, run `./stdbench /tmp` there, and run `stdbench` on MikuOS under QEMU/KVM with the same number of vCPUs. The rounds of 2026-10-27 and 10-28 were driven this way ([../../dev/kernel-core.md](../../dev/kernel-core.md), "Bottlenecks round"); the README quotes the results.

## The developer's QEMU harness

The repository's own entry point is `run=`. The developer drives them with three small scripts kept outside the tracked tree (under `target/heapwork/scripts/` at the time, now not present in the working copy); they are described here so they can be recreated. All three take the built ISO (`miku-os/miku-os.iso`), pull `kernel.elf` and `firmware.img` out of it with `xorriso`, write a one-entry `grub.cfg` with `set timeout=0`, and rebuild a scratch ISO with `grub-mkrescue`, so the tracked `grub.cfg` is never edited.

| Script | Usage | What it does |
|---|---|---|
| `suites.sh` | `suites.sh "<run list>" [mem]` | Boots `noauth "run=<list>"` twice in parallel under KVM, `-cpu qemu64` and `-cpu host`, `-smp 4`, memory 2G by default, `-display none -serial stdio -no-reboot`, 420 s timeout each. Starts `python3 -I -m http.server 8000 --bind 127.0.0.1` for the tests that fetch from the host. Then prints, per CPU model, the `[crash] pid` count, the kernel-crash count, and every `FAIL` and summary line from the serial log |
| `disk.sh` | `disk.sh "<run list>" <cpu> <tag> "<extra qemu args>" [kernel flags]` | One boot on one CPU model with extra QEMU devices (scratch disk images only) and optional extra boot flags; serial to `repro-<tag>/serial.log`. Used for the block-layer and filesystem suites and for reproducing one failure |
| `desk.sh` | `desk.sh <outdir>` | Boots the normal ISO (4 GiB, `-cpu host -smp 4`), logs in as `miku`, opens the overview with Super, types into its search, and takes a screendump at each step, driven through the QEMU monitor socket by a helper (`drive.py`) whose steps are `wait:N`, `type:TEXT`, `key:K`, `keyq:K`, `burst:...`, `shot:NAME` and `quit` |

The pattern to copy:

```
xorriso -osirrox on -indev miku-os/miku-os.iso \
    -extract /boot/kernel.elf iso/boot/kernel.elf \
    -extract /boot/firmware.img iso/boot/firmware.img
cat > iso/boot/grub/grub.cfg <<'CFG'
set timeout=0
set default=0
menuentry "MikuOS selftest" {
    multiboot2 /boot/kernel.elf noauth "run=mm_test;ipc_test;vfs_test;ext_test;tty_test;cred_test;elf_test;mayland_test;nvidia_selftest;poweroff"
    module2 /boot/firmware.img firmware
    boot
}
CFG
grub-mkrescue -o test.iso iso
qemu-system-x86_64 -enable-kvm -cpu host -m 2G -smp 4 -boot d -cdrom test.iso \
    -display none -serial file:serial.log -no-reboot
grep -aE 'FAIL|passed, | of [0-9]+ passed|\[crash\]|\[panic\]' serial.log
```

Use `-boot d`: with `-cdrom` alone QEMU may boot an older kernel from an attached hard disk. The kernel reads nothing from the serial port; for keys use the monitor (`-monitor unix:mon.sock,server,nowait`, then `sendkey`), and for the screen `screendump out.ppm`. QEMU's PS/2 mouse drops jumps larger than about 250 pixels and coalesces moves sent back to back, so pause between `mouse_move` commands ([input-and-terminals.md](input-and-terminals.md#diagnostics-and-testing)).

## Tests that need a host server

QEMU's user network shows the host as 10.0.2.2.

| Test | Needs |
|---|---|
| `stdtokio`, `stdasync` | an HTTP server on the host at port 8000 (`python3 -I -m http.server 8000`); `stdtokio` also expects a refused connection to 10.0.2.2:1 |
| `stdbench --net 10.0.2.2:8000` | the same server, serving `small.txt` and a 64 MiB `big64.bin` |
| `net test` | a network card and QEMU's DHCP; no host server (the earlier download cases were removed when the byte-level tests moved to `lib/net`) |

Bind the server to `127.0.0.1` on the host: QEMU's user network reaches it there, and nothing else on the LAN does. Use `-device virtio-net-pci` rather than e1000 when timing connections; QEMU's e1000 adds about 500 ms to a connect.

## Stress: the storm harness

Fork and thread storms found the hardest bugs in this tree (a copy-on-write page made writable before its TLB shootdown, a reaper race). The developer's storm harness is kept outside the tracked tree (`target/sigtest/storm/` when last used). From the notes:

- `run.sh` boots several VMs at once (three by default, `CPU=qemu64` by default, `SMP=1` or `SMP=4` for the CPU count) each running `stdproc` eight times, detects hangs, and leaves a hung VM running with its monitor on `m<i>.sock` for inspection. `LINE=` overrides the command list.
- `loop.sh` (`ROUNDS=`) repeats `run.sh` and stops at the first `[crash]` or hang, copying the logs to `hit/`.
- The workload is `stdproc --churn 3000 fork` (on the boot line `stdproc_--churn_3000_fork`).
- Put `info` before and after the storm in the list: it prints vnodes, RAM and the kernel heap, so a difference between the two shows a leak.
- The bar for a fix has been dozens of clean rounds: the COW fix was 96 of 96 clean runs against 1 to 5 failures in 24 before it.

## What to run after changing what

| Changed | Host | Boot |
|---|---|---|
| `lib/extfs` | `cd lib/extfs && cargo test` | `run=ext_test;vfs_test` |
| `lib/btrfs` | `cd lib/btrfs && cargo test` | `run=btrfs_test;vfs_test` with `btrfs.img` attached |
| `kernel/fs/vfs`, `kernel/fs/*` | `lib/fshost` if the disk model changed | `run=vfs_test;ext_test`; then `stdfs`, `stdwatch`, `stdbench /tmp` |
| `kernel/drivers/block`, partitions, mkfs | | `run=ext_test` with real disks attached through `disk.sh`; `nvmestress` for NVMe |
| `kernel/mm`, the kernel heap | `kernel/mm/heap/hosttest` | `run=mm_test`; the storm harness; `stdbench` page-fault and alloc cases |
| `lib/libmiku` heap or memory functions | `lib/libmiku/hosttest` | `abitest`, `test_full`, `stdthreads` |
| `kernel/sched`, waits, timers, SMP | | `run=ipc_test` on both CPU models and `-smp 1`/`-smp 4`; `threadtest`, `stdthreads`; the storm harness |
| `kernel/process`, ELF loading, exec | | `run=elf_test;cred_test`; `envtest`, `stdproc` |
| signals | | `sigtest`, `stdsignal` (expect the deliberate crashes) |
| credentials, mikudo, login | `lib/mikuauth` | `run=cred_test`; a login in QEMU |
| `kernel/syscall`, descriptors, pipes, poll | | `run=ipc_test`; `polltest`, `stdpoll`, `stdasync` |
| `kernel/io/tty`, the console, keyboard | | `run=tty_test`; `stdpty`; typing in mayterm |
| `kernel/net`, drivers | `lib/net` | `run=dhcp;net_test`; `stdtokio`, `stddns` with a host server |
| `lib/net` (TLS, X.509, parsers) | `lib/net` | `curl`/`wget` to an HTTPS host |
| `kernel/kcore/klog` | | `run=dmesg_selftest` |
| `kernel/mayland`, `lib/mayland` | `lib/mayland` `cargo test-host` | `run=mayland_test`; a desktop session in QEMU (`desk.sh`); anything touching the effects pass, VRAM copies or borders also goes on the card checklist |
| `system/src/maywm`, `/etc/mayland` | | `run=mayland_test` (it parses the config grammar), then `maywm` complaints on the console |
| `aosh/` | `cd aosh && cargo test-host`, and `AOSH_GPU_CHECK=1` for painter changes | a session in QEMU; `aosh check` on the config |
| `kernel/drivers/gpu/nvidia` | | `run=nvidia_selftest` in QEMU, then the card checklist |
| `lib/mikupkg`, `builder/` | `lib/mikupkg`, `builder` | a full image build |

## Verification levels

The user's notes record each change as one of: verified on the host (host tests), QEMU-verified (a suite or a session in QEMU), or verified on the card (the RTX 5060 Ti machine). QEMU cannot show the NVIDIA driver at all, nor the compositor's compute pass (blur, GPU fades, video-memory window copies), so anything there is "written, unrun" until the card has run it. Reboots of the real machine are expensive, so card checks are batched into one session from [../../dev/nvidia-card-checklist.md](../../dev/nvidia-card-checklist.md).
