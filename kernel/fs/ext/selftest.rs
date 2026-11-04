// The ext driver, tested on the machine
//
// Most tests of this driver are on the host, where a filesystem can be built in
// memory, corrupted and crashed mid-transaction: `cd lib/extfs && cargo test`.
// New tests belong there by default.
//
// Here is what the host cannot reach:
//
//   - two filesystems mounted at once. The routing that sends an operation to
//     the right one is in the kernel's mount table, and an inode number
//     resolved against the wrong filesystem reads a different file
//   - the real block layer underneath: a device registered, written through,
//     flushed and read back
//   - the kernel's own mkfs writing to a real device, and the driver then
//     mounting what it wrote
//
// Reached as `ext test` from the shell, or `run=ext_test` on the GRUB line.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::fs::ext::fsck::FsckOptions;
use crate::fs::ext::mkfs::{mkfs, FsType, MkfsParams};
use crate::fs::ext::mount;
use crate::fs::vfs::types::{BlockDevId, INVALID_U8};
use crate::kcore::selftest::{Report, Runner};

/// Big enough for a filesystem with a journal, small enough for two on the heap
const DISK_BYTES: usize = 8 * 1024 * 1024;

/// A block device backed by a buffer this test owns.
///
/// The buffer is leaked: the block layer holds the device in a registry with no
/// lifetime, and freeing the memory under a registered device would be a
/// use-after-free
struct ScratchDisk {
    dev: BlockDevId,
}

impl ScratchDisk {
    fn new() -> Option<Self> {
        let buf = vec![0u8; DISK_BYTES].into_boxed_slice();
        let ptr = Box::leak(buf);
        let dev = crate::io::block::register_ramdisk_virt(
            ptr.as_ptr() as u64,
            DISK_BYTES as u64,
        );
        if dev == INVALID_U8 {
            return None;
        }
        Some(ScratchDisk { dev })
    }
}

use alloc::boxed::Box;

/// Which mount slot a device ended up in
fn slot_of(dev: BlockDevId) -> Option<usize> {
    (0..mount::MAX_MOUNTS).find(|&s| mount::slot_drive(s) == Some(dev as usize))
}

fn format_and_mount(r: &mut Runner, disk: &ScratchDisk, kind: FsType) -> Option<usize> {
    let mut params = MkfsParams::new(kind, disk.dev as usize);
    params.total_sectors = (DISK_BYTES / 512) as u32;
    if kind == FsType::Ext4 {
        params.block_size = 1024;
        params.inode_size = 256;
    }
    if let Err(e) = mkfs(crate::fs::host::KernelHost::new(disk.dev), &params) {
        r.check("mkfs on a real device", false, format!("{:?}", e));
        return None;
    }
    if !mount::mount_dev(disk.dev) {
        r.check("mount what mkfs wrote", false, String::from("mount refused"));
        return None;
    }
    slot_of(disk.dev)
}

pub fn run() -> Report {
    let mut r = Runner::new();

    let Some(disk_a) = ScratchDisk::new() else {
        r.skip("scratch devices", String::from("no free block device slot"));
        return r.report();
    };
    let Some(disk_b) = ScratchDisk::new() else {
        r.skip("scratch devices", String::from("only one block device slot free"));
        return r.report();
    };

    r.check(
        "two scratch devices are two devices",
        disk_a.dev != disk_b.dev,
        format!("dev {} and dev {}", disk_a.dev, disk_b.dev),
    );

    // the kernel's mkfs writing to a real device, and mounting it back
    let slot_a = format_and_mount(&mut r, &disk_a, FsType::Ext2);
    let slot_b = format_and_mount(&mut r, &disk_b, FsType::Ext4);

    let (Some(slot_a), Some(slot_b)) = (slot_a, slot_b) else {
        r.check(
            "mkfs and mount on real devices",
            false,
            String::from("one of the two filesystems did not come up"),
        );
        cleanup(&disk_a, &disk_b);
        return r.report();
    };

    r.check(
        "mkfs and mount on real devices",
        slot_a != slot_b,
        format!("ext2 in slot {}, ext4 in slot {}", slot_a, slot_b),
    );

    // Each slot reports its own flavour
    //
    // The type is inferred from the feature bits of one superblock. With the
    // firmware store mounted first, reading it off the active slot would report
    // every file on an ext4 root as ext2
    {
        use crate::fs::vfs::types::FsType;
        let a = mount::fs_type_of(slot_a as u8);
        let b = mount::fs_type_of(slot_b as u8);
        r.check(
            "each slot reports its own flavour",
            a == Some(FsType::Ext2) && b == Some(FsType::Ext4),
            format!(
                "slot {} is {:?}, slot {} is {:?}",
                slot_a, a, slot_b, b
            ),
        );
    }

    // Two filesystems at once: the same name in each, different contents
    let mut ok = true;
    let mut detail = String::new();

    let ino_a = mount::with_ext2_for(slot_a as u8, |fs| {
        fs.ext2_create_file(2, "same-name.txt", 0o644)
    });
    let ino_b = mount::with_ext2_for(slot_b as u8, |fs| {
        fs.ext4_create_file(2, "same-name.txt", 0o644)
    });

    match (ino_a, ino_b) {
        (Some(Ok(a)), Some(Ok(b))) => {
            let wa = mount::with_ext2_for(slot_a as u8, |fs| {
                fs.ext2_write_file(a, b"this one is on the ext2 disk\n", 0)
            });
            let wb = mount::with_ext2_for(slot_b as u8, |fs| {
                fs.ext4_write_file(b, b"and this one is on the ext4 disk\n", 0)
            });
            if !matches!(wa, Some(Ok(_))) || !matches!(wb, Some(Ok(_))) {
                ok = false;
                detail = String::from("writing to one of the two failed");
            } else {
                // Read each back through its own slot
                let ra = read_back(slot_a, a);
                let rb = read_back(slot_b, b);
                if ra.as_deref() != Some(b"this one is on the ext2 disk\n".as_ref())
                    || rb.as_deref() != Some(b"and this one is on the ext4 disk\n".as_ref())
                {
                    ok = false;
                    detail = String::from("a file read back as the other filesystem's");
                } else {
                    detail = format!("inode {} on slot {}, inode {} on slot {}", a, slot_a, b, slot_b);
                }
            }
        }
        _ => {
            ok = false;
            detail = String::from("could not create a file on both");
        }
    }
    r.check("two filesystems, one name, no confusion", ok, detail);

    // What reaches the medium: sync, then read the device directly
    let mut ok = true;
    let mut detail = String::new();
    match mount::with_ext2_for(slot_a as u8, |fs| fs.sync()) {
        Some(Ok(())) => {
            // The superblock magic, read off the device, not through the
            // filesystem's cache
            let mut sector = [0u8; 512];
            match crate::io::block::read(disk_a.dev, 2, 1, &mut sector) {
                Ok(()) => {
                    let magic = u16::from_le_bytes([sector[56], sector[57]]);
                    ok = magic == 0xEF53;
                    detail = format!("superblock magic on the medium: {:#06x}", magic);
                }
                Err(e) => {
                    ok = false;
                    detail = format!("reading the device back: {:?}", e);
                }
            }
        }
        other => {
            ok = false;
            detail = format!("sync: {:?}", other.is_some());
        }
    }
    r.check("sync reaches the medium", ok, detail);

    // Cache pressure: more dirty blocks than the cache holds, with a
    // transaction open, so the pinned ones may not be evicted
    let mut ok = true;
    let mut detail = String::new();
    let dir = mount::with_ext2_for(slot_a as u8, |fs| fs.ext2_create_dir(2, "pressure", 0o755));
    match dir {
        Some(Ok(dir)) => {
            let mut created = 0u32;
            for i in 0..200u32 {
                let name = format!("f{:03}.bin", i);
                let made = mount::with_ext2_for(slot_a as u8, |fs| {
                    let ino = fs.ext2_create_file(dir, &name, 0o644)?;
                    let body = [b'x'; 900];
                    fs.ext2_write_file(ino, &body, 0)?;
                    Ok::<(), crate::fs::ext::FsError>(())
                });
                match made {
                    Some(Ok(())) => created += 1,
                    Some(Err(e)) => {
                        ok = false;
                        detail = format!("file {} of 200: {:?}", i, e);
                        break;
                    }
                    None => {
                        ok = false;
                        detail = String::from("slot went away mid-test");
                        break;
                    }
                }
            }
            if ok {
                // Every one has to be findable afterwards: nothing lost to an
                // eviction
                let mut found = 0u32;
                for i in 0..created {
                    let name = format!("f{:03}.bin", i);
                    if let Some(Ok(Some(_))) =
                        mount::with_ext2_for(slot_a as u8, |fs| fs.ext2_lookup_in_dir(dir, &name))
                    {
                        found += 1;
                    }
                }
                ok = found == created;
                detail = format!("{} of {} files still found", found, created);
            }
        }
        other => {
            ok = false;
            detail = format!("could not create the directory: {}", other.is_some());
        }
    }
    r.check("200 files through a cache that cannot hold them", ok, detail);

    // A directory that outgrows one block, on a real device
    //
    // The host tests the index thoroughly. This checks it with the kernel's
    // block cache and journal underneath, where a block staged and not yet
    // committed is read back from somewhere other than the disk
    let mut ok = false;
    let mut detail = String::new();
    match mount::with_ext2_for(slot_a as u8, |fs| fs.ext2_create_dir(2, "indexed", 0o755)) {
        Some(Ok(dir)) => {
            // Keep going until the directory needs a second block, whatever the
            // block size: the index appears then
            let mut created = 0u32;
            let mut indexed = false;
            while created < 2000 && !indexed {
                let name = format!("n{:04}", created);
                let step = mount::with_ext2_for(slot_a as u8, |fs| {
                    fs.ext2_create_file(dir, &name, 0o644)?;
                    let inode = fs.read_inode(dir)?;
                    Ok::<bool, crate::fs::ext::FsError>(fs.dir_is_indexed(&inode))
                });
                match step {
                    Some(Ok(is_indexed)) => {
                        created += 1;
                        // A few more once it turns, so at least one leaf has
                        // split
                        indexed = is_indexed && created % 64 == 0;
                    }
                    other => {
                        detail = format!("file {}: {}", created, other.is_some());
                        break;
                    }
                }
            }
            let checked = mount::with_ext2_for(slot_a as u8, |fs| {
                let inode = fs.read_inode(dir).ok()?;
                if !fs.dir_is_indexed(&inode) {
                    return None;
                }
                // Every name still found, through the index: a fallback to
                // reading the whole directory would show here
                let before = fs.dx_fallbacks;
                for i in 0..created {
                    let name = format!("n{:04}", i);
                    match fs.ext2_lookup_in_dir(dir, &name) {
                        Ok(Some(_)) => {}
                        _ => return None,
                    }
                }
                Some((fs.dx_fallbacks - before, fs.htree_depth(&inode).unwrap_or(0)))
            })
            .flatten();
            match checked {
                Some((0, depth)) => {
                    ok = true;
                    detail = format!(
                        "{} names in an index {} level(s) deep, no scans",
                        created,
                        depth + 1
                    );
                }
                Some((scans, _)) => {
                    detail = format!("the index was walked around {} times", scans);
                }
                None if detail.is_empty() => {
                    detail = String::from("the directory was not indexed, or a name was lost");
                }
                None => {}
            }
        }
        other => {
            detail = format!("could not create the directory: {}", other.is_some());
        }
    }
    r.check("a directory past one block is indexed", ok, detail);

    // What a create and an unlink cost, with a journal and without
    // The ext2 scratch disk has inodes for a few hundred files
    create_cost(&mut r, slot_a, "ext2", 400);
    create_cost(&mut r, slot_b, "ext4", 2000);

    // and it all survives being unmounted and mounted again
    let mut ok = true;
    let mut detail = String::new();
    match mount::with_ext2_for(slot_a as u8, |fs| fs.sync()) {
        Some(Ok(())) => {
            mount::unmount_slot(slot_a);
            if mount::mount_dev(disk_a.dev) {
                match slot_of(disk_a.dev) {
                    Some(again) => {
                        let found = mount::with_ext2_for(again as u8, |fs| {
                            let dir = fs.ext2_lookup_in_dir(2, "pressure")?;
                            match dir {
                                Some(d) => fs.ext2_lookup_in_dir(d, "f199.bin"),
                                None => Ok(None),
                            }
                        });
                        ok = matches!(found, Some(Ok(Some(_))));
                        detail = format!("remounted into slot {}", again);
                        if !ok {
                            detail = String::from("the last file was gone after a remount");
                        }
                    }
                    None => {
                        ok = false;
                        detail = String::from("remounted into no slot");
                    }
                }
            } else {
                ok = false;
                detail = String::from("would not mount again");
            }
        }
        _ => {
            ok = false;
            detail = String::from("sync before unmount failed");
        }
    }
    r.check("a remount finds what was written", ok, detail);

    // The filesystem passes its own check
    //
    // Everything above wrote a few hundred files, an indexed directory and a
    // journal through the kernel's block layer. A block owned by nobody, a link
    // count that does not match the names, or a bitmap that disagrees with
    // either shows here
    let live = slot_of(disk_a.dev);
    let mut ok = false;
    let mut detail = String::from("no slot to check");
    if let Some(slot) = live {
        match mount::with_ext2_for(slot as u8, |fs| fs.fsck(FsckOptions::check())) {
            Some(Ok(report)) => {
                ok = report.checked && report.errors() == 0;
                detail = if ok {
                    format!(
                        "{} blocks and {} inodes used, {} directories, nothing wrong",
                        report.used_blocks, report.used_inodes, report.used_dirs
                    )
                } else {
                    first_complaint(&report)
                };
            }
            other => detail = format!("the check could not run: {}", other.is_some()),
        }
    }
    r.check("the filesystem passes its own check", ok, detail);

    // and a fault put there on purpose is found and repaired
    let mut ok = false;
    let mut detail = String::from("no slot to break");
    if let Some(slot) = live {
        let broken = mount::with_ext2_for(slot as u8, |fs| {
            // A block a live file uses, taken out of the bitmap: the next
            // allocation would hand it to somebody else
            let ino = fs.resolve_path("/pressure/f100.bin")?;
            let block = fs.get_file_block_any(ino, 0)?;
            let first = fs.superblock.first_data_block();
            let group = ((block - first) / fs.blocks_per_group) as usize;
            let index = block - first - group as u32 * fs.blocks_per_group;
            let bitmap = fs.groups[group].block_bitmap();
            fs.set_bitmap_bit(bitmap, index, false)?;
            fs.update_block_bitmap_csum(group)?;
            fs.sync()?;
            Ok::<u32, crate::fs::ext::FsError>(block)
        });
        match broken {
            Some(Ok(block)) => {
                let found = mount::with_ext2_for(slot as u8, |fs| fs.fsck(FsckOptions::check()));
                let noticed = matches!(&found, Some(Ok(r)) if r.errors() > 0);
                let fixed = mount::with_ext2_for(slot as u8, |fs| fs.fsck(FsckOptions::repair()));
                let repaired = matches!(&fixed, Some(Ok(r)) if r.fixed > 0 && r.left == 0);
                let after = mount::with_ext2_for(slot as u8, |fs| fs.fsck(FsckOptions::check()));
                let clean = matches!(&after, Some(Ok(r)) if r.errors() == 0);
                ok = noticed && repaired && clean;
                detail = format!(
                    "block {} taken out of the bitmap: noticed={} repaired={} clean after={}",
                    block, noticed, repaired, clean
                );
            }
            other => detail = format!("could not break anything: {}", other.is_some()),
        }
    }
    r.check("a bitmap fault is found and repaired", ok, detail);

    cleanup(&disk_a, &disk_b);
    r.report()
}

/// `n` files made and removed in one directory through the calls the VFS makes
/// (`ext3_create_file_as`, `ext3_delete_file`), timed, with the journal commits
/// they caused and the most blocks one transaction held
fn create_cost(r: &mut Runner, slot: usize, label: &str, n: usize) {
    let now = crate::kcore::time::now_ns;
    let name = if label == "ext2" { "ext2 create and unlink cost" } else { "ext4 create and unlink cost" };
    let dir = match mount::with_ext2_for(slot as u8, |fs| fs.ext3_create_dir(2, "costs", 0o755)) {
        Some(Ok(d)) => d,
        other => {
            r.check(name, false, format!("no directory: {}", other.is_some()));
            return;
        }
    };
    let seq = || mount::with_ext2_for(slot as u8, |fs| fs.journal_seq).unwrap_or(0);
    let tags = || mount::with_ext2_for(slot as u8, |fs| fs.running.as_ref().map_or(0, |t| t.tags.len())).unwrap_or(0);
    let s0 = seq();
    let mut most = 0;
    let t0 = now();
    let mut made = Vec::with_capacity(n);
    for i in 0..n {
        let f = format!("c{:05}", i);
        if let Some(Ok(ino)) = mount::with_ext2_for(slot as u8, |fs| fs.ext3_create_file_as(dir, &f, 0o644, 0, 0)) {
            made.push(ino);
        }
        if i % 16 == 0 {
            most = most.max(tags());
        }
    }
    let tw = now();
    // What stdbench's "create+write 1 KB" adds to the create: the first
    // block of each file
    let kb = [0x6du8; 1024];
    let mut written = 0;
    for &ino in &made {
        if matches!(mount::with_ext2_for(slot as u8, |fs| fs.ext3_write_file(ino, &kb, 0)), Some(Ok(1024))) {
            written += 1;
        }
    }
    let t1 = now();
    let s1 = seq();
    let mut gone = 0;
    for i in 0..n {
        let f = format!("c{:05}", i);
        if matches!(mount::with_ext2_for(slot as u8, |fs| fs.ext3_delete_file(dir, &f)), Some(Ok(_))) {
            gone += 1;
        }
        if i % 16 == 0 {
            most = most.max(tags());
        }
    }
    let t2 = now();
    let s2 = seq();
    r.check(name, made.len() == n && written == n && gone == n,
        format!("create {} us, write 1 KB {} us, unlink {} us a file; commits {} and {}; most blocks in one transaction {}",
            (tw - t0) / 1000 / n as u64, (t1 - tw) / 1000 / n as u64, (t2 - t1) / 1000 / n as u64,
            s1.wrapping_sub(s0), s2.wrapping_sub(s1), most));
}

/// The first thing the check complained about, for a failure line on a serial
/// console
fn first_complaint(report: &crate::fs::ext::fsck::FsckReport) -> String {
    match report.examples().first() {
        Some(f) => format!("{} ({} problems)", f.problem, report.errors()),
        None => format!("{} problems, none listed", report.errors()),
    }
}

fn read_back(slot: usize, ino: u32) -> Option<Vec<u8>> {
    mount::with_ext2_for(slot as u8, |fs| {
        let inode = fs.read_inode(ino).ok()?;
        let size = inode.size() as usize;
        let mut buf = vec![0u8; size];
        let n = fs.read_file(&inode, 0, &mut buf).ok()?;
        buf.truncate(n);
        Some(buf)
    })
    .flatten()
}

/// Unmount both and take the devices out of the registry. The buffers stay
/// leaked; see `ScratchDisk`
fn cleanup(a: &ScratchDisk, b: &ScratchDisk) {
    for d in [a.dev, b.dev] {
        if let Some(s) = slot_of(d) {
            mount::unmount_slot(s);
        }
        crate::io::block::unregister_ramdisk(d);
    }
}

/// `ext test`: run the selftest and print the result
pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "ext: selftest");
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
            "[ext-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[ext-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("ext: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("ext: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}
