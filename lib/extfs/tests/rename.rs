// Rename
//
// Replacing a target: installers write new bytes beside the old and rename over
// them, so the name never points at half a file.
//
// Across directories: `ext2_move` handles `..` and the parents' link counts.
//
// Each case ends with `fsck`, since failures here are a leaked inode, a freed
// block still referenced, or a directory link count off by one.

use extfs::fsck::FsckOptions;
use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{FsError, MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;
const ROOT: u32 = 2;

fn fs_of(kind: FsType) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

fn file_with<H: extfs::host::ExtHost>(fs: &mut MikuFS<H>, dir: u32, name: &str, body: &[u8]) -> u32 {
    let ino = fs
        .ext3_create_file(dir, name, 0o644)
        .unwrap_or_else(|e| panic!("create {name}: {e:?}"));
    fs.ext3_write_file(ino, body, 0)
        .unwrap_or_else(|e| panic!("write {name}: {e:?}"));
    ino
}

fn body_of<H: extfs::host::ExtHost>(fs: &mut MikuFS<H>, dir: u32, name: &str) -> Vec<u8> {
    let ino = fs
        .ext2_lookup_in_dir(dir, name)
        .unwrap()
        .unwrap_or_else(|| panic!("{name} is not there"));
    let inode = fs.read_inode(ino).unwrap();
    let mut buf = vec![0u8; inode.size() as usize];
    let n = fs.read_file(&inode, 0, &mut buf).unwrap();
    buf.truncate(n);
    buf
}

fn assert_clean<H: extfs::host::ExtHost>(fs: &mut MikuFS<H>, what: &str) {
    let report = fs.fsck(FsckOptions::check()).expect("the check could not run");
    assert!(
        report.is_clean(),
        "{what}: the volume is not consistent afterwards: {} error(s), {:?}",
        report.errors(),
        report.examples()
    );
}

// Replacing

#[test]
fn a_rename_replaces_the_name_it_lands_on() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let mut disk = fs_of(kind);
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

        file_with(&mut fs, ROOT, "new", b"the new program");
        file_with(&mut fs, ROOT, "prog", b"the old program");

        fs.ext3_rename(ROOT, "new", "prog")
            .unwrap_or_else(|e| panic!("{kind:?}: rename over an existing name: {e:?}"));

        assert_eq!(
            body_of(&mut fs, ROOT, "prog"),
            b"the new program",
            "{kind:?}: the name must point at the new bytes"
        );
        assert!(
            fs.ext2_lookup_in_dir(ROOT, "new").unwrap().is_none(),
            "{kind:?}: the source name must be gone"
        );
        assert_clean(&mut fs, &format!("{kind:?} replace"));
    }
}

/// The victim's inode returns to the pool, or an inode and blocks stay in use
/// with nothing pointing at them
#[test]
fn the_replaced_file_is_freed_not_orphaned() {
    let mut disk = fs_of(FsType::Ext4);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    file_with(&mut fs, ROOT, "new", b"x");
    file_with(&mut fs, ROOT, "prog", &[7u8; 40_000]);

    let before = fs.fs_info();
    fs.ext3_rename(ROOT, "new", "prog").unwrap();
    let after = fs.fs_info();

    assert_eq!(
        after.free_inodes,
        before.free_inodes + 1,
        "exactly one inode - the victim's - should have been freed"
    );
    assert!(
        after.free_blocks > before.free_blocks,
        "the victim's data blocks should have been freed too \
         (before {}, after {})",
        before.free_blocks,
        after.free_blocks
    );
    assert_clean(&mut fs, "freeing the victim");
}

/// POSIX: if both names resolve to one inode, the rename succeeds and changes
/// nothing, or the victim's unlink would delete the source
#[test]
fn renaming_a_name_onto_its_own_inode_keeps_the_file() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = file_with(&mut fs, ROOT, "one", b"keep me");
    fs.ext2_hardlink(ROOT, "two", ino).unwrap();

    fs.ext3_rename(ROOT, "one", "two")
        .expect("a rename between two names for one inode must succeed");

    assert_eq!(body_of(&mut fs, ROOT, "two"), b"keep me");
    assert!(
        fs.ext2_lookup_in_dir(ROOT, "one").unwrap().is_some(),
        "nothing should have changed, so the other name is still there"
    );
    assert_clean(&mut fs, "hard link onto its twin");
}

#[test]
fn a_file_may_not_replace_a_directory_and_a_directory_may_not_replace_a_file() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    file_with(&mut fs, ROOT, "file", b"f");
    fs.ext3_create_dir(ROOT, "dir", 0o755).unwrap();

    assert!(
        matches!(fs.ext3_rename(ROOT, "file", "dir"), Err(FsError::IsDirectory)),
        "a file must not replace a directory"
    );
    assert!(
        matches!(fs.ext3_rename(ROOT, "dir", "file"), Err(FsError::NotDirectory)),
        "a directory must not replace a file"
    );
    assert_clean(&mut fs, "refused replacements");
}

#[test]
fn a_directory_replaces_only_an_empty_directory() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    fs.ext3_create_dir(ROOT, "src", 0o755).unwrap();
    let busy = fs.ext3_create_dir(ROOT, "busy", 0o755).unwrap();
    file_with(&mut fs, busy, "inhabitant", b"here");

    assert!(
        matches!(fs.ext3_rename(ROOT, "src", "busy"), Err(FsError::NotEmpty)),
        "a non-empty directory must not be replaced"
    );

    fs.ext3_create_dir(ROOT, "empty", 0o755).unwrap();
    fs.ext3_rename(ROOT, "src", "empty")
        .expect("an empty directory may be replaced");

    assert!(fs.ext2_lookup_in_dir(ROOT, "src").unwrap().is_none());
    assert_clean(&mut fs, "directory over empty directory");
}

// Across directories

#[test]
fn a_file_moves_between_directories() {
    for kind in [FsType::Ext2, FsType::Ext3, FsType::Ext4] {
        let mut disk = fs_of(kind);
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

        let from = fs.ext3_create_dir(ROOT, "from", 0o755).unwrap();
        let to = fs.ext3_create_dir(ROOT, "to", 0o755).unwrap();
        file_with(&mut fs, from, "prog", b"moved");

        fs.ext3_move(from, "prog", to, "prog")
            .unwrap_or_else(|e| panic!("{kind:?}: move across directories: {e:?}"));

        assert!(fs.ext2_lookup_in_dir(from, "prog").unwrap().is_none());
        assert_eq!(body_of(&mut fs, to, "prog"), b"moved");
        assert_clean(&mut fs, &format!("{kind:?} move"));
    }
}

/// What an installer does: a staged file in one directory renamed onto a live
/// name in another
#[test]
fn a_move_replaces_the_name_it_lands_on() {
    let mut disk = fs_of(FsType::Ext4);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let stage = fs.ext3_create_dir(ROOT, "stage", 0o755).unwrap();
    let bin = fs.ext3_create_dir(ROOT, "bin", 0o755).unwrap();
    file_with(&mut fs, stage, "msh", b"version two");
    file_with(&mut fs, bin, "msh", b"version one");

    fs.ext3_move(stage, "msh", bin, "msh").expect("staged install");

    assert_eq!(body_of(&mut fs, bin, "msh"), b"version two");
    assert!(fs.ext2_lookup_in_dir(stage, "msh").unwrap().is_none());
    assert_clean(&mut fs, "staged replace across directories");
}

/// Moving a directory rewrites `..` and moves a link between parents. `fsck`
/// checks both link counts
#[test]
fn a_directory_moves_with_its_parent_link() {
    let mut disk = fs_of(FsType::Ext3);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let from = fs.ext3_create_dir(ROOT, "from", 0o755).unwrap();
    let to = fs.ext3_create_dir(ROOT, "to", 0o755).unwrap();
    let moved = fs.ext3_create_dir(from, "sub", 0o755).unwrap();
    file_with(&mut fs, moved, "keep", b"still here");

    fs.ext3_move(from, "sub", to, "sub").expect("move a directory");

    let landed = fs
        .ext2_lookup_in_dir(to, "sub")
        .unwrap()
        .expect("the directory should be under its new parent");
    assert_eq!(landed, moved, "it is the same directory, not a copy");
    assert_eq!(body_of(&mut fs, landed, "keep"), b"still here");
    assert_clean(&mut fs, "directory move");
}
