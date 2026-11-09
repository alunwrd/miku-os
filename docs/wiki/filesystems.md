# Filesystems

MikuOS has two on-disk filesystem drivers and four in-memory ones. **ext2/ext3/ext4** (`lib/fs/extfs`) is read-write with a JBD2 journal. **btrfs** (`lib/fs/btrfs`) is mounted read-only. **tmpfs** is the root and holds `/tmp`. **devfs** and **procfs** are kernel-generated trees. **memfs** is a heap-backed filesystem used as a second test subject. The on-disk drivers are `no_std` crates that know nothing about the kernel. They reach the disk through one trait (`fshost::BlockHost`) and the VFS through another (`FileSystem`), so a volume can be built, corrupted and crashed in memory with `cargo test`. This page covers what each filesystem supports and refuses, how ext's journal and data path keep a crash consistent, how each one implements the VFS trait and locks itself, and how they are tested.

## Contents

- [Feature matrix](#feature-matrix)
- [Code map](#code-map)
- [The host seam: fshost](#the-host-seam-fshost)
- [ext2, ext3, ext4](#ext2-ext3-ext4)
  - [Features supported and refused](#features-supported-and-refused)
  - [On-disk structures handled](#on-disk-structures-handled)
  - [The jbd2 journal](#the-jbd2-journal)
  - [The data path and crash consistency](#the-data-path-and-crash-consistency)
  - [The metadata cache](#the-metadata-cache)
  - [Extended attributes](#extended-attributes)
  - [fsck](#fsck)
  - [mkfs](#mkfs)
  - [How ext implements FileSystem](#how-ext-implements-filesystem)
  - [Rules that are easy to break](#rules-that-are-easy-to-break)
  - [Tests](#ext-tests)
- [btrfs](#btrfs)
- [tmpfs](#tmpfs)
- [memfs](#memfs)
- [devfs](#devfs)
- [procfs](#procfs)
- [Filesystems that do not exist](#filesystems-that-do-not-exist)

---

## Feature matrix

| | ext2 | ext3 | ext4 | btrfs | tmpfs | memfs | devfs | procfs |
|:--|:--|:--|:--|:--|:--|:--|:--|:--|
| Medium | disk | disk | disk | disk | RAM (frames) | heap | none | none |
| Read | yes | yes | yes | yes | yes | yes | per device | yes |
| Write | yes | yes | yes | no (`EROFS`) | yes | yes | per device | no |
| Journal | no | JBD2 (metadata) | JBD2 (metadata) | CoW (not written here) | n/a | n/a | n/a | n/a |
| Survives reboot | yes | yes | yes | yes | no | no | n/a | n/a |
| Symlinks | yes | yes | yes | read | yes | yes | no | no |
| Hard links | yes | yes | yes | read | yes | yes | no | no |
| Device nodes, FIFOs, sockets | create (`mknod` of the four special types) | same | same | read | no | no | the devices themselves | no |
| xattr | `user.`/`trusted.`/`security.`/`system.` | same | same | no | no | no | no | no |
| Page cache | disk page cache | same | same | disk page cache (read) | its own pages | none (it is memory) | none | none |
| `mmap` `MAP_SHARED` | copy per mapping, written back on `msync`/`munmap` | same | same | read-only copy | the file's own frame (coherent) | copy | no | no |
| inotify | yes (VFS) | yes | yes | n/a (no changes) | yes | yes | no | no |
| `fallocate` / punch hole | both (`fallocate` allocates real blocks) | same | both (`fallocate` on an extent file reserves unwritten extents) | no | no | no | no | no |
| TRIM (`fstrim`) | yes | yes | yes | no | n/a | n/a | n/a | n/a |
| Vnodes evictable | yes | yes | yes | yes | no | yes | no | no |
| Largest file | as the format allows | same | same | as the format allows | 4 GiB | 1 MiB total | n/a | 8 KiB per file |

"VFS" features (inotify, record locks, permissions, the page cache) are provided by the VFS for every filesystem behind it; see [vfs.md](vfs.md).

---

## Code map

| Path | What |
|:--|:--|
| [`lib/fs/fshost/`](../../lib/fs/fshost/) | `BlockHost` (the disk seam), `MemDisk` (an in-memory disk with a volatile write cache and power cuts), crc32c |
| [`lib/fs/extfs/`](../../lib/fs/extfs/) | The ext format: `fs.rs` (`MikuFS`), `mount.rs` (superblock checks, feature masks), `structs.rs`, `reader.rs` (`DiskReader`), `cache.rs`, `xattr.rs`, `trim.rs`, `ext2/` (bitmaps, directories, htree, inodes, writes, hard links), `ext3/` (journal, recovery, journalled operations), `ext4/` (extents, inline data, checksums, writes, upgrade), `fsck/`, `mkfs/` |
| [`lib/fs/btrfs/`](../../lib/fs/btrfs/) | The btrfs format: `mount.rs`, `chunk.rs`, `node.rs`, `volume.rs`, `fs_tree.rs`, `file.rs`; and host-only write machinery (`trans.rs`, `shape.rs`, `commit.rs`, `extent.rs`, `space.rs`) |
| [`kernel/fs/host.rs`](../../kernel/fs/host.rs) | `KernelHost`: `BlockHost` over the kernel block layer |
| [`kernel/fs/ext/`](../../kernel/fs/ext/) | `mount.rs` (slot table `STATE`, probing, root mount, remount), `fsimpl.rs` (`FileSystem` for `MikuFS`), `selftest.rs` |
| [`kernel/fs/btrfs/`](../../kernel/fs/btrfs/) | `mount.rs`, `fsimpl.rs`, `selftest.rs` |
| [`kernel/fs/memfs.rs`](../../kernel/fs/memfs.rs) | memfs and its mount table |
| [`kernel/fs/tmpfs.rs`](../../kernel/fs/tmpfs.rs), [`kernel/fs/vfs/pages.rs`](../../kernel/fs/vfs/pages.rs) | tmpfs (a marker type; the data are VFS pages) |
| [`kernel/fs/devfs.rs`](../../kernel/fs/devfs.rs) | Device nodes |
| [`kernel/fs/procfs.rs`](../../kernel/fs/procfs.rs) | `/proc` files |
| [`kernel/fs/registry.rs`](../../kernel/fs/registry.rs) | Probers and mount-slot dispatch ([vfs.md](vfs.md)) |

Design history: [`dev/fs-architecture.md`](../../dev/fs-architecture.md).

---

## The host seam: fshost

`fshost::BlockHost` ([`lib/fs/fshost/src/host.rs`](../../lib/fs/fshost/src/host.rs)) is "a disk and a clock". `lib/fs/extfs` re-exports it as `ExtHost`.

| Method | Meaning |
|:--|:--|
| `read_sectors` | Absolute 512-byte sectors on the device |
| `write_sectors` | May sit in a cache and reach the medium whenever |
| `write_sectors_sync` | Write-through, ordered against other sync writes |
| `write_sectors_barrier` | On stable media when it returns (FUA, or write plus flush) |
| `flush` | Drain the device's cache |
| `discard`, `total_sectors`, `ticks`, `tick_hz`, `wall_clock` | Defaulted |

**The four write methods are the load-bearing decision.** The journal writes its descriptor and block copies with `write_sync` and its commit record with `write_barrier`; a backend that implements all four the same way degrades crash consistency to whatever the device does.

- **`KernelHost`** ([`kernel/fs/host.rs`](../../kernel/fs/host.rs)) maps them to the block layer's write-back, write-through, FUA and flush paths ([storage.md](storage.md)); `ticks` is the 250 Hz tick, `wall_clock` the kernel clock (`None` when unset).
- **`MemDisk`** models a device with a *volatile* write cache: `crash()` loses everything not flushed, `crash_at(n)` cuts the power after the n-th write. `lib/fs/fshost/tests/memdisk.rs` tests the model itself (a write-through that did not invalidate the cache it bypassed once made the earlier write win; breaking `write_sectors_barrier` passed until a case with no `flush` at all was added).

Blocks (1-4 KiB) are one layer up in `DiskReader`, which owns the partition offset and the read-only flag and is the single point every write goes through.

---

## ext2, ext3, ext4

`lib/fs/extfs` is about 17,000 lines. `MikuFS<H: ExtHost>` is one mounted filesystem. The kernel keeps up to four (`kernel/fs/ext/mount.rs`, `MAX_MOUNTS = 4`). Which flavour a volume is (`fs_type`) comes from its superblock: ext4 if it has ext4 features, ext3 if it has a journal, else ext2.

### Features supported and refused

Mount-time checks are in [`lib/fs/extfs/src/mount.rs`](../../lib/fs/extfs/src/mount.rs) (`check_superblock`). A bit in the supported mask is a promise the code keeps; a bit outside it is refused or forces read-only.

**Incompatible features (`EXT_INCOMPAT_SUPP`)** — anything else refuses the mount:

| Supported | |
|:--|:--|
| `filetype` | Directory entries carry the file type |
| `recover` | The journal needs replay (done when the mount is made writable) |
| `extents` | Extent trees, up to depth 5 |
| `64bit` | Accepted; see the limit below |
| `flex_bg` | Flexible block groups |
| `inline_data` | Small files and their tails in the inode |
| `csum_seed` | Checksums seeded from `s_checksum_seed` |

| Refused | Why |
|:--|:--|
| `meta_bg` | The descriptor table is not one contiguous run |
| `journal_dev` | External journal device: no code to open one |
| `encrypt` | No key handling |
| `compression` | Never implemented by anyone |
| `mmp` | Multi-mount protection needs a heartbeat thread |
| `large_dir` | Three-level directory indexes; the htree code does two (`DX_MAX_LEVELS`) |

**Read-only-compatible features (`EXT_RO_COMPAT_SUPP`)** — anything else mounts read-only:

| Supported (read-write) | Forced read-only |
|:--|:--|
| `sparse_super`, `large_file`, `huge_file`, `gdt_csum`, `dir_nlink`, `extra_isize`, `metadata_csum` | `bigalloc` (clusters), `quota`, `project`, `verity`, `readonly` |

**Compatible features** need no check; `dir_index` (htree) and `ext_attr` are read and written, `has_journal` selects ext3/ext4 journalling.

**Geometry checks** before anything divides by a field: block size, non-zero inodes and blocks per group, inode size a power of two in [128, block size], at most 2^20 groups (`MAX_BLOCK_GROUPS`), the group table fitting before the first data block, the superblock checksum, and a checksum type of crc32c. Each failure is a distinct `MountReject` variant in the mount log.

**Journal features**: replayable journals have `revoke`, `64bit` and `csum_v3` (`JBD_INCOMPAT_SUPP`). A journal with `csum_v2`, `async_commit` or `fast_commit` cannot be replayed here; the volume comes up read-only rather than refused, so it stays rescuable.

**Not implemented** (named absences): `data=ordered` and `data=journal` (only `data=writeback`, see below), delayed allocation, a multi-block allocator beyond runs, online resize, POSIX ACL decoding, in-inode xattr *writes*, inline *directories*, `mknod` beyond the four special types. **64-bit is partial**: the superblock's high halves are read and the feature accepted, but the block allocator and block numbers in the driver are `u32`, so blocks past 2^32 are not addressable.

### On-disk structures handled

- **Block maps**: direct, indirect, double- and triple-indirect (ext2/3 files).
- **Extent trees** of any depth up to 5, with unwritten extents for `fallocate` (`ext4/extents.rs`).
- **Inline data** (`ext4/inline.rs`): the first 60 bytes in `i_block`, the rest in the inode's xattr area as `system.data`. Reads return the whole file (they used to stop at 60 bytes); a write that outgrows the inode converts the file to extents (`ext4_convert_inline_to_extents`).
- **Directories**: linear blocks and hash-indexed (`dir_index`/htree, `ext2/htree.rs`) read and written. The index is built when a directory grows past one block, leaves split at a hash boundary, and the tree gains a level (up to two). Unlink finds the name through the index (`htree_find`) and reads only that leaf.
- **Fast and slow symlinks**: a target under 60 bytes lives in `i_block`.
- **Device nodes**: major and minor in `i_block`.
- **Timestamps**: 32-bit seconds plus the `_extra` words (two more bits of seconds past 2038, 30 bits of nanoseconds) when the inode is large enough.
- **Ownership**: 32-bit uid/gid (`uid_full`/`set_uid_full` write both halves). The VFS folds ids into its credential range (`cred::fold_id`).
- **Checksums**: crc32c (SSE4.2 when present) over superblock, group descriptors, inodes, bitmaps, extent blocks, directory blocks and htree nodes when `metadata_csum` is on; `gdt_csum` otherwise.
- **Orphans**: the orphan list is processed when a volume is made writable (`activate`: journal replay, then orphan cleanup).
- **TRIM**: `trim_free_blocks` discards free runs (`fstrim`, [storage.md](storage.md)).

### The jbd2 journal

[`lib/fs/extfs/src/ext3/`](../../lib/fs/extfs/src/ext3/): `journal.rs`, `ops.rs`, `recovery.rs`.

**One mode, `data=writeback`: metadata only.** The coverage rule is a function, not a policy: *a block is journalled because it went through `write_meta_block`*. Every new directory block, both halves of an htree split, every new index node and indirect block is covered that way. File data never goes through the journal.

**Transactions** collect the blocks operations log, as tags in a growable list (it was a fixed 64 tags, and the 65th block of a transaction was written in place with nothing in the log). A transaction is committed when:

- it has been open for 5 seconds (`COMMIT_INTERVAL_SECS`) and an operation closes a handle;
- it uses a quarter of the journal;
- bdflush's periodic pass finds the filesystem dirty (`ext::mount::periodic_sync`, every 2 s; [storage.md](storage.md));
- `sync`, `fsync` (ext journals the whole filesystem, so `fsync` of a file is a filesystem commit), remount read-only or unmount;
- **midway** (`commit_midway`): a full descriptor, a journal near full, or a metadata cache full of pinned blocks. The next transaction is opened with the operation's handles, so the rest of the operation is logged too.

**Commit order** (`force_commit`), and the reason the host has four write methods:

1. descriptor block(s), write-through;
2. one copy of each logged block, write-through;
3. the revoke record, write-through;
4. the commit record, **barrier** (on stable media when it returns);
5. only then release the pins, so checkpointing may begin.

A crash before step 4 leaves an incomplete transaction that replay discards; from step 4 the change is durable.

**Pins.** A block the open transaction logged is pinned in ext's metadata cache: it may not reach its real location before the commit record. Eviction and flush skip pinned blocks; when every slot is pinned the answer is to commit.

**Revokes**: a freed metadata block is revoked so replay does not resurrect an older logged copy over new contents. Revokes are a set per transaction (tens of thousands once overflowed the one record). **Data blocks get no revokes**: data is never logged.

**Replay and recovery** (`ext3_recover`), at the first read-write activation of a volume that needs it: three passes over one walk of the log — collect revokes with the sequence each applies from, pair each descriptor with its commit record, replay. Sequence comparison handles wrap-around (`seq_before`, JBD2's `tid_lt`). Earlier bugs fixed there: a scan that stopped after 512 blocks, tag walks that broke past 64 tags, revokes applied to the whole log regardless of sequence. Mounting read-only never replays (and never writes).

**Creating a journal**: `ext3mkjournal` in the kernel shell; mkfs makes one of at least 1024 blocks (`MIN_JOURNAL_BLOCKS`), the smallest JBD2 and e2fsck accept.

### The data path and crash consistency

Since 2026-10-28 file data does not go through ext's metadata cache (`ext4/write.rs` `ext4_write_parts`, `ext2/bitmap.rs` `alloc_blocks`):

- **New blocks are allocated in runs**: as many free blocks in a row as the bitmap has from the group's hint, one bitmap write, one checksum, one count update.
- **A new block's data goes to the device (`write_sectors_sync`, one call per run) before the extent that names it.** Neither a commit in between nor a crash can show a file a block with someone else's old contents. The crash test cuts power at every write of a file growing over a deleted file's blocks: with the old path 71 of 83 cut points showed the old bytes, with this one none.
- **Whole blocks already allocated** go to the device the same way, one call per physically contiguous run.
- **Short in-place overwrites (under 64 KiB)** go into the block layer's write-back cache (`write_blocks`), which gathers them.
- **Partial blocks, blocks reserved by `fallocate` and never written, and blocks the open transaction has logged** (a freed tree node reused) still go through the metadata cache, pinned where needed.
- A block written past the cache has any cached copy dropped, and the block layer updates its resident copy in place.
- **Blocks freed by the open transaction are not reused until it commits** (`freed_in_running`): until the free commits, a crash gives them back to the file that had them. When nothing else is free the transaction commits and the search runs again.
- **Freeing goes a run at a time**: one bitmap write, one checksum, one count update per group's part. Truncating 64 MiB costs tens of microseconds of CPU (16 ms of ext's lock before).
- With the VFS write-back above it, the bytes reach ext from page-cache flushes, one transaction per 1 MiB run (see [vfs.md](vfs.md)).

**Rules a crash test checks**: a grown file never shows another file's old bytes; a delete and a write in one transaction leave the deleted file whole if the transaction is lost; every image after replay passes `e2fsck`.

### The metadata cache

`lib/fs/extfs/src/cache.rs`: 2 MiB of metadata blocks (at most 1024 entries; 512 at 4 KiB blocks), indexed by a map from block number to slot with a free list and a dirty count. A hit is a map lookup; only taking a slot from a full cache scans for the least recently used. Its job is the pins, not caching: the block layer below has its own cache ([storage.md](storage.md)). At 128 slots a transaction of 2000 creates filled it with pins and committed inside the create, under the VFS lock; at 2 MiB a whole transaction's pins fit (a descriptor holds 510 tags).

`extcache` shows its state and `extcacheflush` flushes it (kernel shell).

### Extended attributes

[`lib/fs/extfs/src/xattr.rs`](../../lib/fs/extfs/src/xattr.rs).

- **Read** from both places: the inode body after the extra fields (`inode_size - 128 - extra_isize`, with a 4-byte magic) and an external block named by `i_file_acl` (magic `0xEA020000`).
- **Written** to the external block only.
- **Namespaces**: `user.` (1), `trusted.` (4), `security.` (6), `system.` (7) through the VFS trait; POSIX ACL indices (2, 3) exist but are not decoded.
- **Limits**: 16 attributes per list (`MAX_XATTR_ENTRIES`), names up to 64 bytes, values up to 256 bytes.
- **System calls** `getxattr` (38), `setxattr` (39), `listxattr` (81), `removexattr` (82) take an **ext inode number** on the active ext slot, not a path or descriptor, and check permissions themselves (`may_touch_xattr`). They bypass the VFS and raise no inotify events.

### fsck

[`lib/fs/extfs/src/fsck/`](../../lib/fs/extfs/src/fsck/), command `extfsck [-y|--repair]` in the kernel shell. The five passes e2fsck uses, in its order, because each depends on the last:

| Pass | Checks |
|:--|:--|
| 0 | Superblock and group descriptors |
| 1 | Every inode and every block it claims (a block map built from the inodes; doubly claimed and unowned blocks) |
| 2 | Every directory: entries, names, `.` and `..` |
| 3 | Connectivity: everything reachable from the root |
| 4 | Link counts against the references pass 2 counted |
| 5 | Bitmaps and free counters against what passes 1 and 2 found |

With repair, each pass fixes what it found before the next runs, and every write goes through the journal, so a power cut during repair replays like anything else. **It runs on a mounted filesystem** (MikuOS has no second machine to boot); repair insists that the journal has been replayed and the volume is writable. Checking a filesystem another task is writing is the user's risk. Blockless inodes (inline files, fast symlinks, device nodes) are recognised before `i_block` is walked as a block map.

### mkfs

[`lib/fs/extfs/src/mkfs/`](../../lib/fs/extfs/src/mkfs/). Defaults (`MkfsParams::new`):

| Type | Block size | Inode size | Journal | Features |
|:--|:--|:--|:--|:--|
| ext2 | 1024 | 128 | none | `dir_index`, `ext_attr`, `filetype`, `sparse_super`, `large_file` |
| ext3 | 1024 | 128 | 1024 blocks | ext2's plus `has_journal` |
| ext4 | 4096 | 256 | 1024 blocks | ext3's plus `extents`, `huge_file`, `dir_nlink`; with 256-byte inodes also `extra_isize` and `metadata_csum` |

The default label is `miku`. The block-group count is derived from the partition size; a filesystem smaller than its partition shows up only in the `[mkfs]` group count line (the cap bug fixed 2026-08-11). The commands and options are in [storage.md](storage.md).

### How ext implements FileSystem

[`kernel/fs/ext/fsimpl.rs`](../../kernel/fs/ext/fsimpl.rs) implements the trait directly on `MikuFS` and picks the ext function for each request, so the VFS never learns that ext has two of something:

| Trait method | ext call |
|:--|:--|
| `lookup` | `ext2_lookup_in_dir` |
| `create` | `ext3_create_file_as` / `ext3_create_dir_as` / `ext3_create_symlink_as` / `ext3_mknod_as`, by kind |
| `link` | `ext3_hardlink` |
| `unlink` | `ext3_delete_file` or `ext3_delete_dir` (`is_dir`) |
| `rename` | `ext3_rename` within one directory, `ext3_move` across |
| `write_at` / `write_pages` | `ext3_write_file` / `ext3_write_parts` |
| `truncate` | `ext3_truncate` (to 0) or `ext3_truncate_to` |
| `setattr` | `ext2_chmod`, `ext2_chown`, times |
| `touch_atime` | Never on a read-only mount |
| `fsync`, `sync` | A whole-filesystem commit |
| `fallocate`, `punch_hole` | `ext4_fallocate`, `ext2_punch_hole` |
| `getxattr`, `setxattr` | Name split into namespace index and rest |
| `trim` | `trim_free_blocks` |
| `statfs` | Block size, counts, `MAX_NAME`, read-only flag |

The `ext3_*` operations open and close journal handles; on ext2 (no journal) they write directly.

**Locking.** The four slots live in `ext::mount::STATE`, a `SchedMutex` named `ext`. `with_ext2_for(slot, f)` (what `registry::with_mounted` calls) holds it across the whole closure, disk I/O included. It nests inside `VFS_LOCK` and outside `FILE_CACHE` ([vfs.md](vfs.md)). VFS calls that would wait on it with the VFS lock held give way first (`registry::busy`). The longest ext holds in a bench run are commits (3-5 ms) and a flush run (1-1.5 ms, 2026-10-28).

**Read-only first.** Every ext mount starts read-only: no journal replay, no orphan cleanup, no superblock mount-count stamp, no atime. `remount_rw(slot)` (the `fs.rw` path, `rootrw` at boot) does all of that at the moment it is asked; `remount_ro` flushes and marks the filesystem cleanly unmounted. See [storage.md](storage.md) for the commands and the disk-safety rule.

### Rules that are easy to break

From [`dev/fs-architecture.md`](../../dev/fs-architecture.md):

- **A blockless inode has no block map.** Inline files, fast symlinks and device nodes keep other things in `i_block`; ask `has_inline_data() || is_fast_symlink() || is_special()` before walking it (fsck once claimed block 259 for `/dev/null`).
- **A field with a high half has a setter that writes both halves** (`set_uid_full`): a chown from uid 100000 to 0 once left the file owned by 65536.
- **A backup superblock is not the only backup**: write the group descriptor table behind it too, or `e2fsck -b` reports every free count wrong.
- **What the mount check claims, the code must do**: `large_dir` was once accepted for writing while the code refused three-level indexes.
- **A test double that is too forgiving passes**: test the model itself, and break it on purpose to see the right tests fail.

<a id="ext-tests"></a>
### Tests

**Host tests** (no boot):

```
cd lib/fs/extfs && cargo test
```

234 test functions in `src/` and `tests/` (229 when the design note was written): mount checks, checksums against `mke2fs` images, extents, htree (against `e2fsck -D` and `debugfs`), inline data, journal machinery, orphans, truncate, rename, fallocate, timestamps past 2038, ownership, big volumes past 32 block groups, limits, fsck and repair, foreign images. Tests that need **e2fsprogs** (`e2fsck`, `mke2fs`, `debugfs`) skip themselves when it is not installed.

**The crash tests** (`tests/crash.rs`): `MemDisk::crash_at(n)` cuts the power after the n-th write of an operation, for **every** n; each image is mounted (which replays the journal) and handed to `e2fsck`. A journal is right at all cut points or it is not a journal.

`cd lib/fs/fshost && cargo test` runs the 29 tests of `MemDisk` and crc32c.

**On the machine**: `ext test` in the kernel shell or `run=ext_test` on the GRUB line ([`kernel/fs/ext/selftest.rs`](../../kernel/fs/ext/selftest.rs)) covers what the host cannot: the kernel's mkfs on a real device and mounting what it wrote, a remount finding what was written, sync reaching the medium, 200 files through a cache that cannot hold them, a directory indexed past one block, a bitmap fault found and repaired, the filesystem passing its own check, and two filesystems with one name and no confusion. Baseline "ext 10" (2026-10-27). The VFS suite (`vfs test`) also runs against a scratch ext4 ([vfs.md](vfs.md)).

---

## btrfs

`lib/fs/btrfs` reads btrfs volumes; the kernel driver ([`kernel/fs/btrfs/`](../../kernel/fs/btrfs/)) mounts them **read-only**. Every mutating `FileSystem` method returns `ReadOnly` (`EROFS`), not `NotSupported`, because the operation is valid btrfs and only this driver does not write.

**What it reads:**

- the superblock (`mount.rs` `check_superblock`), the bootstrap chunk array and the chunk tree (`chunk.rs`: logical to physical, stripe arithmetic from Linux's `volumes.c`);
- tree nodes and leaves (`node.rs`), validated the way Linux's `tree-checker.c` validates them;
- the default subvolume's filesystem tree (`fs_tree.rs`): inodes, directory items, extended inode refs;
- file contents (`file.rs`): `INLINE`, `REG` (holes included, `disk_bytenr` 0) and `PREALLOC` (reads as zeros) extents;
- symlink targets.

**Incompat features accepted** (`INCOMPAT_SUPP`): `mixed_backref`, `default_subvol`, `big_metadata`, `extended_iref`, `skinny_metadata`, `no_holes`, `raid1c34`. **Read-only-compat accepted** (`COMPAT_RO_SUPP`): `free_space_tree`, `free_space_tree_valid`, `verity`, `block_group_tree` (a mkfs default since v6.x).

**Refused at mount**, each with a reason in the code: `compress_lzo`/`compress_zstd` (compressed extents would read as garbage), `raid56` (parity layout), `zoned`, `extent_tree_v2`, `raid_stripe_tree`, `remap_tree`, `mixed_groups`, `metadata_uuid` and `simple_quota` (untested). Also refused: a bad magic, a checksum type other than crc32c, a bad superblock checksum, unsupported superblock flags, bad sector or node sizes, a node size disagreeing with the leaf size, a root deeper than allowed or not sector-aligned, zero devices, a superblock that says it lives somewhere other than where it was read, a bad system chunk array, a device item naming another filesystem, and `block_group_tree` without the features it was defined with.

**Declared limits:**

- **One device.** A multi-device volume is not refused at mount; it fails at the first chunk on another device, with `NotFound` naming it.
- **The default subvolume only.** A name that points into another subvolume returns `NotSupported`. `NodeId` has two words for `(tree, objectid)`, but a vnode still carries one, so the tree word arrives as zero and `tree_or_default` maps it to the default subvolume.
- **No xattrs, no compression.**

**Write machinery that the kernel does not use.** `lib/fs/btrfs` also contains copy-on-write transactions (`trans.rs`), item insertion and deletion with leaf splits and tree growth (`shape.rs`), the superblock commit in Linux's `write_all_supers` order (`commit.rs`), extent-tree writes (`extent.rs`) and free-space accounting (`space.rs`). They are exercised by host tests against `mkfs.btrfs` images; no kernel path calls them.

**Kernel side.** Up to two volumes (`btrfs::mount::MAX_MOUNTS = 2`) in `STATE`, a `SchedMutex` named `btrfs`; `with_btrfs_for(slot, f)` holds it across the call. Mount with `mount btrfs [/path]` in the kernel shell: it probes every device and partition and mounts the first btrfs it finds (no `rw` to ask for, so no drive to name), at `/mnt` by default ([storage.md](storage.md)).

**Tests.** `cd lib/fs/btrfs && cargo test` (189 test functions: superblocks, the chunk map, trees, reads compared byte for byte with what `mkfs.btrfs` wrote, and the write machinery). `btrfs test` / `run=btrfs_test` on the machine needs a device holding the volume built by `mkfs.btrfs -r` from `miku-os/btrfs.img`; without one every case skips. Cases: mount it, graft it into the tree, statfs reports btrfs, a file in a subdirectory resolves, a 200,000-byte file reports its size and reads back what was put in it to the end, the staged names come back.

---

## tmpfs

tmpfs is not an implementation of `FileSystem` (`kernel/fs/tmpfs.rs` is a seven-line marker type). **It is the vnode table plus the VFS's own page cache**: every boot-time node is `FsType::TmpFS`, and its data live in `MikuVFS`. `with_backing` answers `None` for it, and every VFS operation has an in-memory branch for that case.

- `/` is tmpfs; disk directories are grafted onto it ([vfs.md](vfs.md)). `/tmp` is tmpfs.
- **Pages are physical frames**, 4 KiB each, allocated when first written ([`kernel/fs/vfs/pages.rs`](../../kernel/fs/vfs/pages.rs)).
- **Ceiling: a quarter of RAM** (`tmpfs_limit()`, at least 256 pages), computed when a page is allocated; beyond it writes fail with `ENOSPC`.
- **Files up to 4 GiB** (a two-level index of 1024 x 1024 pages).
- **`MAP_SHARED` maps the file's own frame**, so all mappings and `read`/`write` see one page, and a process-shared futex in such a page keys on the frame (SQLite's `-shm`, LMDB's lock file). See [memory.md](memory.md).
- **No eviction, no swap**: tmpfs vnodes and pages are the only copy.
- Symlinks and hard links work; xattrs do not (the xattr calls are ext's).
- Core dumps written to `/tmp` are capped at 16 MiB by the dumper ([processes.md](processes.md)).
- `TmpfsPages:` in `/proc/meminfo` counts its pages.

History: until 2026-09-25 tmpfs was 1024 pages of 512 bytes (512 KiB total, 64 KiB per file); Zed's SQLite write-ahead log stopped at exactly 65,536 bytes.

---

## memfs

[`kernel/fs/memfs.rs`](../../kernel/fs/memfs.rs): a full `FileSystem` implementation, the write half included, over a `BTreeMap` on the heap. It exists to be a **second filesystem** for the VFS selftest, so the VFS's per-vnode routing decisions are tested on any machine with one disk (the bug that stamped btrfs children `Ext4` was found the first time two filesystems were mounted).

- Root node number 1 (0 is the "no node" value in two places).
- Budgets: 1 MiB of file data (`MAX_BYTES`) and 262,144 nodes (`MAX_NODES`), so `ENOSPC` can be produced on purpose.
- Up to two mounts (`MAX_MOUNTS = 2`); a mount is `MemFs::new()`. Its slot table and lock (`SchedMutex` named `memfs`) are at the bottom of the file.
- Regular files, directories, symlinks, hard links, rename.
- Not a tmpfs and not meant to become one. It is mounted only by `vfs test` (`mount_memfs`, `unmount_memfs`); there is no shell command for it.

---

## devfs

`/dev`, built at boot by the VFS ([`kernel/fs/devfs.rs`](../../kernel/fs/devfs.rs)); read-only as a directory. Nodes: `null`, `zero`, `random`/`urandom`, `console` (0620), `tty` (the caller's controlling terminal), `ptmx` (opens a new pty master), `ptmN`/`ptsN` (pty masters, mode 0, and slaves), `net0` (raw Ethernet frames, root with `CAP_NET_RAW`), and `blkN`/`blkNpM` (raw block devices and partitions, major 8, root only, 0600). Opening a device node routes the descriptor outside the VFS lock where possible. The terminal nodes are described in [input-and-terminals.md](input-and-terminals.md), the block nodes in [storage.md](storage.md), and the file-by-file reference is in [diagnostics.md](diagnostics.md).

---

## procfs

`/proc`, read-only: twelve flat files generated when read (`PROC_ENTRIES`): `version`, `uptime`, `meminfo`, `mounts`, `cpuinfo`, `stat`, `heap`, `diskstats`, `net`, `clock`, `slabinfo`, `lockstat`. A read is formatted into an 8192-byte buffer under the VFS lock (the VFS hands procfs its own counters so it never takes the lock again), so no file is longer than 8 KiB. There are **no per-process directories** (`/proc/<pid>`, `/proc/self`). The formats are documented in [diagnostics.md](diagnostics.md).

---

## Filesystems that do not exist

No FAT/vfat, no ISO 9660, no NTFS, no exFAT, no network filesystems, no FUSE, no overlay or bind mounts, no `mount(2)`. The installer and the boot image use ext4 ([system.md](system.md), [building.md](building.md)).

Related: [vfs.md](vfs.md), [storage.md](storage.md), [memory.md](memory.md), [diagnostics.md](diagnostics.md).
