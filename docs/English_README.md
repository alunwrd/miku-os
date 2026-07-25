<div align="center">

# Miku OS

**An experimental operating system kernel written in Rust**

*Powered by Rust and one developer :D*

<img src="https://raw.githubusercontent.com/alunwrd/miku-os/main/docs/miku.png" width="220" alt="Miku Logo">

[![Language](https://img.shields.io/badge/language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Architecture](https://img.shields.io/badge/arch-x86__64-blue.svg)]()
[![Status](https://img.shields.io/badge/status-experimental-yellow.svg)]()
[![License](https://img.shields.io/badge/license-MIT-lightgrey.svg)]()

</div>

---

> **Translations:** [Russian](Russian_README.md) · [Japanese](Japanese_README.md) · [main README](../README.md)
> **ABI reference:** [docs/MikuOS_ABI.md](MikuOS_ABI.md)
> **GPU notes:** [TU116](Nvidia_tu116.md) · [GB206](Nvidia_gb206.md)

---

## About

**Miku OS** is an operating system built from scratch in a `no_std` Rust environment. There is no libc
underneath it and no host runtime: memory layout, interrupt handling, scheduling, filesystems and drivers
are all written here.

What actually runs today: a preemptive SMP kernel, ring-3 user processes with `fork`/`exec`/`wait` and
signals, a dynamic linker with shared libraries, an init daemon (mikuD), a VFS with ext2/ext3/ext4, a
block layer with write-back caching, a TCP/IP stack, and a userspace shell.

> All code is Rust. Assembly appears only in the boot entry, the syscall bridge, the AP trampoline and
> the context switch - places where the calling convention has to be exact.

**Scale:** ~71 500 lines of kernel across 251 files, plus ~30 700 lines of userspace libraries and programs.

---

## Status and honest limitations

This is an experimental system. The list below is what is genuinely missing or provisional, so that the
feature tables further down are not read as more than they are.

| Area | State |
|:--|:--|
| **Preemption** | New. Verified in QEMU on 1-16 CPUs. An earlier naked-asm timer entry wedged real hardware, so it sits behind `PREEMPTIVE_TIMER` in `kernel/arch/x86_64/interrupts.rs` - set it to `false` to fall back to cooperative scheduling if a physical machine hangs after the first tick |
| **Userspace threads** | Absent. No `clone`, no `futex`. Only the kernel uses multiple CPUs; a user process is single-threaded |
| **`poll` / `select`** | Absent. A program cannot wait on several descriptors at once |
| **Terminal** | `termios` (canonical/raw, echo, signal characters) and `ioctl` exist. Sessions, process groups, job control, pty and `/dev/tty` do not |
| **Shell** | Split. `/bin/msh` runs in ring 3, but most commands still live in the in-kernel shell because they call kernel internals with no syscall equivalent |
| **TLS** | Implemented inside the kernel. Parsing untrusted network input at ring 0 is not where it belongs |
| **Tests** | No unit tests. Correctness is guarded by a QEMU boot smoke-test in CI |
| **NVIDIA** | Bring-up in progress: GSP-RM boot path for TU116/TU117 and GB206. Not a usable display driver yet |

---

## Kernel

| Component | Description |
|:--|:--|
| **Architecture** | x86_64, `#![no_std]`, `#![no_main]` |
| **Boot** | GRUB2 + Multiboot2; framebuffer with BGR/RGB auto-detection |
| **Address space** | Higher-half kernel at `0xFFFFFFFF80000000`, HHDM direct map at `0xFFFF800000000000` |
| **Protection** | GDT + TSS + IST (double fault, page fault, GPF), ring 0 / ring 3 |
| **Interrupts** | IDT with timer, keyboard, ATA IRQ 14/15, NMI, MCE, #UD, #NM, #PF, #GP, double fault, LAPIC error, spurious, 3 IPI vectors, 16 MSI vectors |
| **Interrupt controller** | LAPIC + IO-APIC. The legacy 8259 PIC is initialised and then fully masked |
| **Timer** | LAPIC timer, 250 Hz, calibrated against PIT channel 2 with a sanity range and a fallback |
| **SMP** | Up to 64 CPUs (`MAX_CPUS`); ACPI MADT enumeration, INIT/SIPI bring-up, per-CPU GDT/TSS/GS |
| **Scheduling** | CFS-style vruntime, per-CPU run queues, work stealing, preemptive timer |
| **SSE** | CR0.EM=0, CR0.MP=1, CR4.OSFXSR=1, CR4.OSXMMEXCPT=1 |
| **Kernel heap** | 128 MiB, static in `.bss`, `linked_list_allocator` |
| **Kernel stacks** | 1 MiB for the BSP, 512 KiB per thread, 64 KiB per AP - all fenced by a guard page |
| **Syscalls** | 72 (0-71) via `SYSCALL`/`SYSRET` MSRs, naked asm bridge |
| **Real-time clock** | CMOS/MC146818 read at boot, refined by NTP when the network comes up |

### Boot sequence

Each step is reported as `[boot] ok <name>` on the serial console and the framebuffer:

```
Physical memory manager → ACPI (RSDP/MADT) → APIC → IO-APIC → LAPIC timer →
Real-time clock → IRQ routing → Virtual file system → Shared library cache →
Block device probe → Block device nodes (/dev) → Network subsystem →
Firmware store → NVIDIA GPU probe → Scheduler → Firmware SMI silence →
PS/2 keyboard → Interrupts → Timer calibration → SMP (AP bringup) →
mikuD init daemon
```

### Memory safety of the kernel itself

Each of these exists because it caught a real, hard-to-diagnose bug:

- **Stack guard pages** (`kernel/mm/kstack.rs`) - every thread stack takes `pages + 1` contiguous frames
  and unmaps the lowest one from the HHDM. Overflow faults on the guard page, at the instruction that did
  it, and the fault handler names the owning pid. The kernel image is mapped by a single 1 GiB huge page,
  so without this an overflow silently overwrites whatever the allocator handed out next.
- **BSP stack canary** (`kernel/kcore/stack_guard.rs`) - a poisoned page below the boot stack, checked
  after every init step.
- **`IrqMutex`** (`kernel/kcore/irq_lock.rs`) - disables interrupts for the critical section. Required for
  every lock an interrupt handler can touch (`PMM`, `SWAP_MAP`, `EMERGENCY_POOL`, `REFCOUNTS`,
  `MSI_HANDLERS`); a plain spin lock there deadlocks a CPU against itself.
- **`SchedMutex`** (`kernel/kcore/sched_lock.rs`) - spins briefly, then yields to the scheduler. Used for
  the VFS lock, which is held across disk I/O where spinning wastes whole timeslices.

---

## Memory management

### Physical (`kernel/mm/pmm.rs`)

Bitmap frame allocator with word-parallel scanning, separate hints for single-frame and contiguous
allocation, refcounting for copy-on-write, and an emergency reserve pool used by the swap-in path.

### Virtual (`kernel/mm/vmm.rs`)

Four-level paging behind an `AddressSpace` type. User address spaces copy PML4 entries 256-511 from the
kernel tables, so kernel and HHDM mappings are shared and stay in sync. Huge pages are split on demand
when a 4 KiB mapping is needed (MMIO, guard pages).

### Swap (`kernel/mm/swap.rs`, `swap_map.rs`)

Reverse mapping from physical frame to `(cr3, virt)`, clock-sweep eviction with aging and pinning, and a
swap PTE encoding that preserves the original flags. Reclaim runs on the **kswapd** thread - the timer
tick only raises a flag, because eviction performs disk I/O and takes locks that must never be entered
from an interrupt handler.

### mmap (`kernel/mm/mmap.rs`)

Anonymous and file-backed mappings, demand paging, copy-on-write across `fork`, `mprotect`, `msync`.

---

## Processes and scheduling

| Feature | Detail |
|:--|:--|
| **Model** | One `Process` per thread; kernel threads share the kernel CR3, user processes get their own |
| **Creation** | `fork` (CoW), `exec`/`execve`, `wait4`, `kill`, zombie reaping |
| **Scheduler** | CFS-style vruntime with priority weights, per-CPU run queues, min-vruntime selection |
| **Load balancing** | Placement on the lightest eligible CPU plus work stealing when a queue runs dry |
| **Preemption** | Timer-driven. The naked stub builds the 15-GPR + iret frame the scheduler expects and resumes on whatever stack it returns |
| **Affinity** | 64-bit CPU mask per process |
| **Signals** | User-registered dispatch entry, `sigreturn`, SIGINT/SIGQUIT/SIGTERM/SIGKILL/SIGCHLD |
| **Worker pool** | Sized from the ACPI CPU count, clamped to 4-32 threads |
| **Descriptors** | Per-process FD table, cloned on `fork`, released at `exit` rather than at reap |

---

## System calls

72 entries, dispatched from a naked `SYSCALL` bridge that swaps to the per-CPU kernel stack via `gs`.

| Range | Area |
|:--|:--|
| 0-10 | `exit`, `write`, `read`, `mmap`, `munmap`, `mprotect`, `brk`, `getpid`, `getcwd`, `set_tls`, `get_tls` |
| 11-17 | `open`, `close`, `seek`, `fsize`, `map_lib`, `sleep`, `uptime` |
| 18-27 | `stat`, `fstat`, `mkdir`, `rmdir`, `unlink`, `readdir`, `rename`, `link`, `chmod`, `chown` |
| 28-42 | `dup`, `dup2`, `truncate`, `write_file`, `symlink`, `readlink`, `pipe`, `chdir`, `statfs`, `fallocate`, `getxattr`, `setxattr`, `utimensat`, `fsync`, `punch_hole` |
| 43-47 | `fork`, `wait4`, `kill`, `exec`, `umask` |
| 48-55 | `getuid`, `getgid`, `geteuid`, `getegid`, `setuid`, `setgid`, `seteuid`, `setegid` |
| 56-66 | `socket`, `connect`, `send`, `recv`, `mmap_file`, `msync`, `bind`, `listen`, `accept`, `sendto`, `recvfrom` |
| 67-69 | `execve`, `sigentry`, `sigreturn` |
| 70-71 | `clock_gettime`, `ioctl` |

**User pointer validation** (`kernel/syscall/usercopy.rs`): every range must lie in the canonical user
half, every page is walked to confirm `PRESENT | USER` (and `WRITABLE` where needed), lazy VMA pages are
faulted in first, and paths are copied into kernel memory before use so they cannot be rewritten after
validation.

---

## Virtual file system

| Property | Value |
|:--|:--|
| Vnodes | 256 |
| Open files per process | 128 |
| Mount slots | 8 (VFS) / 4 (ext driver) |
| Name length | 64 bytes |
| Page cache | 1024 pages × 512 B = 512 KiB |
| Filesystem types | tmpfs, devfs, procfs, ext2, ext3, ext4, cowfs, pipefs |

Features: hierarchical namespace, hard and symbolic links, permissions with uid/gid/umask, extended
attributes, file locks, per-process cwd, dentry cache, journal hooks, quota accounting, content-addressed
storage helpers, and `/dev` block-device nodes generated from the block layer.

`/proc` exposes `uptime`, `meminfo`, `diskstats` and friends; `/dev` carries the console, null, zero,
random and the block devices; `/lib` holds the ten preloaded shared libraries.

---

## Filesystems

### ext2 / ext3 / ext4 (`kernel/fs/ext/`)

The driver detects the on-disk flavour from the superblock feature bits and reports it as such -
`ext2`, `ext3` or `ext4` - in the mount log, in `statfs()` and in `/proc`:

```
[miku_extfs] slot 0 drive 1 lba 0 - ext4 (journal=true extents=true 64bit=true
                                          metadata_csum=true flex_bg=true dir_index=true)
```

| Capability | Detail |
|:--|:--|
| **Read** | Direct, indirect, double- and triple-indirect blocks; ext4 extent trees; inline data |
| **Write** | Allocation with per-group hints, truncate, punch hole, fallocate, rename, links |
| **ext3** | JBD2-style journal: transactions, descriptor and revoke blocks, replay on mount |
| **ext4** | Extent trees, 64-bit block numbers, flex_bg, metadata checksums (CRC32c), inline data |
| **Volume size** | Limited by the filesystem, not the driver - block group tables grow on the heap |
| **Integrity** | fsck, orphan inode cleanup on mount, TRIM/discard, `fiemap` |
| **mkfs** | ext2/ext3/ext4 creation with a dry-run mode |

### Others

- **tmpfs** - the root filesystem, page-cache backed
- **devfs**, **procfs** - synthesised
- **GPT** - partition table parsing, partition nodes, `partprobe`

---

## Storage stack

```
VFS  →  block layer  →  buffer cache  →  driver  →  device
```

- **Block layer** (`kernel/io/block/`) - device registry, partition mapping, per-device I/O statistics,
  discard and write-zeroes, health monitoring (NVMe log pages and ATA SMART), `/proc/diskstats`
- **Buffer cache** - 512 chunks × 4 KiB, 8-way set associative, write-back with a **bdflush** flusher
  thread
- **Queues** - `&self` drivers with lockless dispatch; NVMe uses four per-CPU submission queues

**Drivers** (`kernel/drivers/block/`): `ata` (PIO + bus-master DMA), `ahci`, `nvme`, `virtio_blk`,
`ramdisk` (used for the firmware image handed over by GRUB).

---

## Network stack

| Layer | Support |
|:--|:--|
| **Drivers** | Intel e1000 (82540EM/82545EM/82574L/82579LM/I217), Realtek RTL8139/8168/8169, virtio-net |
| **Link** | Ethernet, ARP with cache |
| **Network** | IPv4, ICMP (ping, traceroute) |
| **Transport** | UDP, TCP with a full state machine, retransmission and listeners |
| **Application** | DHCP client, DNS resolver, HTTP/1.1, HTTP/2, NTP |
| **Security** | TLS 1.2/1.3 client: RSA, ECDHE, AES-GCM, bignum arithmetic - currently in ring 0 |
| **Sockets** | Per-process socket table; `socket`, `bind`, `listen`, `accept`, `connect`, `send`, `recv`, `sendto`, `recvfrom` |

`netd` runs DHCP at boot and brings the link up automatically.

---

## Input and console

- **PS/2 keyboard** - controller init, scancode set 1, lock-free ring buffer
- **USB** - xHCI controller, HID keyboard, BIOS/EHCI handoff
- **Console** - framebuffer text console with colour, scrolling and a JetBrains Mono bitmap font generated
  at build time. All console output is mirrored to the serial port, which is what makes headless boots and
  CI logs useful
- **Terminal** (`kernel/io/input/user_stdin.rs`) - line discipline with canonical and raw modes, echo
  control, Ctrl-C → SIGINT, Ctrl-\ → SIGQUIT, Ctrl-D → EOF, Ctrl-U → kill line. Settings are reachable
  from ring 3 through `ioctl` (`TCGETS`, `TCSETS`, `TIOCGWINSZ`)

---

## mikuD - init daemon

PID 1. Owns service lifecycle, dependencies and restarts.

| Concept | Values |
|:--|:--|
| **Targets** | `SysInit`, `MultiUser`, `Graphical`, `Rescue` |
| **Restart policies** | `Always`, `Never`, `OnFailure`, `OnSuccess`, `OnAbnormal` |
| **Service entry** | A kernel `fn()` or an ELF binary path on disk |
| **Extras** | Dependency graph, ordering, watchdogs, restart delays, masking, journal, timer units, socket activation, `.service` unit files |

Services registered at boot: `kbd`, `shell`, `netd`, `usbd`, `bdflush`, `kswapd`.

The `shell` service prefers `/bin/msh` (ring 3) when the root filesystem carries it, and falls back to the
in-kernel shell otherwise. The choice is logged.

---

## Userspace

### Dynamic linking

`ld-miku` is the dynamic loader: ELF64 parsing, `PT_LOAD` mapping, relocation processing (`RELA`,
`JMPREL`, `GLOB_DAT`, `JUMP_SLOT`, `RELATIVE`), symbol resolution across libraries, `DT_NEEDED` dependency
walking, TLS setup and a full auxv.

Ten shared libraries are preloaded into the VFS at `/lib` and served straight from kernel image memory
without touching the page cache:

`core_miku`, `sys_miku`, `text_miku`, `ds_miku`, `algo_miku`, `codec_miku`, `fs_miku`, `net_miku`,
`parse_miku`, `libc_miku`

Pages are shared between processes that map the same library; only writable segments are private.

### `/bin/msh` - the userspace shell

A ring-3 shell built strictly on the syscall ABI, which is what keeps that ABI honest.

**Builtins:** `pwd`, `cd`, `ls`, `cat`, `stat`, `mkdir`, `rm`, `write`, `echo`, `wc`, `head`, `grep`,
`uptime`, `date`, `stty`, `pid`, `help`, `exit`

**Shell features:** redirection `>` `>>` `<`, pipelines `|` (one forked process per stage), and a startup
script at `/etc/msh.rc`.

```
$ cat /p.txt | wc
3 3 14
$ ls /bin | grep msh
msh
```

### Building userspace programs

```bash
cd src/lib/userspace
cargo +nightly build --release --target x86_64-miku-app.json \
    -Z json-target-spec -Z build-std=core \
    -Z build-std-features=compiler-builtins-mem --bin msh
```

The builder does this automatically and stages the binaries into `/bin` on the root image.

---

## In-kernel shell

Still the larger of the two shells, because these commands reach into kernel internals that have no
syscall yet. Roughly 190 commands, including:

- **Files** - `ls`, `cat`, `cd`, `cp`, `mv`, `rm`, `mkdir`, `tree`, `du`, `stat`, `ln`, `chmod`, `chattr`
- **ext (version-agnostic)** - `extls`, `extcat`, `extwrite`, `extfsck`, `extinfo`, `extsync`, … plus the
  `ext2*` / `ext3*` / `ext4*` families
- **Mounting** - `mount`, `umount`, `fs.list`, `fs.select`, `partprobe`, `gpt`, `mkfs.ext2/3/4`
- **Storage** - `blkstat`, `blkdiscard`, `blkzero`, `smart`, `fstrim`, `fiemap`, `nvmestress`
- **Swap** - `mkswap`, `swapon`, `swapoff`, `swapinfo`
- **Processes** - `ps`, `top`, `kill`, `nice`, `affinity`, `exec`
- **Services** - `sv start|stop|restart|status|enable|disable|mask|journal|timer|analyze`
- **Network** - `net`, `ping`, `dhcp`, `ntp`, `wget`, `curl`, `fetch`, `traceroute`, `socket`
- **Linking** - `ldd`, `ldconfig`, `load`
- **GPU** - `nvidia` subcommands
- **System** - `info`, `heap`, `memmap`, `history`, `reboot`, `poweroff`

---

## NVIDIA GPU driver

Bring-up work in progress - not a display driver.

| Chip family | Target | State |
|:--|:--|:--|
| TU116 / TU117 (Turing) | GTX 1650 / 1660 | Falcon bring-up, FWSEC, ACR, SEC2, GSP-RM boot args, message queues |
| GB206 (Blackwell) | RTX 5060 / 5060 Ti | FSP, FMC, GSP bootloader path |

Module layout (`kernel/drivers/gpu/nvidia/`): `pci`, `mmio`, `chip`, `vbios`, `reset`, `msi`, `fb`,
`profile`, `generic`, a shared `gsp_common/` (RPC, sysinfo) and per-chip `gtx1650/` and `rtx5060/`.

Firmware is not embedded in the kernel. It is staged into a `/lib/firmware` tree, packed into an ext2
image, handed to the kernel as a GRUB module and mounted on demand - so a single ISO carries kernel and
firmware without needing a second disk.

---

## Build and run

### Requirements

| Tool | Purpose |
|:--|:--|
| Rust nightly + `rust-src` | `build-std` for the bare-metal target |
| `grub-mkrescue`, `xorriso`, `mtools` | ISO creation |
| `qemu-system-x86_64` | Running |
| `e2fsprogs` (`mke2fs`, `debugfs`) | Root image and firmware staging |

### Build everything

```bash
cd builder
cargo run
```

The builder builds `ld-miku`, the miku libraries, the userspace programs and the kernel (release), creates
the ISO, provisions `disk.img` with `/lib/firmware` and `/bin`, and can launch QEMU.

### Run by hand

```bash
qemu-system-x86_64 \
  -boot d -cdrom miku-os/miku-os.iso \
  -drive file=miku-os/disk.img,format=raw,if=none,id=disk0,cache=unsafe,aio=threads \
  -device ide-hd,drive=disk0,bus=ide.0,unit=1 \
  -serial stdio -display gtk -m 4G -smp 4 \
  -device qemu-xhci,id=xhci -device usb-kbd,bus=xhci.0
```

### Write to a USB stick

```bash
sudo dd if=miku-os/miku-os.iso of=/dev/sdX bs=4M status=progress conv=fdatasync
```

---

## Repository layout

```
kernel/                 kernel sources
  arch/x86_64/          boot entry, GDT, IDT, APIC, ACPI, SMP, RTC, per-CPU, serial
  mm/                   pmm, vmm, heap, mmap, swap, kernel stacks
  sched/                CFS scheduler, run queues, lifecycle, workers
  process/              Process, ELF loader, dynamic linking, signals
  syscall/              dispatch, adapters, usercopy, errno
  fs/                   VFS, ext2/3/4, tmpfs, devfs, procfs, mkfs, GPT
  io/                   block layer, console, framebuffer, input
  net/                  Ethernet through TLS
  drivers/              block, bus (PCI), net, input (PS/2, USB), gpu (NVIDIA)
  mikud/                init daemon
  shell/                in-kernel shell
  kcore/                boot state, clock, firmware, locks, power, RNG, time
src/lib/                userspace
  ld_miku/              dynamic loader
  libmiku/, mikulibs/   standard library sources and per-domain libraries
  userspace/            ring-3 programs (msh, hello, tests)
builder/                build orchestration and ISO/disk provisioning
docs/                   translations, ABI reference, GPU notes
```

---

## Author

**alunwrd** - [github.com/alunwrd](https://github.com/alunwrd)

Written in Rust, from scratch, by one person.

## License

MIT - see [LICENSE](../LICENSE).
