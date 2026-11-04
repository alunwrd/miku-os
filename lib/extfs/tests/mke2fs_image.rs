// The shipped image, written here and judged by e2fsprogs
//
// Starts from an image `mke2fs` made, with the arguments `builder/src/main.rs`
// uses for `disk.img`, mounts it read-write, does what installing software
// does, and asks `e2fsck` if the result is a filesystem. `-t ext4` brings a
// journal, extents, 64-bit group descriptors, and metadata checksums.
//
// Skipped where e2fsprogs is not installed.

use extfs::{MemDisk, MikuFS};
use std::path::{Path, PathBuf};
use std::process::Command;

const ROOT: u32 = 2;

/// What `builder/src/main.rs` passes for `disk.img`. `^resize_inode` stays
/// since resize is unsupported; the rest is e2fsprogs' ext4 default
const MKE2FS_TYPE: &str = "ext4";
const MKE2FS_FEATURES: &str = "^resize_inode";

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-V")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty())
        .unwrap_or(false)
}

fn tmp(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("extfs-mke2fs-{tag}-{}.img", std::process::id()));
    p
}

/// A 64 MiB ext4 image made by e2fsprogs, with a `/bin` in it
fn mke2fs_image(path: &Path) {
    let mut staging = std::env::temp_dir();
    staging.push(format!("extfs-mke2fs-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(staging.join("bin")).unwrap();
    std::fs::write(staging.join("bin/msh"), vec![0xCCu8; 120_000]).unwrap();
    std::fs::write(staging.join("bin/ls"), vec![0xAAu8; 40_000]).unwrap();

    let ok = Command::new("dd")
        .args([
            "if=/dev/zero",
            &format!("of={}", path.display()),
            "bs=1M",
            "count=64",
        ])
        .output()
        .expect("dd did not run")
        .status
        .success();
    assert!(ok, "dd could not make the image file");

    let out = Command::new("mke2fs")
        .args([
            "-q", "-F",
            "-t", MKE2FS_TYPE,
            "-O", MKE2FS_FEATURES,
            "-d", staging.to_str().unwrap(),
            path.to_str().unwrap(),
        ])
        .output()
        .expect("mke2fs did not run");
    assert!(
        out.status.success(),
        "mke2fs failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&staging);
}

fn e2fsck(path: &Path) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

#[test]
fn we_can_install_into_an_image_e2fsprogs_made() {
    if !have("mke2fs") || !have("e2fsck") {
        eprintln!("e2fsprogs is not installed; skipping");
        return;
    }

    let path = tmp("install");
    mke2fs_image(&path);

    let mut disk = MemDisk::from_image(std::fs::read(&path).unwrap());
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("we cannot mount it");
        assert!(
            fs.journal_active,
            "the whole point of the change is that this image has a live journal"
        );
        assert!(fs.superblock.has_extents(), "and extents");
        // The driver forces read-only on a ro_compat feature it does not
        // maintain. Asserted, since that would be a silent downgrade
        assert!(
            !fs.reader.read_only,
            "mke2fs's ext4 defaults must not force this mount read-only"
        );
        fs.activate().expect("activate failed");

        let bin = fs
            .ext2_lookup_in_dir(ROOT, "bin")
            .unwrap()
            .expect("/bin should be there, mke2fs put it there");

        // What an install does, in order: a new file staged beside the old,
        // made durable, then renamed over it
        let staged = fs.ext3_create_file_as(bin, ".msh.new", 0o755, 0, 0).unwrap();
        fs.ext3_write_file(staged, &vec![0x55u8; 130_000], 0).unwrap();
        fs.sync().unwrap();
        fs.ext3_rename(bin, ".msh.new", "msh").expect("rename over the live name");

        // ...and the parts either side of it
        let pkg = fs.ext3_create_dir_as(ROOT, "pkg", 0o755, 0, 0).unwrap();
        let db = fs.ext3_create_dir_as(pkg, "db", 0o755, 0, 0).unwrap();
        let rec = fs.ext3_create_file_as(db, "miku-base.mpkr", 0o644, 0, 0).unwrap();
        fs.ext3_write_file(rec, b"a record of what is installed", 0).unwrap();
        fs.ext3_delete_file(bin, "ls").unwrap();

        fs.sync().expect("sync failed");
    }

    std::fs::write(&path, disk.image_flushed()).unwrap();
    let (code, report) = e2fsck(&path);
    assert_eq!(
        code, 0,
        "e2fsck has something to say about what we left behind:\n{report}"
    );

    // ...and the bytes are the new ones
    let mut disk = MemDisk::from_image(std::fs::read(&path).unwrap());
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let bin = fs.ext2_lookup_in_dir(ROOT, "bin").unwrap().unwrap();
    let msh = fs.ext2_lookup_in_dir(bin, "msh").unwrap().unwrap();
    let inode = fs.read_inode(msh).unwrap();
    assert_eq!(inode.size(), 130_000, "the name points at the new file");
    assert!(
        fs.ext2_lookup_in_dir(bin, ".msh.new").unwrap().is_none(),
        "and the staged name is gone"
    );

    if std::env::var_os("EXTFS_KEEP_IMAGES").is_none() {
        let _ = std::fs::remove_file(&path);
    }
}
