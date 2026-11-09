// The VFS, tested on the machine
//
// `lib/fs/extfs`'s only contact with the world is one trait, so its tests run on
// the host. `MikuVFS` depends on the scheduler, credentials, the page cache and
// the block layer, so these checks are on the machine, driven through
// `MikuVFS`:
//
//   - a directory of more than 256 entries lists them all, and is not re-read
//     from the disk on every path resolution through it
//   - a name longer than 64 bytes stays whole, so unlinking it names the file
//     on the disk
//   - a symlink target longer than 64 bytes stays whole, as links into
//     `/usr/lib/...` need
//
// They run twice: against a real ext4 filesystem on a ramdisk, and against a
// `fs::memfs::MemFs`, a full `FileSystem` with a `BTreeMap` behind it. Which
// filesystem answers for a vnode is decided at every lookup, directory fill and
// write, and with one mounted every one of those decisions is trivially right.
// `no_contamination` below checks that, with no second disk.
//
// Reached as `vfs test` from the shell, or `run=vfs_test` on the GRUB line.

extern crate alloc;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::fs::ext::mkfs::{mkfs, FsType as MkfsType, MkfsParams};
use crate::fs::ext::mount;
use crate::fs::memfs;
use crate::fs::vfs::core::with_vfs;
use crate::fs::vfs::fsops::FileSystem;
use crate::fs::vfs::types::*;
use crate::kcore::selftest::{Report, Runner};

/// Big enough for a journal and three hundred files
const DISK_BYTES: usize = 8 * 1024 * 1024;

/// More than 256 entries
const MANY: usize = 300;

/// Where the scratch ext filesystem is grafted
const MNT: &str = "/vfstest";

/// And the memory one: a second path, not a second disk
const MEM_MNT: &str = "/memtest";

/// One mounted filesystem, and what it must say about itself.
///
/// The checks below take one of these, so the ext section and the memfs section
/// are the same code. A case name is a `&'static str` and both runs share it,
/// so every detail string starts with the mount point
struct Subject {
    mnt: &'static str,
    /// What `statfs` on it must report
    fs_type: FsType,
    block_size: u32,
}

/// A block device backed by a buffer this test owns. Leaked: the block layer's
/// registry holds no lifetime, and freeing the memory under a registered device
/// is a use-after-free
struct ScratchDisk {
    dev: BlockDevId,
}

impl ScratchDisk {
    fn new() -> Option<Self> {
        let buf = vec![0u8; DISK_BYTES].into_boxed_slice();
        let ptr = Box::leak(buf);
        let dev = crate::io::block::register_ramdisk_virt(ptr.as_ptr() as u64, DISK_BYTES as u64);
        if dev == INVALID_U8 {
            return None;
        }
        Some(ScratchDisk { dev })
    }
}

pub fn run() -> Report {
    let mut r = Runner::new();

    let Some(disk) = ScratchDisk::new() else {
        r.skip("scratch device", String::from("no free block device slot"));
        return r.report();
    };

    let mut params = MkfsParams::new(MkfsType::Ext4, disk.dev as usize);
    params.total_sectors = (DISK_BYTES / 512) as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    if let Err(e) = mkfs(crate::fs::host::KernelHost::new(disk.dev), &params) {
        r.check("mkfs a scratch ext4", false, format!("{:?}", e));
        return r.report();
    }
    if !mount::mount_dev(disk.dev) {
        r.check("mount the scratch filesystem", false, String::from("refused"));
        return r.report();
    }
    r.check("mount the scratch filesystem", true, String::from("mounted read-write"));

    // Grafting makes the subtree reachable by path, the only way to exercise
    // the VFS
    let grafted = with_vfs(|vfs| {
        let _ = vfs.mkdir(0, "vfstest", FileMode::new(0o755));
        vfs.graft_ext_dir("/", "vfstest", 2)
    });
    let root = match grafted {
        Ok(id) => id,
        Err(e) => {
            r.check("graft it into the tree", false, format!("{:?}", e));
            return r.report();
        }
    };
    r.check("graft it into the tree", true, format!("vnode {}", root));

    let ext = Subject { mnt: MNT, fs_type: FsType::Ext4, block_size: 1024 };
    substantive(&mut r, &ext);
    // A disk's only: memfs is memory and is not cached
    page_cache(&mut r);
    write_back(&mut r, disk.dev);
    descriptors_without_a_lock(&mut r);
    readers_beside_a_renamer(&mut r);
    closers_beside_each_other(&mut r);
    a_create_gives_way(&mut r);
    truncate_after_the_lock(&mut r);

    // and every one of them again, against a filesystem with no medium
    let Some((mem_slot, mem_root)) = mount_memfs(&mut r) else {
        return r.report();
    };
    let mem = Subject {
        mnt: MEM_MNT,
        fs_type: FsType::MemFs,
        block_size: BLOCK_SIZE as u32,
    };
    substantive(&mut r, &mem);
    beyond_the_table(&mut r, &mem);
    listing_resumes(&mut r, &mem);
    the_cache_gives_back(&mut r, &mem);

    // the case that needs both of them at once
    no_contamination(&mut r);

    unmount_memfs(mem_slot, mem_root);
    r.report()
}

/// A file read whole through `read_fd`, the path of `read(2)`, in reads of an
/// odd size, so every page boundary falls inside one
fn read_whole(fd: usize) -> VfsResult<Vec<u8>> {
    with_vfs(|vfs| vfs.seek(fd, SeekFrom::Start(0)))?;
    let mut out = Vec::new();
    let mut buf = [0u8; 1000];
    loop {
        let n = crate::fs::vfs::core::read_fd(fd, &mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// The page cache for files on a disk (`vfs::filecache`). A read has to find
/// what the disk has, after a write, a truncate and a write that went round the
/// VFS by inode number, and a second read finds it there without asking the
/// disk
fn page_cache(r: &mut Runner) {
    let path = format!("{}/cached", MNT);
    // Three pages and a bit
    let mut want: Vec<u8> = (0..3 * PAGE_SIZE + 100).map(|i| (i * 7 + i / PAGE_SIZE) as u8).collect();
    let made = with_vfs(|vfs| -> VfsResult<(usize, usize)> {
        let fl = OpenFlags(OpenFlags::RDWR | OpenFlags::CREATE | OpenFlags::TRUNCATE);
        let fd = vfs.open(0, &path, fl, FileMode::new(0o644))?;
        vfs.write(fd, &want)?;
        Ok((fd, vfs.fds().get(fd)?.vnode_id as usize))
    });
    let (fd, vid) = match made {
        Ok(x) => x,
        Err(e) => {
            r.check("page-cache/a file to read", false, format!("{:?}", e));
            return;
        }
    };
    let counts = || {
        let fc = crate::fs::vfs::filecache::lock();
        (fc.hits.load(core::sync::atomic::Ordering::Relaxed), fc.fills)
    };
    // The write went into the cache (`core/writeback.rs`): on the disk and
    // out of the cache first, so the read below has to fill
    let _ = crate::fs::vfs::core::writeback::flush_vnode(vid);
    crate::fs::vfs::core::drop_file_cache(vid);

    let (_, f0) = counts();
    let watch = crate::kcore::time::Stopwatch::start();
    let first = read_whole(fd);
    let cold_us = watch.elapsed_us();
    let (h1, f1) = counts();
    r.check("page-cache/first read fills", first.as_ref() == Ok(&want) && f1 > f0,
        format!("{} bytes right: {}, fills {} -> {}, {} us", want.len(), first.as_ref() == Ok(&want), f0, f1, cold_us));

    let unlocked = || crate::fs::vfs::filecache::UNLOCKED_HITS.load(core::sync::atomic::Ordering::Relaxed);
    let u1 = unlocked();
    let watch = crate::kcore::time::Stopwatch::start();
    let second = read_whole(fd);
    let warm_us = watch.elapsed_us();
    let (h2, f2) = counts();
    let u2 = unlocked();
    r.check("page-cache/second read hits", second.as_ref() == Ok(&want) && f2 == f1 && h2 > h1,
        format!("hits {} -> {}, fills {} -> {}, {} us", h1, h2, f1, f2, warm_us));
    // Every one of its reads, with no VFS lock (`cached::read_hit`): four
    // pages of 1000-byte reads, and the one that finds the end
    r.check("page-cache/second read takes no VFS lock", u2 - u1 >= (want.len() / 1000) as u64,
        format!("unlocked hits {} -> {}", u1, u2));

    // Across the first page boundary, through the VFS: the cache's copy and
    // the disk's both
    let patch = b"MIKU-ACROSS-A-PAGE";
    let at = PAGE_SIZE - 7;
    let wrote = with_vfs(|vfs| -> VfsResult<usize> {
        vfs.seek(fd, SeekFrom::Start(at as u64))?;
        vfs.write(fd, patch)
    });
    want[at..at + patch.len()].copy_from_slice(patch);
    let after = read_whole(fd);
    let (_, f3) = counts();
    r.check("page-cache/a write is read back at once", wrote.is_ok() && after.as_ref() == Ok(&want) && f3 == f2,
        format!("write {:?}, right {}, fills {} -> {}", wrote, after.as_ref() == Ok(&want), f2, f3));

    let _ = crate::fs::vfs::core::writeback::flush_vnode(vid);
    crate::fs::vfs::core::drop_file_cache(vid);
    let from_disk = read_whole(fd);
    r.check("page-cache/and the disk has it", from_disk.as_ref() == Ok(&want),
        format!("{} bytes read again from the disk", from_disk.as_ref().map(|v| v.len()).unwrap_or(0)));

    let cut = 6000;
    let truncated = with_vfs(|vfs| vfs.truncate_to(vid, cut as u64));
    want.truncate(cut);
    let short = read_whole(fd);
    r.check("page-cache/a truncate is read back", truncated.is_ok() && short.as_ref() == Ok(&want),
        format!("{:?}, {} bytes", truncated, short.as_ref().map(|v| v.len()).unwrap_or(0)));

    // Past the VFS, by inode, as the shell's ext builtins write: the next
    // open sees the size changed under the cache and drops it
    let extra = [0x5au8; 300];
    let (fs_type, slot, ino) = with_vfs(|vfs| (vfs.nodes[vid].fs_type, vfs.nodes[vid].ext2_slot, vfs.nodes[vid].ext2_ino));
    let around = crate::fs::registry::with_mounted(fs_type, slot, |fs| {
        fs.write_at(crate::fs::vfs::fsops::NodeId::ino(ino), cut as u64, &extra)
    });
    want.extend_from_slice(&extra);
    let fd2 = with_vfs(|vfs| vfs.open(0, &path, OpenFlags(OpenFlags::READ), FileMode::new(0o644)));
    let seen = fd2.as_ref().map_err(|e| *e).and_then(|&f| read_whole(f));
    r.check("page-cache/a write round the VFS is seen at the next open", seen.as_ref() == Ok(&want),
        format!("write {:?}, {} bytes, right {}", around.map(|r| r.is_ok()),
            seen.as_ref().map(|v| v.len()).unwrap_or(0), seen.as_ref() == Ok(&want)));

    with_vfs(|vfs| {
        let _ = vfs.close(fd);
        if let Ok(f) = fd2 {
            let _ = vfs.close(f);
        }
        let _ = vfs.unlink(0, &path);
    });
}

/// Write-back (`core/writeback.rs`, `dev/fs-architecture.md`): a write to a
/// disk file is dirty pages first, read back from there, written out once
/// per page by a flush, never evicted or dropped before, cut by a truncate,
/// written out for a reader that goes around the cache, and gone with an
/// unlinked file. The filesystem is checked at the end
fn write_back(r: &mut Runner, dev: BlockDevId) {
    use crate::fs::vfs::core::writeback::flush_vnode;
    use crate::fs::vfs::filecache;
    let path = format!("{}/wb", MNT);
    let dirty = |vid: usize| filecache::lock().is_dirty(vid);
    let cleaned = || filecache::lock().cleaned;
    let rw = OpenFlags(OpenFlags::RDWR | OpenFlags::CREATE | OpenFlags::TRUNCATE);
    // Two pages and a bit, in writes of 256 bytes: sixteen to a page
    let mut want: Vec<u8> = (0..2 * PAGE_SIZE + 300).map(|i| (i * 13 + i / 251) as u8).collect();
    let made = with_vfs(|vfs| -> VfsResult<(usize, usize, u64)> {
        let fd = vfs.open(0, &path, rw, FileMode::new(0o644))?;
        for chunk in want.chunks(256) {
            vfs.write(fd, chunk)?;
        }
        let vid = vfs.fds().get(fd)?.vnode_id as usize;
        Ok((fd, vid, vfs.stat(0, &path)?.size))
    });
    let Ok((fd, vid, size)) = made else {
        r.check("write-back/written bytes are read back before the disk has them", false, format!("{:?}", made.err()));
        return;
    };
    let before = read_whole(fd);
    r.check(
        "write-back/written bytes are read back before the disk has them",
        dirty(vid) && size == want.len() as u64 && before.as_ref() == Ok(&want),
        format!("dirty {}, size {}, right {}", dirty(vid), size, before.as_ref() == Ok(&want)),
    );

    let c0 = cleaned();
    let flushed = flush_vnode(vid);
    let c1 = cleaned();
    r.check(
        "write-back/a flush writes each page once",
        flushed.is_ok() && !dirty(vid) && c1 - c0 == 3,
        format!("{:?}, {} page(s) cleaned of 48 writes, dirty after {}", flushed, c1 - c0, dirty(vid)),
    );
    crate::fs::vfs::core::drop_file_cache(vid);
    let from_disk = read_whole(fd);
    r.check(
        "write-back/and the disk has them",
        from_disk.as_ref() == Ok(&want),
        format!("{} bytes read from the disk", from_disk.as_ref().map(|v| v.len()).unwrap_or(0)),
    );

    // The mtime is the write's, before the flush and after it, when `stat`
    // asks the disk again
    let times = with_vfs(|vfs| -> VfsResult<u64> {
        vfs.seek(fd, SeekFrom::Start(10))?;
        vfs.write(fd, b"later")?;
        let m1 = vfs.stat(0, &path)?.mtime;
        Ok(m1)
    })
    .and_then(|m1| {
        flush_vnode(vid)?;
        with_vfs(|vfs| -> VfsResult<(u64, u64)> {
            vfs.nodes[vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
            Ok((m1, vfs.stat(0, &path)?.mtime))
        })
    });
    want[10..15].copy_from_slice(b"later");
    r.check(
        "write-back/stat's mtime is the write's after the flush too",
        matches!(times, Ok((a, b)) if a == b && a != 0),
        format!("before, after = {:?}", times),
    );

    // Dirty and closed: reclaim and the cache's own eviction pass it over
    let kept = with_vfs(|vfs| -> VfsResult<(bool, bool)> {
        vfs.seek(fd, SeekFrom::Start(PAGE_SIZE as u64))?;
        vfs.write(fd, b"kept")?;
        vfs.close(fd)?;
        let evicted = vfs.try_evict(vid);
        Ok((evicted, true))
    });
    want[PAGE_SIZE..PAGE_SIZE + 4].copy_from_slice(b"kept");
    let shrunk = {
        let mut fc = filecache::lock();
        fc.shrink(usize::MAX, usize::MAX);
        fc.frame(vid, 1).is_some()
    };
    r.check(
        "write-back/a dirty vnode is not evicted, nor its pages",
        matches!(kept, Ok((false, _))) && shrunk && dirty(vid),
        format!("evicted {:?}, page kept {}, dirty {}", kept.map(|k| k.0), shrunk, dirty(vid)),
    );

    // Around the cache: `read_path` reads the disk, and writes the vnode out
    // first
    let around = with_vfs(|vfs| vfs.read_path(&path));
    r.check(
        "write-back/a reader around the cache gets the dirty bytes",
        around.as_ref() == Ok(&want) && !dirty(vid),
        format!("right {}, dirty after {}", around.as_ref() == Ok(&want), dirty(vid)),
    );

    // A truncate with dirty pages past the cut and before it
    let cut = PAGE_SIZE + 1000;
    let truncated = with_vfs(|vfs| -> VfsResult<usize> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::RDWR), FileMode::new(0o644))?;
        vfs.seek(fd, SeekFrom::Start(0))?;
        vfs.write(fd, &[0x77u8; 3 * PAGE_SIZE])?;
        vfs.truncate_to(vid, cut as u64)?;
        Ok(fd)
    });
    want = vec![0x77u8; cut];
    let cut_read = truncated.and_then(|fd| {
        let a = read_whole(fd)?;
        flush_vnode(vid)?;
        crate::fs::vfs::core::drop_file_cache(vid);
        let b = read_whole(fd)?;
        with_vfs(|vfs| vfs.close(fd))?;
        Ok((a, b))
    });
    r.check(
        "write-back/a truncate cuts dirty pages, and the disk agrees",
        matches!(&cut_read, Ok((a, b)) if *a == want && *b == want),
        format!("{:?}", cut_read.as_ref().map(|(a, b)| (a.len(), b.len()))),
    );

    // O_APPEND's end is the VFS's while the disk's is behind
    let appended = with_vfs(|vfs| -> VfsResult<(usize, u64)> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE | OpenFlags::APPEND), FileMode::new(0o644))?;
        vfs.write(fd, b"one")?;
        vfs.write(fd, b"two")?;
        let size = vfs.stat(0, &path)?.size;
        vfs.close(fd)?;
        Ok((fd, size))
    });
    want.extend_from_slice(b"onetwo");
    let tail = with_vfs(|vfs| vfs.read_path(&path));
    r.check(
        "write-back/O_APPEND writes at the end the VFS knows",
        matches!(appended, Ok((_, s)) if s == want.len() as u64) && tail.as_ref() == Ok(&want),
        format!("size {:?}, right {}", appended.map(|a| a.1), tail.as_ref() == Ok(&want)),
    );

    // Unlinked with dirty pages: they go with it, nothing is written into
    // the inode after, and the filesystem checks clean
    let gone = with_vfs(|vfs| -> VfsResult<bool> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE), FileMode::new(0o644))?;
        vfs.write(fd, &[1u8; 2 * PAGE_SIZE])?;
        vfs.close(fd)?;
        vfs.unlink(0, &path)?;
        Ok(filecache::lock().is_dirty(vid))
    });
    crate::fs::vfs::core::writeback::writeback_all();
    let slot = (0..mount::MAX_MOUNTS).find(|&s| mount::slot_drive(s) == Some(dev as usize));
    let fsck = slot.and_then(|s| mount::with_ext2_for(s as u8, |fs| fs.fsck(crate::fs::ext::fsck::FsckOptions::check())));
    let clean = matches!(&fsck, Some(Ok(rep)) if rep.checked && rep.errors() == 0);
    r.check(
        "write-back/an unlinked file's dirty pages go with it, and fsck is clean",
        matches!(gone, Ok(false)) && clean,
        format!(
            "dirty after unlink {:?}, fsck {}",
            gone,
            match &fsck {
                Some(Ok(rep)) => format!("{} error(s)", rep.errors()),
                _ => String::from("did not run"),
            }
        ),
    );
}

/// A descriptor read and its offset moved with no lock (`vfs::fd`, `peek_pos`,
/// `advance`, `seek`): the swap is from the word the reader saw, generation
/// and all, so it fails for the same file opened again at the same number,
/// even at the same offset
fn descriptors_without_a_lock(r: &mut Runner) {
    use crate::fs::vfs::fd;
    let path = format!("{}/fdslot", MNT);
    let rw = OpenFlags(OpenFlags::RDWR | OpenFlags::CREATE);
    let Ok(n) = with_vfs(|vfs| vfs.open(0, &path, rw, FileMode::new(0o644))) else {
        r.check("fd/an offset moves with no lock, and only for its own descriptor", false, String::from("no file"));
        return;
    };
    let Some((f, first)) = fd::peek_pos(n) else {
        r.check("fd/an offset moves with no lock, and only for its own descriptor", false, String::from("not seen"));
        return;
    };
    let moved = fd::advance(n, f.vnode_id, first, 10);
    let at = fd::peek(n).map(|f| f.offset);
    let sought = crate::fs::vfs::core::seek_fd(n, SeekFrom::Current(5));
    let (_, later) = fd::peek_pos(n).unwrap_or((f, 0));
    let again = with_vfs(|vfs| -> VfsResult<usize> {
        vfs.close(n)?;
        vfs.open(0, &path, OpenFlags(OpenFlags::RDWR), FileMode::new(0o644))
    });
    // The first word was at offset 0, and so is the new descriptor
    let stale_at_zero = fd::advance(n, f.vnode_id, first, 7);
    let stale = fd::advance(n, f.vnode_id, later, 99);
    let now = fd::peek(n).map(|f| f.offset);
    r.check(
        "fd/an offset moves with no lock, and only for its own descriptor",
        moved && at == Some(10) && sought == Ok(15) && again == Ok(n) && !stale_at_zero && !stale && now == Some(0),
        format!(
            "moved {} to {:?}, seek {:?}, reopened at {:?}, stale swaps {} {}, offset {:?}",
            moved, at, sought, again, stale_at_zero, stale, now
        ),
    );
    with_vfs(|vfs| {
        let _ = vfs.close(n);
        let _ = vfs.unlink(0, &path);
    });
}

/// `O_TRUNC` of a big file truncates the disk after the VFS lock, under
/// ext's (`truncate::LateTruncate`): the file is empty at once, a write and
/// an `fsync` in the same call come after the truncate on the disk, and a
/// truncate the next call reads the disk after
fn truncate_after_the_lock(r: &mut Runner) {
    let path = format!("{}/latetrunc", MNT);
    let big = vec![0x42u8; 512 * 1024];
    let made = with_vfs(|vfs| -> VfsResult<usize> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::RDWR | OpenFlags::CREATE), FileMode::new(0o644))?;
        vfs.write(fd, &big)?;
        vfs.close(fd)?;
        Ok(vfs.resolve_path(0, &path)?)
    });
    let Ok(vid) = made else {
        r.check("O_TRUNC of a big file: empty, and later writes kept", false, format!("{:?}", made.err()));
        return;
    };
    let _ = crate::fs::vfs::core::writeback::flush_vnode(vid);
    let trunc = OpenFlags(OpenFlags::RDWR | OpenFlags::TRUNCATE);
    // Truncated, written and synced in one call
    let one = with_vfs(|vfs| -> VfsResult<u64> {
        let fd = vfs.open(0, &path, trunc, FileMode::new(0o644))?;
        vfs.write(fd, b"after")?;
        vfs.fsync(fd)?;
        vfs.close(fd)?;
        Ok(vfs.stat(0, &path)?.size)
    });
    crate::fs::vfs::core::drop_file_cache(vid);
    let disk1 = with_vfs(|vfs| vfs.read_path(&path));
    // Big again, then truncated in one call and looked at in the next
    let again = with_vfs(|vfs| -> VfsResult<()> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE), FileMode::new(0o644))?;
        vfs.write(fd, &big)?;
        vfs.close(fd)
    });
    let _ = crate::fs::vfs::core::writeback::flush_vnode(vid);
    let opened = with_vfs(|vfs| -> VfsResult<u64> {
        let fd = vfs.open(0, &path, trunc, FileMode::new(0o644))?;
        let size = vfs.fstat(fd)?.size;
        vfs.close(fd)?;
        Ok(size)
    });
    let disk2 = with_vfs(|vfs| -> VfsResult<(u64, usize)> {
        vfs.nodes[vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
        Ok((vfs.stat(0, &path)?.size, vfs.read_path(&path)?.len()))
    });
    r.check(
        "O_TRUNC of a big file: empty, and later writes kept",
        one == Ok(5) && disk1.as_deref() == Ok(&b"after"[..]) && again.is_ok() && opened == Ok(0) && disk2 == Ok((0, 0)),
        format!("one call {:?}, disk {:?}, then {:?} {:?}, disk {:?}", one, disk1.map(|v| v.len()), again, opened, disk2),
    );
    with_vfs(|vfs| {
        let _ = vfs.unlink(0, &path);
    });
}

static RACE_GO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static RACE_STOP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static RACE_DONE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static RACE_READS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RACE_FOUND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RACE_BAD: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const RACE_NAMES: usize = 32;

/// A reader of `readers_beside_a_renamer`: `stat` of the names, as
/// `sys_stat` makes it (`with_vfs_read`), until told to stop
fn race_reader() -> ! {
    use core::sync::atomic::Ordering;
    use crate::fs::vfs::core::path_resolve::Links;
    while !RACE_GO.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    let mut i = 0usize;
    while !RACE_STOP.load(Ordering::Acquire) {
        let path = format!("{}/race/{}{}", MNT, if i % 2 == 0 { 'a' } else { 'b' }, (i / 2) % RACE_NAMES);
        let got = crate::fs::vfs::core::with_vfs_read(
            |vfs, cred| match vfs.stat_as(cred, 0, &path, Links::All) {
                Err(VfsError::Deferred) => None,
                r => Some(r),
            },
            |vfs| vfs.stat(0, &path),
        );
        match got {
            Ok(st) if st.kind == VNodeKind::Regular => { RACE_FOUND.fetch_add(1, Ordering::Relaxed); }
            Err(VfsError::NotFound) => {}
            _ => { RACE_BAD.fetch_add(1, Ordering::Relaxed); }
        }
        RACE_READS.fetch_add(1, Ordering::Relaxed);
        i = i.wrapping_add(7);
    }
    RACE_DONE.fetch_add(1, Ordering::AcqRel);
    loop {
        crate::sched::sleep_ns(10_000_000);
    }
}

/// `stat`s under the VFS lock held to read, from three CPUs, while this task
/// renames, unlinks and makes the same names under the lock held to write
/// (`dev/vfs-locking.md`, "4c"). Every answer must be a regular file or
/// `NotFound`
fn readers_beside_a_renamer(r: &mut Runner) {
    use core::sync::atomic::Ordering;
    let at = |c: char, i: usize| format!("{}/race/{}{}", MNT, c, i);
    let made = with_vfs(|vfs| -> VfsResult<usize> {
        let root = vfs.resolve_path(0, MNT)?;
        let d = vfs.mkdir(root, "race", FileMode::new(0o755))?;
        for i in 0..RACE_NAMES {
            vfs.create_file(d, &format!("a{}", i), FileMode::new(0o644))?;
        }
        Ok(d)
    });
    let Ok(d) = made else {
        r.check("vfs/readers beside a renamer see a file or nothing", false, format!("{:?}", made.err()));
        return;
    };
    for a in [&RACE_READS, &RACE_FOUND, &RACE_BAD] {
        a.store(0, Ordering::Release);
    }
    RACE_DONE.store(0, Ordering::Release);
    RACE_STOP.store(false, Ordering::Release);
    RACE_GO.store(false, Ordering::Release);
    let workers: Vec<u64> = (0..3).map(|_| crate::sched::spawn_named(race_reader, "vfs-test-race", 10)).collect();
    crate::sched::sleep_ns(20_000_000);
    RACE_GO.store(true, Ordering::Release);
    let mut changes = 0u64;
    let mut failed = None;
    for round in 0..200 {
        let i = round % RACE_NAMES;
        let step = with_vfs(|vfs| -> VfsResult<()> {
            vfs.rename(0, &at('a', i), &at('b', i))?;
            vfs.unlink(0, &at('b', i))?;
            vfs.create_file(d, &format!("a{}", i), FileMode::new(0o644))?;
            Ok(())
        });
        match step {
            Ok(()) => changes += 3,
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
        crate::sched::yield_now();
    }
    RACE_STOP.store(true, Ordering::Release);
    let mut stopped = false;
    for _ in 0..1000 {
        if RACE_DONE.load(Ordering::Acquire) == workers.len() {
            stopped = true;
            break;
        }
        crate::sched::sleep_ns(1_000_000);
    }
    for w in workers {
        crate::sched::kill(w);
    }
    let (reads, found, bad) =
        (RACE_READS.load(Ordering::Acquire), RACE_FOUND.load(Ordering::Acquire), RACE_BAD.load(Ordering::Acquire));
    r.check(
        "vfs/readers beside a renamer see a file or nothing",
        failed.is_none() && stopped && bad == 0 && reads > 0 && found > 0,
        format!("{} changes ({:?}), {} reads, {} found, {} wrong, readers stopped {}", changes, failed, reads, found, bad, stopped),
    );
}

static CLOSE_PHASE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// A closer of `closers_beside_each_other`: opens and closes the file as
/// `sys_open` and `sys_close` do (phase 1), then holds one descriptor
/// (phase 2) and closes it with the others once the file is unlinked
/// (phase 3)
fn close_worker() -> ! {
    use core::sync::atomic::Ordering;
    let path = format!("{}/closing", MNT);
    let open = |p: &str| {
        crate::fs::vfs::core::with_vfs_read(
            |vfs, cred| match vfs.open_as(cred, 0, p, OpenFlags(OpenFlags::READ)) {
                Err(VfsError::Deferred) => None,
                r => Some(r),
            },
            |vfs| vfs.open(0, p, OpenFlags(OpenFlags::READ), FileMode::new(0)),
        )
    };
    while CLOSE_PHASE.load(Ordering::Acquire) == 0 {
        core::hint::spin_loop();
    }
    while CLOSE_PHASE.load(Ordering::Acquire) == 1 {
        let ok = open(&path).map(|fd| crate::fs::vfs::core::close_fd(fd).is_ok()).unwrap_or(false);
        if !ok {
            RACE_BAD.fetch_add(1, Ordering::Relaxed);
        }
        RACE_READS.fetch_add(1, Ordering::Relaxed);
    }
    let held = open(&path);
    if held.is_err() {
        RACE_BAD.fetch_add(1, Ordering::Relaxed);
    }
    RACE_FOUND.fetch_add(1, Ordering::AcqRel);
    while CLOSE_PHASE.load(Ordering::Acquire) < 3 {
        crate::sched::yield_now();
    }
    if let Ok(fd) = held {
        if crate::fs::vfs::core::close_fd(fd).is_err() {
            RACE_BAD.fetch_add(1, Ordering::Relaxed);
        }
    }
    RACE_DONE.fetch_add(1, Ordering::AcqRel);
    loop {
        crate::sched::sleep_ns(10_000_000);
    }
}

/// `close` beside other readers: three CPUs open and close one file, then hold
/// a descriptor each while it is unlinked and close them together. The count
/// has to come out exact, three holders after thousands of opens and closes,
/// and the last close of the unlinked file has to free it
fn closers_beside_each_other(r: &mut Runner) {
    use core::sync::atomic::Ordering;
    let path = format!("{}/closing", MNT);
    let made = with_vfs(|vfs| -> VfsResult<usize> {
        let root = vfs.resolve_path(0, MNT)?;
        vfs.create_file(root, "closing", FileMode::new(0o644))
    });
    let Ok(vid) = made else {
        r.check("vfs/closers beside each other count exactly", false, format!("{:?}", made.err()));
        return;
    };
    for a in [&RACE_READS, &RACE_FOUND, &RACE_BAD] {
        a.store(0, Ordering::Release);
    }
    RACE_DONE.store(0, Ordering::Release);
    CLOSE_PHASE.store(0, Ordering::Release);
    let workers: Vec<u64> = (0..3).map(|_| crate::sched::spawn_named(close_worker, "vfs-test-close", 10)).collect();
    crate::sched::sleep_ns(20_000_000);
    CLOSE_PHASE.store(1, Ordering::Release);
    crate::sched::sleep_ns(50_000_000);
    CLOSE_PHASE.store(2, Ordering::Release);
    let wait = |what: &core::sync::atomic::AtomicU64, n: u64| {
        (0..2000).any(|_| {
            let there = what.load(Ordering::Acquire) >= n;
            if !there {
                crate::sched::sleep_ns(1_000_000);
            }
            there
        })
    };
    let held = wait(&RACE_FOUND, workers.len() as u64);
    let refs = with_vfs(|vfs| vfs.nodes[vid].refs());
    let unlinked = with_vfs(|vfs| vfs.unlink(0, &path));
    CLOSE_PHASE.store(3, Ordering::Release);
    let stopped = (0..2000).any(|_| {
        let done = RACE_DONE.load(Ordering::Acquire) == workers.len();
        if !done {
            crate::sched::sleep_ns(1_000_000);
        }
        done
    });
    for w in workers {
        crate::sched::kill(w);
    }
    let gone = with_vfs(|vfs| !vfs.valid_vnode(vid) || vfs.nodes[vid].nlinks != 0);
    let (cycles, bad) = (RACE_READS.load(Ordering::Acquire), RACE_BAD.load(Ordering::Acquire));
    r.check(
        "vfs/closers beside each other count exactly",
        held && stopped && bad == 0 && cycles > 0 && refs == 3 && unlinked.is_ok() && gone,
        format!(
            "{} open+close, {} wrong, holders {} with {} references, unlink {:?}, closers stopped {}, freed {}",
            cycles, bad, held, refs, unlinked, stopped, gone
        ),
    );
}

static EXT_HELD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PROBE_GO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PROBE_DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static PROBE_WORST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Holds ext's lock for 50 ms, as a flush's write-out would
fn ext_holder() -> ! {
    {
        let _state = crate::fs::ext::mount::STATE.lock();
        EXT_HELD.store(true, core::sync::atomic::Ordering::Release);
        crate::sched::sleep_ns(50_000_000);
    }
    EXT_HELD.store(false, core::sync::atomic::Ordering::Release);
    loop {
        crate::sched::sleep_ns(10_000_000);
    }
}

/// Takes the VFS lock alone every millisecond for 40 ms, and keeps the
/// longest it waited
fn vfs_prober() -> ! {
    use core::sync::atomic::Ordering;
    while !PROBE_GO.load(Ordering::Acquire) {
        crate::sched::sleep_ns(1_000_000);
    }
    let end = crate::kcore::time::now_ns() + 40_000_000;
    while crate::kcore::time::now_ns() < end {
        let t = crate::kcore::time::now_ns();
        with_vfs(|vfs| vfs.op);
        PROBE_WORST.fetch_max(crate::kcore::time::now_ns() - t, Ordering::Relaxed);
        crate::sched::sleep_ns(1_000_000);
    }
    PROBE_DONE.store(true, Ordering::Release);
    loop {
        crate::sched::sleep_ns(10_000_000);
    }
}

/// A create on the disk while ext's lock is held elsewhere for 50 ms
/// (`with_vfs_disk`): it gives way and waits with the VFS lock released, so a
/// call taking the VFS lock meanwhile is answered at once; it still makes its
/// file
fn a_create_gives_way(r: &mut Runner) {
    use core::sync::atomic::Ordering;
    let path = format!("{}/gives-way", MNT);
    // Known not to be there, so the create walks to its `give_way` and not to a
    // read of the directory (which also waits with the lock released)
    let absent = with_vfs(|vfs| vfs.stat(0, &path).is_err());
    for f in [&EXT_HELD, &PROBE_GO, &PROBE_DONE] {
        f.store(false, Ordering::Release);
    }
    PROBE_WORST.store(0, Ordering::Release);
    let prober = crate::sched::spawn_named(vfs_prober, "vfs-test-probe", 10);
    let holder = crate::sched::spawn_named(ext_holder, "vfs-test-ext", 10);
    let wait = |flag: &core::sync::atomic::AtomicBool| {
        (0..1000).any(|_| {
            let h = flag.load(Ordering::Acquire);
            if !h {
                crate::sched::sleep_ns(1_000_000);
            }
            h
        })
    };
    let held = wait(&EXT_HELD);
    PROBE_GO.store(true, Ordering::Release);
    let before = crate::fs::vfs::core::GAVE_WAY.load(Ordering::Relaxed);
    let started = crate::kcore::time::now_ns();
    let made = crate::fs::vfs::core::with_vfs_disk(|vfs| {
        vfs.open(0, &path, OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))
    });
    let took = crate::kcore::time::now_ns() - started;
    let gave = crate::fs::vfs::core::GAVE_WAY.load(Ordering::Relaxed) - before;
    if let Ok(fd) = made {
        let _ = crate::fs::vfs::core::close_fd(fd);
    }
    let probed = wait(&PROBE_DONE);
    let worst = PROBE_WORST.load(Ordering::Acquire);
    crate::sched::kill(holder);
    crate::sched::kill(prober);
    let found = with_vfs(|vfs| vfs.stat(0, &path).is_ok());
    with_vfs(|vfs| {
        let _ = vfs.unlink(0, &path);
    });
    r.check(
        "vfs/a create gives way to a busy disk",
        absent && held && made.is_ok() && gave >= 1 && probed && worst < 10_000_000 && found,
        format!(
            "lock held {}, create {:?} after {} us, gave way {} time(s), the VFS lock waited for at most {} us meanwhile, there {}",
            held, made.map(|_| ()), took / 1000, gave, worst / 1000, found
        ),
    );
}

/// Everything about the VFS, not about one filesystem.
fn substantive(r: &mut Runner, s: &Subject) {
    big_directory(r, s);
    walks_read_unlocked(r, s);
    long_name(r, s);
    long_symlink(r, s);
    rename_across_directories(r, s);
    wide_owner(r, s);
    which_filesystem(r, s);
    names_are_entries(r, s);
    times_from_the_disk(r, s);
    big_files_leave_after_the_lock(r, s);
}

/// A big file's blocks are freed after the VFS lock is let go
/// (`core/unlinked.rs`), and still before `unlink` or its last `close`
/// returns: by then the name is gone from the disk and free to create again
fn big_files_leave_after_the_lock(r: &mut Runner, s: &Subject) {
    let path = format!("{}/drain/big", s.mnt);
    let chunk = vec![0x5au8; 64 * 1024];
    let write_big = |vfs: &mut crate::fs::vfs::core::MikuVFS| -> VfsResult<usize> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
        // 320 KiB: over `DRAIN_MIN`, and inside the memfs budget of 1 MiB
        for _ in 0..5 {
            vfs.write(fd, &chunk)?;
        }
        Ok(fd)
    };
    // Gone from the tree and the disk: no whiteout left, and a lookup that
    // asks the disk again does not find it. Then made again, and removed
    let after = |d: usize| {
        with_vfs(|vfs| {
            vfs.evict_backed_children(d);
            let gone = matches!(vfs.stat(0, &path), Err(VfsError::NotFound));
            let whiteouts = vfs.nodes[d].children.whiteouts();
            let again = vfs.create_file(d, "big", FileMode::new(0o644)).is_ok() && vfs.unlink(0, &path).is_ok();
            (gone, whiteouts, again)
        })
    };

    let made = with_vfs(|vfs| -> VfsResult<usize> {
        let root = vfs.resolve_path(0, s.mnt)?;
        let d = vfs.mkdir(root, "drain", FileMode::new(0o755))?;
        let fd = write_big(vfs)?;
        vfs.close(fd)?;
        Ok(d)
    });
    let Ok(d) = made else {
        r.check("a big file unlinked is gone when unlink returns", false, format!("{}: {:?}", s.mnt, made.err()));
        return;
    };
    let unlinked = with_vfs(|vfs| vfs.unlink(0, &path));
    let got = after(d);
    r.check(
        "a big file unlinked is gone when unlink returns",
        unlinked.is_ok() && got == (true, 0, true),
        format!("{}: unlink {:?}; not found, whiteouts, made again = {:?}", s.mnt, unlinked, got),
    );

    let held = with_vfs(|vfs| -> VfsResult<usize> {
        let fd = write_big(vfs)?;
        vfs.unlink(0, &path)?;
        Ok(fd)
    });
    let closed = match held {
        Ok(fd) => with_vfs(|vfs| vfs.close(fd)),
        Err(e) => Err(e),
    };
    let got = after(d);
    r.check(
        "a big file unlinked while open is gone when it is closed",
        closed.is_ok() && got == (true, 0, true),
        format!("{}: close {:?}; not found, whiteouts, made again = {:?}", s.mnt, closed, got),
    );
}

/// A file's times are its filesystem's: `stat` reporting when the vnode was
/// made would move a file's mtime with every eviction
fn times_from_the_disk(r: &mut Runner, s: &Subject) {
    let path = format!("{}/names/timed", s.mnt);
    // A time well in the past, which neither now nor the moment of the fill can
    // be mistaken for
    const LONG_AGO: u64 = 1_000_000_000;
    let written = with_vfs(|vfs| -> VfsResult<(usize, u64, u64)> {
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
        vfs.close(fd)?;
        let v = vfs.resolve_path(0, &path)?;
        vfs.set_times(v, Some(LONG_AGO), Some(LONG_AGO))?;
        let before = vfs.stat(0, &path)?.mtime;
        let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE), FileMode::new(0))?;
        vfs.write(fd, b"later")?;
        vfs.close(fd)?;
        Ok((v, before, vfs.stat(0, &path)?.mtime))
    });
    // Written and written out, not synced: evictable all the same. Dirty
    // pages would hold it (`core/writeback.rs`)
    let got = written.and_then(|(v, before, written)| {
        crate::fs::vfs::core::writeback::flush_vnode(v)?;
        with_vfs(|vfs| -> VfsResult<(u64, bool, u64, u64)> {
            let gone = vfs.try_evict(v);
            let again = vfs.stat(0, &path)?.mtime;
            let _ = vfs.unlink(0, &path);
            Ok((before, gone, written, again))
        })
    });
    r.check(
        "a file's times are its filesystem's, and a written file can be evicted",
        matches!(got, Ok((LONG_AGO, true, w, a)) if w == a && w > LONG_AGO),
        format!("{}: set, evicted, written, after = {:?}", s.mnt, got),
    );
}

/// A directory of 2000 names read in passes of 16, as `getdents` reads it:
/// every name once, and the child table walked about once, not from the start
/// on every pass
fn listing_resumes(r: &mut Runner, s: &Subject) {
    let dir = format!("{}/wide/d03", s.mnt);
    let got = with_vfs(|vfs| -> VfsResult<(usize, usize, u64, usize)> {
        let d = vfs.resolve_path(0, &dir)?;
        // Read in first, so the walk below is the listing and not the fill
        let mut one = [DirEntry::empty(); 1];
        vfs.readdir_at(d, 0, &mut one)?;
        let slots = vfs.nodes[d].children.slots.len();
        let before = vfs.dcache.listed_slots.get();
        let mut batch = [DirEntry::empty(); 16];
        let (mut at, mut names) = (0usize, 0usize);
        let mut seen = alloc::collections::BTreeSet::new();
        loop {
            let n = vfs.readdir_at(d, at, &mut batch)?;
            if n == 0 {
                break;
            }
            for e in &batch[..n] {
                if seen.insert(String::from(e.get_name())) {
                    names += 1;
                }
            }
            at += n;
        }
        Ok((names, at, vfs.dcache.listed_slots.get() - before, slots))
    });
    r.check(
        "a directory read in passes walks its table about once",
        matches!(got, Ok((2002, 2002, walked, slots)) if walked <= 2 * slots as u64),
        format!("{}: names, positions, slots looked at, table size = {:?}", dir, got),
    );
}

/// Names and inodes (`dev/vfs-dentries-inodes.md`, "Stage 5"). A hard link is
/// one vnode under two names, before an eviction and after it, and each
/// directory lists the name it has. `st_ino` survives an eviction. A miss is
/// asked of the filesystem once, and of a complete directory never. A file
/// unlinked while open is hidden until its last close, and its name is free
/// after it.
fn names_are_entries(r: &mut Runner, s: &Subject) {
    let at = |p: &str| format!("{}/names/{}", s.mnt, p);
    let listed = |dir: usize| {
        with_vfs(|vfs| {
            let mut e = vec![DirEntry::empty(); 16];
            let n = vfs.readdir(dir, &mut e).unwrap_or(0);
            e[..n].iter().skip(2).map(|e| String::from(e.get_name())).collect::<Vec<_>>()
        })
    };

    let made = with_vfs(|vfs| -> VfsResult<(usize, usize, usize, usize)> {
        let root = vfs.resolve_path(0, s.mnt)?;
        let top = vfs.mkdir(root, "names", FileMode::new(0o755))?;
        let d1 = vfs.mkdir(top, "d1", FileMode::new(0o755))?;
        let d2 = vfs.mkdir(top, "d2", FileMode::new(0o755))?;
        let d3 = vfs.mkdir(top, "d3", FileMode::new(0o755))?;
        let fd = vfs.open(0, &at("d1/a"), OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
        vfs.write(fd, b"hello")?;
        // Written is dirty until synced, and reclaim keeps what is dirty
        vfs.fsync(fd)?;
        vfs.close(fd)?;
        vfs.link(0, &at("d1/a"), d2, "b")?;
        Ok((d1, d2, d3, vfs.resolve_path(0, &at("d1/a"))?))
    });
    let Ok((d1, d2, d3, v)) = made else {
        r.check("a file with a name in two directories", false, format!("{}: {:?}", s.mnt, made.err()));
        return;
    };

    let linked = with_vfs(|vfs| -> VfsResult<(usize, u64, u64, u16)> {
        let b = vfs.resolve_path(0, &at("d2/b"))?;
        Ok((b, vfs.stat(0, &at("d1/a"))?.ino, vfs.stat(0, &at("d2/b"))?.ino, vfs.nodes[v].nlinks))
    });
    r.check(
        "a hard link is one vnode and one st_ino",
        matches!(linked, Ok((b, i1, i2, 2)) if b == v && i1 == i2),
        format!("{}: {:?}, vnode {}", s.mnt, linked, v),
    );
    let ino = linked.map(|l| l.1).unwrap_or(0);
    let (l1, l2) = (listed(d1), listed(d2));
    r.check(
        "each directory lists the name it has",
        l1 == ["a"] && l2 == ["b"],
        format!("{}: {:?} and {:?}", s.mnt, l1, l2),
    );

    // Evicted, then met by its second name first and its first name after
    let again = with_vfs(|vfs| -> VfsResult<(bool, usize, usize, u64, u64)> {
        let gone = vfs.try_evict(v);
        let b = vfs.resolve_path(0, &at("d2/b"))?;
        let a = vfs.resolve_path(0, &at("d1/a"))?;
        Ok((gone, a, b, vfs.stat(0, &at("d1/a"))?.ino, vfs.stat(0, &at("d2/b"))?.ino))
    });
    r.check(
        "after an eviction both names find one vnode again",
        matches!(again, Ok((true, a, b, _, _)) if a == b),
        format!("{}: evicted, a, b = {:?}", s.mnt, again.map(|a| (a.0, a.1, a.2))),
    );
    r.check(
        "st_ino is the same after an eviction",
        matches!(again, Ok((_, _, _, i1, i2)) if i1 == ino && i2 == ino && ino != 0),
        format!("{}: {:#x}, then {:x?}", s.mnt, ino, again.map(|a| (a.3, a.4))),
    );

    // The second name moves to a third directory and goes; the first stays
    let moved = with_vfs(|vfs| -> VfsResult<(bool, bool, bool, u16)> {
        let a = vfs.resolve_path(0, &at("d1/a"))?;
        vfs.rename(0, &at("d2/b"), &at("d3/c"))?;
        let same = vfs.resolve_path(0, &at("d3/c"))? == a;
        let b_gone = vfs.resolve_path(0, &at("d2/b")).is_err();
        vfs.unlink(0, &at("d3/c"))?;
        let c_gone = vfs.resolve_path(0, &at("d3/c")).is_err();
        let a = vfs.resolve_path(0, &at("d1/a"))?;
        Ok((same, b_gone, c_gone, vfs.nodes[a].nlinks))
    });
    let body = with_vfs(|vfs| vfs.open(0, &at("d1/a"), OpenFlags(OpenFlags::READ), FileMode::new(0)))
        .and_then(|fd| {
            let got = read_whole(fd);
            let _ = with_vfs(|vfs| vfs.close(fd));
            got
        });
    let (l1, l2, l3) = (listed(d1), listed(d2), listed(d3));
    r.check(
        "the second name renamed across directories and unlinked there",
        moved == Ok((true, true, true, 1))
            && body.as_deref() == Ok(&b"hello"[..])
            && l1 == ["a"]
            && l2.is_empty()
            && l3.is_empty(),
        format!("{}: {:?}, a reads {:?}, lists {:?} {:?} {:?}", s.mnt, moved, body.map(|b| b.len()), l1, l2, l3),
    );

    // d2 forgets that it is complete, as an eviction of one of its names
    // makes it: a miss asks the filesystem once, and the negative entry
    // answers after. d1 was listed just now, so it is complete, and asks
    // nothing
    let misses = with_vfs(|vfs| {
        vfs.evict_backed_children(d2);
        let (l0, n0, c0) = (vfs.dcache.fs_lookups.get(), vfs.dcache.negative_hits.get(), vfs.dcache.complete_misses.get());
        let twice = vfs.stat(0, &at("d2/nothing")).is_err() && vfs.stat(0, &at("d2/nothing")).is_err();
        let (l1, n1) = (vfs.dcache.fs_lookups.get(), vfs.dcache.negative_hits.get());
        let complete = vfs.stat(0, &at("d1/nothing")).is_err();
        let (l2, c2) = (vfs.dcache.fs_lookups.get(), vfs.dcache.complete_misses.get());
        let made = vfs.create_file(d2, "nothing", FileMode::new(0o644)).is_ok()
            && vfs.stat(0, &at("d2/nothing")).is_ok();
        (twice, l1 - l0, n1 - n0, complete, l2 - l1, c2 - c0, made)
    });
    r.check(
        "a missing name is asked of the filesystem once",
        matches!(misses, (true, 1, 1, ..)),
        format!("{}: {} lookup(s), {} negative hit(s)", s.mnt, misses.1, misses.2),
    );
    r.check(
        "a complete directory answers a miss without the filesystem",
        matches!(misses, (_, _, _, true, 0, 1, _)),
        format!("{}: {} lookup(s), {} complete miss(es)", s.mnt, misses.4, misses.5),
    );
    r.check(
        "a name created after a miss is found",
        misses.6,
        String::from(s.mnt),
    );

    // Unlinked while open: hidden from a lookup and from a listing read
    // again from the disk, which keeps the name until the last close
    let held = with_vfs(|vfs| -> VfsResult<usize> {
        let fd = vfs.open(0, &at("d1/held"), OpenFlags(OpenFlags::READ | OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
        vfs.write(fd, b"still here")?;
        vfs.unlink(0, &at("d1/held"))?;
        vfs.evict_backed_children(d1);
        Ok(fd)
    });
    let Ok(fd) = held else {
        r.check("a file unlinked while open is hidden", false, format!("{}: {:?}", s.mnt, held.err()));
        return;
    };
    let hidden = with_vfs(|vfs| {
        let l0 = vfs.dcache.fs_lookups.get();
        let gone = vfs.stat(0, &at("d1/held")).is_err();
        (gone, vfs.dcache.fs_lookups.get() - l0)
    });
    let l1 = listed(d1);
    let refused = with_vfs(|vfs| {
        (vfs.create_file(d1, "held", FileMode::new(0o644)).err(), vfs.rename(0, &at("d1/a"), &at("d1/held")).err())
    });
    let reads = read_whole(fd);
    r.check(
        "a file unlinked while open is hidden from lookups and listings",
        hidden == (true, 0) && l1 == ["a"],
        format!("{}: not found {} with {} lookup(s), lists {:?}", s.mnt, hidden.0, hidden.1, l1),
    );
    r.check(
        "and its name stays taken, and it reads, until it is closed",
        refused == (Some(VfsError::AlreadyExists), Some(VfsError::Busy)) && reads.as_deref() == Ok(&b"still here"[..]),
        format!("{}: create {:?}, rename onto it {:?}, reads {:?}", s.mnt, refused.0, refused.1, reads.map(|b| b.len())),
    );
    let free = with_vfs(|vfs| -> VfsResult<()> {
        vfs.close(fd)?;
        vfs.create_file(d1, "held", FileMode::new(0o644))?;
        vfs.unlink(0, &at("d1/held"))
    });
    r.check(
        "its name is free once it is closed",
        free.is_ok(),
        format!("{}: {:?}", s.mnt, free),
    );
}

/// Mount a `MemFs` and graft it, so the checks above can be asked of a second
/// filesystem on a one-disk machine.
///
/// The graft is written out field by field: `graft_ext_dir`, the only grafting
/// helper, reads ext's mount table for the slot and the flavour.
/// `btrfs::selftest` has the same lines
fn mount_memfs(r: &mut Runner) -> Option<(u8, usize)> {
    let Some(slot) = memfs::mount_new() else {
        r.check("mount a filesystem with no disk", false, String::from("no free memfs slot"));
        return None;
    };
    let Some(root_node) = memfs::with_memfs_for(slot, |fs| fs.root().as_ino()) else {
        r.check("mount a filesystem with no disk", false, String::from("slot went away"));
        return None;
    };
    r.check(
        "mount a filesystem with no disk",
        true,
        format!("memfs slot {}, root node {}", slot, root_node),
    );

    let grafted = with_vfs(|vfs| -> VfsResult<usize> {
        let id = match vfs.resolve_path(0, MEM_MNT) {
            Ok(id) => id,
            Err(_) => vfs.mkdir(0, "memtest", FileMode::new(0o755))?,
        };
        vfs.nodes[id].fs_type = FsType::MemFs;
        vfs.nodes[id].ext2_ino = root_node;
        vfs.nodes[id].ext2_slot = slot;
        vfs.nodes[id].children_loaded = false;
        // `ext2_mount_active` is not set: it is ext's flag, and
        // `lazy_lookup_backed` consults it only for a parent on ext
        Ok(id)
    });
    match grafted {
        Ok(id) => {
            r.check("graft it at a second path", true, format!("{} is vnode {}", MEM_MNT, id));
            Some((slot, id))
        }
        Err(e) => {
            r.check("graft it at a second path", false, format!("{:?}", e));
            memfs::unmount_slot(slot);
            None
        }
    }
}

/// Unmount memfs and return the mount point to an ordinary empty directory.
///
/// A standing graft leaves a path answering `NotMounted` and spends a mount
/// slot per run
fn unmount_memfs(slot: u8, mount_id: usize) {
    with_vfs(|vfs| {
        vfs.evict_backed_children(mount_id);
        vfs.nodes[mount_id].fs_type = FsType::TmpFS;
        vfs.nodes[mount_id].ext2_ino = 0;
        vfs.nodes[mount_id].ext2_slot = INVALID_U8;
        vfs.nodes[mount_id].children_loaded = true;
    });
    memfs::unmount_slot(slot);
}

/// A directory over 256 entries.
///
/// The names are made on the filesystem directly, so the VFS reads a directory
/// it did not build, as on a real disk.
fn big_directory(r: &mut Runner, s: &Subject) {
    let made = with_vfs(|vfs| -> VfsResult<usize> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        let d = vfs.mkdir(dir, "many", FileMode::new(0o755))?;
        for i in 0..MANY {
            let name = format!("f{:04}", i);
            vfs.create_file(d, &name, FileMode::new(0o644))?;
        }
        Ok(d)
    });
    let dir = match made {
        Ok(d) => d,
        Err(e) => {
            r.check("make a directory of 300 names", false, format!("{}: {:?}", s.mnt, e));
            return;
        }
    };
    r.check(
        "make a directory of 300 names",
        true,
        format!("{}: {} files", s.mnt, MANY),
    );

    // Drop the tree's picture of it and make the VFS rebuild it from the
    // filesystem. Otherwise the children are the vnodes just created and the
    // fill path does not run
    with_vfs(|vfs| vfs.evict_backed_children(dir));

    // `readdir` synthesises `.` and `..` ahead of the children: two more than
    // the names on the filesystem
    let mut entries = vec![DirEntry::empty(); MANY + 64];
    let n = with_vfs(|vfs| vfs.readdir(dir, &mut entries));
    match n {
        Ok(count) => r.check(
            "every name in a 300-entry directory comes back",
            count == MANY + 2,
            format!("{}: {} of {} plus . and ..", s.mnt, count, MANY),
        ),
        Err(e) => r.check(
            "every name in a 300-entry directory comes back",
            false,
            format!("{}: {:?}", s.mnt, e),
        ),
    }

    // What a warm listing costs a name inside the kernel, in the batches of 32
    // libmiku's `readdir` asks for: stdbench's `read_dir entry` against this
    // says how much of it is the kernel's
    let mut batch = vec![DirEntry::empty(); 32];
    let watch = crate::kcore::time::Stopwatch::start();
    let mut listed = 0usize;
    for _ in 0..20 {
        let mut at = 0;
        loop {
            let got = with_vfs(|vfs| vfs.readdir_at(dir, at, &mut batch)).unwrap_or(0);
            if got == 0 {
                break;
            }
            at += got;
            listed += got;
        }
    }
    let ns = watch.elapsed_us() * 1000 / listed.max(1) as u64;
    r.check(
        "a warm listing's cost a name, inside the kernel",
        listed == 20 * (MANY + 2),
        format!("{}: {} ns a name over {} names, in batches of 32", s.mnt, ns, listed),
    );

    // Reading it once has to settle it; otherwise the directory is re-read from
    // the disk on every path resolution through it
    let settled = with_vfs(|vfs| vfs.children_are_loaded(dir));
    r.check(
        "reading it once is enough",
        settled,
        format!("{}: children_loaded = {}", s.mnt, settled),
    );

    // Hand the vnodes back
    with_vfs(|vfs| vfs.evict_backed_children(dir));
}

/// A walk under `with_vfs_walk` reads what the cache does not have with the VFS
/// lock released (a name, then a whole listing) and gets what the walk under
/// the lock gets (`dev/vfs-locking.md`, "4d"). Over `big_directory`'s 300
/// names, which it leaves evicted
fn walks_read_unlocked(r: &mut Runner, s: &Subject) {
    use crate::fs::vfs::core::with_vfs_walk;
    let at = |p: &str| format!("{}/{}", s.mnt, p);
    let dir = match with_vfs(|vfs| vfs.resolve_path(0, &at("many"))) {
        Ok(d) => d,
        Err(e) => {
            r.check("a cold stat reads its name with the VFS lock let go", false, format!("{}: {:?}", s.mnt, e));
            return;
        }
    };
    let reads = || with_vfs(|vfs| vfs.dcache.deferred_reads.get());

    with_vfs(|vfs| vfs.evict_backed_children(dir));
    let r0 = reads();
    let st = with_vfs_walk(|vfs| vfs.stat(0, &at("many/f0123")).map(|st| st.kind));
    let missing = with_vfs_walk(|vfs| vfs.stat(0, &at("many/f9999")).map(|st| st.kind));
    let r1 = reads();
    r.check(
        "a cold stat reads its name with the VFS lock let go",
        st == Ok(VNodeKind::Regular) && missing == Err(VfsError::NotFound) && r1 - r0 == 2,
        format!("{}: {:?}, missing {:?}, {} deferred read(s)", s.mnt, st, missing, r1 - r0),
    );

    with_vfs(|vfs| vfs.evict_backed_children(dir));
    let mut entries = vec![DirEntry::empty(); MANY + 64];
    let n = with_vfs_walk(|vfs| vfs.readdir(dir, &mut entries));
    let r2 = reads();
    let loaded = with_vfs(|vfs| vfs.children_are_loaded(dir));
    r.check(
        "a cold listing reads the directory with the VFS lock let go",
        n == Ok(MANY + 2) && loaded && r2 > r1,
        format!("{}: {:?} of {} plus . and .., complete {}, {} deferred read(s)", s.mnt, n, MANY, loaded, r2 - r1),
    );
    with_vfs(|vfs| vfs.evict_backed_children(dir));
}

/// Two hundred thousand files, made and walked as a program would: one path per
/// call, each call its own `with_vfs`.
///
/// The table has no size (`vfs::table`), and `core::reclaim` gives back cached
/// vnodes. On `MemFs` only: the ext scratch disk has inodes for a few hundred
/// files, and eviction asks nothing of the filesystem beyond the `FileSystem`
/// trait
fn beyond_the_table(r: &mut Runner, s: &Subject) {
    const DIRS: usize = 100;
    const FILES: usize = 2000;
    let total = DIRS * FILES;

    let made = (|| -> Result<(), String> {
        with_vfs(|vfs| {
            let base = vfs.resolve_path(0, s.mnt)?;
            vfs.mkdir(base, "wide", FileMode::new(0o755)).map(|_| ())
        })
        .map_err(|e| format!("wide: {:?}", e))?;
        for d in 0..DIRS {
            let dir = format!("{}/wide/d{:02}", s.mnt, d);
            with_vfs(|vfs| -> VfsResult<()> {
                let p = vfs.resolve_path(0, &format!("{}/wide", s.mnt))?;
                vfs.mkdir(p, &format!("d{:02}", d), FileMode::new(0o755)).map(|_| ())
            })
            .map_err(|e| format!("{}: {:?}", dir, e))?;
            for f in 0..FILES {
                let path = format!("{}/f{:04}", dir, f);
                with_vfs(|vfs| -> VfsResult<()> {
                    let fd = vfs.open(0, &path, OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
                    vfs.close(fd)
                })
                .map_err(|e| format!("{}: {:?}", path, e))?;
            }
        }
        Ok(())
    })();
    r.check(
        "make two hundred thousand files",
        made.is_ok(),
        match &made {
            Ok(()) => format!("{}: {} files in {} directories, {} vnodes cached", s.mnt, total, DIRS,
                              with_vfs(|vfs| vfs.total_vnodes())),
            Err(e) => format!("{}: {}", s.mnt, e),
        },
    );
    if made.is_err() {
        return;
    }

    // Every directory listed whole, each through a fresh lookup
    let mut short = Vec::new();
    let mut entries = vec![DirEntry::empty(); FILES + 8];
    for d in 0..DIRS {
        let dir = format!("{}/wide/d{:02}", s.mnt, d);
        let n = with_vfs(|vfs| -> VfsResult<usize> {
            let id = vfs.resolve_path(0, &dir)?;
            vfs.readdir(id, &mut entries)
        });
        if n != Ok(FILES + 2) {
            short.push(format!("d{:02}: {:?}", d, n));
        }
    }
    r.check(
        "and list every one of them back",
        short.is_empty(),
        if short.is_empty() { format!("{}: {} directories of {}", s.mnt, DIRS, FILES) } else { short.join(", ") },
    );

    // The first file made, by its name
    let first = format!("{}/wide/d00/f0000", s.mnt);
    let found = with_vfs(|vfs| vfs.resolve_path(0, &first));
    let used = with_vfs(|vfs| vfs.total_vnodes());
    // What was walked can be given back: the cache is not full of what cannot
    // go
    let gives_back = with_vfs(|vfs| vfs.reclaim(64)) > 0;
    r.check(
        "the first file is found by its name, and the cache can give back",
        found.is_ok() && gives_back,
        format!("{}: {:?}, {} vnodes cached, given back: {}", first, found, used, gives_back),
    );
}

/// The name bug. 200 bytes is past the old 64 and inside ext's 255.
fn long_name(r: &mut Runner, s: &Subject) {
    let name = "n".repeat(200);

    let created = with_vfs(|vfs| -> VfsResult<()> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        vfs.create_file(dir, &name, FileMode::new(0o644))?;
        Ok(())
    });
    if let Err(e) = created {
        r.check("create a 200-byte name", false, format!("{}: {:?}", s.mnt, e));
        return;
    }
    r.check("create a 200-byte name", true, format!("{}: 200 bytes", s.mnt));

    let path = format!("{}/{}", s.mnt, name);
    let found = with_vfs(|vfs| vfs.resolve_path(0, &path));
    r.check(
        "resolve it by its whole name",
        found.is_ok(),
        format!("{}: {:?}", s.mnt, found.err()),
    );

    // The vnode's cached name is handed back to the filesystem here; truncated,
    // it would name a file that does not exist
    let removed = with_vfs(|vfs| vfs.unlink(0, &path));
    r.check(
        "unlink it by its whole name",
        removed.is_ok(),
        format!("{}: {:?}", s.mnt, removed.err()),
    );

    let gone = with_vfs(|vfs| vfs.resolve_path(0, &path)).is_err();
    r.check("and it is gone", gone, String::from(s.mnt));
}

/// A symlink target longer than a name: it is a path
fn long_symlink(r: &mut Runner, s: &Subject) {
    let target = alloc::string::String::from("/usr/lib/") + &"d".repeat(900);

    let made = with_vfs(|vfs| -> VfsResult<()> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        vfs.symlink(dir, "deep", &target)?;
        Ok(())
    });
    if let Err(e) = made {
        r.check("create a 909-byte symlink target", false, format!("{}: {:?}", s.mnt, e));
        return;
    }

    let path = format!("{}/deep", s.mnt);
    let read = with_vfs(|vfs| vfs.readlink(0, &path));
    match read {
        Ok(bytes) => r.check(
            "a long symlink target survives whole",
            bytes == target.as_bytes(),
            format!("{}: {} bytes back of {}", s.mnt, bytes.len(), target.len()),
        ),
        Err(e) => r.check(
            "a long symlink target survives whole",
            false,
            format!("{}: {:?}", s.mnt, e),
        ),
    }

    // From the filesystem, not from the vnode just built
    let from_disk = with_vfs(|vfs| -> VfsResult<Vec<u8>> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        vfs.evict_backed_children(dir);
        vfs.readlink(0, &path)
    });
    match from_disk {
        Ok(bytes) => r.check(
            "and again after it is re-read from the filesystem",
            bytes == target.as_bytes(),
            format!("{}: {} bytes back of {}", s.mnt, bytes.len(), target.len()),
        ),
        Err(e) => r.check(
            "and again after it is re-read from the filesystem",
            false,
            format!("{}: {:?}", s.mnt, e),
        ),
    }
}

/// Cross-directory rename: ext has two calls and the filesystem picks. The
/// wrong one leaves `..` pointing at the old parent
fn rename_across_directories(r: &mut Runner, s: &Subject) {
    let done = with_vfs(|vfs| -> VfsResult<()> {
        let root = vfs.resolve_path(0, s.mnt)?;
        let a = vfs.mkdir(root, "a", FileMode::new(0o755))?;
        let _b = vfs.mkdir(root, "b", FileMode::new(0o755))?;
        vfs.create_file(a, "moved", FileMode::new(0o644))?;
        vfs.rename(0, &format!("{}/a/moved", s.mnt), &format!("{}/b/moved", s.mnt))
    });
    r.check(
        "a file moves between directories",
        done.is_ok(),
        format!("{}: {:?}", s.mnt, done.err()),
    );

    let there = with_vfs(|vfs| vfs.resolve_path(0, &format!("{}/b/moved", s.mnt))).is_ok();
    let not_here = with_vfs(|vfs| vfs.resolve_path(0, &format!("{}/a/moved", s.mnt))).is_err();
    r.check("it is at the new name", there, String::from(s.mnt));
    r.check("and not at the old one", not_here, String::from(s.mnt));

    // A directory moving carries `..`, which a plain rename does not
    let dir_moved = with_vfs(|vfs| -> VfsResult<()> {
        let root = vfs.resolve_path(0, s.mnt)?;
        let a = vfs.resolve_path(root, "a")?;
        vfs.mkdir(a, "sub", FileMode::new(0o755))?;
        vfs.rename(0, &format!("{}/a/sub", s.mnt), &format!("{}/b/sub", s.mnt))
    });
    r.check(
        "a directory moves between directories",
        dir_moved.is_ok(),
        format!("{}: {:?}", s.mnt, dir_moved.err()),
    );
}

/// An owner past 16 bits. The disk can hold it, this kernel's credentials
/// cannot; the fold has to be deliberate, not a truncation that hands the file
/// to someone who exists
fn wide_owner(r: &mut Runner, s: &Subject) {
    let path = format!("{}/owned", s.mnt);
    let made = with_vfs(|vfs| -> VfsResult<()> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        vfs.create_file(dir, "owned", FileMode::new(0o644))?;
        vfs.chown(0, &path, Some(1000), Some(100))
    });
    r.check("chown a file", made.is_ok(), format!("{}: {:?}", s.mnt, made.err()));

    let owner = with_vfs(|vfs| -> VfsResult<(u16, u16)> {
        let dir = vfs.resolve_path(0, s.mnt)?;
        vfs.evict_backed_children(dir);
        let st = vfs.stat(0, &path)?;
        Ok((st.uid, st.gid))
    });
    match owner {
        Ok((uid, gid)) => r.check(
            "the owner survives a trip to the filesystem",
            uid == 1000 && gid == 100,
            format!("{}: uid {} gid {}", s.mnt, uid, gid),
        ),
        Err(e) => r.check(
            "the owner survives a trip to the filesystem",
            false,
            format!("{}: {:?}", s.mnt, e),
        ),
    }
}

/// `statfs` asks the filesystem, not the stamp on the vnode
fn which_filesystem(r: &mut Runner, s: &Subject) {
    let st = with_vfs(|vfs| vfs.statfs(0, s.mnt));
    match st {
        Ok(st) => r.check(
            "statfs reports the filesystem that is actually there",
            st.fs_type == s.fs_type
                && st.block_size == s.block_size
                && st.max_name_len >= 255,
            format!(
                "{}: {:?}, {}-byte blocks, names to {}",
                s.mnt, st.fs_type, st.block_size, st.max_name_len
            ),
        ),
        Err(e) => r.check(
            "statfs reports the filesystem that is actually there",
            false,
            format!("{}: {:?}", s.mnt, e),
        ),
    }
}

/// With two filesystems mounted, does either one ever answer for the other?
///
/// Both mounts are given the same shape, a directory `both` holding a file
/// `witness`, so a lookup that went to the wrong filesystem still finds
/// something. Only the flavour stamped on the vnode and the bytes inside tell
/// the two files apart, and both are checked.
///
/// A child's flavour comes from its parent. Taken from ext's mount table, every
/// child of the memfs mount would be stamped `Ext4` while carrying a memfs node
/// number, and the read would go to the ext driver with a number that means
/// something else there. The mount points are stamped by hand at graft time and
/// would look right either way, so everything checked is a child or grandchild,
/// rebuilt by the fill path after the tree has forgotten it.
fn no_contamination(r: &mut Runner) {
    const EXT_SAYS: &str = "this file is on the ext4 mount\n";
    const MEM_SAYS: &str = "this file is on the memfs mount\n";

    let staged = with_vfs(|vfs| -> VfsResult<()> {
        for (mnt, body) in [(MNT, EXT_SAYS), (MEM_MNT, MEM_SAYS)] {
            let root = vfs.resolve_path(0, mnt)?;
            let dir = vfs.mkdir(root, "both", FileMode::new(0o755))?;
            let file = vfs.create_file(dir, "witness", FileMode::new(0o644))?;
            let n = vfs.write_at_vnode(file, 0, body.as_bytes())?;
            if n != body.len() {
                return Err(VfsError::IoError);
            }
        }
        // One name that exists on exactly one of them
        let mem = vfs.resolve_path(0, MEM_MNT)?;
        vfs.create_file(mem, "only-in-memfs", FileMode::new(0o644))?;
        Ok(())
    });
    if let Err(e) = staged {
        r.check("stage the same names on both mounts", false, format!("{:?}", e));
        return;
    }
    r.check(
        "stage the same names on both mounts",
        true,
        format!("{0}/both/witness and {1}/both/witness", MNT, MEM_MNT),
    );

    // Make the tree forget both subtrees, so every lookup below is the fill
    // path reading from a filesystem, not a vnode just created from the other
    // side
    let forgotten = with_vfs(|vfs| -> VfsResult<(bool, bool)> {
        let ext = vfs.resolve_path(0, MNT)?;
        let mem = vfs.resolve_path(0, MEM_MNT)?;
        vfs.evict_backed_children(ext);
        vfs.evict_backed_children(mem);
        Ok((vfs.children_are_loaded(ext), vfs.children_are_loaded(mem)))
    });
    match forgotten {
        Ok((ext_loaded, mem_loaded)) => r.check(
            "the tree forgets both mounts first",
            !ext_loaded && !mem_loaded,
            format!("children_loaded: ext {} memfs {}", ext_loaded, mem_loaded),
        ),
        Err(e) => {
            r.check("the tree forgets both mounts first", false, format!("{:?}", e));
            return;
        }
    }

    // The flavour, on nodes the fill path just rebuilt. A child is on its
    // parent's filesystem and on no other
    let stamps = with_vfs(|vfs| -> VfsResult<(FsType, FsType, FsType, FsType)> {
        let ext_dir = vfs.stat(0, &format!("{}/both", MNT))?.fs_type;
        let mem_dir = vfs.stat(0, &format!("{}/both", MEM_MNT))?.fs_type;
        let ext_file = vfs.stat(0, &format!("{}/both/witness", MNT))?.fs_type;
        let mem_file = vfs.stat(0, &format!("{}/both/witness", MEM_MNT))?.fs_type;
        Ok((ext_dir, mem_dir, ext_file, mem_file))
    });
    match stamps {
        Ok((ext_dir, mem_dir, ext_file, mem_file)) => {
            r.check(
                "a directory under each mount keeps its own filesystem",
                ext_dir == FsType::Ext4 && mem_dir == FsType::MemFs,
                format!("{:?} under {} and {:?} under {}", ext_dir, MNT, mem_dir, MEM_MNT),
            );
            r.check(
                "and so does a file one level below that",
                ext_file == FsType::Ext4 && mem_file == FsType::MemFs,
                format!("{:?} under {} and {:?} under {}", ext_file, MNT, mem_file, MEM_MNT),
            );
        }
        Err(e) => {
            r.check(
                "a directory under each mount keeps its own filesystem",
                false,
                format!("{:?}", e),
            );
            return;
        }
    }

    // The bytes, which a flavour stamp cannot fake: same name in the same place
    // on two filesystems, so a read from the wrong one returns the wrong
    // sentence
    let ext_read = with_vfs(|vfs| vfs.read_path(&format!("{}/both/witness", MNT)));
    let mem_read = with_vfs(|vfs| vfs.read_path(&format!("{}/both/witness", MEM_MNT)));
    match (ext_read, mem_read) {
        (Ok(e), Ok(m)) => r.check(
            "each witness reads back its own filesystem's bytes",
            e == EXT_SAYS.as_bytes() && m == MEM_SAYS.as_bytes(),
            format!(
                "{:?} / {:?}",
                core::str::from_utf8(&e).unwrap_or("?").trim_end(),
                core::str::from_utf8(&m).unwrap_or("?").trim_end()
            ),
        ),
        (e, m) => r.check(
            "each witness reads back its own filesystem's bytes",
            false,
            format!("{:?} / {:?}", e.err(), m.err()),
        ),
    }

    // A name only one of them has must not be found on the other
    let here = with_vfs(|vfs| vfs.resolve_path(0, &format!("{}/only-in-memfs", MEM_MNT)));
    let there = with_vfs(|vfs| vfs.resolve_path(0, &format!("{}/only-in-memfs", MNT)));
    r.check(
        "a name on one mount is not found on the other",
        here.is_ok() && there.is_err(),
        format!("{:?} on memfs, {:?} on ext", here.err(), there.err()),
    );

    // Moving between them is a copy, not a rename, and the VFS refuses it
    // before either filesystem is asked
    let across = with_vfs(|vfs| {
        vfs.rename(
            0,
            &format!("{}/only-in-memfs", MEM_MNT),
            &format!("{}/only-in-memfs", MNT),
        )
    });
    let survived = with_vfs(|vfs| vfs.resolve_path(0, &format!("{}/only-in-memfs", MEM_MNT))).is_ok();
    r.check(
        "a rename from one mount to the other is refused",
        matches!(across, Err(VfsError::CrossDevice)) && survived,
        format!("{:?}, still on memfs: {}", across.err(), survived),
    );
}

pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "vfs: selftest");
    let report = run();

    for c in &report.cases {
        if c.skipped {
            crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!(
            "[vfs-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[vfs-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("vfs: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("vfs: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}

/// The vnode cache has no size: it gives back what a shrink asks, through its
/// shrinker, and keeps what something holds (a working directory, a mapping, a
/// watch). An unlinked file lives while it is mapped, and goes with its last
/// mapping. On the wide tree `beyond_the_table` made
fn the_cache_gives_back(r: &mut Runner, s: &Subject) {
    use crate::fs::vfs::core::VNODE_SHRINKER;
    // A mapping table no address space has: a file mapping is a VMA there,
    // and `vma_cleanup` takes all of it
    const NOBODY: u64 = 0x7E57_0000_0000;
    let vnode_slabs = || {
        let mut bytes = 0;
        crate::mm::heap::each_cache(|c| if c.name == "vnode" { bytes = c.slabs * c.pages_per_slab * 4096 });
        bytes
    };
    let (cwd_path, mapped_path, watched_path) = (
        format!("{}/wide/d05", s.mnt),
        format!("{}/wide/d06/f0001", s.mnt),
        format!("{}/wide/d07/f0002", s.mnt),
    );
    let held = with_vfs(|vfs| -> VfsResult<(usize, usize, usize, usize, usize)> {
        let c = vfs.resolve_path(0, &cwd_path)?;
        vfs.change_cwd(c);
        let m = vfs.resolve_path(0, &mapped_path)?;
        let w = vfs.resolve_path(0, &watched_path)?;
        let fd = vfs.inotify_init(0)?;
        let inst = vfs.inotify_of(fd)?;
        Ok((c, m, w, fd, inst))
    });
    let Ok((c, m, w, fd, inst)) = held else {
        r.check("hold a directory, a file and a file", false, format!("{:?}", held.err()));
        return;
    };
    let mapped = crate::mm::mmap::sys_mmap_file(NOBODY, 0, 4096, 1, 0, m as u32, 0, 4096, false) > 0;
    let watched = crate::fs::vfs::notify::add_watch(inst, w, crate::fs::vfs::notify::IN_MODIFY).is_ok();

    let before = with_vfs(|vfs| vfs.total_vnodes());
    let slabs_before = vnode_slabs();
    let frames = (VNODE_SHRINKER.scan)(usize::MAX, false);
    let after = with_vfs(|vfs| vfs.total_vnodes());
    let slabs_after = vnode_slabs();
    r.check(
        "the cache gives back what a shrink asks",
        before > 200_000 && after < before / 10 && slabs_after < slabs_before / 4,
        format!("{} vnodes to {}, {} KiB of vnode slabs to {} KiB, {} frames by the shrinker's count",
                before, after, slabs_before / 1024, slabs_after / 1024, frames),
    );
    let first = format!("{}/wide/d00/f0000", s.mnt);
    let found = with_vfs(|vfs| vfs.resolve_path(0, &first).map(|id| vfs.nodes[id].get_name() == "f0000"));
    r.check(
        "an evicted file is found again by its name",
        found == Ok(true),
        format!("{}: {:?}", first, found),
    );
    let kept = with_vfs(|vfs| {
        let named = |id: usize, name: &str| vfs.valid_vnode(id) && vfs.nodes[id].get_name() == name;
        (named(c, "d05"), named(m, "f0001"), named(w, "f0002"))
    });
    r.check(
        "and keeps what is held",
        mapped && watched && kept == (true, true, true),
        format!("working directory {}, mapped {} ({}), watched {} ({})", kept.0, kept.1, mapped, kept.2, watched),
    );
    // Its directories above it have a cached child each, so they stay too
    let cwd_now = with_vfs(|vfs| vfs.abs_path(c));
    r.check(
        "the working directory's path survives the shrink",
        cwd_now == cwd_path,
        format!("{} (was {})", cwd_now, cwd_path),
    );

    // Each holder lets go
    with_vfs(|vfs| {
        vfs.change_cwd(0);
        let _ = vfs.close(fd);
    });
    crate::mm::mmap::vma_cleanup(NOBODY);

    // An unlinked file, mapped: alive until its last mapping goes
    let gone_path = format!("{}/wide/gone", s.mnt);
    let g = with_vfs(|vfs| -> VfsResult<usize> {
        let fd = vfs.open(0, &gone_path, OpenFlags(OpenFlags::WRITE | OpenFlags::CREATE), FileMode::new(0o644))?;
        vfs.close(fd)?;
        vfs.resolve_path(0, &gone_path)
    });
    let Ok(g) = g else {
        r.check("an unlinked file lives while it is mapped", false, format!("{:?}", g));
        return;
    };
    let mapped = crate::mm::mmap::sys_mmap_file(NOBODY, 0, 4096, 1, 0, g as u32, 0, 4096, false) > 0;
    let unlinked = with_vfs(|vfs| vfs.unlink(0, &gone_path));
    let alive = with_vfs(|vfs| vfs.valid_vnode(g));
    crate::mm::mmap::vma_cleanup(NOBODY);
    // The next operation frees it (`reclaim::release_unmapped`)
    let freed = with_vfs(|vfs| !vfs.valid_vnode(g) || vfs.nodes[g].nlinks > 0);
    r.check(
        "an unlinked file lives while it is mapped, and goes with its last mapping",
        mapped && unlinked.is_ok() && alive && freed,
        format!("mapped {}, unlinked {:?}, alive while mapped {}, gone after {}", mapped, unlinked, alive, freed),
    );
}
