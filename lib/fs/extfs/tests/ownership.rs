// Who owns a new inode
//
// `ext2_create_file` and `ext4_create_file` write `init_file(mode, 0, 0)`,
// owned by root. Installers must place files with a given owner and be able to
// verify it.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;
const ROOT: u32 = 2;

fn fs_of(kind: FsType) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn a_file_is_created_owned_by_whoever_asked() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let mut disk = fs_of(kind);
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

        let ino = fs
            .ext3_create_file_as(ROOT, "theirs", 0o644, 1000, 100)
            .unwrap_or_else(|e| panic!("{kind:?}: create: {e:?}"));

        let inode = fs.read_inode(ino).unwrap();
        assert_eq!(inode.uid(), 1000, "{kind:?}: the file belongs to its creator");
        assert_eq!(inode.gid(), 100, "{kind:?}: and to their group");
    }
}

#[test]
fn a_directory_and_a_symlink_are_created_owned_too() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let dir = fs.ext3_create_dir_as(ROOT, "theirs", 0o755, 1000, 100).unwrap();
    let link = fs
        .ext3_create_symlink_as(ROOT, "pointer", "theirs", 1000, 100)
        .unwrap();

    for (what, ino) in [("directory", dir), ("symlink", link)] {
        let inode = fs.read_inode(ino).unwrap();
        assert_eq!(inode.uid(), 1000, "{what}: uid");
        assert_eq!(inode.gid(), 100, "{what}: gid");
    }
}

/// Ownership is in the creation's transaction, and survives a fresh mount
/// replaying the journal
#[test]
fn the_ownership_survives_a_remount() {
    let mut disk = fs_of(FsType::Ext4);
    let ino = {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs.ext3_create_file_as(ROOT, "theirs", 0o644, 1000, 100).unwrap();
        fs.sync().unwrap();
        ino
    };

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.uid(), 1000);
    assert_eq!(inode.gid(), 100);
}

/// The old entry points still mean root
#[test]
fn the_plain_create_still_makes_a_root_owned_file() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.ext3_create_file(ROOT, "systems", 0o644).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.uid(), 0);
    assert_eq!(inode.gid(), 0);
}

// Wide ids
//
// The read side is `uid_full`: the low half in `i_uid` and the high half in
// `l_i_uid_high`, as ext4 represents an id above 65535. The write side must set
// both.

/// A disk from a real Linux can name uid 100000, and writing it back names the
/// same owner
#[test]
fn an_id_above_sixteen_bits_round_trips() {
    let mut disk = fs_of(FsType::Ext4);
    let ino = {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs.ext3_create_file(ROOT, "theirs", 0o644).unwrap();
        fs.ext2_chown(ino, 100_000, 100_001).unwrap();
        fs.sync().unwrap();
        ino
    };

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.uid_full(), 100_000, "the whole uid, not its low half");
    assert_eq!(inode.gid_full(), 100_001, "the whole gid");
}

/// `chown 0` on a file owned by uid 100000 must clear `l_i_uid_high`, or it
/// reads back as 65536
#[test]
fn chown_down_from_a_high_id_clears_the_high_half() {
    let mut disk = fs_of(FsType::Ext4);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.ext3_create_file(ROOT, "theirs", 0o644).unwrap();
    fs.ext2_chown(ino, 100_000, 100_000).unwrap();
    assert_eq!(fs.read_inode(ino).unwrap().uid_full(), 100_000);

    fs.ext2_chown(ino, 0, 0).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.uid_full(), 0, "chown root gives root");
    assert_eq!(inode.uid_hi(), 0, "and leaves nothing of the old owner behind");
    assert_eq!(inode.gid_full(), 0);
    assert_eq!(inode.gid_hi(), 0);
}

/// Creators take the same width, so a package for a high id is owned from the
/// first write
#[test]
fn a_file_created_for_a_high_id_is_owned_by_them() {
    let mut disk = fs_of(FsType::Ext4);
    let ino = {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs
            .ext3_create_file_as(ROOT, "theirs", 0o644, 70_000, 70_000)
            .unwrap();
        fs.sync().unwrap();
        ino
    };

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.uid_full(), 70_000);
    assert_eq!(inode.gid_full(), 70_000);
}
