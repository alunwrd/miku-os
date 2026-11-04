// btrfs on the machine
//
// `lib/btrfs`'s host tests cover the format: superblocks, the chunk map, the
// trees, reading a file back byte for byte against what `mkfs.btrfs` put there.
// Here is what they cannot reach:
//
//   - the probe registry finding btrfs on a real block device, among other
//     devices
//   - the kernel's block layer underneath, not a `Vec<u8>`
//   - the mount table, the graft, and a path resolved through `MikuVFS`
//
// It needs a device with a btrfs volume, made by `mkfs.btrfs -r` from
// `miku-os/btrfs.img`. Without one every case skips.
//
// Reached as `btrfs test` from the shell, or `run=btrfs_test` on the GRUB line.

extern crate alloc;
use alloc::format;
use alloc::string::String;
use alloc::vec;

use crate::fs::vfs::core::with_vfs;
use crate::fs::vfs::types::*;
use crate::kcore::selftest::{Report, Runner};

/// What `miku-os/btrfs.img` was built from. If the volume says something else,
/// the image was rebuilt and this was not, or the driver is misreading
const MNT: &str = "/btrfs";
const HELLO: &str = "btrfs read by MikuOS\n";
const NOTES: &str = "line one\nline two\nline three\n";
const BLOB_BYTES: u64 = 200_000;

pub fn run() -> Report {
    let mut r = Runner::new();

    // the probe registry, over the machine's real devices
    let found = crate::fs::ext::mount::scan_volumes();
    let probe = found.iter().find(|p| p.fs_type == FsType::Btrfs).cloned();

    let Some(probe) = probe else {
        r.skip(
            "find a btrfs volume",
            String::from("no btrfs on any drive - attach miku-os/btrfs.img"),
        );
        return r.report();
    };
    r.check(
        "the probe registry finds btrfs among the other devices",
        true,
        format!(
            "drive {} lba {} label {:?}, {} byte sectors",
            probe.drive,
            probe.start_lba,
            probe.label_str(),
            probe.block_size
        ),
    );
    r.check(
        "and it is not confused for ext",
        found.iter().any(|p| p.fs_type.is_ext_family()),
        String::from("the ext prober still finds the root disk"),
    );

    // mount it through the real block layer
    let dev = crate::fs::ext::mount::dev_for_idx(probe.drive);
    let Some(slot) = super::mount::try_mount_on(dev, probe.drive, probe.start_lba) else {
        r.check("mount it", false, String::from("try_mount_on refused"));
        return r.report();
    };
    r.check("mount it", true, format!("slot {}", slot));

    // Graft it into the tree, so a path resolves
    let grafted = with_vfs(|vfs| -> VfsResult<usize> {
        let id = match vfs.resolve_path(0, MNT) {
            Ok(id) => id,
            Err(_) => vfs.mkdir(0, "btrfs", FileMode::new(0o755))?,
        };
        vfs.nodes[id].fs_type = FsType::Btrfs;
        vfs.nodes[id].ext2_ino = 256;
        vfs.nodes[id].ext2_slot = slot;
        vfs.nodes[id].children_loaded = false;
        vfs.ext2_mount_active = true;
        Ok(id)
    });
    let Ok(mount_id) = grafted else {
        r.check("graft it into the tree", false, format!("{:?}", grafted.err()));
        super::mount::unmount_slot(slot);
        return r.report();
    };
    r.check("graft it into the tree", true, format!("vnode {}", mount_id));

    // Every check below goes through MikuVFS, not lib/btrfs: the trait is under
    // test
    listing(&mut r, mount_id);
    contents(&mut r);
    a_read_only_mount_refuses_writes(&mut r);
    statfs(&mut r);

    super::mount::unmount_slot(slot);
    r.report()
}

/// The names staged come back through `readdir`
fn listing(r: &mut Runner, mount_id: usize) {
    let mut entries = vec![DirEntry::empty(); 64];
    let n = with_vfs(|vfs| vfs.readdir(mount_id, &mut entries));
    match n {
        Ok(count) => {
            // `.`, `..`, hello.txt, docs, blob.bin, link, f1..f10
            let want = 2 + 4 + 10;
            let names: alloc::vec::Vec<&str> =
                entries[..count].iter().map(|e| e.get_name()).collect();
            r.check(
                "the staged names come back",
                count == want
                    && names.contains(&"hello.txt")
                    && names.contains(&"docs")
                    && names.contains(&"blob.bin")
                    && names.contains(&"link"),
                format!("{} entries, expected {}", count, want),
            );
        }
        Err(e) => r.check("the staged names come back", false, format!("{:?}", e)),
    }
}

/// The bytes staged come back through `read_path`
fn contents(r: &mut Runner) {
    let hello = with_vfs(|vfs| vfs.read_path(&format!("{}/hello.txt", MNT)));
    match hello {
        Ok(data) => r.check(
            "a file reads back what was put in it",
            data == HELLO.as_bytes(),
            format!("{} bytes: {:?}", data.len(), core::str::from_utf8(&data).unwrap_or("?")),
        ),
        Err(e) => r.check("a file reads back what was put in it", false, format!("{:?}", e)),
    }

    // A path through a subdirectory: the lookup walks two levels
    let notes = with_vfs(|vfs| vfs.read_path(&format!("{}/docs/notes.txt", MNT)));
    match notes {
        Ok(data) => r.check(
            "a file in a subdirectory resolves",
            data == NOTES.as_bytes(),
            format!("{} bytes", data.len()),
        ),
        Err(e) => r.check("a file in a subdirectory resolves", false, format!("{:?}", e)),
    }

    // Bigger than one sector: the read crosses extents and consults the
    // checksum tree many times
    let blob = with_vfs(|vfs| vfs.path_size(&format!("{}/blob.bin", MNT)));
    match blob {
        Ok(size) => r.check(
            "a file of 200000 bytes reports its size",
            size == BLOB_BYTES,
            format!("{} bytes", size),
        ),
        Err(e) => r.check("a file of 200000 bytes reports its size", false, format!("{:?}", e)),
    }

    let big = with_vfs(|vfs| vfs.read_path(&format!("{}/blob.bin", MNT)));
    match big {
        Ok(data) => r.check(
            "and reads to the end",
            data.len() as u64 == BLOB_BYTES,
            format!("{} of {} bytes", data.len(), BLOB_BYTES),
        ),
        Err(e) => r.check("and reads to the end", false, format!("{:?}", e)),
    }

    // A name that is not there has to fail, not find a neighbour
    let missing = with_vfs(|vfs| vfs.read_path(&format!("{}/not-here", MNT)));
    r.check(
        "a name that is not there is not found",
        missing.is_err(),
        format!("{:?}", missing.err()),
    );
}

/// Every write is refused as read-only, not as unsupported
fn a_read_only_mount_refuses_writes(r: &mut Runner) {
    let created = with_vfs(|vfs| -> VfsResult<usize> {
        let dir = vfs.resolve_path(0, MNT)?;
        vfs.create_file(dir, "should-not-appear", FileMode::new(0o644))
    });
    r.check(
        "creating a file is refused as read-only",
        matches!(created, Err(VfsError::ReadOnly)),
        format!("{:?}", created.err()),
    );

    let removed = with_vfs(|vfs| vfs.unlink(0, &format!("{}/hello.txt", MNT)));
    r.check(
        "unlinking is refused as read-only",
        matches!(removed, Err(VfsError::ReadOnly)),
        format!("{:?}", removed.err()),
    );

    // And the file is still there afterwards
    let still = with_vfs(|vfs| vfs.read_path(&format!("{}/hello.txt", MNT)));
    r.check(
        "and the volume is untouched",
        still.map(|d| d == HELLO.as_bytes()).unwrap_or(false),
        String::new(),
    );
}

fn statfs(r: &mut Runner) {
    let st = with_vfs(|vfs| vfs.statfs(0, MNT));
    match st {
        Ok(st) => r.check(
            "statfs reports btrfs",
            st.fs_type == FsType::Btrfs && st.block_size == 4096 && st.flags & 1 != 0,
            format!(
                "{:?}, {} byte blocks, {} total, read-only flag {}",
                st.fs_type,
                st.block_size,
                st.total_blocks,
                st.flags & 1
            ),
        ),
        Err(e) => r.check("statfs reports btrfs", false, format!("{:?}", e)),
    }
}

pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "btrfs: selftest");
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
            "[btrfs-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name,
            c.detail
        );
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[btrfs-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("btrfs: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("btrfs: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}
