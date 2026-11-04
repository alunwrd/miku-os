// Directories larger than a caller's array. `read_dir` cannot say if there were
// more, so lookups must see the whole directory: entry 65 must open, delete,
// and block a duplicate name

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;
/// Comfortably past the old limits of 64 and 256
const FILES: usize = 400;

fn fs_with_a_big_directory() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
        for i in 0..FILES {
            fs.ext2_create_file(dir, &format!("file-{i:04}.txt"), 0o644)
                .unwrap_or_else(|e| panic!("create {i}: {e:?}"));
        }
        fs.sync().unwrap();
    }
    disk
}

#[test]
fn every_file_can_be_found() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();

    for i in 0..FILES {
        let name = format!("file-{i:04}.txt");
        assert!(
            fs.ext2_lookup_in_dir(dir, &name).unwrap().is_some(),
            "{name} cannot be found"
        );
    }
}

/// The linear scan behind `lookup`, which had its own limit of 256
#[test]
fn the_linear_scan_sees_the_whole_directory() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir_ino = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();
    let dir = fs.read_inode(dir_ino).unwrap();

    for i in [0usize, 63, 64, 255, 256, FILES - 1] {
        let name = format!("file-{i:04}.txt");
        fs.lookup(dir_ino, &dir, &name)
            .unwrap_or_else(|e| panic!("lookup {name}: {e:?}"));
    }
}

#[test]
fn the_walk_reports_every_entry() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir_ino = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();
    let dir = fs.read_inode(dir_ino).unwrap();

    let mut seen = 0usize;
    let total = fs
        .read_dir_each(&dir, |e| {
            let n = e.name_str();
            if n != "." && n != ".." {
                seen += 1;
            }
            true
        })
        .unwrap();
    assert_eq!(seen, FILES, "the walk found {seen} of {FILES} files");
    assert_eq!(total, FILES + 2, "including . and ..");
}

/// An existing name is refused wherever it sits in the directory
#[test]
fn a_duplicate_name_is_still_refused_deep_in() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();

    for i in [0usize, 100, FILES - 1] {
        let name = format!("file-{i:04}.txt");
        assert!(
            fs.ext2_create_file(dir, &name, 0o644).is_err(),
            "{name} was created a second time"
        );
    }
}

/// And deleting one of the deep entries actually removes it
#[test]
fn a_deep_entry_can_be_deleted() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();

    let name = format!("file-{:04}.txt", FILES - 1);
    fs.ext2_delete_file(dir, &name)
        .unwrap_or_else(|e| panic!("delete {name}: {e:?}"));
    assert!(fs.ext2_lookup_in_dir(dir, &name).unwrap().is_none());

    // and the rest are still there
    assert!(fs.ext2_lookup_in_dir(dir, "file-0300.txt").unwrap().is_some());
}

/// Emptying a large directory empties all of it before freeing the parent
#[test]
fn a_large_directory_is_emptied_completely() {
    let mut disk = fs_with_a_big_directory();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let freed = fs.ext2_delete_recursive(2, "many").expect("delete failed");
    assert!(
        freed as usize >= FILES,
        "only {freed} entries were removed of {FILES}"
    );
    assert!(fs.ext2_lookup_in_dir(2, "many").unwrap().is_none());
}
