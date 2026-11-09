# The VFS

The VFS (`kernel/fs/vfs/`) is the layer every file system call goes through. It keeps a cache of what the filesystems know: one vnode per file, one entry per name, and the pages of files. It owns the descriptor tables, the page cache for disk files with its write-back, pipes, inotify and record locks. Each filesystem is reached through one trait, `FileSystem`. tmpfs, devfs and procfs are not filesystems behind that trait: their contents are the vnode table itself. The whole tree sits behind one read-write lock, `VFS_LOCK`, but most hot paths now run beside each other under the read side, or with no VFS lock at all. This page explains how the pieces fit, what each system call does to them, and how to measure and debug them.

## Contents

- [Code map](#code-map)
- [Layers](#layers)
- [The FileSystem trait](#the-filesystem-trait)
- [The registry and mounting](#the-registry-and-mounting)
- [Vnodes](#vnodes)
- [Names: entries, aliases, negative entries, whiteouts](#names-entries-aliases-negative-entries-whiteouts)
- [Vnode eviction](#vnode-eviction)
- [The path walk](#the-path-walk)
- [File descriptors](#file-descriptors)
- [The page cache for disk files](#the-page-cache-for-disk-files)
- [Write-back](#write-back)
- [The tmpfs page cache](#the-tmpfs-page-cache)
- [Directory listings and getdents](#directory-listings-and-getdents)
- [Locking](#locking)
- [Unlinking and truncating big files](#unlinking-and-truncating-big-files)
- [inotify](#inotify)
- [Record locks](#record-locks)
- [Pipes and FIFOs](#pipes-and-fifos)
- [Attributes and permissions](#attributes-and-permissions)
- [System calls](#system-calls)
- [Boot flags](#boot-flags)
- [/proc counters](#proc-counters)
- [Selftests](#selftests)
- [Measured numbers](#measured-numbers)
- [Known gaps](#known-gaps)
- [Troubleshooting](#troubleshooting)

---

## Code map

| File | What it holds |
|:--|:--|
| [`kernel/fs/vfs/fsops.rs`](../../kernel/fs/vfs/fsops.rs) | The `FileSystem` trait, `NodeId`, `NodeMeta`, `RawDirent`, `FsSetAttr` |
| [`kernel/fs/registry.rs`](../../kernel/fs/registry.rs) | Probers, `with_mounted` (mount slot to filesystem), `busy`/`wait_idle` |
| [`kernel/fs/vfs/core/mod.rs`](../../kernel/fs/vfs/core/mod.rs) | `MikuVFS`, `VFS_LOCK`, `with_vfs*`, `read_fd`, `close_fd`, `seek_fd`, the file-page shrinker |
| [`kernel/fs/vfs/core/backing.rs`](../../kernel/fs/vfs/core/backing.rs) | `with_backing`, `with_backing_pair`, `backing_of`: the only place that knows where a filesystem lives |
| [`kernel/fs/vfs/core/path_resolve.rs`](../../kernel/fs/vfs/core/path_resolve.rs) | The walk (`walk`, `follow`, `walk_follow`) over `Writer` and `Reader`, `abs_path` |
| [`kernel/fs/vfs/core/fill.rs`](../../kernel/fs/vfs/core/fill.rs) | Filling vnodes and directories from a filesystem; deferred reads |
| [`kernel/fs/vfs/core/names.rs`](../../kernel/fs/vfs/core/names.rs) | Negative entries, complete directories, the icache, dcache counters |
| [`kernel/fs/vfs/core/reclaim.rs`](../../kernel/fs/vfs/core/reclaim.rs) | Vnode eviction and the vnode shrinker |
| [`kernel/fs/vfs/core/cached.rs`](../../kernel/fs/vfs/core/cached.rs) | Reads through the disk page cache: unlocked hits, fills without the lock |
| [`kernel/fs/vfs/core/writeback.rs`](../../kernel/fs/vfs/core/writeback.rs) | Dirty pages and their flushes |
| [`kernel/fs/vfs/core/unlinked.rs`](../../kernel/fs/vfs/core/unlinked.rs) | Big files deleted after the lock |
| [`kernel/fs/vfs/core/truncate.rs`](../../kernel/fs/vfs/core/truncate.rs) | Truncation, `late_truncate` |
| [`kernel/fs/vfs/core/`](../../kernel/fs/vfs/core/) `file_ops.rs`, `dir_ops.rs`, `links.rs`, `attr.rs`, `access.rs`, `vnode_mgmt.rs`, `bootstrap.rs`, `watch.rs` | The operations themselves; the boot-time tree |
| [`kernel/fs/vfs/vnode.rs`](../../kernel/fs/vfs/vnode.rs) | `VNode` |
| [`kernel/fs/vfs/table.rs`](../../kernel/fs/vfs/table.rs) | `Table<T>`: the growable vnode table |
| [`kernel/fs/vfs/lru.rs`](../../kernel/fs/vfs/lru.rs) | Intrusive LRU over vnode ids |
| [`kernel/fs/vfs/children.rs`](../../kernel/fs/vfs/children.rs) | A directory's name table |
| [`kernel/fs/vfs/fd.rs`](../../kernel/fs/vfs/fd.rs) | Descriptor tables, shards, lockless slots |
| [`kernel/fs/vfs/filecache.rs`](../../kernel/fs/vfs/filecache.rs) | `FILE_CACHE`: the disk page cache |
| [`kernel/fs/vfs/pages.rs`](../../kernel/fs/vfs/pages.rs), [`address_space.rs`](../../kernel/fs/vfs/address_space.rs) | Page frames and the per-file page index |
| [`kernel/fs/vfs/pipe.rs`](../../kernel/fs/vfs/pipe.rs) | Pipes |
| [`kernel/fs/vfs/notify.rs`](../../kernel/fs/vfs/notify.rs) | inotify |
| [`kernel/fs/vfs/lock.rs`](../../kernel/fs/vfs/lock.rs) | POSIX record locks |
| [`kernel/fs/vfs/mount.rs`](../../kernel/fs/vfs/mount.rs) | The VFS mount table (tmpfs, devfs, procfs) |
| [`kernel/fs/vfs/types.rs`](../../kernel/fs/vfs/types.rs) | `VfsError`, `FsType`, `VNodeKind`, `OpenFlags`, `VNodeStat`, limits |
| [`kernel/fs/vfs/permissions.rs`](../../kernel/fs/vfs/permissions.rs) | Mode-bit checks for callers without a vnode |
| [`kernel/fs/vfs/selftest.rs`](../../kernel/fs/vfs/selftest.rs) | `vfs test` / `run=vfs_test` |
| [`kernel/syscall/adapters/file.rs`](../../kernel/syscall/adapters/file.rs), [`fs.rs`](../../kernel/syscall/adapters/fs.rs), [`notify.rs`](../../kernel/syscall/adapters/notify.rs) | The system calls |

Design notes: [`dev/fs-architecture.md`](../../dev/fs-architecture.md) (layers, page cache, write-back, pipes, inotify), [`dev/vfs-dentries-inodes.md`](../../dev/vfs-dentries-inodes.md) (vnode table, names, eviction), [`dev/vfs-locking.md`](../../dev/vfs-locking.md) (the lock and what came out from under it).

---

## Layers

```
  ring 3            system calls (open, read, stat, getdents, ...). No mount(2)
  ----------------------------------------------------------- syscall adapters
  kernel/fs/vfs     MikuVFS: vnodes, names, descriptors, page caches
  ----------------------------------------------------------- trait FileSystem
  kernel/fs/ext     fsimpl.rs  |  kernel/fs/btrfs  |  kernel/fs/memfs.rs
  lib/fs/extfs         the format |  lib/fs/btrfs        |
  ----------------------------------------------------------- trait ExtHost / BlockHost
  kernel/io/block   block cache, blk-mq, bdflush
  ----------------------------------------------------------- trait BlockDriver
  drivers           ahci, nvme, virtio_blk, ata, ramdisk
```

The filesystems are described in [filesystems.md](filesystems.md), the block layer and disks in [storage.md](storage.md).

---

## The FileSystem trait

[`kernel/fs/vfs/fsops.rs`](../../kernel/fs/vfs/fsops.rs). Object-safe, `&mut self` throughout (an implementation that caches blocks or holds a transaction open needs no interior mutability). The method list was taken from what ext is actually asked.

**Types.**

| Type | Meaning |
|:--|:--|
| `NodeId { tree, node }` | Where a node lives on its filesystem. ext uses `node` = inode number, `tree` = 0 (`NodeId::ino(n)`); btrfs can put a subvolume tree and an objectid. `NodeId::NONE` is node 0 |
| `NodeMeta` | Everything the VFS caches about a node: `id`, `kind`, permission bits, full 32-bit `uid`/`gid`, `size`, `blocks`, `nlinks`, `atime`/`mtime`/`ctime`, `rdev`. Folding foreign ids to what kernel credentials represent is the VFS's job (`cred::fold_id`) |
| `RawDirent<'a>` | One directory entry: the name borrowed from the filesystem's buffer at full width, its `NodeId` and kind |
| `FsSetAttr` | Optional `mode`, `uid`, `gid`, `size`, `atime`, `mtime` |

**Required methods.**

| Method | Purpose |
|:--|:--|
| `fs_type`, `read_only`, `root` | Identity. Every mutating method must return `ReadOnly` (not `NotSupported`) on a read-only mount |
| `statfs` | Block size, block and inode counts, name length |
| `sync` | Everything staged in memory onto the medium |
| `lookup(dir, name)` | One name |
| `readdir(dir, f)` | Hand every entry to a callback; the callback may stop early, the filesystem may not |
| `create(dir, name, kind, mode, uid, gid, rdev, target)` | One creator for files, directories, symlinks and device nodes |
| `link`, `unlink(dir, name, is_dir)`, `rename` | Namespace changes; `is_dir` picks the removal |
| `read_at`, `write_at`, `truncate` | Data |
| `readlink` | A symlink's target |
| `getattr`, `setattr` | Attributes |

**Optional methods** (defaults in brackets): `write_pages` (gathers the pages for `write_at`), `touch_atime` (nothing), `fsync` (`sync`), `fallocate`, `punch_hole`, `getxattr`, `setxattr`, `trim` (all `NotSupported`).

To add a VFS operation: a trait method, defaulted to `NotSupported` unless every filesystem can answer it, plus one call site through `with_backing`. Not a branch on `FsType`.

### How the VFS reaches a filesystem

`MikuVFS::with_backing(vid, |fs, node| ...)` ([`backing.rs`](../../kernel/fs/vfs/core/backing.rs)):

- `backing_of(vid)` reads the vnode's `fs_type`, mount slot and inode number. A vnode with no filesystem (tmpfs, devfs, procfs) returns `None`, and every caller keeps its in-memory branch for that case.
- A vnode that has a filesystem whose slot is not ready returns `Some(Err(NotMounted))`, never `None`: collapsing the two would let a write to a vanished disk land in the page cache and report success.
- Every call clears the vnode's `attrs_fresh` flag, so the next `stat` reads the inode again.
- It is a closure, not a returned reference: the filesystem lives in its own table behind its own lock, and a borrow could not outlive that guard.
- `with_backing_pair` serves `rename` and `link` across two vnodes, and refuses two different mounts (`CrossDevice`).

**Which filesystem a child is on** is always its parent's. Taking a node's type from anywhere other than the node itself (for example a mount table that numbers slots from zero for each filesystem) is the bug that stamped btrfs files `Ext4`.

---

## The registry and mounting

### The registry

[`kernel/fs/registry.rs`](../../kernel/fs/registry.rs):

- `PROBERS`: `ext` (`ext::mount::probe_ext`) and `btrfs` (`btrfs::mount::probe_btrfs`), asked in order by `probe(drive, start_lba)`, each returning an `FsProbe` (type, drive, start LBA, block size, version, label).
- `with_mounted(fs_type, slot, f)`: the one place that maps a mount slot to the code that serves it. ext2/3/4 go to `ext::mount::with_ext2_for`, btrfs to `btrfs::mount::with_btrfs_for`, memfs to `memfs::with_memfs_for`. tmpfs, devfs and procfs return `None`.
- `busy(fs_type)` and `wait_idle(fs_type)`: whether that filesystem's `STATE` lock is held, and a wait for it without keeping it (used by `give_way`, below).

Adding a filesystem is a prober line, an arm in `with_mounted`, and an implementation of `FileSystem`.

### Mount tables

There are two kinds:

- **The VFS mount table** (`MountTable`, [`mount.rs`](../../kernel/fs/vfs/mount.rs), `MAX_MOUNTS` = 8) records the in-memory trees made at boot: tmpfs at `/`, devfs at `/dev`, procfs at `/proc` (read-only).
- **Each disk filesystem's own slot table**: ext's `STATE` in `kernel/fs/ext/mount.rs`, btrfs's in `kernel/fs/btrfs/mount.rs`, memfs's in `kernel/fs/memfs.rs`. A slot holds one mounted filesystem behind that table's `SchedMutex`.

### Mounting is a graft

There is no `mount(2)`. Mounting is done by kernel-shell commands (see [storage.md](storage.md)) and at boot. A mount **grafts** a filesystem onto a VFS directory vnode: the vnode's `fs_type`, slot (`ext2_slot`) and root inode number (`ext2_ino`) are overwritten in place, its cached children are evicted, its negative entries dropped, and `children_loaded` cleared (`MikuVFS::graft_ext_dir`, `mount_btrfs_to_vfs`). From then on, lookups in that directory go to the filesystem.

- No covered vnode is kept underneath. Unmounting resets the directory to tmpfs rather than revealing what was there.
- **Crossing a mount needs no special step**: the mount point *is* the mounted root (`effective_node` is the identity). `..` from a mounted root goes to the vnode's parent, the tmpfs directory it was grafted under.
- **The installed root** is grafted directory by directory: `graft_root_dir("bin")` and its siblings graft the root disk's top-level directories onto the tmpfs `/` (`kernel/fs/ext/mount.rs`). The firmware GRUB module is mounted read-only and grafted at `/lib/firmware`. Disk mounts start read-only; see [storage.md](storage.md) and [boot.md](boot.md) (`root=`, `rootrw`).
- `/proc/mounts` lists tmpfs, devfs, procfs and every ext slot with its path and `ro`/`rw`.

---

## Vnodes

A vnode (`VNode`, [`vnode.rs`](../../kernel/fs/vfs/vnode.rs)) is the VFS's inode: one per file, however many names lead to it.

| Field | Meaning |
|:--|:--|
| `id` | Its number in the table (`InodeId = u32`; `INVALID_ID = u32::MAX`) |
| `parent`, `name` | Its first name (`NameBuf`, up to 255 bytes) |
| `aliases` | Other cached names of a hard-linked file: `(directory, name)` |
| `kind` | `Regular`, `Directory`, `Symlink`, `CharDevice`, `BlockDevice`, `Pipe`, `Socket`, `Fifo` |
| `fs_type`, `ext2_slot`, `ext2_ino`, `mount_id` | Which filesystem and which node on it |
| `mode`, `uid`, `gid` (16-bit), `size`, `nlinks`, `atime`/`mtime`/`ctime`/`btime` | Attributes (times are Unix seconds) |
| `refcount` | `AtomicU32`: descriptors, working directories and in-flight operations holding it |
| `used`, `referenced`, `lru_prev`/`lru_next`/`on_lru` | Recency for eviction |
| `children`, `children_loaded` | The directory's name table, and whether it holds every name |
| `addr_space` | tmpfs pages of the file |
| `symlink_target` | Boxed, up to `SYMLINK_MAX` (4096) bytes |
| `dev_major`, `dev_minor` | Device numbers |
| `flags` | `dirty`, `immutable`, `append_only`, `no_atime`, ... |
| `attrs_fresh` | Size and times are known to be the filesystem's (see [Attributes](#attributes-and-permissions)) |
| `static_data` | Read-only bytes in the kernel image (embedded system libraries), served without the page cache |

### The table

`MikuVFS.nodes` is a `Table<VNode>` ([`table.rs`](../../kernel/fs/vfs/table.rs)):

- slots in chunks of 512 pointers, so growing never moves anything;
- a stack of free ids;
- a dead slot reads as a static empty vnode, so `nodes[id].active` checks stay valid;
- each vnode is allocated from the typed heap cache `KCache<VNode>` named `vnode` (visible in `/proc/slabinfo`).

There is no fixed size: the table holds what memory allows (200,163 vnodes in a 2026-10-27 test) and gives memory back through the vnode shrinker. `/proc/meminfo` (`Vnodes:`), `/proc/stat` (`vnodes:`) and `info` show the live count.

**Only `free_vnode(id)` frees a vnode.** It takes every name it still has out of its directories, drops its file-cache entry, pipe state and watches, and returns the memory. Nothing else clears `active`.

**Holders take counted references.** A descriptor, a working directory (`change_cwd`, inherited by `fork_fds`, released by `drop_fds`) and an operation in flight hold `refcount`. A file mapping is counted by `mm::mmap` (`MAPPINGS`, `is_mapped`) and a watch by `notify` (`is_watched`), because those tables live under locks the VFS lock cannot be taken under. Reclaim asks each about one vnode in O(log n).

**`drop_ref`** lets go of one reference. A vnode with no name (an anonymous pipe end or inotify instance) is freed by its last reference; an unlinked file is finished (`finish_unlinked`) when its last descriptor and its last mapping are gone. Every descriptor drop goes through it (close, `dup2` over, the close-on-exec sweep, exit, a descriptor in flight over a dying socket), which is what keeps pipes from leaking (fixed 2026-09-26).

### Stable st_ino

| Vnode | `st_ino` / `d_ino` |
|:--|:--|
| On a disk filesystem | `((1 + mount_key) << 32) | inode_number`: the same for every name of a file and after an eviction |
| tmpfs, devfs, procfs | The vnode id, which never changes because those vnodes are never evicted |

The mount is in `st_ino`, not `st_dev`, because `std` reports `dev` as 0 and `libc_miku` reports the filesystem type there.

---

## Names: entries, aliases, negative entries, whiteouts

A directory's names are a hash table in the directory vnode ([`children.rs`](../../kernel/fs/vfs/children.rs)): open addressing with tombstones, rebuilt in place when tombstones and entries fill three quarters of it. Each occupied slot is one of:

| Entry | Meaning |
|:--|:--|
| **Positive** | A name and the vnode it names |
| **Negative** | A name the filesystem does not have. The next lookup is answered here |
| **Whiteout** | A name still on the disk whose file was unlinked while open. Lookups and listings do not see it, a listing does not re-read it from the disk, and a create or rename onto it is refused until the last close |

`probe(name)` answers `Node(id)`, `Absent` (negative or whiteout) or `Unknown`.

- **Complete directories.** `children_loaded` says every name is cached. A miss in a complete directory is answered with no entry and no disk read at all (except `lost+found`, which listings omit). Reading a directory whole with `getdents` makes it complete, so `find`-style walks ask the filesystem for no name after the listing.
- **Negative entries** are left by a lookup that missed on a disk; a create, a rename onto the name or a listing replaces them. At most `NEGATIVE_PER_DIR` (256) per directory; reaching it drops them all. All are dropped when the directory becomes complete.
- **The icache** (`MikuVFS::icache`, `(mount, inode) -> vnode`) indexes disk files that have more than one name. A lookup or listing that meets such a file by another name adds an alias to the existing vnode instead of making a second vnode with its own size and pages. A file with one name is found by that name and costs the icache nothing.
- **Unlink, rename and link act on the entry named in the path**, not on the vnode's first name.
- **Counters** in `/proc/stat` (`dcache:`): lookups that reached a filesystem, misses answered by a negative entry, misses answered by a complete directory, the icache size.
- **Changes behind the VFS** (the kernel shell's `ext*` builtins write by inode number) call `changed_below_vfs()`: every file's cached pages go, and every disk directory forgets its negative entries and completeness.

---

## Vnode eviction

[`kernel/fs/vfs/core/reclaim.rs`](../../kernel/fs/vfs/core/reclaim.rs), [`lru.rs`](../../kernel/fs/vfs/lru.rs).

**When:**

- the vnode shrinker (`VNODE_SHRINKER`, "vnode cache") when free frames are under a sixteenth of RAM (kswapd) or an allocation found none (direct, `try_lock` only); see [memory.md](memory.md);
- inside an operation, when allocating a vnode fails.

**How:** every vnode joins the head of an intrusive LRU when made, and a lookup that finds it moves it to the head (a reader under the read lock sets the atomic `referenced` instead, a clock-style second chance). Reclaim reads the tail and gives each vnode a verdict:

| Verdict | When |
|:--|:--|
| **Evict** | On a mounted filesystem, with an inode number, linked, not a mount point, held by nothing, a leaf of the cached tree |
| **Busy** (back to the head) | A descriptor, a working directory, a mapping or a watch holds it; it has dirty pages in the file cache; it was `referenced`; it was touched in the last 1024 operations (`PROTECT_OPS`); a directory naming it is open (being listed); it has cached children or a whiteout |
| **Never** (off the list) | tmpfs, devfs, procfs (the vnode is the only copy); an unlinked file (freed by its last holder); a mount point |

- A directory with cached children is busy; once they are evicted it is a leaf and goes on a later pass. Negative entries do not count.
- A dirty vnode *flag* does not spare it (the filesystem holds everything it was given, times included); dirty *pages* do.
- One request looks at no more than twice the list. The shrinker evicts in batches of 64 (`RECLAIM_BATCH`) and lets the VFS lock go between them (one hold was 58-85 ms on 20,712 names before 2026-10-28).
- Evicting a vnode removes every name of it from its directories, which are then no longer complete.

---

## The path walk

### resolve_path and friends

[`path_resolve.rs`](../../kernel/fs/vfs/core/path_resolve.rs). The walk is written once over a `Names` trait with two implementations:

| Walker | Lock | A miss |
|:--|:--|:--|
| `Writer` | `VFS_LOCK` held to write | Asked of the filesystem (`lookup_child_or_load` -> `lazy_lookup_backed`), or deferred when `vfs.defer` is set |
| `Reader` | `VFS_LOCK` held to read | `Deferred`: only the cache is consulted, with the caller's own credential (`check_search_as`) |

Entry points: `resolve_path(cwd, path)` (follows every symlink), `resolve_path_lstat` (not the last component), `resolve_path_follow` (also follows a symlink the result is), `split_path` (parent and last name).

Rules:

- An absolute path starts at vnode 0 (`/`), a relative one at the base directory.
- **Search permission** (`x`) is checked on every directory before descending and before climbing out through `..`.
- `..` goes to the vnode's `parent`; at `/` it stays.
- At most 32 non-dot components (`MAX_PATH_DEPTH`), else `InvalidPath`. Names up to 255 bytes (`NAME_LEN`).
- Symlinks: at most 8 levels (`MAX_SYMLINK_DEPTH`), else `TooManySymlinks` (`ELOOP`). A relative target is walked from the link's directory. A target that is not UTF-8 or is empty is `InvalidPath`.
- `abs_path(id)` walks parent links back to `/`. It is what `getcwd` (8), `realpath` (119) and `/proc/mounts` answer with; a corrupt parent cycle is an error, not a shortened path.

### Deferred misses: disk reads outside the VFS lock

A lookup that misses the cache, or a listing of a directory that is not complete, used to read the disk with `VFS_LOCK` held. Now (`with_vfs_walk`, `walk_rounds`):

1. Under the lock, `f` runs with `vfs.defer = true`. A walk that needs the filesystem records a plan (which mount, which node, the directory's `gen`, a reference on the directory) and returns `VfsError::Deferred`.
2. The lock is released. `fill::fetch` does `fs.lookup` or `fs.readdir` plus `getattr` under the filesystem's own slot lock.
3. Under the lock again: if the directory is still the same node and its `gen` has not moved, the result is inserted (a listing in parts of 128 names, `LISTING_PART`, with the lock released between parts; only the last part marks the directory complete). Then `f` runs again from the start.

Up to 48 rounds (`WALK_ROUNDS`); after 3 rounds in a row whose reads were thrown away (`WALK_STALLS`) the walk reads under the lock. So `f` must change nothing before it walks. Calls that walk this way: `stat`, `lstat`, `readlink`, `getdents`, `open` (through `with_vfs_read`'s fallback), and creates and unlinks through `with_vfs_disk`. `Deferred` never reaches a program. `walklocked` on the boot line restores reading under the lock.

### *at calls and the path word

Every path argument is a pointer plus a 64-bit **path word** (`usercopy::read_user_path_at`):

| High 32 bits | Low 32 bits | Base |
|:--|:--|:--|
| 0 | length | The current working directory |
| `n` (non-zero) | length > 0 | The directory open on descriptor `n - 1` |
| `n` (non-zero) | 0 | The object open on descriptor `n - 1` itself (an empty path) |

That is how libc's `openat`, `fstatat`, `unlinkat`, `mkdirat`, `renameat` and friends are expressed without new system calls. `realpath` (119) with an empty path and a descriptor base returns that descriptor's path; libc's `fcntl(F_GETPATH)` (50, libc-only, not the kernel's `fcntl` 113) is built on it. See [MikuOS_ABI.md](../MikuOS_ABI.md).

---

## File descriptors

### Tables and shards

[`kernel/fs/vfs/fd.rs`](../../kernel/fs/vfs/fd.rs).

- One `FdTable` per thread group (all threads of a process share it), at most 512 descriptors (`MAX_OPEN_FILES`); allocation takes the lowest free number. Sockets live in a separate range handled by the network stack (see [network.md](network.md)).
- Tables are kept in **64 shards** by tgid, each a `SchedMutex` named `fd`. **A shard is a leaf**: nothing is locked under it, and a guard lives for one statement. Taking it twice from one task panics naming the fd module rather than hanging.
- `TABLES[tgid]` publishes a pointer to each table, so hot paths find it without the shard.

### Lockless reads

A slot is atomics:

- what it names (vnode, flags, route, open) changes only under the shard lock, inside a per-slot sequence count (odd while being written); a reader copies the fields and re-reads the count;
- `pos` holds the offset in its low 48 bits and the slot's generation in its high 16. Every open, dup or close of the slot moves the generation, so a compare-and-swap of the offset for a descriptor that was closed and reused fails instead of moving the new file's offset;
- a removed table is freed only after a grace period: every CPU has switched tasks at least once or is idle (`fd-retired`). Ring 0 is not preempted and an unlocked reader never sleeps holding the pointer, so that is enough.

Lockless users: `route`/`classify`, `cached::read_hit`, `seek_fd` from the start or the current position, `fd_is_nonblocking`, `fd::peek`, `fd::advance` (tmpfs reads under the read lock).

### OpenFile and routes

`OpenFile { vnode_id, flags, offset, active, route }`. `route` says what the system-call layer does with the descriptor:

| Route | |
|:--|:--|
| `Unknown` | Not worked out yet; the first `classify` computes it under the VFS lock and stores it |
| `Vfs { cached }` | A file, directory or device the VFS answers; `cached` when its pages go through the disk page cache |
| `Pipe`, `Inotify`, `NetRaw` (`/dev/net0`), `Tty` (the caller's controlling terminal, resolved per call), `Console`, `PtySlave(n)`, `PtyMaster(n)` | Handled outside the VFS lock |

A route cannot go stale: it is a property of the vnode the descriptor references, and a reused descriptor is a new `OpenFile`. So pipes, terminals, ptys and inotify never take `VFS_LOCK` to read or write.

### Semantics

- **No shared open file description.** `dup`, `dup2`, `F_DUPFD` and `fork` copy the descriptor entry, **offset included at that moment**; afterwards each has its own offset. Two descriptors on one file are "the same file" when they name the same vnode (this is how libc answers `kcmp(KCMP_FILE)`).
- **Close-on-exec** belongs to the descriptor: `dup`, `dup2` and `F_DUPFD` clear it, `F_DUPFD_CLOEXEC` sets it. `execve` closes every `O_CLOEXEC` descriptor (`close_cloexec_fds`), releasing their record locks, and wakes pipe readers whose last writer went.
- **`fork`** copies the table and takes a reference on every vnode and on the working directory.
- **Exit** releases descriptors as soon as a process exits (`release_fds_on_exit`), not at reap: a zombie holding a pipe's write end used to keep the reader waiting for ever.
- **Offsets**: `lseek` `SEEK_SET`/`SEEK_CUR` touch only the descriptor; `SEEK_END` asks the VFS for the size. A shared descriptor's concurrent reads still come one after another: an unlocked read that finds the offset moved redoes the read under the lock.
- **`O_APPEND`** writes at the VFS's idea of the end (including dirty pages not yet written).

**Open flags** (MikuOS values in [`types.rs`](../../kernel/fs/vfs/types.rs); libc translates POSIX names):

| Flag | Value | Flag | Value |
|:--|:--|:--|:--|
| `READ` | 0x0001 | `NOFOLLOW` | 0x0080 |
| `WRITE` | 0x0002 | `DIRECT` | 0x0100 |
| `RDWR` | 0x0003 | `SYNC` | 0x0200 |
| `APPEND` | 0x0004 | `NONBLOCK` | 0x0400 |
| `CREATE` | 0x0008 | `CLOEXEC` | 0x0800 |
| `EXCLUSIVE` | 0x0010 | `NOATIME` | 0x1000 |
| `TRUNCATE` | 0x0020 | `DIRECTORY` | 0x0040 |

An `open` with flags 0 means read-only; mode 0 means the default file mode.

**fcntl** (113): `F_DUPFD` (0), `F_GETFD` (1), `F_SETFD` (2, `FD_CLOEXEC` = 1), `F_GETFL` (3), `F_SETFL` (4; only `O_APPEND` and `O_NONBLOCK` can change), `F_GETLK`/`F_SETLK`/`F_SETLKW` (5/6/7), `F_DUPFD_CLOEXEC` (1030). Anything else is `EINVAL`. On a socket descriptor, `F_GETFL`/`F_SETFL`/`F_SETFD`/`F_DUPFD_CLOEXEC` are answered by the socket layer.

---

## The page cache for disk files

[`kernel/fs/vfs/filecache.rs`](../../kernel/fs/vfs/filecache.rs), [`core/cached.rs`](../../kernel/fs/vfs/core/cached.rs). ext and btrfs files only; memfs is memory already and tmpfs pages are their own storage.

### Entries

`FILE_CACHE` is a `SchedRwLock` (`filecache`) holding a map from vnode id to `Entry`:

| Field | Meaning |
|:--|:--|
| `pages` | The file's pages: an index into the cache's own `PageCache` of frames (evictable, not counted against tmpfs) |
| `gen` | Moved by every change to the file's data: a VFS write, truncate, `fallocate`, punching a hole, dropping the entry. A fill that started under another `gen` inserts nothing |
| `next_page` | Where the last read ended, for readahead (atomic) |
| `stamp` | Size and mtime the filesystem gave when the pages were read; `None` once the VFS itself wrote the file |
| `size` | The file's size as the cache knows it: what an unlocked read stops at |

Per vnode it also keeps the dirty page numbers, each with the sequence number of its last write, and when the vnode first went dirty. `DIRTY_PAGES` is the global count.

**Sizes:** the cache may hold half of RAM (`cache_limit`, at least 256 pages). When a fill needs room it evicts the coldest whole files until a sixteenth of the cap is free. The dirty limit is an eighth of the cap.

### Reads

1. **Unlocked hit** (`cached::read_hit`), for a descriptor routed `Vfs { cached: true }`:
   1. copy the descriptor (vnode, offset, flags, route) with no lock;
   2. under `FILE_CACHE` held to read: check the vnode is still the file the descriptor named (`FORGOTTEN`, the count of vnodes ever freed, unchanged; else ask the shard), find every page the read covers, pin each frame (`pmm::try_ref`), account the hit and readahead. A file among the last 32 read keeps its LRU place (`TOUCH_SLACK`);
   3. with no lock: copy to the caller and unpin (`pmm::free_frame_cow`, which frees a frame the cache dropped meanwhile);
   4. move the offset by compare-and-swap only if it is still what step 1 read; otherwise the read is redone under the lock.
2. **Miss**, in three steps (`read_step`, `fill_and_insert`): under the VFS lock plan the pages and take a reference; without it read them through the filesystem under its own lock into fresh frames, 64 KiB per filesystem read; under the lock insert them if the vnode and `gen` are unchanged, then start over. Up to 4 fills (`FILL_ROUNDS`) before the read falls back to reading under the lock.
3. **Readahead**: a read starting where the last ended is sequential and its fill runs 32 pages past the request (`READAHEAD_PAGES`). One fill is at most 64 pages (`MAX_FILL_PAGES`).

In-kernel whole-file readers (firmware, wallpapers: `read_path`, `vfs_read::read_from_vfs`) do not go through the cache.

**Coherence with writers outside the VFS:** an `open` compares the filesystem's size and mtime with the entry's stamp and drops the pages on a difference (`cache_validate`). The kernel shell's inode-numbered `ext*` builtins drop everything (`changed_below_vfs`).

### Eviction

- **The cap** (above), by recency of whole files.
- **Memory pressure**: the "file pages" shrinker, first in the shrinker list.
- **The vnode goes**: its entry goes with it (last close of an unlinked file, vnode eviction, unmount).
- **Dirty pages are never evicted.** Eviction passes over a dirty vnode, vnode reclaim counts it busy, and `drop_file` (a change around the VFS, a failed open check) keeps its pages, which are newer than the disk.
- Pages of a dropped file are taken off whole (`address_space::Detached`) and freed 1024 at a time after the lock (`filecache::reap`).

### mmap

A file mapping's fault reads through the VFS (`read_at_vnode`) and so is served from the cache, but it gets a **copy** of the page, not the cache's frame: private mappings because they can be written, shared mappings because their write-back is per mapping (`msync`, `munmap`). Two `MAP_SHARED` mappings of a disk file are therefore not coherent with each other before `msync`. tmpfs shared mappings map the file's own frame. Details in [memory.md](memory.md).

`nopagecache` on the boot line turns the disk page cache off: every read goes to the filesystem under the lock.

---

## Write-back

[`core/writeback.rs`](../../kernel/fs/vfs/core/writeback.rs).

### What goes into the cache

A write (`cache_write`, under the VFS lock and `FILE_CACHE`) is cached as dirty pages when all of these hold:

- a regular file on a disk filesystem, not opened `O_SYNC`;
- the mount has room: its free space (from `statfs`, believed for one second, kept per mount in `wb-room`) exceeds twice the dirty pages plus the write;
- the write starts at or before the file's end;
- every page it touches is cached already, wholly covered by the write, or at or past the end (a fresh page, zeroed only where the write does not cover it).

Otherwise it goes through to the filesystem and gets its own error (`ENOSPC`, ...). The vnode's size, the entry's size and the mtime move as the write's would have, so the bytes between the disk's end and the VFS's end are always cached pages.

### Flushing

Three steps, like a fill:

1. **Plan** (VFS lock): up to 1024 dirty pages (`FLUSH_PAGES`), the vnode's mtime, a reference.
2. **Write out** (no VFS lock; the filesystem's slot lock taken per run): for each run of contiguous dirty pages up to 1 MiB (`RUN_PAGES` = 256), note each page's sequence number and pin its frame under `FILE_CACHE`, then hand the frames to `FileSystem::write_pages` (ext: `ext3_write_parts`, one transaction per run). The last page is cut at the file's end. Then `setattr` of the VFS's mtime, so `stat` after the flush says what it said before.
3. **Finish** (VFS lock): a page whose sequence number did not move is clean.

A write during step 2 moves its page's number and leaves it dirty for the next flush. A truncate takes the slot lock, so it lands between runs, never inside one.

### Who flushes

| Trigger | What |
|:--|:--|
| `fsync` (41) | The file's dirty pages (`flush_fd`), then the filesystem's own `fsync` |
| bdflush | Every 2 s (500 ticks), vnodes dirty for 5 s or more (`DIRTY_AGE_NS`, `writeback_old`), then each ext mount's periodic journal commit, then the block cache's dirty chunks (see [storage.md](storage.md)) |
| A writer over the dirty limit | After its write and before it returns (`balance`, Linux's `balance_dirty_pages`) |
| `sync`, `poweroff`/`reboot`, unmount | Everything (`writeback_all`) |
| A reader that goes around the cache | Writes the vnode out first under the VFS lock (`settle`): `read_path`, `path_size`, the locked fallback of a read or fault whose fill could not get frames |

`stat`, the `O_APPEND` end, the size `mmap` maps and a fill's size take the VFS's numbers while the vnode is dirty, so they need no flush.

**Truncate** drops dirty pages past the new end, writes out the rest, then truncates. Dirty pages are discarded without being written only with the file: an unlinked file's last holder, `O_TRUNC`.

**Errors**: a failed flush is logged, its pages are taken as written (the cache still reads them), and the next `fsync` on that file returns the error.

`writethrough` on the boot line sends every write straight to the filesystem.

---

## The tmpfs page cache

tmpfs file data are frames in `MikuVFS.page_cache` ([`pages.rs`](../../kernel/fs/vfs/pages.rs)), indexed per file by `AddressSpace` ([`address_space.rs`](../../kernel/fs/vfs/address_space.rs)): a two-level radix of index pages, 1024 index pages of 1024 data pages each, so files up to 4 GiB. A hole is a zero entry.

- A page is allocated (zeroed) when first written and freed when the file drops it.
- The whole of tmpfs may hold a quarter of RAM (`tmpfs_limit`); past it a write fails with `NoSpace`.
- No eviction and no swap: a tmpfs page has nowhere to go.
- `MAP_SHARED` maps the page's own frame (`shared_frame`), so mappings, `read` and `write` see one page. Frames are reference-counted through the frame allocator's copy-on-write counts.
- Reads of regular tmpfs files run under the read lock (`read_as`), copying from the pages with no `&mut`; an access-time update that relatime would make, or an offset advance that lost to another reader, defers to the locked read.
- Emptying a big tmpfs file detaches its pages and frees them 1024 at a time after the lock (`reap_detached`).

See [filesystems.md](filesystems.md) for tmpfs as a filesystem.

---

## Directory listings and getdents

`getdents` (111) takes an open directory, a buffer, a maximum count and an **offset that is an entry position**: the caller advances it by the number returned and stops at zero. `readdir` (23) is the older path-based form. Both write entries of `UDIRENT_SIZE` bytes (name up to 255 bytes plus terminator, inode, kind, name length); see [MikuOS_ABI.md](../MikuOS_ABI.md).

- **At most 120 entries a call** (`DIRENTS_PER_CALL`): 120 entries of 272 bytes are the heap's largest slab. libmiku and `std` ask for 128 and come back.
- **Filled in place**: only the name bytes, terminator and fields after it are written; the rest of the user's buffer is left alone.
- **Cookies**: the directory's table remembers where the last listing stopped (position and slot, `Children::resume_at`, an `AtomicU64` with a generation) for as long as its positive entries have not moved (`gen`), so a listing in passes does not restart from the top each call.
- **Complete vs partial**: a complete directory is listed under the read lock (`readdir_at_as`). One not complete yet is read from the filesystem with the VFS lock released (deferred), put in 128 names per hold, and marked complete by the last part.
- A directory fills as many names as a quarter of free memory holds at about a kilobyte per name (at least 4096), so a corrupt directory claiming millions of entries cannot take all of memory.
- Listing makes a vnode for every name (the cursor counts through cached children); see [Known gaps](#known-gaps).

---

## Locking

### VFS_LOCK

`VFS_LOCK` is a `SchedRwLock` named `vfs` ([`core/mod.rs`](../../kernel/fs/vfs/core/mod.rs)); see [scheduler.md](scheduler.md) for the lock type. Entry points:

| Function | Mode | Use |
|:--|:--|:--|
| `with_vfs(f)` | write | Every call that changes the tree. Copies the caller's credential into `vfs.ctx`, counts the operation (`vfs.op`), releases unmapped unlinked files, and after unlocking: finishes a `late_truncate`, drains big deletions, reaps detached pages, wakes inotify readers, runs terminal hangups |
| `with_vfs_read(read, write)` | read, then write | `read` beside other readers with the caller's credential; `None` (from `Deferred`) re-runs as `write` under `with_vfs_walk` |
| `with_vfs_read_only(read)` | read | For callers with their own fallback |
| `with_vfs_walk(f)` | write, in rounds | Disk reads for names with the lock released |
| `with_vfs_disk(f)` | write, in rounds | `with_vfs_walk` for calls that change a disk, which also give way to the filesystem's lock |
| `with_vfs_ro(f)` | write | Legacy read-only closure |

**Read side (beside other readers):** `stat`, `lstat`, `readlink` (`stat_as`, `readlink_as`); `open` without `O_CREAT` or `O_TRUNC` of a file the cache can answer (`open_as`; a disk file whose cached pages may be stale, a device, a pipe or a pty defers); `getdents` of a complete directory; reads of regular tmpfs files; `close` of a file or directory (`close_as`). What a reader may change is atomics only: `VNode::referenced`, `refcount` increments, name-cache counters, the listing cursor.

**`close_as`**: the descriptor is removed from its table, record locks and the inotify close event need nothing more, and the count drops with `dec_ref_quietly` when another holder is left or the file keeps a name. The last reference to an unlinked or nameless vnode comes back as `Closed::Last(vid)`, and `drop_ref` lets go of it under the write lock. Pipe ends and terminals close the old way.

**Write side:** everything else: creates, unlinks, renames, links, `mkdir`, `chmod`/`chown`, writes, reads that miss the cache, `access`, `fstat`.

**No VFS lock at all:** cached reads of disk files (`read_hit`), `lseek` except `SEEK_END`, descriptor routing, pipes, terminals, ptys, inotify reads, `O_NONBLOCK` checks.

### Giving way to ext

A call run by `with_vfs_disk` (an `open` with `O_CREAT`, `unlink`) asks `MikuVFS::give_way(id)` before it changes anything: if the filesystem's `STATE` lock is held (`registry::busy`), it returns `Deferred`, the VFS lock is released, the filesystem lock is waited for without it (`registry::wait_idle`), and the call runs again. After two such rounds it waits under the VFS lock. `/proc/lockstat` counts calls that gave way and calls that waited under the VFS lock anyway.

### Lock order

From outermost to innermost:

1. `VFS_LOCK` (read or write);
2. a filesystem's slot lock (`registry::with_mounted`: ext `STATE`, btrfs `STATE`, memfs);
3. `FILE_CACHE` (read or write);
4. a descriptor shard (a leaf).

Also leaves: `unlinked` (pending deletions), `wb-room` (free space per mount), `fd-retired`. The inotify table and the record-lock table are taken inside the VFS lock, never the other way round. A write-back flush and a big file's deletion take a slot lock and `FILE_CACHE` inside it with no `VFS_LOCK`. **Nothing takes `VFS_LOCK` while holding `FILE_CACHE`, a slot lock or a shard.** A file fault takes the VFS lock to read, so kernel code reaches file pages through `fault_in_user_range` before taking any VFS lock.

### Boot flags

| Flag | Effect |
|:--|:--|
| `vfsmutex` | Every `with_vfs_read` call goes to its write path: the VFS lock is effectively a mutex again |
| `walklocked` | Every walk reads the disk under the lock (no deferred misses) |
| `lockstat` | Time `vfs`, `fd`, `filecache`, `ext`, ... holds; `/proc/lockstat` (see [diagnostics.md](diagnostics.md)) |
| `nopagecache` | No disk page cache |
| `writethrough` | No write-back |

### Longest hold

The longest `vfs` hold in a whole stdbench run on ext4 was 0.28-0.40 ms on 2026-10-28, none over 1 ms, with zero device writes under the VFS lock (from 37-58 ms in the first measurement on 2026-10-27). In `target/heapwork/bench/miku-r3.txt` it was 283 us, taken at `kernel/syscall/adapters/io.rs:190`, with 15 disk reads under the VFS lock totalling 473 us.

---

## Unlinking and truncating big files

[`core/unlinked.rs`](../../kernel/fs/vfs/core/unlinked.rs). For a regular disk file of at least 256 KiB (`DRAIN_MIN`):

1. Under the lock: the name stays hidden behind a whiteout, one reference keeps reclaim off the vnode, and it goes on a pending list.
2. After the lock, by whoever released it (`with_vfs`'s epilogue): the filesystem looks the name up and, if it is still this file's, removes it and frees the inode under its own slot lock only.
3. Under the lock again: the whiteout becomes a negative entry, cached pages and the vnode are freed.

The whiteout keeps creates and renames off the name meanwhile (`EEXIST` for those milliseconds). An `unlink` of a file nobody had open does all three steps before returning.

**A file unlinked while open** keeps its name on the disk until its last close (the filesystems have no orphan list); the directory carries a whiteout, a create of that name is refused, and the last close removes the name if it still names this inode.

**`O_TRUNC` of a big ext file** (`late_truncate`): the VFS side (size 0, pages and dirty pages gone) is done under the lock; `with_vfs` takes ext's lock before releasing the VFS lock and truncates with ext's lock alone. Anything that would touch that disk takes ext's lock and so comes after.

---

## inotify

[`kernel/fs/vfs/notify.rs`](../../kernel/fs/vfs/notify.rs), syscalls in [`adapters/notify.rs`](../../kernel/syscall/adapters/notify.rs). Linux's ABI byte for byte: `struct inotify_event { i32 wd; u32 mask; u32 cookie; u32 len; char name[] }` records and Linux's `IN_*` values.

| Call | No. | Arguments |
|:--|:--|:--|
| `inotify_init1` | 116 | `flags`: `IN_NONBLOCK`, `IN_CLOEXEC` (MikuOS's `O_NONBLOCK`/`O_CLOEXEC`); anything else `EINVAL` |
| `inotify_add_watch` | 117 | `fd, path_ptr, path_word, mask`; honours `IN_DONT_FOLLOW`, `IN_ONLYDIR`, `IN_MASK_ADD`, `IN_MASK_CREATE` (both together is `EINVAL`), `IN_ONESHOT`, `IN_EXCL_UNLINK` (kept in the mask) |
| `inotify_rm_watch` | 118 | `fd, wd` |

An instance is an anonymous vnode (in no directory, `nlinks` 0); it is destroyed when its last descriptor reference goes (`VNode::dec_ref`). Reading it returns queued events; `poll` reports it readable.

**Events raised** (in `kernel/fs/vfs/core`, once each operation has succeeded):

| Operation | Events |
|:--|:--|
| create, mkdir, symlink, link | `IN_CREATE` on the parent |
| unlink, rmdir | `IN_DELETE` on the parent; `IN_DELETE_SELF` on the victim once its last name is gone, then `IN_IGNORED` and the watch goes |
| rename | `IN_MOVED_FROM`/`IN_MOVED_TO` paired by cookie, `IN_MOVE_SELF` |
| write, truncate | `IN_MODIFY` |
| chmod, chown, `setattr` (times included) | `IN_ATTRIB` |
| close | `IN_CLOSE_WRITE` / `IN_CLOSE_NOWRITE` (regular files) |

An event on a file goes to its own watches (no name) and its directory's (with the name). With no watch anywhere each raise is one atomic load. Events are queued under the VFS lock and readers are woken after it.

**Limits:** 128 instances on the machine (`MAX_INSTANCES`), 8192 watches per instance (`MAX_WATCHES`), 4096 queued events per instance (`MAX_QUEUED`); overflow drops the event and queues one `IN_Q_OVERFLOW`.

**Not raised:** `IN_ACCESS`, `IN_OPEN`, `IN_UNMOUNT`; writes through a shared mapping; the inode-numbered calls that go to ext past the VFS (`utimensat` (40), the xattr calls 38/39/81/82). A watch cannot fire for a stranger: freeing a vnode drops any watch on it.

`stdapps/src/stdwatch.rs` drives the same `inotify` crate and mio registration that Zed's `notify` backend uses.

---

## Record locks

[`kernel/fs/vfs/lock.rs`](../../kernel/fs/vfs/lock.rs). POSIX advisory byte-range locks through `fcntl` `F_GETLK` (5), `F_SETLK` (6), `F_SETLKW` (7), with a 32-byte `struct flock` (`l_type`: `F_RDLCK` 0, `F_WRLCK` 1, `F_UNLCK` 2). SQLite's locking is built on exactly this.

- Keyed by (vnode, process): two descriptors on one file in one process share its locks; locks of one process never conflict with each other; a new lock replaces what the process held over the same bytes, splitting ranges.
- **Closing any descriptor on a file releases all of that process's locks on it**; the close-on-exec sweep does the same; exit releases everything the process held.
- `l_len == 0` means to the end of the file however it grows.
- `F_SETLKW` waits on a queue with `wait::join` (lost-wake free).
- One global table behind a spin lock.

**Not implemented:** `flock(2)` (no whole-file `flock` locks at all), deadlock detection for `F_SETLKW` (a waiter that would complete a cycle waits until a signal ends it instead of getting `EDEADLK`), and Linux's open-file-description locks (`F_OFD_*`).

---

## Pipes and FIFOs

[`kernel/fs/vfs/pipe.rs`](../../kernel/fs/vfs/pipe.rs). `pipe` (34) and `pipe2` (120).

- **The ring**: 64 KiB (`CAPACITY`) with a lock of its own, allocated by the first write. Bytes move under that lock only, never `VFS_LOCK`.
- **Two anonymous vnodes per pipe**, one per end, made by `anon_node` (not in any directory: pipes used to be `/_pipe_N` files visible to anyone). An end's vnode reference count is its number of descriptors, kept through `fork`, `dup`, `close`, close-on-exec, exit and descriptor passing; `VNode::inc_ref`/`dec_ref` call `pipe::ref_changed`.
- **Writes**: up to 4096 bytes (`ATOMIC_WRITE`, `PIPE_BUF`) go in whole or wait; longer ones go in as room appears; a non-blocking write returns what fit; a write with no reader left raises `SIGPIPE`, then returns `EPIPE`.
- **Waiting**: both ends wait on one key, the read end's (`pipe::wait_key`), with `wait_any_unless`, so a write or close between a look and the sleep is never lost.
- **End of file**: closing the last write end wakes the reader; this is done by `close`, exit and the close-on-exec sweep (the last was missing once, which cost every `Command::spawn` a 100 ms backstop).

**FIFOs**: `VNodeKind::Fifo` exists, but named FIFOs on a disk are not pipes here: their bytes go to the inode. There is no working `mkfifo`.

---

## Attributes and permissions

### stat

`VNodeStat` has: `ino` (stable, above), `kind`, `mode`, `size`, `blocks` (512-byte units), `nlinks`, `uid`, `gid`, `fs_type`, device major/minor, `atime`, `mtime`, `ctime`, `btime`. Times are whole Unix seconds (the wall clock, or seconds since boot before it is set). The user-visible layout is in [MikuOS_ABI.md](../MikuOS_ABI.md).

**`attrs_fresh`**: a disk vnode's size and times are taken from the filesystem when it is filled (`fill_vnode_from`) and by `getattr`, which set the flag. Any call into the filesystem about the node, a page fill (atime) and a change around the VFS clear it. `stat` and `open`'s cache check (`refresh_attrs_from_fs`, `cache_validate`) ask the filesystem only when it is clear. That keeps `stat` off ext's lock behind a listing or a journal commit.

**atime** follows Linux's relatime rule: updated when older than mtime or ctime, or more than a day old; not at all with `no_atime`/`O_NOATIME`.

### Permission checks

Search (`x`) on every directory of a walk, including before `..` ([`core/access.rs`](../../kernel/fs/vfs/core/access.rs)). Opening checks read/write bits against the open flags. Capabilities replace "uid 0 bypasses everything":

| Check | Rule |
|:--|:--|
| Read, search | Mode bits for owner/group/other (supplementary groups included), or the read-bypass capability |
| Write | Mode bits, or `CAP_DAC_OVERRIDE` |
| Execute | Mode bits; no capability grants execute on a file with no `x` bit anywhere |
| `chmod` | Owner or `CAP_FOWNER`; setgid is stripped unless the caller is in the file's group or has `CAP_FOWNER` |
| `chown` | Changing the user needs `CAP_CHOWN`; changing the group of a file you own to a group you are in needs nothing; set-id bits are stripped after any owner change, privileged or not |
| Read-only filesystems | devfs, procfs, and a read-only disk mount refuse with `ReadOnly` (`EROFS`) |
| `immutable` flag | `setattr` refused |

The privilege model is in [MikuOS_Security.md](../MikuOS_Security.md).

`utimensat` (40) and the xattr calls (38, 39, 81, 82) work by ext inode number and do their own permission check (`permissions::check_access`); see [filesystems.md](filesystems.md).

---

## System calls

All paths take the [path word](#at-calls-and-the-path-word). Full signatures are in [MikuOS_ABI.md](../MikuOS_ABI.md).

| No. | Call | VFS path |
|:--|:--|:--|
| 1, 2 | `write`, `read` | Routed by descriptor; disk-file reads try `read_hit` first |
| 8 | `getcwd` | `abs_path` of the working directory |
| 11 | `open` | `O_CREAT`: `with_vfs_disk`; otherwise `with_vfs_read` (`open_as`) |
| 12 | `close` | `close_fd` (`close_as`, then `drop_ref`) |
| 13 | `seek` | `seek_fd` |
| 14 | `fsize` | File size |
| 18, 19, 112 | `stat`, `fstat`, `lstat` | `stat`/`lstat` under the read lock |
| 20, 21, 22 | `mkdir`, `rmdir`, `unlink` | `unlink` through `with_vfs_disk` |
| 23, 111 | `readdir`, `getdents` | Listings |
| 24, 25, 32, 33 | `rename`, `link`, `symlink`, `readlink` | |
| 26, 27 | `chmod`, `chown` | |
| 28, 29, 113 | `dup`, `dup2`, `fcntl` | |
| 30, 37, 42, 41 | `truncate`, `fallocate`, `punch_hole`, `fsync` | |
| 31 | `write_file` | `write` that goes to the VFS only, never the console |
| 34, 120 | `pipe`, `pipe2` | |
| 35 | `chdir` | `change_cwd` |
| 36 | `statfs` | The filesystem's `statfs` |
| 38, 39, 81, 82 | `getxattr`, `setxattr`, `listxattr`, `removexattr` | ext by inode number |
| 40 | `utimensat` | ext by inode number |
| 60, 61 | `mmap_file`, `msync` | [memory.md](memory.md) |
| 116-118 | inotify | |
| 119 | `realpath` | |

`VfsError` values map to errnos in `syscall::errno::vfs_err`; for example `NotFound` -> `ENOENT`, `PermissionDenied` -> `EACCES`, `ReadOnly` -> `EROFS`, `NoSpace` -> `ENOSPC`, `TooManySymlinks` -> `ELOOP`, `CrossDevice` -> `EXDEV`, `NotSupported` -> `ENOSYS`, `NotMounted` -> `ENODEV`, `Corrupted` -> `EUCLEAN`, `NoTerminal` -> `ENXIO`.

---

## Boot flags

The full table is in [boot.md](boot.md). VFS flags: `vfsmutex`, `walklocked`, `nopagecache`, `writethrough`, `lockstat` (above).

---

## /proc counters

| File | VFS content |
|:--|:--|
| `/proc/stat` | `vnodes:` (live count); `dcache:` (filesystem lookups, negative hits, complete-directory misses, icache size); `filecache:` (hits, misses, fills, hits answered with no VFS lock, pages dirty now, pages dirtied, pages cleaned) |
| `/proc/meminfo` | `Vnodes:`, `TmpfsPages:` |
| `/proc/lockstat` | `vfs`, `fd`, `filecache`, `unlinked`, `wb-room`, `fd-retired`, `ext`, ...; then calls that gave way, calls that waited under the VFS lock, and disk reads/writes made under the VFS lock (count, total, longest) |
| `/proc/slabinfo` | The `vnode` cache |
| `/proc/mounts` | Mounted filesystems |

`/proc` files are read through an 8192-byte buffer. File-by-file formats are in [diagnostics.md](diagnostics.md).

---

## Selftests

`vfs test` in the kernel shell, or `run=vfs_test` on the GRUB line (see [testing.md](testing.md)). [`kernel/fs/vfs/selftest.rs`](../../kernel/fs/vfs/selftest.rs) makes a scratch ext4 filesystem (1 KiB blocks, 256-byte inodes) on a ramdisk, mounts it read-write, grafts it at `/vfstest`, runs the groups below, then repeats the filesystem-independent groups against a memfs mount.

| Group | Checks |
|:--|:--|
| `substantive` (ext4 and memfs) | A directory of more than 256 entries listed whole and marked complete (`big_directory`); a 200-byte name unlinked by its whole name (`long_name`); a 909-byte symlink target (`long_symlink`); cross-directory rename; ownership with ids above 16 bits (`wide_owner`); `statfs`; which filesystem answers for a node (`which_filesystem`); no contamination between two mounted filesystems (`no_contamination`); hard links as one vnode and one `st_ino`, the same after an eviction, unlinked-while-open whiteouts (`names_are_entries`); times from the disk (`times_from_the_disk`); walks that read the disk unlocked (`walks_read_unlocked`); big files deleted after the lock |
| `page_cache` (ext4) | Reads served from the cache, fills, eviction, coherence with `open`'s stamp check |
| `write_back` (ext4) | A read before the flush; one write per page; the disk has the data after a flush; mtime across a flush; a dirty vnode kept from both evictions; a reader around the cache; truncate across dirty pages; `O_APPEND`; unlink with dirty pages, then fsck clean |
| `descriptors_without_a_lock` | Lockless slot reads, generation-checked offsets |
| `readers_beside_a_renamer` | Three CPUs `stat` names under the read lock while a fourth renames, unlinks and creates them; every answer is a regular file or `NotFound` |
| `closers_beside_each_other` | Concurrent `close_as` |
| `a_create_gives_way` | ext's lock held for 50 ms; a create gives way and another task takes the VFS lock meanwhile without waiting |
| `truncate_after_the_lock` | `late_truncate` |
| `beyond_the_table` (memfs) | 200,000 files made and listed back, in one directory and across many |
| `listing_resumes` (memfs) | `getdents` resumes from the cursor |
| `the_cache_gives_back` (memfs) | Vnode memory returned after a shrink; a working directory, a mapping and a watch survive a full shrink |
| `mount_memfs`, `unmount_memfs` | Mounting and unmounting a memfs |

Baseline counts over time: vfs 52 (2026-10-27, heap stage), 56 (stage 4), 79 (stage 5), 100/100 after write-back (2026-10-28).

---

## Measured numbers

QEMU/KVM, 4 vCPUs, ext4 on `walk.img` over virtio unless noted; Linux is the dev box on bare metal. Sources: [`dev/vfs-locking.md`](../../dev/vfs-locking.md), [`dev/kernel-core.md`](../../dev/kernel-core.md), `target/heapwork/bench/miku-r3.txt` (2026-10-28).

| | Before | After (2026-10-28) | Linux |
|:--|:--|:--|:--|
| `stat`, short path, ext4 | 712 ns | 416-425 ns | |
| `stat`, 4 threads of one process, ext4 | 1.78 M/s | 4.70-4.86 M/s | |
| `stat` during a cold walk of 20,712 names, average | 3.9 us | 0.63 us | |
| cached `read` 4 KiB, 4 threads, ext4 | 1.58 M/s | 7.6 M/s | |
| tmpfs `read` 4 KiB, 4 threads | 2.5 M/s | 8.0 M/s | |
| `open`+`close`, 4 threads | 0.77 M/s | 1.4-1.5 M/s | 0.8-1.4 M/s |
| write 4 KiB in place, ext4 | 1.31-1.46 us | 0.44 us (`writethrough`: 1.3-1.8 us) | |
| create + write 1 KB, ext4 | 24.6 us | 11.6-11.9 us | |
| `unlink`, ext4 | 22 us | 9.8-10.3 us | |
| 64 MiB write + `fsync`, ext4 | 405-427 MB/s | 860-900 MB/s first write of a region, 1.4-1.5 GB/s again; 675-765 MB/s on a raw copy | |
| reading it back | 1.2 GB/s | 12.9-14 GB/s | |
| `read_dir`, per name | 514 ns | 140-147 ns | 130-142 ns |
| cold walk of 20,722 names in 2,999 directories (`stdfs --walk`, 2026-10-27) | | 146 ms cold, 54 ms warm, 0 lookups reaching ext | |
| 11,996 `stat`s of missing names after the walk | each a disk lookup | 13 ms, 0 reaching ext | |
| longest `vfs` hold in a bench run | 37-58 ms | 0.28-0.40 ms | |

Page cache on an emulated NVMe, 32 MiB file, 64 KiB reads (`stdfs --read`): cold 690-776 MB/s (856-860 with `nopagecache`), warm 9-11 GB/s (about 1 GB/s without).

---

## Known gaps

- **No lockless lookup** (RCU walk): a lookup takes `VFS_LOCK`, read or write.
- **Creates, unlinks, renames, links, `mkdir`, writes and cache misses run under the write lock**, ext's own work included (10-12 us each).
- **`open`+`close` from many threads of one process** stops at the descriptor table's lock (one per process, as Linux's `file_lock`).
- **Listing instantiates a vnode per name.**
- **No orphan list**: a file unlinked while open keeps its name on the disk until its last close, and that name cannot be created again until then (Linux unlinks at once and keeps the inode on the orphan list).
- **No shared open file descriptions**: `dup` and `fork` give independent offsets.
- **Disk-file `MAP_SHARED` mappings are not coherent** with each other until `msync`/`munmap`.
- **No `flock`, no OFD locks, no `F_SETLKW` deadlock detection.**
- **No named FIFOs, no `mount(2)`, no bind mounts, no overlay mounts.** A graft hides nothing underneath.
- **Paths of more than 32 components** are refused.
- **`utimensat` and xattr calls bypass the VFS** (no inotify events, ext only).
- **ext writes a new block's data synchronously**, a 1 MiB run at a time with its lock held (about 1.3 ms in QEMU); Linux issues it and lets the commit wait.

---

## Troubleshooting

| Symptom | Look at |
|:--|:--|
| A slow `stat` or `open` while something else uses the disk | `/proc/lockstat` with `lockstat`: which lock (`vfs`, `ext`, `filecache`, `fd`) waits and where it is held; the "disk reads/writes under the VFS lock" lines |
| Suspected VFS lock regression | Compare with `vfsmutex` (all calls on the write side) and `walklocked` |
| Data not on disk after a crash | Write-back holds data up to about 5-7 s (bdflush every 2 s, pages 5 s old); call `fsync`, or boot with `writethrough` to rule write-back out |
| Stale data after changing a disk with `ext*` builtins | Those call `changed_below_vfs`; anything else that writes around the VFS must drop the cache the same way |
| `EEXIST` creating a name just unlinked | A file unlinked while open is still open somewhere, or a big deletion is draining (milliseconds) |
| A walk returns `EINVAL` on a deep path | More than 32 components (`MAX_PATH_DEPTH`) |
| Vnodes keep growing | `Vnodes:` in `/proc/meminfo`; the shrinker runs only under memory pressure. Held vnodes (open descriptors, cwds, mappings, watches) are never evicted |
| A pipe reader never sees end of file | Some process still holds the write end (check close-on-exec flags on spawned children) |

Related: [filesystems.md](filesystems.md), [storage.md](storage.md), [memory.md](memory.md), [scheduler.md](scheduler.md), [diagnostics.md](diagnostics.md), [MikuOS_ABI.md](../MikuOS_ABI.md).
