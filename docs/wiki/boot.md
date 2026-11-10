# Boot

This page follows a MikuOS boot from GRUB to the desktop: the multiboot2 hand-off and the 32-bit entry stub, the early console that marks progress before there is a real console, every `boot_step` of `kernel_main` in order and what it prints, how `/bin` and `/etc` are mounted, the services mikuD starts, and the greeter and the session. It then describes the GRUB menu, lists every kernel command-line flag, describes the serial log, and explains how to read a boot that hangs or panics.

## Contents

- [Overview](#overview)
- [GRUB and the entry stub](#grub-and-the-entry-stub)
- [Before the console: earlycon](#before-the-console-earlycon)
- [The boot steps](#the-boot-steps)
- [Mounting /bin, /etc and the firmware](#mounting-bin-etc-and-the-firmware)
- [Services and targets](#services-and-targets)
- [Login and the desktop](#login-and-the-desktop)
- [The GRUB menu](#the-grub-menu)
- [The boot line](#the-boot-line)
- [Kernel command-line reference](#kernel-command-line-reference)
- [The serial log](#the-serial-log)
- [Reading a boot that hangs or panics](#reading-a-boot-that-hangs-or-panics)
- [Boot time](#boot-time)
- [Code map](#code-map)

## Overview

```
BIOS -> GRUB (grub.cfg, 3 s menu)
          multiboot2 /boot/kernel.elf <flags>
          module2   /boot/firmware.img firmware
     -> _start32 (32-bit, 1 MiB)    page tables, long mode
     -> _start (high half)          1 MiB boot stack
     -> kernel_main_grub            latch the boot line
     -> kernel_main
          earlycon steps            gdt ... framebuffer console
          boot_step!s               memory, ACPI, APIC, PCI, VFS, disks, network,
                                    firmware store, GPU probe, Mayland, scheduler,
                                    keyboard, interrupts, SMP, GPU display
          services registered       kbd, mikudod, shell, netd, usbd, bdflush, kswapd,
                                    compositor, greeter
          mikuD started             target: graphical or multi-user
          run= line (if any)        on its own task
          "[boot] up in N ms"
          idle
     -> maygreet -> login -> maysession -> maywm, aosh, mayterm
```

Everything up to `mark_done` runs on the boot processor. The boot is BIOS only: under UEFI, GRUB 2.14 faults in its own `multiboot2` command ([building.md](building.md#running-in-qemu)).

## GRUB and the entry stub

**The menu.** `grub.cfg` at the repository root sets `timeout=3` and `default=0`. The builder copies it into the ISO and adds `module2 /boot/firmware.img firmware` after every `multiboot2` line ([building.md](building.md#the-steps-in-order)). The entry runs `insmod all_video` and `set gfxpayload=keep`, so the kernel receives the video mode GRUB set as a linear framebuffer.

**The multiboot2 header** (`kernel/arch/x86_64/boot_entry.rs`, section `.multiboot2_header` at physical 1 MiB) asks for a framebuffer (tag 5, any size and depth) and, as an optional request, the ELF section headers (tag 9), so the kernel can find its own `.symtab` and name functions in a panic.

**`_start32`** is the ELF entry (`linker.ld`: `ENTRY(_start32)`), running in 32-bit protected mode with interrupts off. It clears the direction flag, builds page tables at physical 0x1000-0x4FFF and enters long mode:

| Mapping | How | Purpose |
|---|---|---|
| Virtual 0 .. 4 GiB → physical 0 .. 4 GiB | PML4[0], four 1 GiB pages | Identity, for the jump |
| `0xFFFF800000000000` + 512 GiB → physical 0 .. 512 GiB | PML4[256], 512 1 GiB pages | The direct map (HHDM) |
| `0xFFFFFFFF80000000` → physical 0 | PML4[511], PDPT[510], one 1 GiB page | The kernel image (`KERNEL_VMA`) |

It sets CR4.PAE, EFER.LME, CR0.PG, loads a three-entry boot GDT, far-jumps to 64-bit code, then jumps to `_start` in the high half, which loads `_stack_top` (a 1 MiB boot stack with a 4 KiB guard pattern below it, checked by `stack_guard::check` after every boot step) and calls `kernel_main_grub(mb2_info)`. The kernel's text is linked at `0xFFFFFFFF80200000` and loaded at physical 2 MiB.

**`kernel_main_grub`** records where the multiboot2 information is and copies the command line out of it at once (up to 1024 bytes, `CMDLINE_MAX` in `grub.rs`), because GRUB places the information in memory the frame allocator will later hand out. Then `kernel_main`.

## Before the console: earlycon

Until `init_framebuffer` there is no console. `kernel/arch/x86_64/earlycon.rs` paints progress straight into the framebuffer GRUB handed over, with a static font and no heap or locks, and writes the same line to serial as `[early] ok: <step>`. Lines are drawn from the bottom up so the real console, which starts at the top, does not cover them. On a good boot only the last survives; on a hang the trail stays and the topmost line is the last phase that finished. No line at all means the kernel never got control. `earlycon::init` prints `[early] handoff ok, firmware path: ...`, or `[early] no RGB framebuffer from GRUB, screen stays blank`.

The early phase of `kernel_main`, in order:

| Step | What happens |
|---|---|
| `earlycon::init`, stack guard | `[kern] MikuOS starting (Release v1.0.2)` |
| `gdt` | The kernel's GDT and TSS |
| `sse` | CR0.EM off, CR0.MP, CR4.OSFXSR and OSXMMEXCPT on |
| `cpu guards` | CR0.WP, SMEP and SMAP on where the CPU has them (`nosmep`/`nosmap` turn them off); `[cpu] CR0.WP=on SMEP=on SMAP=on` and a `[cpu] CET:` line (CET is detected, never enabled) |
| `tsc` | The TSC calibrated against PIT channel 2 (10 ms); every later delay is a cycle count |
| `syscall MSRs` | `serial::set_trace` from the `trace` flag first, then the SYSCALL MSRs |
| `idt` | The interrupt table |
| `kernel heap` | The slab heap ([memory.md](memory.md)) |
| `pat (write-combining)` | The page attribute table, before anything maps an aperture |
| `scheduler` | The run queues are reset |
| `framebuffer console` | The console exists from here |

Then, without earlycon lines: the frame allocator is given GRUB's memory map; the low 24 KiB, the AP trampoline page, the kernel image, the multiboot2 information and every GRUB module (`[kern] GRUB module ... reserved`) are reserved; the persistent store (pstore) is claimed and last boot's log recovered; with `klogpstore` the whole log is mirrored into it; the frame bitmap is laid out; vmap is set up.

## The boot steps

Each `boot_step!(name, expr)` (`kernel/kcore/boot_state.rs`) times the step, checks the boot stack's guard, and prints `[  ok  ] name` or `[ fail ] name: reason` on the console and `[boot] ok  name` or `[boot] fail name : reason` on serial. A step of 5 ms or more also prints its time: `[boot] ok  NVIDIA display (812 ms)`. A failed step is recorded and the boot goes on.

| # | Step | What it does | Typical serial lines |
|---|---|---|---|
| 1 | Physical memory manager | Marks the allocator ready; then the kernel's symbols are loaded and the vDSO built (`[early] ok: symbols`, `vdso`), and the framebuffer is remapped write-combining and given a shadow copy (`fbdirect` keeps drawing on the framebuffer) | `[kern] framebuffer ... mapped write-combining`, `[kern] console draws into a N KiB shadow` |
| 2 | ACPI (RSDP/MADT) | Finds the CPUs and the I/O APICs | `[acpi] ...` |
| 3 | APIC | The boot CPU's local APIC | `[apic] ...` |
| 4 | IO-APIC | Programs the I/O APIC | |
| 5 | LAPIC timer | Calibrates the local APIC timer; chooses TSC-deadline, one-shot or periodic 250 Hz (`tickonly` forces periodic) | `[apic] calibrated: ...` |
| 6 | Real-time clock | Wall time from the RTC | |
| 7 | IRQ routing | Keyboard (1), mouse (12), ATA (14, 15) to the boot CPU | |
| 8 | PCI bus scan | One bridge-following enumeration that every driver then filters | `[pci] N functions on the bus` |
| 9 | Virtual file system | The VFS and its tmpfs root ([vfs.md](vfs.md)) | `[vfs] ...` |
| 10 | Shared library cache | Preloads the shared libraries embedded in the kernel and runs `ldconfig` | `[solib] preloaded 10 libraries, N KB` |
| 11 | Block device probe | ATA, AHCI, NVMe, virtio-blk ([storage.md](storage.md)) | `[ata]`, `[ahci]`, `[nvme]`, `[block]` |
| 12 | Block device nodes (/dev) | `/dev` entries for each disk | |
| 13 | Network subsystem | NIC drivers and the stack ([network.md](network.md)) | `[net] ...` |
| 14 | Firmware store | Mounts the boot image, grafts `/bin`, `/etc`, `/pkg`, `/lib/firmware` ([below](#mounting-bin-etc-and-the-firmware)) | `[miku_extfs] ...`, `[fwload] ...` |
| 15 | NVIDIA GPU probe | Finds NVIDIA cards, maps BARs, prints the `MIKU_FW=` value for each card's firmware ([../Nvidia_gb206.md](../Nvidia_gb206.md)) | `[nvidia] ...` |
| 16 | Mayland protocol | The display server's tables ([mayland.md](mayland.md)) | |
| 17 | Scheduler | The main thread and the kernel worker threads | `[sched] ...` |
| 18 | Firmware SMI silence | Switches the machine to ACPI mode, turns off the Intel and AMD chipset SMI sources, the HPET's SMI and GPE0, and programs LINT0/LINT1 of the local APIC as the MADT says (`kcore/firmware/mod.rs::run`) | `[firmware] cpu vendor: ...`, `[firmware] LVT LINT0=... LINT1=...` (on the console) |
| 19 | PS/2 keyboard | The i8042 keyboard, then the mouse (`[ps2] no mouse: ...` when there is none) | `[ps2] ...` |
| 20 | Interrupts | Interrupts on for good, after a guarded test enable; the LAPIC timer re-armed | `[apic] timer armed: vec=... masked=false` |
| 21 | Timer calibration | The scheduler clock | |
| 22 | SMP (AP bringup) | The other CPUs, with one INIT wait for all | `[smp] ...` |
| 23 | NVIDIA display | On an RTX 50 card: boots GSP-RM, sets the mode, brings up the channels and the GPU programs; skipped with `nogpu` | `[nvidia] display up: WxH at vram ..., head H window W` |
| 24 | mikuD init daemon | Starts mikuD after the services below are registered | `[mikud] booting to graphical (screen true, greeter true, noauth false)` |

After step 24: the console is cleared, the kernel shell is initialised, a `run=` list is handed to a task named `bootline` ([below](#the-boot-line)), `mark_done` prints `[boot] up in N ms (M ms of it inside init steps)`, and the boot processor becomes an idle loop.

## Mounting /bin, /etc and the firmware

Step 14 (`kcore/firmware/load.rs::init`, through `fs/ext/mount.rs::mount_root_disk`):

1. If GRUB loaded a module named `firmware`, its memory is registered as a RAM disk and mounted read-only as ext2: `[miku_extfs] firmware store mounted read-only from GRUB module (dev N)`. This is the image the builder made from the packages.
2. `/bin`, `/etc` and `/pkg` of that filesystem are grafted onto the VFS.
3. If the boot line has `root=`, the disk it names is mounted, read-only unless `rootrw` is also there, and its `/bin`, `/etc` and `/pkg` replace the boot image's (`[fwload] /etc now comes from the disk named by root=`). `/lib/firmware` stays the boot image's. Without `root=`: `[fwload] /etc is the read-only boot copy; add 'root=<drive>:<lba> rootrw' to keep account changes across a reboot`.
4. `/lib/firmware` of the boot image is grafted under the tmpfs `/lib`: `[fwload] /lib/firmware mounted from root disk`, or `... not present on root disk; firmware unavailable`.

With no GRUB module, `root=` names the only root. **No disk is mounted that the boot line does not name**: without `root=` the kernel prints `[miku_extfs] no root= on the boot line; no disk mounted automatically (use 'ext2mount <drive>' to pick one)`. Everything else (`/`, `/tmp`, `/home`, `/run`) is tmpfs and starts empty each boot ([filesystems.md](filesystems.md)).

## Services and targets

`kernel_main` registers these services with mikuD before starting it ([system.md](system.md) describes mikuD itself and `sv`):

| Service | Kind | Target | Restart | Notes |
|---|---|---|---|---|
| `kbd` | kernel thread | multi-user | always | Keyboard input; critical |
| `mikudod` | `/bin/mikudod` | multi-user | always | The elevation broker |
| `shell` | kernel thread | multi-user | always | The in-kernel console shell; depends on `kbd`; critical; runs `login` on the console unless `noauth` |
| `netd` | kernel thread | multi-user | on failure | DHCP |
| `usbd` | kernel thread | multi-user | on failure | xHCI and USB HID; not registered with `nousb` |
| `bdflush` | kernel thread | multi-user | always | Block cache write-back |
| `kswapd` | kernel thread | multi-user | always | Page reclaim; also reports overdue sleepers (`schedwatch`) |
| `compositor` | kernel thread | graphical | always | Mayland composition |
| `greeter` | `/bin/maygreet` | graphical | always | Only when `/etc/passwd` has a `greeter` account; runs as that account with `CAP_MAY_PRIV`; depends on `mikudod` and `compositor`; started while seat 0 is free |

The default target is **graphical** when there is a framebuffer, `/bin/maygreet` exists and the boot line has no `noauth`; otherwise **multi-user**. The `net=raw` flag, read at the same point, hands the network card to `/dev/net0` for a ring-3 stack and leaves the kernel's idle.

## Login and the desktop

On the graphical target, `maygreet` draws the login screen. A password goes to `mikudod`, which checks it and starts the session as that user ([system.md](system.md)). The session program `maysession` reads `/etc/mayland/session.d/*.conf`; the shipped `10-desktop.conf` starts `/bin/maywm` (window manager), `/bin/aosh` (bar and launcher, [aosh.md](aosh.md)) and `/bin/mayterm` (a terminal running `msh`). While the desktop is up there is no kernel console on the screen; the log goes to serial and `dmesg` ([input-and-terminals.md](input-and-terminals.md)).

On the multi-user target, the kernel shell runs `login` on the console. With `noauth`, the console stays a root kernel shell and no login is asked for (`[shell] 'noauth' - the console stays root`).

## The GRUB menu

The menu has one entry, `MikuOS`, with nothing on its boot line. It shows for three seconds so that a flag can be added for one boot ([the boot line](#the-boot-line)): the ways back and the selftests are flags, not entries.

## The boot line

To add a flag for one boot, press `e` at the GRUB menu, append the words to the `multiboot2 /boot/kernel.elf` line and press Ctrl-X. To make it permanent, edit `grub.cfg` and rebuild the ISO.

- Flags are bare words; arguments are `key=value` words. Matching is exact and case-sensitive: `nogpu`, not `nogpu=1`.
- The kernel keeps the first 1024 bytes of the line. There is no `/proc/cmdline`; programs in ring 3 never see the line.
- **`run=`** takes commands separated by `;`, with `_` standing for a space (`run=dhcp;net_test;poweroff` runs `dhcp`, `net test`, `poweroff`). In `grub.cfg`, put the whole word in double quotes when it contains `;`. The list runs in the kernel shell as root, one command after another, on a kernel task named `bootline` that is started after the services, so the commands run preempted like the rest of the system. Each command prints `[shell] boot line asked for: '<command>'`. The value is cut at 1024 bytes; a cut list drops its partial last command and says so. End a headless list with `poweroff` (or `reboot`) or the machine stays up. Put `dhcp` first when a later command needs the network, or it races netd's own DHCP.

## Kernel command-line reference

Every word the kernel reads from the boot line, from `grub::cmdline_flag` and `grub::cmdline_arg` (and `kstack.rs`'s cached reader). Most are escape hatches: each switches one mechanism back to what it was before a change, so a machine that misbehaves after that change can be booted without a rebuild.

### Boot, login and diagnostics

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `run=<cmd>[;<cmd>...]` | Runs kernel shell commands after boot on the `bootline` task | Headless selftests and scripted checks ([testing.md](testing.md)) | `kernel/main.rs` |
| `noauth` | Default target multi-user, no greeter, and the console stays a root shell with no login | Scripted boots (there is no serial input), working on login, recovering a machine | `kernel/main.rs`, `kernel/shell/mod.rs` |
| `root=<drive>[:<lba>]`, `root=LABEL=<name>` | Mounts that ext disk and takes `/bin`, `/etc`, `/pkg` from it. Drive is the kernel's block device number, LBA the filesystem's first sector (default 0); a label is 1 to 16 bytes | A persistent install, or keeping account changes | `kernel/fs/ext/mount.rs`, `kernel/kcore/firmware/load.rs` |
| `rootrw` | Mounts the `root=` disk read-write (default read-only) | With `root=` | `kernel/fs/ext/mount.rs` |
| `trace` | Every `trace_println!` and `kdebug!` line goes down the serial wire too (per-node VFS, per-library, per-syscall and per-RPC traces) | Following a boot or a syscall in detail; slow on real serial | `kernel/main.rs`, `arch/x86_64/serial.rs` |
| `klogpstore` | The whole kernel log is mirrored into the persistent RAM store as it is written | A run whose screen goes dark, to be read after a warm reset | `kernel/main.rs` |
| `fbdirect` | The console draws on the framebuffer itself, with no shadow copy | Only to compare; reading a GPU aperture made scrolling 0.78 s a line on the GB206 | `kernel/io/console/mod.rs` |
| `net=raw` | The network card is handed to `/dev/net0` for a ring-3 stack; the kernel stack stays idle. Default `net=kernel` | Developing a network stack in ring 3 | `kernel/main.rs` |
| `nousb` | usbd is not started; USB is left to the firmware and its PS/2 emulation | A boot that hangs in USB bring-up | `kernel/main.rs` |

### CPU, memory and scheduling

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `nosmap` | SMAP off | First thing to try if a boot dies after a change that touches user memory from the kernel | `arch/x86_64/cpu.rs` |
| `nosmep` | SMEP off | As above, for executing user pages | `arch/x86_64/cpu.rs` |
| `nocet` | Only changes the `[cpu] CET:` line to `off (cmdline)`; CET is never enabled | — | `arch/x86_64/cpu.rs` |
| `tickonly` | The periodic 250 Hz LAPIC timer instead of TSC-deadline or one-shot | Timer or wake-up problems | `arch/x86_64/apic.rs` |
| `nopoll` | Idle CPUs halt at once instead of polling briefly; every cross-CPU wake is an IPI | Idle or wake-up problems | `kernel/sched/idle.rs` |
| `schedwatch` | kswapd also reports tasks READY or RUNNING for a second on no CPU, a line per CPU, and the whole task table every 5 s (without it, only a sleeper more than 1 s overdue is reported) | A hang where nothing else names the stuck task | `kernel/sched/stats.rs` |
| `lockstat` | Times every SchedMutex and SchedRwLock hold; `/proc/lockstat`, and a `[lockstat]` warning for holds of 1 ms or more | Finding lock contention ([diagnostics.md](diagnostics.md)) | `kernel/kcore/sched_lock.rs` |
| `tlbalways` | Every unmap, `mprotect` and copy-on-write break shoots down every CPU, not only CPUs with the address space loaded; logs `[tlb] tlbalways` | Suspected stale translations | `kernel/mm/tlb.rs` |
| `slabdebug` | The heap's per-CPU magazines stay off, and every cache is poisoned and red-zoned | Heap corruption | `kernel/mm/heap/mod.rs` |
| `bigstacks` | Program threads' kernel stacks 512 KiB instead of 128 KiB | First try on a `kernel stack overflow` panic | `kernel/mm/kstack.rs` |
| `stackdepth` | Measures only: every kernel stack is read at exit, each new deepest is logged as `[kstack] deepest so far`, and `/proc/meminfo` shows `KernelStackMax:` | Sizing kernel stacks | `kernel/mm/kstack.rs` |

### Disks and files

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `nvmepoll` | NVMe queues without MSI-X and AHCI without MSI: every command polls | Disk I/O that hangs or loses completions | `drivers/block/nvme.rs`, `drivers/block/ahci.rs` |
| `nopagecache` | No page cache for disk files: every read goes to the filesystem under the VFS lock | Suspected stale or wrong file data | `fs/vfs/core/cached.rs` |
| `writethrough` | No write-back: every write to a disk file goes straight to the filesystem | Suspected lost writes | `fs/vfs/core/writeback.rs` |
| `walklocked` | `stat`, `open` and `getdents` read the disk under the VFS lock instead of deferring the read | Suspected path-walk races | `fs/vfs/core/mod.rs` |
| `vfsmutex` | `stat`, `lstat` and `readlink` take the VFS lock exclusively instead of shared | Suspected read-side locking bugs | `fs/vfs/core/mod.rs` |

See [vfs.md](vfs.md) and [storage.md](storage.md) for what these mechanisms are.

### Network

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `netpoll` | No NIC MSI or MSI-X; netd pumps the cards every tick | Network that stops receiving | `kernel/net/irq.rs` |
| `net=raw` | See above | | |

### GPU and display

All but `vrr` are ways back for one change; most matter only on an NVIDIA card. [../Nvidia_gb206.md](../Nvidia_gb206.md) describes each mechanism.

| Flag | Effect | When to use | Code |
|---|---|---|---|
| `nogpu` | The NVIDIA display boot is skipped; the card stays as the firmware set it, the desktop composes on the processor. `nvidia gb206-boot` still works by hand | A boot that dies at `NVIDIA display` | `drivers/gpu/nvidia/mod.rs` |
| `nointr` | The GPU's MSI is never enabled; every GPU wait polls | GPU interrupt problems | `drivers/gpu/nvidia/intr.rs` |
| `nononstall` | Only the fence (non-stall) interrupts stay off; fence waits spin | Fence wake-up problems | `drivers/gpu/nvidia/blackwell/interrupts.rs` |
| `gspbase` | GSP-RM is booted without the sysmem flush buffer, the wall-clock time, `bIsPrimary` from the machine, the larger heap, the CrashCat queue, and with the chipset half of the system information as zeros | GSP-RM stops answering `GSP_INIT` | `drivers/gpu/nvidia/blackwell/bringup/mod.rs` |
| `gsprecover` | When the event thread sees GSP-RM die, a kernel worker unloads and reloads it | Experimenting with recovery | `drivers/gpu/nvidia/blackwell/bringup/recover.rs` |
| `pciebase` | The GPU's PCIe Device Control is left as the firmware set it (no Extended Tag, Relaxed Ordering, No Snoop setup as OpenRM does it) | Copies or the picture look wrong | `drivers/gpu/nvidia/chipset/mod.rs` |
| `nobar1` | BAR1 is not switched to physical mode at boot; programs' video memory has no CPU address and the console does not write video memory directly | BAR1 problems | `drivers/gpu/nvidia/blackwell/bringup/mod.rs` |
| `gpu4k` | Every GPU mapping in 4 KiB pages, no 64 KiB or 2 MiB pages | GPU page-table problems | `drivers/gpu/nvidia/gmmu.rs` |
| `nogpuvm` | GPU page tables are never unlinked or freed (implies `gpu4k`) | The picture disappears after an unmap | `drivers/gpu/nvidia/gmmu.rs` |
| `tlball` | Every GPU TLB invalidate also targets all page directories | Stale GPU translations | `drivers/gpu/nvidia/gmmu.rs` |
| `nochangrp` | Channels are built bare, without a timeslice group | A boot that fails at channel creation | `drivers/gpu/nvidia/gsp_common/chan.rs` |
| `ctxuvm` | Compute context buffers described the UVM way, by asking GSP-RM; GSP-RM refuses it (`NV_ERR_INVALID_OBJECT`, on the card 2026-09-14). Kept to re-run the reasoning | Never, normally | `drivers/gpu/nvidia/gsp_common/compute.rs` |
| `gpuown` | A process's GPU programs get a private GPU address space and context instead of failing with "a private GPU address space is off by default"; never run on the card | Testing that path | `drivers/gpu/nvidia/blackwell/compute_chan.rs` |
| `coldhead` | The head the firmware lit is programmed from cold at the driver's mode (when that mode fits the trained link) | Testing the cold modeset | `drivers/gpu/nvidia/blackwell/modeset.rs` |
| `edidbase` | Only the EDID's base block is parsed | A fast mode misbehaves | `drivers/gpu/nvidia/blackwell/modeset.rs` |
| `vrr` | Asks for variable refresh on the primary output once the picture is up (as `display vrr on` does) | Trying VRR | `drivers/gpu/nvidia/mod.rs` |
| `nohwcursor` | The compositor draws the pointer; the head's cursor plane is not used | A head with no plane; a cursor-plane hang | `drivers/gpu/nvidia/blackwell/window.rs` |
| `noflipsema` | The flip is not held back by the acquire semaphore; the processor waits for the copies | A window that stops arming | `mayland/render/mod.rs` |
| `nobackdrop` | No backdrop buffer: the blur pass reads the screen it writes | Wrong picture under a window | `drivers/gpu/nvidia/blur.rs` |
| `notex` | No video-memory copies of windows; every window is read from its client's memory every frame | A copy shows an older picture | `mayland/render/texture.rs` |

### Not kernel flags

| Word | What it is |
|---|---|
| `i8042=off` | A QEMU machine option (`-machine q35,i8042=off`) that removes the emulated keyboard controller, so a test of the USB keyboard cannot be answered by PS/2 ([building.md](building.md#running-in-qemu)) |
| `MIKU_FW=` | A build-time environment variable for the builder ([building.md](building.md#staging-the-root-tree)) |

## The serial log

- COM1 at 115200 baud, about 87 µs a character. Output only: the kernel never reads the port, so nothing can be typed over it.
- A line is written with interrupts off on that CPU, so each line costs its length in time; a 40-character line is about 3.5 ms. Detail lines are behind `trace` for this reason.
- Every line that goes to serial also goes into the kernel log (klog) with a level and the source location of the print. `kerror!`, `kwarn!`, `knotice!`, `kinfo!` print and record; `kdebug!` only records, unless `trace`. `dmesg` reads the log in a running system ([diagnostics.md](diagnostics.md)).
- Lines start with a bracketed subsystem tag. The common ones:

| Tag | Source |
|---|---|
| `[early]` | earlycon, before the console |
| `[kern]`, `[cpu]`, `[sse]` | `kernel_main` itself |
| `[boot]` | Boot step results and the closing `[boot] up in` line |
| `[acpi]`, `[apic]`, `[smp]`, `[pmm]`, `[heap]`, `[vmap]`, `[tlb]`, `[kstack]` | Architecture and memory |
| `[pci]` | The bus scan |
| `[vfs]`, `[mount]`, `[miku_extfs]`, `[btrfs]`, `[gpt]`, `[fwload]`, `[firmware]` | Files and mounts |
| `[block]`, `[ata]`, `[ahci]`, `[nvme]`, `[bdflush]`, `[writeback]`, `[swap]`, `[swap_map]` | Storage |
| `[net]`, `[netd]`, `[dhcp]` | Network |
| `[ps2]`, `[usb]`, `[usb-handoff]` | Input |
| `[nvidia]`, `[gsp]`, `[gsprm]`, `[rm]`, `[gb206]`, `[turing]`, `[sec2]`, `[fwsec]`, `[nvdec]` | The NVIDIA driver |
| `[mayland]`, `[session]` | The display server and sessions |
| `[mikud]`, `[shell]`, `[setup]` | Init, the kernel shell, the installer |
| `[solib]`, `[elf]`, `[exec]`, `[vdso]`, `[sym]`, `[signal]`, `[crash]` | Programs |
| `[sched]`, `[timing]`, `[lockstat]` | Scheduling and lock timing |
| `[pstore]`, `[power]` | Persistent store, power-off and reboot |
| `[panic]` | A kernel panic |

A ring-3 program's own log lines (aosh's `aosh:` lines, for example) go into klog as user records and are read with `dmesg`.

## Reading a boot that hangs or panics

**Where it stopped.**

1. Nothing on the screen at all: GRUB never handed over, or the machine booted UEFI ([building.md](building.md#running-in-qemu)).
2. Only `[early]` lines on the screen: the topmost is the last phase that finished, before the console existed.
3. Console lines: the last `[  ok  ]` or `[ fail ]` line names the last step. On serial, the last `[boot]` line does the same, and the lines after it say what the next step was doing.
4. `[boot] up in` printed but no desktop: look at the `[mikud]` lines (`booting to ...`, services failing and restarting) and at `greeter` ([system.md](system.md)).

**Panics.** `[panic] <message>` on serial and `kernel panic: <message>` in red on the screen; all CPUs are halted. The panic handler takes the screen back from the compositor before printing, so a panic is visible even with the desktop up. A fault in the kernel prints `[crash] kernel <cause> at <rip>` with the faulting address and a backtrace named from the kernel's symbol table ([diagnostics.md](diagnostics.md)). A stack overflow is a panic: `kernel stack overflow during '<step>'` on the boot stack, or `kernel stack overflow in pid N '<name>'` on a thread's stack.

**Escape hatches, roughly in order.** For a boot that dies at a GPU step: `nogpu`, then `nointr`. After a change near user memory: `nosmap`, `nosmep`. In USB bring-up: `nousb`. After a kernel-core change: the flag for that mechanism above (`tickonly`, `nopoll`, `nvmepoll`, `nopagecache`, `writethrough`, `walklocked`, `vfsmutex`, `netpoll`, `tlbalways`, `slabdebug`). For a stack overflow: `bigstacks`. To boot past the login: `noauth`.

**When the screen goes dark.** On the real machine there is usually no serial cable. Boot with `klogpstore` (or start a `run=` list with `pstore_mirror`), reproduce, then warm-reset with the reset button, not the power button (the kernel does not handle Ctrl-Alt-Del; from a shell, `mikudo reboot` is warm) and read the previous boot's log on the next boot with `mikudo pstore err`, `mikudo pstore -g <text>` or `mikudo pstore -n <lines>` ([diagnostics.md](diagnostics.md)).

**A hang with no message.** `schedwatch` names tasks that sat runnable on no CPU; `lockstat` names long lock holds. In QEMU, the monitor's `info registers` and `screendump` show where each CPU is and what the screen shows.

## Boot time

`[boot] up in N ms (M ms of it inside init steps)` is the kernel's own total, from the TSC calibration to `mark_done`; steps of 5 ms or more print their own times.

- **QEMU/KVM:** about 115 ms (113-136 ms when measured on 2026-08-21, down from 592 ms). The savings were one PCI enumeration instead of five sweeps, two 10 ms LAPIC calibration windows instead of one 50 ms window, and one INIT wait for all APs. Serial costs nothing under QEMU, so this understates real hardware.
- **The RTX 5060 Ti machine:** about 5 s on 2026-10-01, most of it bringing up GSP-RM and the display. Before the console shadow (2026-09-29), about 50 s more went to the console scrolling by reading the framebuffer through the GPU's aperture.
- **Before the kernel:** the largest cost is GRUB reading `boot/firmware.img` whole into RAM. Building with the `MIKU_FW=` value the kernel prints for the machine's card leaves other cards' firmware out ([building.md](building.md#real-hardware-and-usb-sticks)).

## Code map

| Path | Contents |
|---|---|
| `grub.cfg` | The menu |
| `linker.ld` | Load and link addresses, the boot stack |
| `kernel/arch/x86_64/boot_entry.rs` | Multiboot2 header, `_start32`, `_start` |
| `kernel/arch/x86_64/grub.rs` | Multiboot2 tags, the boot line, modules, the memory map, the framebuffer |
| `kernel/arch/x86_64/earlycon.rs` | Progress before the console |
| `kernel/main.rs` | `kernel_main`, the service table, the `run=` task, the panic handler |
| `kernel/kcore/boot_state.rs` | `boot_step!`, `mark_done` |
| `kernel/kcore/firmware/load.rs`, `kernel/fs/ext/mount.rs` | The boot image, `root=`, the grafts |
| `kernel/mikud/` | Service management ([system.md](system.md)) |
| `kernel/arch/x86_64/serial.rs`, `kernel/kcore/klog/` | Serial output and the kernel log |
