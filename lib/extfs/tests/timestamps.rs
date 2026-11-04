// Timestamps wider than 32 bits
//
// `i_atime`, `i_ctime`, and `i_mtime` are 32-bit seconds, running out in
// January 2038. On a large inode each has an `_extra` word: the low two bits
// carry seconds past 2^32, the upper thirty nanoseconds.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::structs::*;
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 64 * 1024;

/// 2040-01-01, past 32 bits of seconds
const AFTER_2038: u64 = 2_209_075_200;

fn ext4() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn the_epoch_bits_are_where_linux_puts_them() {
    let mut inode = Inode::zeroed();
    inode.on_disk_size = 256;
    inode.write_u16(128, 32);

    inode.set_mtime_full(AFTER_2038, 123_456_789);
    // The seconds field keeps the low 32 bits
    assert_eq!(inode.mtime(), AFTER_2038 as u32);
    // The extra word has the rest: two bits of epoch, thirty of nanoseconds
    let extra = inode.mtime_extra();
    assert_eq!((extra & 3) as u64, AFTER_2038 >> 32);
    assert_eq!(extra >> 2, 123_456_789);
    assert_eq!(inode.mtime_full(), (AFTER_2038, 123_456_789));
}

#[test]
fn a_small_inode_has_nowhere_to_put_them() {
    // A 128-byte inode has no extra area; writing one would land in whatever
    // follows in the table
    let mut inode = Inode::zeroed();
    inode.set_mtime_full(AFTER_2038, 500);
    assert_eq!(inode.mtime(), AFTER_2038 as u32);
    assert_eq!(inode.mtime_extra(), 0);
    // So it reads back truncated, as its inodes cannot hold the date
    assert_eq!(inode.mtime_full().0, AFTER_2038 as u32 as u64);
}

#[test]
fn setting_a_time_clears_the_nanoseconds_beside_it() {
    let mut inode = Inode::zeroed();
    inode.on_disk_size = 256;
    inode.write_u16(128, 32);

    inode.set_mtime_full(1_000_000, 999_999_999);
    inode.set_mtime(2_000_000);
    assert_eq!(
        inode.mtime_full(),
        (2_000_000, 0),
        "the old nanoseconds outlived the second they belonged to"
    );
}

/// The fields survive the inode table, where a wrong offset shows
#[test]
fn a_wide_timestamp_survives_a_remount() {
    let mut disk = ext4();
    let ino;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        ino = fs.ext4_create_file(2, "dated.bin", 0o644).unwrap();
        let mut inode = fs.read_inode(ino).unwrap();
        assert!(
            inode.extra_isize() >= 4,
            "a new inode on a 256-byte filesystem must declare its extra area"
        );
        inode.set_mtime_full(AFTER_2038, 250_000_000);
        inode.set_atime_full(AFTER_2038 + 60, 1);
        fs.write_inode(ino, &inode).unwrap();
        fs.sync().unwrap();
    }

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.mtime_full(), (AFTER_2038, 250_000_000));
    assert_eq!(inode.atime_full(), (AFTER_2038 + 60, 1));
}

/// The checksum covers them, so a wide timestamp must still verify
#[test]
fn a_wide_timestamp_keeps_the_inode_checksum_valid() {
    let mut disk = ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(fs.superblock.has_metadata_csum());

    let ino = fs.ext4_create_file(2, "dated.bin", 0o644).unwrap();
    let mut inode = fs.read_inode(ino).unwrap();
    inode.set_ctime_full(AFTER_2038, 7);
    fs.write_inode(ino, &inode).unwrap();

    // read_inode is what verifies it
    let back = fs.read_inode(ino).expect("the inode failed its own checksum");
    assert_eq!(back.ctime_full(), (AFTER_2038, 7));
}

/// `debugfs` decodes the same bytes and must reach the same date
#[test]
fn debugfs_reads_our_wide_timestamp_as_2040() {
    use std::io::Write;
    use std::process::Command;

    if Command::new("debugfs").arg("-V").output().is_err() {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }

    let mut disk = ext4();
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs.ext4_create_file(2, "dated.bin", 0o644).unwrap();
        let mut inode = fs.read_inode(ino).unwrap();
        inode.set_mtime_full(AFTER_2038, 250_000_000);
        fs.write_inode(ino, &inode).unwrap();
        fs.sync().unwrap();
    }

    let mut path = std::env::temp_dir();
    path.push(format!("extfs-timestamps-{}.img", std::process::id()));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(disk.image_flushed()).unwrap();
    f.sync_all().ok();

    let out = Command::new("debugfs")
        .args(["-R", "stat dated.bin"])
        .arg(&path)
        .output()
        .expect("debugfs did not run");
    let _ = std::fs::remove_file(&path);
    let report = String::from_utf8_lossy(&out.stdout).into_owned();

    let line = report
        .lines()
        .find(|l| l.trim_start().starts_with("mtime:"))
        .unwrap_or_else(|| panic!("debugfs printed no mtime:\n{report}"));
    assert!(
        line.contains("2040"),
        "debugfs reads our post-2038 timestamp as {line:?}"
    );
    // debugfs prints the `_extra` word raw after the colon. 250 million
    // nanoseconds shifted up by the two epoch bits is 0x3b9aca00
    assert!(
        line.contains("3b9aca00"),
        "debugfs found a different extra word in {line:?}"
    );
}
