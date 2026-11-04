# Storage

This page covers storage from the device up to the mount: drivers (NVMe, AHCI, ATA, virtio-blk, ramdisk), the block layer and its buffer cache, GPT partitions, formatting, checking, swap devices, and mounting. Every storage operation is a command of the in-kernel console. From a ring-3 shell they run as `mikudo <command>`, because there is no `mount(2)` and no ring-3 `mkfs`. The rule above everything here is **disk safety**: the kernel never chooses a disk on its own, every mount starts read-only, and writing needs an explicit command. The filesystems themselves are in [filesystems.md](filesystems.md), the VFS page cache in [vfs.md](vfs.md), and swap internals in [memory.md](memory.md).

## Contents

- [The stack](#the-stack)
- [Disk safety](#disk-safety)
- [Devices and naming](#devices-and-naming)
- [Drivers](#drivers)
- [The block layer](#the-block-layer)
- [The buffer cache and bdflush](#the-buffer-cache-and-bdflush)
- [Device health and SMART](#device-health-and-smart)
- [GPT partitions](#gpt-partitions)
- [Formatting: mkfs](#formatting-mkfs)
- [Checking: fsck](#checking-fsck)
- [Mounting](#mounting)
- [Swap devices](#swap-devices)
- [Command reference](#command-reference)
- [Boot flags](#boot-flags)
- [Limits](#limits)
- [Known issues](#known-issues)
- [Troubleshooting](#troubleshooting)

---

## The stack

```
  VFS (vfs.md) -> page cache -> filesystem (filesystems.md)
       -> fshost::BlockHost (kernel/fs/host.rs: KernelHost)
       -> kernel/io/block: registry, buffer cache, dispatch, accounting, failfast
       -> trait BlockDriver
       -> kernel/drivers/block: nvme, ahci, ata, virtio_blk, ramdisk
```

| File | What |
|:--|:--|
| [`kernel/io/block/mod.rs`](../../kernel/io/block/mod.rs) | Device registry, `read`/`write`/`write_sync`/`write_barrier`/`flush`/`discard`/`write_zeroes`, dispatch with retries and failfast, partitions, `bdflush` |
| [`kernel/io/block/cache.rs`](../../kernel/io/block/cache.rs) | The buffer cache |
| [`kernel/io/block/driver.rs`](../../kernel/io/block/driver.rs) | `BlockDriver`, `BlockDevInfo`, `HealthInfo`, `BlkError` |
| [`kernel/drivers/block/`](../../kernel/drivers/block/) | The drivers |
| [`kernel/fs/gpt.rs`](../../kernel/fs/gpt.rs) | GPT read, create, add, delete |
| [`kernel/fs/host.rs`](../../kernel/fs/host.rs) | `KernelHost`: `BlockHost` over the block layer |
| [`kernel/mm/swap.rs`](../../kernel/mm/swap.rs) | Swap device format |
| [`kernel/fs/ext/mount.rs`](../../kernel/fs/ext/mount.rs), [`kernel/fs/btrfs/mount.rs`](../../kernel/fs/btrfs/mount.rs) | Mount slots, probing, root mount |
| [`kernel/shell/commands/`](../../kernel/shell/commands/) `disk_cmds.rs`, `mkfs_cmds.rs`, `ext2_cmds.rs`, `fs.rs`, `ext*_cmds.rs` | The commands |

Design notes: [`dev/fs-architecture.md`](../../dev/fs-architecture.md) ("The layers", "NVMe and AHCI on interrupts").

---

## Disk safety

The ISO is booted on real machines that carry other operating systems' filesystems. So:

- **No automatic disk selection.** A physical disk is mounted only when named: `root=` on the boot line, or an explicit `ext4mount`/`mount`. The firmware GRUB module (RAM) is the one thing mounted without being named, read-only.
- **Every mount comes up read-only.** A read-only ext mount does not replay its journal, clean orphans, stamp the superblock's mount count, or write access times: the disk is byte-for-byte untouched. Writing needs `fs.rw [slot]`, `ext4mount <drive> [part] rw`, or `rootrw` next to `root=` on the boot line.
- **Raw-device commands bypass the read-only flag.** `mkfs.*`, `gpt.init`/`gpt.add`/`gpt.del`, `mkswap`, `swapon` (it writes pages to the partition), `blkzero`, `blkdiscard`, `dd of=`, and writes to `/dev/blkN` act on the device directly. **None of them asks for confirmation**; `mkfs` prints a warning and formats.
- All of these are admin-only (`admin_only` in the console; `mikudo` from ring 3), and `/dev/blk*` nodes are root's, mode 0600.

---

## Devices and naming

Up to 8 block devices (`MAX_BLOCK_DEVICES`), each with a stable id:

| Id | Device |
|:--|:--|
| 0 | Legacy ATA primary master |
| 1 | Legacy ATA primary slave |
| 2 | Legacy ATA secondary master |
| 3 | Legacy ATA secondary slave |
| 4-7 | Probed PCI devices, in probe order: AHCI ports (up to 4), then virtio-blk devices (up to 4), then one NVMe controller. Ramdisks (the firmware GRUB module, selftest scratch disks) take ids from this range too |

The legacy ATA slots are registered on first use (`register_ata`); the PCI devices by `io::block::probe()` at boot (once). Commands take this id as the **drive index** (some usage messages still say `0-3`; most accept `0-7`).

**Device nodes** in `/dev` ([filesystems.md](filesystems.md)): `/dev/blkN` is the whole device, `/dev/blkNpM` partition M (1-15) of it; major 8, minor `N * 16 + M`, root only. They are created from the GPT when devices are registered and rebuilt by `partprobe`.

---

## Drivers

| Driver | Transport | Queues | Completion | Max per command | Discard | Write-zeroes | FUA | Health | 32-bit DMA |
|:--|:--|:--|:--|:--|:--|:--|:--|:--|:--|
| NVMe (`nvme.rs`) | PCI, BAR0 | 1 admin + 4 I/O queue pairs, one command outstanding per queue | MSI-X per I/O queue, delivered to the boot CPU | 64 KiB (bounce buffer, PRP1 + PRP list) | DSM deallocate | Write Zeroes | yes | health log page | no (64-bit) |
| AHCI (`ahci.rs`) | PCI, BAR5 MMIO | one command slot per port | one MSI for the HBA | 64 KiB (one PRD) | ATA TRIM | fallback | `WRITE DMA FUA EXT` | none | below 4 GiB |
| ATA (`ata.rs`) | legacy I/O ports | one per channel, the two channels' devices share a bus lock | polled | | TRIM (with DMA) | fallback | fallback | ATA SMART | below 4 GiB |
| virtio-blk (`virtio_blk.rs`) | legacy/transitional virtio-pci, port I/O | one request at a time | polled | 64 KiB (bounce buffer) | discard | `WRITE_ZEROES` | fallback | none | no |
| ramdisk (`ramdisk.rs`) | RAM through the direct map | | immediate | | | | | | |

- **ATA** does PIO (LBA28 and LBA48 commands) and bus-master DMA when the controller supports it (`dma_init` at probe; cached per drive).
- **NVMe I/O queues** are chosen by CPU: CPU *n* uses queue *n* when there is one, otherwise round robin. Each queue's waiters sleep on its own wait queue; the interrupt handler only wakes them and the waiter reads its completion.
- **NVMe and AHCI waits**: spin for 50 us (`SPIN_BEFORE_SLEEP_NS`), then sleep. A fast SSD completes inside the spin and pays no context switch. Still polled: the NVMe admin queue, early boot, interrupts off, the idle task, a task already marked dead, and everything with `nvmepoll`. MSI-X is enabled before queues are created (a controller refuses a queue with a vector otherwise).
- **"Fallback"** for write-zeroes means zero-filled buffer writes (from one static buffer of zeros); for FUA it means write plus flush.
- **ramdisk** writes land in RAM and are lost at reboot; the GRUB firmware module is mounted read-only anyway.
- **Device memory** for the controllers is `DmaBox`/`DmaBuffer` ([memory.md](memory.md)); AHCI and ATA ask for memory below 4 GiB.

Verified (2026-10-27, QEMU): a megabyte of random bytes written through NVMe, AHCI, IDE and virtio-blk, read back and written again further on, both copies compared on the host. `nvmestress` (4 kernel threads x 120 writes, each verified) passes on interrupts.

---

## The block layer

[`kernel/io/block/mod.rs`](../../kernel/io/block/mod.rs).

### The trait

`BlockDriver: Send + Sync`, units of 512-byte sectors, `lba: u64`:

| Method | |
|:--|:--|
| `read_blocks`, `write_blocks`, `flush`, `info` | Required |
| `write_blocks_fua` | Default: write then flush; NVMe and AHCI set the FUA bit |
| `discard` | Default `Unsupported` |
| `write_zeroes` | Default `Unsupported` (the layer falls back) |
| `health` | Default `None` |

Methods take `&self`: the layer dispatches with **no device-wide lock**, so several CPUs can have I/O in flight on one NVMe at once (Linux's blk-mq shape). Single-queue backends serialise on their own lock; legacy ATA devices also take a shared bus lock because the channels share I/O ports.

### The request path

| Call | Semantics |
|:--|:--|
| `read(dev, lba, count, buf)` | Served from the buffer cache where possible; misses are fetched in whole 4 KiB chunks and cached, with readahead (below). Reads of the device's tail, or before the cache exists, go straight to the driver |
| `write(dev, lba, count, buf)` | **Write-back**: whole chunks go into the cache dirty; a partial chunk is merged into the resident copy, or read-modify-written. Returns when the data is in the cache. When more than 256 chunks (1 MiB) are dirty, the writer drains its device before returning |
| `write_sync` | Write-through, ordered: the device write completes before return; resident chunks are updated in place (or dropped if the write failed). Used for journal records, GPT tables, swap headers, and ext's new data blocks |
| `write_barrier` | `write_sync` plus FUA (or flush): on stable media when it returns. The journal's commit record |
| `flush(dev)` | Drain the device's dirty chunks in LBA order, then flush the device's own cache |
| `discard(dev, lba, count)` | TRIM; cache chunks wholly inside the range are dropped first (dirty ones included, their data is dead), partial edges stay |
| `write_zeroes(dev, lba, count)` | The chunk-aligned middle uses the device command and drops its chunks; unaligned edges go through `write`; devices without the command get zero buffers |

**Dispatch** (`dispatch`): every command is pushed onto an accounting queue (`BioQueue`), run through the driver, retried up to twice on `Timeout` or `DeviceFault` (sector I/O is idempotent), and counted (sectors read/written/discarded, latency, retries, errors).

**Failfast states**: `Online`; `Degraded` after an error (back to `Online` on the next success); `Offline` after 8 consecutive failures (`OFFLINE_THRESHOLD`), when every request fails at once with `Offline` instead of waiting out hardware timeouts. `blkonline <drive>` brings a device back.

**Merging**: dirty chunks are written in ascending LBA order (one elevator sweep), adjacent chunks coalesced into one command of up to 64 KiB (`flush_dirty_only`, `MERGE_CHUNKS = 16`). There is no plugging and no I/O scheduler beyond that sweep; each command is waited for.

**I/O under the VFS lock** is counted (`IO_UNDER_VFS`): device reads and writes made by a task holding `VFS_LOCK` to write, shown at the end of `/proc/lockstat` ([vfs.md](vfs.md)).

---

## The buffer cache and bdflush

[`kernel/io/block/cache.rs`](../../kernel/io/block/cache.rs):

- 4 KiB chunks (8 sectors) keyed by (device, chunk index), 8-way set-associative with per-set LRU: a lookup touches at most 8 entries.
- **Size**: a 128th of RAM, between 2 and 16 MiB. File data are cached a layer up (the VFS page cache), so what is left here is metadata (inode tables, directories, extent and journal blocks) and short in-place overwrites.
- **Readahead**: a sequential miss fetches up to 8 chunks (32 KiB), ramping to 16 (64 KiB) for sustained streams.
- Counters (`cache_stats`): hits, misses, readaheads, write merges, dirty chunks.

**bdflush** (`writeback_thread`, a mikuD service) wakes every 2 s (500 ticks) and, in order:

1. writes out VFS page-cache pages dirty for 5 s or more (`writeback_old`, [vfs.md](vfs.md));
2. runs each ext mount's periodic commit (`ext::mount::periodic_sync`: a journal commit and the dirty metadata, [filesystems.md](filesystems.md));
3. sweeps every device's dirty chunks out (`flush_dirty_only`) and logs `[bdflush] wrote back N chunks`.

So data written through write-back reaches the disk within roughly 5-7 s without a `sync`. `sync` (or `extsync`), `fsync`, unmount, `poweroff` and `reboot` force it.

**Relation to ext's data path**: since 2026-10-28 ext writes new and whole file blocks with `write_sync` past this cache (updating any resident copy), and only short in-place overwrites through `write` (see [filesystems.md](filesystems.md#the-data-path-and-crash-consistency)).

`/proc/diskstats` has per-device counters; `blkstat` prints the same with device states. Formats in [diagnostics.md](diagnostics.md).

---

## Device health and SMART

`smart <drive>` prints the device's `HealthInfo`: overall verdict, temperature (C), wear used (NVMe percentage used), power-on hours, lifetime data read and written (MB). Sources: the NVMe SMART/health log page, ATA SMART on legacy ATA drives. AHCI, virtio-blk and ramdisk report none.

---

## GPT partitions

[`kernel/fs/gpt.rs`](../../kernel/fs/gpt.rs). GPT only (no MBR partition tables; a protective MBR is written).

- **Reading** checks the signature and header (`NotGpt`, `InvalidFormat`), reads up to 128 entries, and registers each used partition as `/dev/blkNpM` (only the first 15 get nodes). `partprobe [drive]` rereads and removes stale partitions.
- **Types** recognised: Linux filesystem, Linux swap, BIOS boot (the "Hah!IdontNeedEFI" GUID, used by the installer for GRUB's core image).
- **Creating** (`gpt.init <drive>`): protective MBR, primary header and entry array at LBA 1-33, backup at the end; first usable LBA 34, last usable `total - 34`. Refused for disks under 2048 sectors. **Overwrites LBA 0-33 and the tail without asking.**
- **Adding** (`gpt.add <drive> <fs|linux|swap> <size_mb> [name]`): the new partition starts at the first LBA after the last used one (not aligned to 1 MiB) in the first free slot; errors for no space, no free slot (128) or no GPT.
- **Deleting** (`gpt.del <drive> <partition>`): partitions are numbered from 1.
- **Showing** (`gpt <drive>`): disk size, first/last usable LBA, and per partition its number, type, LBA range, size in MB and name.

Example, a data partition and a swap partition on drive 4:

```
gpt.init 4
gpt.add 4 fs 2048 data
gpt.add 4 swap 512 swap
mkfs.ext4 4 1
mkswap 4 2
```

The installer partitions the target disk itself; see [system.md](system.md).

---

## Formatting: mkfs

```
mkfs.ext2 <drive> [partition|sectors]
mkfs.ext3 <drive> [partition|sectors]
mkfs.ext4 <drive> [partition|sectors]
mkfs.dry  <drive> <ext2|ext3|ext4>
```

- The second argument is a **partition number** (1-128, read from the GPT; a swap-type partition is refused) or a **sector count** (above 128) for the start of the disk; omitted, the whole disk is formatted.
- Mounted slots on that drive and LBA are invalidated first.
- **No confirmation**: the command prints `!! warning: ... will be erased` and formats.
- Fixed parameters (`MkfsParams::new`): ext2 1 KiB blocks and 128-byte inodes; ext3 the same plus a 1024-block journal; ext4 4 KiB blocks, 256-byte inodes, a 1024-block journal, extents, `metadata_csum`. The label is `miku`. There are no options for block size, inode size or label. Features per type are in [filesystems.md](filesystems.md#mkfs).
- The report lists block size, inode size, blocks, inodes, groups, journal size, free blocks and inodes, and how to mount it.
- `mkfs.dry` prints the layout for a drive without writing.
- **Check the group count.** Until 2026-08-11 mkfs capped every filesystem at 32 groups (256 MB at 1 KiB blocks) whatever the disk; the only symptom was the `[mkfs] ... groups=` line. Now the group table is sized to the partition (up to 2^20 groups; a 4 GB target is 512 groups).
- Sector counts are 32-bit, so mkfs addresses at most 2^32 sectors (2 TiB).
- `ext3mkjournal` adds a journal to a mounted ext2 filesystem; `ext4extents` enables extents on the active one.

---

## Checking: fsck

```
extfsck            check the active ext slot (also ext2fsck, ext4fsck)
extfsck -y         check and repair (--repair)
```

Five passes as e2fsck's, on the mounted filesystem; repair needs the volume read-write with its journal replayed. Details in [filesystems.md](filesystems.md#fsck). Other ext inspection commands: `extinfo`, `ext4checksums` (feature report), `ext3journal`, `ext3recover` (replay now), `ext3clean`, `extcache`, `ext4extinfo <path>`, `fiemap <path>`, `ext4tree`, `ext4du`.

---

## Mounting

### Mount slots

| Filesystem | Slots | Lock |
|:--|:--|:--|
| ext2/3/4 | 4 (`fs.list` shows them; one is "active") | `ext` |
| btrfs | 2 | `btrfs` |
| memfs | 2 (selftest only) | `memfs` |

A mount is two steps: **open a slot** (probe the device, check the superblock, read-only), then **graft** it onto a VFS directory ([vfs.md](vfs.md#mounting-is-a-graft)).

### Commands

| Command | Effect |
|:--|:--|
| `ext4mount` (also `ext2mount`, `ext3mount`) | With no arguments: scan every drive and GPT partition; mount the single ext filesystem found, or list the candidates if several |
| `ext4mount <drive> [partition] [rw]` | Mount that drive (whole disk) or GPT partition; read-only unless `rw` |
| `mount` | List mounts |
| `mount <ext2|ext3|ext4> [path]` | Graft the **active** ext slot at `path` (default `/mnt`; created if missing; existing cached children evicted) |
| `mount btrfs [path]` | Find the first btrfs on any device or partition, open it read-only, graft it (default `/mnt`) |
| `umount <path>` | btrfs: ungraft and close the slot. ext: write back dirty pages and metadata, then ungraft and close the slot behind that path |
| `fs.list` | The ext slots: flavour, `ro`/`rw`, drive, LBA, free/total blocks, the active marker |
| `fs.select <slot>` | Make a slot active (what `mount ext4`, `extfsck` and the `ext*` builtins act on) |
| `fs.rw [slot]` | Remount read-write: journal replay, orphan cleanup and superblock stamping happen now |
| `fs.ro [slot]` | Flush and drop back to read-only, marking the filesystem cleanly unmounted |
| `fs.umount [slot]` | Close a slot (flushing it first if writable) |
| `sync` (or `extsync`) | Write everything out |

Example: inspect a Linux disk on drive 4, partition 2, without touching it, then copy a file off it.

```
ext4mount 4 2
mount ext4 /mnt
ls /mnt/home
cp /mnt/home/me/notes.txt /tmp/
umount /mnt
```

To write: `ext4mount 4 2 rw`, or `fs.rw` after mounting.

From a ring-3 shell prefix each with `mikudo`, for example `mikudo ext4mount 4 2`. `mikudo` passes the line to the kernel console (`kcmd`, syscall 104), which refuses only `login` and `logout`; see [system.md](system.md) and [commands.md](commands.md).

### At boot

- The firmware GRUB module (`firmware`) is registered as a ramdisk, mounted read-only, and grafted at `/lib/firmware`.
- `root=<drive>[:<lba>]` or `root=LABEL=<label>` (1-16 characters) on the boot line names a persistent root; its top-level directories (`/bin`, `/etc`, ...) are grafted onto the tmpfs `/` (`graft_root_dir`). Nothing is mounted unless `root=` names it. `rootrw` makes it writable. A refused root is explained on the console once it exists (`take_root_trouble`). See [boot.md](boot.md).

---

## Swap devices

Swap is a partition (or a raw range) with Linux's version-1 swap header; only one is active, and at most 65,536 pages (256 MiB) of it are used. How pages go out and come back, and the limits of the reverse map, are in [memory.md](memory.md#swap).

| Command | Effect |
|:--|:--|
| `mkswap <drive> <partition>` | Write a swap header (label `miku-swap`) on a GPT partition of swap type; other types are refused |
| `swapon <drive> <partition>` | Activate it: check the `SWAPSPACE2` magic and version, fill the emergency frame pool |
| `swapoff` | Deactivate; refused while any page is swapped out |
| `swapinfo` | Total, used, free, percentage (warns above 80 %) |
| `mkswap.raw <drive> <start_lba> <size_mb>` | Swap header on a raw range |
| `swapon.raw <drive> <start_lba> <size_sectors>` | Activate a raw range |
| `swapon.auto` | Scan drives 0-3 for a swap-type GPT partition (or a whole disk) carrying a valid header, and activate the first found |

Example:

```
gpt.add 4 swap 512 swap
mkswap 4 2
swapon 4 2
swapinfo
```

There are no swap files. `info` shows swap usage; `/proc/meminfo`'s `SwapTotal`/`SwapFree` stay 0.

---

## Command reference

All are console commands, admin-only, and reachable from ring 3 through `mikudo`.

| Command | Syntax | What |
|:--|:--|:--|
| `blkstat` | `blkstat` | Every registered device: kind, model, size, LBA48, read-only and discard flags; KB read, written and discarded, average latency; failed requests and retries; a warning when `Degraded` or `Offline`; the bio queue counters |
| `smart` | `smart <drive>` | Health report |
| `partprobe` | `partprobe [drive]` | Reread GPTs, rebuild `/dev/blkNpM` |
| `blkonline` | `blkonline <drive>` | Put an `Offline` device back online |
| `blkdiscard` | `blkdiscard <drive> [lba count]` | TRIM a range (whole device without a range) |
| `blkzero` | `blkzero <drive> <lba> <count>` | Zero a range |
| `fstrim` | `fstrim` | Discard the free blocks of the active ext filesystem |
| `nvmestress` | `nvmestress` | 4 threads x 120 verified NVMe writes |
| `dd` | `dd if=<src> of=<dst> [bs=N] [count=N] [skip=N] [seek=N] [conv=notrunc,fsync]` | Copy between files and devices |
| `gpt`, `gpt.init`, `gpt.add`, `gpt.del` | above | Partition tables |
| `mkfs.ext2`/`3`/`4`, `mkfs.dry` | above | Formatting |
| `extfsck` | `extfsck [-y]` | Check and repair |
| `ext4mount`, `mount`, `umount`, `fs.*`, `sync` | above | Mounting |
| `mkswap`, `swapon`, `swapoff`, `swapinfo`, `*.raw`, `swapon.auto` | above | Swap |
| `ext*` builtins | `extls`, `extcat`, `extstat`, `extwrite`, `extappend`, `exttouch`, `extmkdir`, `extrm [-rf]`, `extrmdir`, `extmv`, `extcp`, `extln -s`, `extlink`, `extchmod`, `extchown`, `extdu`, `exttree` | Work on the active slot **by inode number, past the VFS**; afterwards the VFS drops its caches (`changed_below_vfs`). Prefer the ordinary ring-3 tools on a grafted mount |
| xattrs, flags | `getxattr <path> <name>`, `setxattr <path> <name> <value>`, `listxattr <path>`, `chattr <+/-flags> <path>` (i, a, d, A), `lsattr <path>` | ext extended attributes and inode flags |

The full command list is in [commands.md](commands.md).

---

## Boot flags

| Flag | Effect |
|:--|:--|
| `root=<drive>[:<lba>]`, `root=LABEL=<name>` | The persistent root to mount (read-only) |
| `rootrw` | Make that root writable |
| `nvmepoll` | NVMe I/O queues without MSI-X and AHCI without MSI: every command polled |
| `nopagecache`, `writethrough` | VFS page cache off; write-back off ([vfs.md](vfs.md)) |
| `lockstat` | Lock timing, including the `ext` lock and I/O under the VFS lock |

The full table is in [boot.md](boot.md).

---

## Limits

- 8 block devices: 4 legacy ATA slots and 4 dynamic ids shared by AHCI ports, virtio-blk devices, the NVMe controller and ramdisks.
- One NVMe controller, one namespace, 4 I/O queues, one command outstanding per queue.
- 64 KiB per command on NVMe, AHCI and virtio-blk; each command is waited for. No queue depth beyond one per queue.
- GPT only, 128 entries, 15 partitions with device nodes per disk; no 1 MiB alignment on `gpt.add`.
- 4 ext slots, 2 btrfs slots; mkfs up to 2^32 sectors.
- One swap device, 256 MiB used at most.
- No hot-plug, no RAID or LVM, no USB mass storage, no MBR partitions.

---

## Known issues

- Usage messages lag the code: the mkfs error "too many block groups (max 32)" predates the 2026-08-11 fix; several commands say `drive 0-3` and accept 0-7; the multiple-candidate hint of `ext4mount` says `<drive> <lba>` where the second argument is a partition number.
- The header comment of `cache.rs` still calls the buffer cache write-through; `write` is write-back (`mod.rs`).

---

## Troubleshooting

| Symptom | Look at |
|:--|:--|
| A mount is refused | The mount log names the `MountReject` reason (unsupported feature, bad geometry, checksum); unsupported ro-compat features mount read-only instead ([filesystems.md](filesystems.md)) |
| Writes fail with `EROFS` | The slot is read-only: `fs.list`, then `fs.rw` |
| A filesystem is smaller than its partition | The `[mkfs] ... groups=` line |
| I/O hangs or is slow on real hardware | `nvmepoll` to rule out interrupt delivery; `blkstat` for errors, retries and state |
| A device reports `Offline` | 8 consecutive failures; check `smart`, then `blkonline` |
| `[block] dev N ... retry 1/2` lines | Transient timeouts or device faults |
| Data missing after a power cut | Write-back holds data for up to about 7 s; `sync`/`fsync` first, or boot with `writethrough` |
| A new partition has no `/dev/blkNpM` | `partprobe` |

Related: [filesystems.md](filesystems.md), [vfs.md](vfs.md), [memory.md](memory.md), [boot.md](boot.md), [system.md](system.md), [commands.md](commands.md).
