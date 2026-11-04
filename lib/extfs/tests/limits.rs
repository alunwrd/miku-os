// Limits on values from userspace or loops: over-long filenames, enough hard
// links, a deep tree, a large file

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::structs::{EXT2_LINK_MAX, MAX_NAME};
use extfs::{FsError, MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn fs_of(kind: FsType) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

// Names

/// A directory entry stores its name length in one byte. `name_len as u8` would
/// truncate modulo 256 and overrun the block buffer past about 4088 bytes
#[test]
fn an_over_long_name_is_refused() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    for len in [MAX_NAME + 1, 300, 1000, 5000] {
        let name = "a".repeat(len);
        let r = fs.ext2_create_file(2, &name, 0o644);
        assert!(
            matches!(r, Err(FsError::NameTooLong)),
            "a {len}-byte name must be refused, got {r:?}"
        );
    }
}

/// And the longest legal one still works
#[test]
fn the_longest_legal_name_is_accepted() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let name = "b".repeat(MAX_NAME);
    let ino = fs.ext2_create_file(2, &name, 0o644).expect("255 bytes must be legal");
    assert_eq!(fs.ext2_lookup_in_dir(2, &name).unwrap(), Some(ino));
}

#[test]
fn an_empty_name_is_refused() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(matches!(
        fs.ext2_create_file(2, "", 0o644),
        Err(FsError::NameTooLong)
    ));
}

/// The dot entries are structural
#[test]
fn the_dot_entries_cannot_be_added_again() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_create_dir(2, "d", 0o755).unwrap();
    assert!(fs.add_dir_entry(dir, ".", dir, 2).is_err());
    assert!(fs.add_dir_entry(dir, "..", 2, 2).is_err());
}

// Link counts

/// `i_links_count` is a u16. A wrap would make the next unlink free blocks
/// other names point at
#[test]
fn the_link_ceiling_is_below_the_counter_width() {
    assert!(EXT2_LINK_MAX < u16::MAX);
    assert_eq!(EXT2_LINK_MAX, 65000);
}

/// Reaching the ceiling is refused. The count is set directly, testing the
/// check
#[test]
fn a_hardlink_at_the_ceiling_is_refused() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.ext2_create_file(2, "target", 0o644).unwrap();
    let mut inode = fs.read_inode(ino).unwrap();
    inode.set_links_count(EXT2_LINK_MAX);
    fs.write_inode(ino, &inode).unwrap();

    let r = fs.ext2_hardlink(2, "another", ino);
    assert!(
        matches!(r, Err(FsError::TooManyLinks)),
        "a link at the ceiling must be refused, got {r:?}"
    );

    // The refusal left no entry: the check precedes the name
    assert_eq!(fs.ext2_lookup_in_dir(2, "another").unwrap(), None);
    assert_eq!(fs.read_inode(ino).unwrap().links_count(), EXT2_LINK_MAX);
}

/// A parent gains a link per subdirectory, so it can reach the ceiling too
#[test]
fn mkdir_at_the_parent_ceiling_is_refused() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let parent = fs.ext2_create_dir(2, "parent", 0o755).unwrap();
    let mut inode = fs.read_inode(parent).unwrap();
    inode.set_links_count(EXT2_LINK_MAX);
    fs.write_inode(parent, &inode).unwrap();

    assert!(matches!(
        fs.ext2_create_dir(parent, "child", 0o755),
        Err(FsError::TooManyLinks)
    ));
}

// Recursive delete

/// A directory with many children has all of them removed, leaving no orphaned
/// inodes or blocks
#[test]
fn recursive_delete_empties_a_large_directory() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let before_blocks = fs.superblock.free_blocks_count();
    let before_inodes = fs.superblock.free_inodes_count();

    let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
    // Comfortably past both the old 32-child and 64-entry limits
    for i in 0..150u32 {
        let ino = fs.ext2_create_file(dir, &alloc::format!("f{i:04}"), 0o644).unwrap();
        fs.ext2_write_file(ino, b"some contents", 0).unwrap();
    }

    let removed = fs.ext2_delete_recursive(2, "many").expect("recursive delete failed");
    assert!(
        removed >= 151,
        "150 files plus the directory must be reported, got {removed}"
    );

    assert_eq!(fs.ext2_lookup_in_dir(2, "many").unwrap(), None);
    assert_eq!(
        fs.superblock.free_inodes_count(),
        before_inodes,
        "every inode must come back - anything short of this is an orphan"
    );
    assert_eq!(
        fs.superblock.free_blocks_count(),
        before_blocks,
        "every block must come back"
    );
}

/// Nested, exercising the recursion
#[test]
fn recursive_delete_handles_nesting() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let before = fs.superblock.free_inodes_count();

    let mut parent = fs.ext2_create_dir(2, "top", 0o755).unwrap();
    for d in 0..8 {
        parent = fs.ext2_create_dir(parent, &alloc::format!("d{d}"), 0o755).unwrap();
        for f in 0..5 {
            fs.ext2_create_file(parent, &alloc::format!("f{f}"), 0o644).unwrap();
        }
    }

    fs.ext2_delete_recursive(2, "top").expect("nested delete failed");
    assert_eq!(fs.ext2_lookup_in_dir(2, "top").unwrap(), None);
    assert_eq!(fs.superblock.free_inodes_count(), before);
}

/// A tree deeper than the guard is refused. Real trees never reach it; a
/// corrupt one with a cycle would exhaust the stack
#[test]
fn recursive_delete_stops_at_the_depth_limit() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let mut parent = fs.ext2_create_dir(2, "deep", 0o755).unwrap();
    for d in 0..40 {
        parent = match fs.ext2_create_dir(parent, &alloc::format!("l{d}"), 0o755) {
            Ok(p) => p,
            Err(_) => break,
        };
    }

    let r = fs.ext2_delete_recursive(2, "deep");
    assert!(
        matches!(r, Err(FsError::LinkLoop)),
        "a tree past the depth guard must be refused, got {r:?}"
    );
}

// 64-bit size and block accounting

/// `set_size_full` writes offset 108, which is `i_size_high` only for regular
/// files and `i_dir_acl` otherwise. `size()` reads it under the same condition
#[test]
fn the_high_size_half_is_only_for_regular_files() {
    use extfs::structs::{Inode, S_IFDIR, S_IFREG};

    let mut reg = Inode::zeroed();
    reg.on_disk_size = 256;
    reg.set_mode(S_IFREG | 0o644);
    reg.set_size_full(0x1_0000_0000);
    assert_eq!(reg.size(), 0x1_0000_0000);

    let mut dir = Inode::zeroed();
    dir.on_disk_size = 256;
    dir.set_mode(S_IFDIR | 0o755);
    dir.set_size_full(0x1_0000_0000);
    // The high half was not written, so the size is the low half, and i_dir_acl
    // at 108 is untouched
    assert_eq!(dir.size(), 0);
    assert_eq!(dir.size_hi(), 0, "i_dir_acl at offset 108 must not have been written");
}

/// The 48-bit sector count round-trips
#[test]
fn the_block_count_round_trips_past_32_bits() {
    use extfs::structs::{Inode, S_IFREG};

    let mut inode = Inode::zeroed();
    inode.on_disk_size = 256;
    inode.set_mode(S_IFREG | 0o644);

    let big = 0x3_0000_0000u64;
    inode.set_blocks_full(big);
    assert_eq!(inode.blocks_full(), big);
    assert_eq!(inode.blocks(), (big & 0xFFFF_FFFF) as u32);
}

extern crate alloc;

// Symlink targets

/// A target longer than 60 bytes goes into blocks. A 3000-byte target at a 1
/// KiB block size needs several, which `ext2` here gives by default
#[test]
fn a_symlink_target_longer_than_a_block_round_trips() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert_eq!(fs.block_size, 1024, "this case needs a target past one block");

    let target = "d".repeat(3000);
    let ino = fs.ext2_create_symlink(2, "long", &target).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.size() as usize, target.len(), "the size is the whole target");

    let mut buf = vec![0u8; target.len()];
    let n = fs.read_file(&inode, 0, &mut buf).unwrap();
    assert_eq!(n, target.len(), "every byte of the target comes back");
    assert_eq!(&buf[..], target.as_bytes(), "and they are the bytes we wrote");

    // The blocks the target occupies must be what the inode claims, or fsck
    // sees size and block count disagree
    let r = fs.fsck(extfs::fsck::FsckOptions::check()).expect("the check could not run");
    assert!(r.checked, "the check refused to run");
    assert_eq!(r.errors(), 0, "a long symlink left the volume inconsistent");
}

/// The fast path: up to 60 bytes in `i_block`, costing no block
#[test]
fn a_sixty_byte_target_is_still_stored_in_the_inode() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let target = "e".repeat(60);
    let ino = fs.ext2_create_symlink(2, "short", &target).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    assert!(inode.is_fast_symlink(), "sixty bytes fit in the inode");
    assert_eq!(inode.blocks(), 0, "and cost no block");

    let mut buf = vec![0u8; 60];
    let n = fs.read_file(&inode, 0, &mut buf).unwrap();
    assert_eq!(&buf[..n], target.as_bytes());
}

/// The target survives the medium
#[test]
fn a_long_symlink_target_survives_a_remount() {
    let target = "/usr/lib/".to_string() + &"f".repeat(2000);
    let mut disk = fs_of(FsType::Ext3);
    let ino = {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs.ext3_create_symlink(2, "deep", &target).unwrap();
        fs.sync().unwrap();
        ino
    };

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    let mut buf = vec![0u8; target.len()];
    let n = fs.read_file(&inode, 0, &mut buf).unwrap();
    assert_eq!(n, target.len());
    assert_eq!(&buf[..], target.as_bytes());
}
