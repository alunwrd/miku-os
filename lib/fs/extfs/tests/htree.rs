// Writing the directory index
//
// Linux's `ext4_find_entry` asks the index for one leaf and on a miss reports
// "no such file" without scanning, so inserts must follow the index. A round
// trip here cannot see that, so this file asks `e2fsck` and `debugfs` and reads
// bytes `mke2fs` wrote.
//
// Reference blocks in `tests/data/` came from:
//
//     mke2fs -t ext4 -b 1024 -O dir_index,metadata_csum -I 256
//     debugfs -w -f <8000 mkdir commands>
//     e2fsck -fDy          # e2fsprogs builds the index in pass 3A
//
// with e2fsprogs 1.47.4: the directory's dx_root, one of its dx_nodes, and
// their superblock.

use extfs::ext2::dir::ext2_dx_hash;
use extfs::ext2::htree::{DX_NODE_ENTRIES, DX_ROOT_ENTRIES};
use extfs::ext4::crc32c;
use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::structs::Superblock;
use extfs::{MemDisk, MikuFS};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

// bytes mke2fs wrote

const DX_SB: &[u8] = include_bytes!("data/mke2fs-dx-sb.bin");
const DX_ROOT: &[u8] = include_bytes!("data/mke2fs-dx-root.bin");
const DX_NODE: &[u8] = include_bytes!("data/mke2fs-dx-node.bin");

/// Inode 13, generation 0: the directory debugfs filled
const DX_INO: u32 = 13;
const DX_GEN: u32 = 0;
const DX_BS: usize = 1024;

fn reference_superblock() -> Superblock {
    let mut sb = Superblock::zeroed();
    sb.data.copy_from_slice(DX_SB);
    sb
}

fn limit_of(block: &[u8], entries_off: usize) -> usize {
    u16::from_le_bytes([block[entries_off], block[entries_off + 1]]) as usize
}

fn count_of(block: &[u8], entries_off: usize) -> usize {
    u16::from_le_bytes([block[entries_off + 2], block[entries_off + 3]]) as usize
}

#[test]
fn the_reference_index_is_what_we_think_it_is() {
    assert_eq!(DX_ROOT.len(), DX_BS);
    assert_eq!(DX_NODE.len(), DX_BS);

    // The root keeps `.` and `..`, with `..` stretched over everything after,
    // so an index-unaware reader sees one empty record
    assert_eq!(u16::from_le_bytes([DX_ROOT[4], DX_ROOT[5]]), 12, "'.' rec_len");
    assert_eq!(
        u16::from_le_bytes([DX_ROOT[16], DX_ROOT[17]]) as usize,
        DX_BS - 12,
        "'..' covers the rest of the block, checksum record and all"
    );
    assert_eq!(DX_ROOT[28], 1, "half_md4");
    assert_eq!(DX_ROOT[29], 8, "dx_root_info is eight bytes");
    assert_eq!(DX_ROOT[30], 1, "the sample is a two-level tree");

    // An interior node is one fake entry covering the block
    assert_eq!(u32::from_le_bytes([DX_NODE[0], DX_NODE[1], DX_NODE[2], DX_NODE[3]]), 0);
    assert_eq!(u16::from_le_bytes([DX_NODE[4], DX_NODE[5]]) as usize, DX_BS);
}

/// The index gives up an entry to `metadata_csum`, unlike the extent tree,
/// where `eh_max` stays
#[test]
fn a_checksummed_node_holds_one_entry_fewer() {
    assert_eq!(limit_of(DX_ROOT, DX_ROOT_ENTRIES), (DX_BS - DX_ROOT_ENTRIES - 8) / 8);
    assert_eq!(limit_of(DX_NODE, DX_NODE_ENTRIES), (DX_BS - DX_NODE_ENTRIES - 8) / 8);
    assert_ne!(
        limit_of(DX_ROOT, DX_ROOT_ENTRIES),
        (DX_BS - DX_ROOT_ENTRIES) / 8,
        "the sample must be able to tell the two rules apart"
    );
}

fn reference_csum(block: &[u8], entries_off: usize) -> (u32, u32) {
    let sb = reference_superblock();
    let seed = crc32c::ext4_csum_seed(sb.uuid(), Some(sb.checksum_seed()));
    let at = entries_off + limit_of(block, entries_off) * 8;
    let used = entries_off + count_of(block, entries_off) * 8;
    let stored = u32::from_le_bytes([block[at + 4], block[at + 5], block[at + 6], block[at + 7]]);
    let computed = crc32c::ext4_dx_csum(seed, DX_INO, DX_GEN, &block[..used], &block[at..at + 8]);
    (computed, stored)
}

#[test]
fn we_reproduce_a_real_dx_root_checksum() {
    let (computed, stored) = reference_csum(DX_ROOT, DX_ROOT_ENTRIES);
    assert_eq!(computed, stored, "e2fsprogs says {stored:#010x}, we say {computed:#010x}");
}

#[test]
fn we_reproduce_a_real_dx_node_checksum() {
    let (computed, stored) = reference_csum(DX_NODE, DX_NODE_ENTRIES);
    assert_eq!(computed, stored, "e2fsprogs says {stored:#010x}, we say {computed:#010x}");
}

/// The checksum covers entries in use. The sample's root holds four of a
/// possible 123, so summing to the tail must differ
#[test]
fn summing_the_unused_entries_too_gives_a_different_answer() {
    let sb = reference_superblock();
    let seed = crc32c::ext4_csum_seed(sb.uuid(), Some(sb.checksum_seed()));
    let at = DX_ROOT_ENTRIES + limit_of(DX_ROOT, DX_ROOT_ENTRIES) * 8;
    let used = DX_ROOT_ENTRIES + count_of(DX_ROOT, DX_ROOT_ENTRIES) * 8;
    assert!(used < at, "the sample's root must have room left");
    let right = crc32c::ext4_dx_csum(seed, DX_INO, DX_GEN, &DX_ROOT[..used], &DX_ROOT[at..at + 8]);
    let wrong = crc32c::ext4_dx_csum(seed, DX_INO, DX_GEN, &DX_ROOT[..at], &DX_ROOT[at..at + 8]);
    assert_ne!(right, wrong);
}

// External tools

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-V")
        .output()
        .map(|o| o.status.success() || !o.stderr.is_empty())
        .unwrap_or(false)
}

fn tmp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("extfs-htree-{tag}-{}.img", std::process::id()));
    path
}

fn dump(disk: &mut MemDisk, tag: &str) -> PathBuf {
    let path = tmp_path(tag);
    let mut f = std::fs::File::create(&path).expect("cannot create image file");
    f.write_all(disk.image_flushed()).expect("cannot write image");
    f.sync_all().ok();
    path
}

/// The image is the evidence when a checker complains; kept if this is set
fn cleanup(path: &Path) {
    if std::env::var_os("EXTFS_KEEP_IMAGES").is_some() {
        eprintln!("kept image: {}", path.display());
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// `e2fsck -f -n`: checks everything, changes nothing. Anything but status 0 is
/// a complaint
fn fsck(path: &Path) -> (i32, String) {
    let out = Command::new("e2fsck")
        .args(["-f", "-n"])
        .arg(path)
        .output()
        .expect("e2fsck did not run");
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    report.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), report)
}

fn debugfs(path: &Path, request: &str) -> String {
    let out = Command::new("debugfs")
        .arg("-R")
        .arg(request)
        .arg(path)
        .output()
        .expect("debugfs did not run");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A filesystem of ours with `count` names in one directory
fn ours_with_a_directory(tag: &str, count: usize, sectors: usize) -> (PathBuf, u32) {
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let dir;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
        for i in 0..count {
            fs.ext2_create_file(dir, &name_of(i), 0o644)
                .unwrap_or_else(|e| panic!("create {i}: {e:?}"));
        }
        assert_eq!(fs.dx_fallbacks, 0, "the index had to be walked, not scanned around");
        fs.sync().unwrap();
    }
    (dump(&mut disk, tag), dir)
}

fn name_of(i: usize) -> String {
    format!("f{i:05}")
}

/// Enough names for the root to fill and the tree to gain a level. At 1 KiB the
/// root holds 123 entries and a leaf about forty names
const DEEP: usize = 6000;
/// Comfortably past one block, and quick
const SHALLOW: usize = 600;

#[test]
fn a_directory_we_grow_is_indexed() {
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();

    let inode = fs.read_inode(dir).unwrap();
    assert!(!fs.dir_is_indexed(&inode), "a fresh directory is one linear block");

    // The index appears when the directory needs a second block, as in Linux
    for i in 0..SHALLOW {
        fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
    }
    let inode = fs.read_inode(dir).unwrap();
    assert!(fs.dir_is_indexed(&inode), "a directory past one block is indexed");
    assert_eq!(fs.htree_depth(&inode).unwrap(), 0, "one level is enough for {SHALLOW}");

    for i in 0..SHALLOW {
        assert!(
            fs.ext2_lookup_in_dir(dir, &name_of(i)).unwrap().is_some(),
            "{} is missing",
            name_of(i)
        );
    }
    assert_eq!(fs.dx_fallbacks, 0);
}

#[test]
fn the_tree_deepens_when_the_root_fills() {
    let sectors = 256 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_create_dir(2, "deep", 0o755).unwrap();
    for i in 0..DEEP {
        fs.ext2_create_file(dir, &name_of(i), 0o644)
            .unwrap_or_else(|e| panic!("create {i}: {e:?}"));
    }
    let inode = fs.read_inode(dir).unwrap();
    assert_eq!(fs.htree_depth(&inode).unwrap(), 1, "{DEEP} names need two levels");

    // and every one of them is still reachable through the index alone
    for i in (0..DEEP).step_by(7) {
        assert!(
            fs.ext2_lookup_in_dir(dir, &name_of(i)).unwrap().is_some(),
            "{} is missing",
            name_of(i)
        );
    }
    assert!(fs.ext2_lookup_in_dir(dir, "f99999").unwrap().is_none());
    assert_eq!(fs.dx_fallbacks, 0);
}

#[test]
fn a_split_loses_nothing() {
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
    for i in 0..SHALLOW {
        fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
    }

    // The walk sees leaves and steps over index blocks, reaching what was
    // created plus two structural entries
    let inode = fs.read_inode(dir).unwrap();
    let mut seen = 0usize;
    let total = fs
        .read_dir_each(&inode, |e| {
            let n = e.name_str();
            if n != "." && n != ".." {
                seen += 1;
            }
            true
        })
        .unwrap();
    assert_eq!(seen, SHALLOW, "the walk found {seen} of {SHALLOW}");
    assert_eq!(total, SHALLOW + 2);
}

#[test]
fn an_indexed_directory_is_fsck_clean() {
    if !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let (path, _) = ours_with_a_directory("clean", SHALLOW, 64 * 1024);
    let (code, report) = fsck(&path);
    assert_eq!(code, 0, "e2fsck rejected our index:\n{report}");
    cleanup(&path);
}

#[test]
fn a_two_level_tree_is_fsck_clean() {
    if !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let (path, _) = ours_with_a_directory("deep", DEEP, 256 * 1024);
    let (code, report) = fsck(&path);
    assert_eq!(code, 0, "e2fsck rejected our two-level index:\n{report}");
    cleanup(&path);
}

/// Linux's tools recognize this index, with each name where it says.
/// `htree_dump` walks the tree and prints its leaves
#[test]
fn debugfs_walks_our_index_and_finds_every_name() {
    if !have("debugfs") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let (path, _) = ours_with_a_directory("dump", SHALLOW, 64 * 1024);

    let dump = debugfs(&path, "htree_dump /many");
    assert!(
        dump.contains("Root node dump"),
        "debugfs does not see an index at all:\n{dump}"
    );
    assert!(dump.contains("Hash Version: 1"), "{dump}");

    let listing = debugfs(&path, "ls -l /many");
    for i in 0..SHALLOW {
        assert!(listing.contains(&name_of(i)), "debugfs cannot see {}", name_of(i));
    }
    cleanup(&path);
}

// an index Linux built

/// A directory of `count` subdirectories indexed by e2fsprogs. `debugfs` builds
/// linear directories, so the index comes from `e2fsck -D`
fn theirs_with_an_index(tag: &str, count: usize) -> Option<PathBuf> {
    if !have("mke2fs") || !have("debugfs") || !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return None;
    }
    let path = tmp_path(tag);
    let blocks = 8192 + count * 8;
    let ok = Command::new("mke2fs")
        .args(["-q", "-t", "ext4", "-b", "1024", "-I", "256"])
        .args(["-O", "dir_index,metadata_csum"])
        .args(["-N", &format!("{}", count + 4096)])
        .args(["-F", path.to_str().unwrap(), &format!("{blocks}")])
        .status()
        .expect("mke2fs did not run");
    assert!(ok.success(), "mke2fs failed");

    let mut script = String::from("mkdir /many\n");
    for i in 0..count {
        script.push_str(&format!("mkdir /many/{}\n", name_of(i)));
    }
    let script_path = path.with_extension("cmds");
    std::fs::write(&script_path, script).unwrap();
    let ok = Command::new("debugfs")
        .args(["-w", "-f"])
        .arg(&script_path)
        .arg(&path)
        .output()
        .expect("debugfs did not run");
    assert!(ok.status.success(), "debugfs failed");
    let _ = std::fs::remove_file(&script_path);

    // pass 3A is where e2fsprogs builds the index
    let ok = Command::new("e2fsck")
        .args(["-fDy"])
        .arg(&path)
        .output()
        .expect("e2fsck did not run");
    // -D rewrites the directory, reporting "filesystem was modified"
    assert!(
        ok.status.code().unwrap_or(-1) & !1 == 0,
        "e2fsck -D failed: {}",
        String::from_utf8_lossy(&ok.stdout)
    );
    let dump = debugfs(&path, "htree_dump /many");
    assert!(
        dump.contains("Root node dump"),
        "e2fsprogs did not index the directory - the test needs a bigger one:\n{dump}"
    );
    Some(path)
}

fn load(path: &Path) -> MemDisk {
    MemDisk::from_image(std::fs::read(path).expect("cannot read image"))
}

/// Every name in a foreign index, found through it. `dx_fallbacks` counts falls
/// back to the whole directory
#[test]
fn we_read_an_index_linux_built() {
    let Some(path) = theirs_with_an_index("read", 1200) else {
        return;
    };
    let mut disk = load(&path);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs
        .ext2_lookup_in_dir(2, "many")
        .unwrap()
        .expect("the directory is not there");
    let inode = fs.read_inode(dir).unwrap();
    assert!(fs.dir_is_indexed(&inode));

    for i in 0..1200 {
        assert!(
            fs.ext2_lookup_in_dir(dir, &name_of(i)).unwrap().is_some(),
            "{} is missing from an index e2fsprogs built",
            name_of(i)
        );
    }
    assert_eq!(fs.dx_fallbacks, 0, "the index was walked, not scanned around");
    cleanup(&path);
}

/// Adding to that directory: the entry must land in the leaf the index assigns
/// its hash
#[test]
fn we_write_into_an_index_linux_built() {
    let Some(path) = theirs_with_an_index("write", 1200) else {
        return;
    };
    let mut disk = load(&path);
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let dir = fs.ext2_lookup_in_dir(2, "many").unwrap().unwrap();
        for i in 0..200 {
            fs.ext2_create_file(dir, &format!("ours-{i:04}"), 0o644)
                .unwrap_or_else(|e| panic!("create {i}: {e:?}"));
        }
        assert_eq!(fs.dx_fallbacks, 0);
        fs.sync().unwrap();
    }
    let out = tmp_path("written");
    std::fs::write(&out, disk.image_flushed()).unwrap();

    let (code, report) = fsck(&out);
    assert_eq!(code, 0, "e2fsck rejected the directory we wrote into:\n{report}");

    // Names are where the index says. `ls` walks the blocks, so this concerns
    // `htree_dump` seeing a tree
    let listing = debugfs(&out, "ls -l /many");
    for i in 0..200 {
        assert!(listing.contains(&format!("ours-{i:04}")), "debugfs cannot see ours-{i:04}");
    }
    let dump = debugfs(&out, "htree_dump /many");
    assert!(dump.contains("Root node dump"), "the index did not survive:\n{dump}");

    cleanup(&path);
    cleanup(&out);
}

// without an index

/// A directory on a non-indexing filesystem keeps growing past twelve blocks,
/// about five hundred names at 1 KiB
#[test]
fn a_linear_directory_grows_past_twelve_blocks() {
    let sectors = 128 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");

    // Removes the index feature, so the directory must grow
    let mut image = disk.image_flushed().to_vec();
    let at = 1024 + 92; // s_feature_compat
    let compat = u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]]);
    image[at..at + 4].copy_from_slice(&(compat & !0x0020).to_le_bytes());
    let mut disk = MemDisk::from_image(image);

    const NAMES: usize = 2000;
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(!fs.superblock.has_dir_index());
    let dir = fs.ext2_create_dir(2, "flat", 0o755).unwrap();
    for i in 0..NAMES {
        fs.ext2_create_file(dir, &name_of(i), 0o644)
            .unwrap_or_else(|e| panic!("create {i}: {e:?}"));
    }
    let inode = fs.read_inode(dir).unwrap();
    assert!(
        inode.size() > 12 * 1024,
        "the directory is {} bytes - the test never reached the old ceiling",
        inode.size()
    );
    assert!(!fs.dir_is_indexed(&inode));
    for i in (0..NAMES).step_by(11) {
        assert!(fs.ext2_lookup_in_dir(dir, &name_of(i)).unwrap().is_some());
    }
    fs.sync().unwrap();
    drop(fs);

    if !have("e2fsck") {
        return;
    }
    let path = dump(&mut disk, "flat");
    let (code, report) = fsck(&path);
    assert_eq!(code, 0, "e2fsck rejected the grown directory:\n{report}");
    cleanup(&path);
}

/// Checks `e2fsck` notices the failure above: an entry spliced into the first
/// leaf under a name hashing far above that leaf's range
#[test]
fn e2fsck_would_have_caught_the_old_insert() {
    if !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
        let mut victim = 0;
        for i in 0..SHALLOW {
            victim = fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
        }
        let inode = fs.read_inode(dir).unwrap();
        assert!(fs.dir_is_indexed(&inode));

        let seed = [
            fs.superblock.hash_seed(0),
            fs.superblock.hash_seed(1),
            fs.superblock.hash_seed(2),
            fs.superblock.hash_seed(3),
        ];
        // Whatever the first leaf holds, the intruder has to hash above it
        let leaf = fs.get_file_block(&inode, 1).unwrap();
        let mut block = vec![0u8; 1024];
        fs.read_block_into(leaf, &mut block).unwrap();
        let mut ceiling = 0u32;
        let mut pos = 0usize;
        while pos + 8 <= 1012 {
            let rec_len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
            let name_len = block[pos + 6] as usize;
            if rec_len == 0 {
                break;
            }
            if block[pos] | block[pos + 1] | block[pos + 2] | block[pos + 3] != 0 && name_len > 0 {
                let h = ext2_dx_hash(&block[pos + 8..pos + 8 + name_len], 1, &seed);
                ceiling = ceiling.max(h);
            }
            pos += rec_len;
        }
        let intruder = (0..10_000)
            .map(|i| format!("intruder-{i:05}"))
            .find(|n| ext2_dx_hash(n.as_bytes(), 1, &seed) > ceiling)
            .expect("no name hashes above the first leaf");

        // Spliced in at the first record with room, index untouched
        let name = intruder.as_bytes();
        let need = (8 + name.len() + 3) / 4 * 4;
        let mut pos = 0usize;
        let mut spliced = false;
        while pos + 8 <= 1012 {
            let rec_len = u16::from_le_bytes([block[pos + 4], block[pos + 5]]) as usize;
            let name_len = block[pos + 6] as usize;
            let held = (8 + name_len + 3) / 4 * 4;
            if rec_len >= held + need {
                block[pos + 4..pos + 6].copy_from_slice(&(held as u16).to_le_bytes());
                let at = pos + held;
                let rest = rec_len - held;
                block[at..at + 4].copy_from_slice(&victim.to_le_bytes());
                block[at + 4..at + 6].copy_from_slice(&(rest as u16).to_le_bytes());
                block[at + 6] = name.len() as u8;
                block[at + 7] = 1;
                block[at + 8..at + 8 + name.len()].copy_from_slice(name);
                spliced = true;
                break;
            }
            pos += rec_len;
        }
        assert!(spliced, "no room in the first leaf to splice into");
        fs.write_dir_block(dir, leaf, &mut block).unwrap();
        fs.sync().unwrap();
    }

    let path = dump(&mut disk, "wrongleaf");
    let (code, report) = fsck(&path);
    assert_ne!(code, 0, "e2fsck accepted an entry in the wrong leaf:\n{report}");
    assert!(
        report.contains("HTREE"),
        "e2fsck complained, but not about the index:\n{report}"
    );
    cleanup(&path);
}

/// Moving an indexed directory rewrites `..` in its root, covered by the root's
/// checksum. Stamping it as a leaf does nothing, as a leaf's record is absent
#[test]
fn moving_an_indexed_directory_keeps_its_index_valid() {
    if !have("e2fsck") {
        eprintln!("e2fsprogs not installed - skipping");
        return;
    }
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
        for i in 0..SHALLOW {
            fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
        }
        let inode = fs.read_inode(dir).unwrap();
        assert!(fs.dir_is_indexed(&inode));

        let elsewhere = fs.ext2_create_dir(2, "elsewhere", 0o755).unwrap();
        fs.ext2_move(2, "many", elsewhere, "moved").unwrap();

        // and it is still readable through its index from its new home
        let moved = fs.ext2_lookup_in_dir(elsewhere, "moved").unwrap().unwrap();
        assert!(fs.ext2_lookup_in_dir(moved, &name_of(0)).unwrap().is_some());
        assert_eq!(fs.dx_fallbacks, 0);
        fs.sync().unwrap();
    }
    let path = dump(&mut disk, "moved");
    let (code, report) = fsck(&path);
    assert_eq!(code, 0, "e2fsck rejected the moved directory:\n{report}");
    cleanup(&path);
}

/// A three-level index is `large_dir`, absent from `EXT_INCOMPAT_SUPP`, so such
/// a volume reached here with a superblock not describing it. Keeps the refusal
/// in `dx_descend`, the second defense behind the mount check
#[test]
fn a_three_level_root_is_still_refused_in_the_descent() {
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    // ext2: the dx block tail checksum is verified before the level count, so
    // on metadata_csum poking byte 30 reads as a bad checksum. Without
    // checksums the descent reaches it
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
    for i in 0..SHALLOW {
        fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
    }

    let inode = fs.read_inode(dir).unwrap();
    assert!(
        fs.dir_is_indexed(&inode),
        "byte 30 is only the level count when there is a dx_root there"
    );

    // Claims a third level as a `large_dir` volume would. Byte 30 of the root
    // is `dx_root_info.indirect_levels`
    let bs = fs.block_size as usize;
    let root_phys = fs.get_file_block(&inode, 0).unwrap();
    let mut root = vec![0u8; bs];
    fs.read_block_into(root_phys, &mut root).unwrap();
    root[30] = 2;
    fs.write_block_data_direct(root_phys, &root).unwrap();

    let r = fs.ext2_create_file(dir, "one-more", 0o644);
    assert!(
        matches!(r, Err(extfs::FsError::UnsupportedFeature)),
        "a third index level must be refused, got {r:?}"
    );
}

/// An unlink in an indexed directory finds its name through the index and
/// changes that leaf. Half the names go: the rest are found through the index,
/// the removed ones are not, and `e2fsck` agrees
#[test]
fn an_unlink_goes_through_the_index() {
    let sectors = 64 * 1024;
    let mut disk = MemDisk::new(sectors);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = sectors as u32;
    params.block_size = 1024;
    params.inode_size = 256;
    mkfs(&mut disk, &params).expect("mkfs failed");
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let dir = fs.ext2_create_dir(2, "many", 0o755).unwrap();
        for i in 0..SHALLOW {
            fs.ext2_create_file(dir, &name_of(i), 0o644).unwrap();
        }
        let inode = fs.read_inode(dir).unwrap();
        assert!(fs.dir_is_indexed(&inode));
        for i in (0..SHALLOW).step_by(2) {
            fs.ext2_delete_file(dir, &name_of(i))
                .unwrap_or_else(|e| panic!("delete {}: {e:?}", name_of(i)));
        }
        for i in 0..SHALLOW {
            let found = fs.ext2_lookup_in_dir(dir, &name_of(i)).unwrap().is_some();
            assert_eq!(found, i % 2 == 1, "{} after the unlinks", name_of(i));
        }
        assert_eq!(fs.dx_fallbacks, 0, "the index had to be walked around");
        assert!(fs.ext2_delete_file(dir, &name_of(0)).is_err(), "a name unlinked twice");
        fs.sync().unwrap();
    }
    if have("e2fsck") {
        let path = dump(&mut disk, "unlink-index");
        let (code, report) = fsck(&path);
        cleanup(&path);
        assert_eq!(code, 0, "e2fsck after the unlinks:\n{report}");
    }
}
